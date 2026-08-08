use crate::base64;
use crate::ffmpeg_exe;
use crate::wav_codec;
use crate::{constants, error, info, warn};
use std::path::{Path, PathBuf};

/// 音频处理错误（B-07：thiserror 统一错误体系，Display 文案与改造前一致，
/// source() 链保留底层错误供诊断）
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error(transparent)]
    WavError(#[from] wav_codec::WavError),
    #[error(transparent)]
    FfmpegExeError(#[from] ffmpeg_exe::FfmpegExeError),
    #[error("IO 错误: {0}")]
    IoError(#[from] std::io::Error),
    #[error("编码错误: {0}")]
    Encoding(String),
}

/// 应用专属临时根目录：所有任务临时目录只能由后端在此根下创建
pub fn app_temp_root() -> PathBuf {
    std::env::temp_dir().join("mimo-asr-assistant")
}

/// 任务临时目录元数据（pid + 创建时间），供启动清理甄别归属。
/// P4-03：新增 schema_version / task_generation / run_id，均带 serde default，
/// 兼容旧版（无新字段）元数据——旧目录启动清理仍可正常读取与删除。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TaskTempMeta {
    pub pid: u32,
    pub created_ms: u64,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub task_generation: u64,
    #[serde(default)]
    pub run_id: String,
}

/// 当前任务元数据 schema 版本（P4-03）。
pub const SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

const TASK_META_FILE: &str = ".mimo_meta.json";

/// 后端创建任务临时目录：目录名不可预测（pid + 纳秒 + 随机哈希），
/// 并写元数据文件。返回受信任路径，由调用方登记进 TaskRegistry。
///
/// P4-03：目录创建与元数据落盘构成单一事务——元数据经原子写
/// （`atomic_write_text_no_backup`，含 flush + sync_all）成功后才返回目录；
/// 序列化/写入/刷新/同步任一步失败都删除刚创建的目录并返回 `AudioError`，
/// 不残留无元数据的孤儿目录，调用方也不会把它登记进 TaskRegistry。
pub fn create_task_temp_dir() -> Result<PathBuf, AudioError> {
    let root = app_temp_root();
    std::fs::create_dir_all(&root)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // RandomState 每次 new 随机种子，叠加不可预测性
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_usize(nanos as usize);
    let extra = hasher.finish();
    let dir = root.join(format!(
        "mimo_{}_{:x}",
        std::process::id(),
        nanos.wrapping_add(extra as u128)
    ));
    // 先序列化（不涉及磁盘），失败直接返回，无需回滚
    let meta = TaskTempMeta {
        pid: std::process::id(),
        created_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        schema_version: SCHEMA_VERSION,
        task_generation: 0,
        run_id: String::new(),
    };
    let json = serde_json::to_string(&meta)
        .map_err(|e| AudioError::Encoding(format!("任务元数据序列化失败: {}", e)))?;
    // create_dir 在重名时直接失败，不覆盖任何已存在目录
    std::fs::create_dir(&dir)?;
    // 元数据原子写（同目录 tmp -> flush -> sync_all -> 原子 replace）。
    // 任一步失败即回滚：删除刚创建的目录并返回错误。
    if let Err(e) = crate::atomic::atomic_write_text_no_backup(&dir.join(TASK_META_FILE), &json) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e.into());
    }
    Ok(dir)
}

/// 读取任务目录元数据（启动清理用）
pub fn read_task_meta(dir: &Path) -> Option<TaskTempMeta> {
    let content = std::fs::read_to_string(dir.join(TASK_META_FILE)).ok()?;
    serde_json::from_str(&content).ok()
}

/// 检查路径组件是否为符号链接/junction/reparse point（失败即拒绝，fail-closed）
#[cfg(target_os = "windows")]
fn is_reparse_point(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    match path.symlink_metadata() {
        Ok(m) => (m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0,
        Err(_) => true,
    }
}

#[cfg(not(target_os = "windows"))]
fn is_reparse_point(path: &Path) -> bool {
    match path.symlink_metadata() {
        Ok(m) => m.file_type().is_symlink(),
        Err(_) => true,
    }
}

/// 公开的 reparse point 检查（供启动缓存清理等跨模块复用；失败即视为重解析点）
pub fn is_reparse_point_pub(path: &Path) -> bool {
    is_reparse_point(path)
}

/// 检查 root→target 链路上每一层（含最终目录）是否存在重解析点
fn chain_has_reparse_point(root: &Path, target: &Path) -> bool {
    let rel = match target.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return true,
    };
    let mut cur = root.to_path_buf();
    for comp in rel.components() {
        cur.push(comp);
        if is_reparse_point(&cur) {
            return true;
        }
    }
    false
}

/// 完整路径守卫（B-04）：
/// 1. 拒绝非绝对路径、含 `..`/空组件的路径；
/// 2. 目标必须存在且是目录（禁止对不存在路径做字符串规范化后删除）；
/// 3. canonicalize 后必须严格位于规范化根目录内，且不等于根本身；
/// 4. 链路上任一层是符号链接/junction/reparse point 即拒绝。
///
/// 成功后返回规范化路径，调用方应立即对其操作（最小竞态窗口）。
pub fn verify_task_dir(dir: &Path, root: &Path) -> Result<PathBuf, String> {
    if !dir.is_absolute() {
        return Err("路径守卫拒绝: 非绝对路径".to_string());
    }
    for comp in dir.components() {
        match comp {
            std::path::Component::ParentDir => {
                return Err("路径守卫拒绝: 路径含 .. 组件".to_string());
            }
            std::path::Component::Normal(c) if c.is_empty() => {
                return Err("路径守卫拒绝: 路径含空组件".to_string());
            }
            _ => {}
        }
    }
    if !dir.is_dir() {
        return Err("路径守卫拒绝: 目标不存在或不是目录".to_string());
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| format!("路径守卫拒绝: 根目录不可解析 ({})", e))?;
    let canon = dir
        .canonicalize()
        .map_err(|e| format!("路径守卫拒绝: 目标不可解析 ({})", e))?;
    if canon == canon_root || !canon.starts_with(&canon_root) {
        return Err("路径守卫拒绝: 目标不在应用临时根内".to_string());
    }
    if chain_has_reparse_point(&canon_root, &canon) {
        return Err("路径守卫拒绝: 路径含符号链接/junction 重解析点".to_string());
    }
    Ok(canon)
}

/// 对应用临时根内的任务目录执行守卫校验（公共入口）
pub fn verify_app_task_dir(dir: &Path) -> Result<PathBuf, String> {
    verify_task_dir(dir, &app_temp_root())
}

/// 守卫式删除：校验通过后立即删除。仅用于受信任的、已登记的任务目录。
/// 日志只记录结果，不记录路径全文。
pub fn remove_task_dir_guarded(dir: &Path) -> Result<(), String> {
    let root = app_temp_root();
    if !root.exists() {
        return Ok(());
    }
    let canon = verify_task_dir(dir, &root)?;
    match std::fs::remove_dir_all(&canon) {
        Ok(()) => {
            info!("守卫式临时目录清理成功");
            Ok(())
        }
        Err(e) => {
            // Windows 句柄延迟释放，重试一次
            std::thread::sleep(std::time::Duration::from_millis(200));
            match std::fs::remove_dir_all(&canon) {
                Ok(()) => {
                    info!("守卫式临时目录清理成功（重试）");
                    Ok(())
                }
                Err(e2) => {
                    warn!("守卫式临时目录清理失败: {} / {}", e, e2);
                    Err(format!("临时目录清理失败: {}", e2))
                }
            }
        }
    }
}

/// 将音频文件转换为 WAV 并切分，通过回调逐片返回 Base64 data URL
/// callback 参数：(当前分片序号, 总分片估算, data_url)，返回 false 表示立即停止切片
/// （B-05：channel 关闭/取消时发送端返回 false，避免浪费 CPU 与磁盘）
/// temp_dir 由调用方创建和管理，本函数不再负责清理
pub fn convert_and_slice(
    input_path: &Path,
    temp_dir: &Path,
    _slice_seconds: Option<u32>,
    cancel: &std::sync::atomic::AtomicBool,
    bin: &crate::ffmpeg_exe::FfmpegBin,
    callback: &dyn Fn(u32, u32, String) -> bool,
) -> Result<usize, AudioError> {
    let file_name = input_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    // H-04：日志只保留经过消毒的文件名，不记录完整输入路径
    info!("[{}] convert_and_slice 开始", file_name);

    std::fs::create_dir_all(temp_dir)?;
    info!("[{}] 临时目录已就绪", file_name);

    // 容器魔数 + fmt 直读判定：已是 16kHz/mono/16bit WAV 则零转换直接复制
    let wav_path = if wav_codec::is_target_wav(input_path) {
        let out = temp_dir.join("converted.wav");
        std::fs::copy(input_path, &out)?;
        info!(
            "[{}] 已是目标格式 (16kHz/mono/16bit WAV)，跳过转码，已复制原文件到临时目录",
            file_name
        );
        out
    } else {
        let out = temp_dir.join("converted.wav");
        info!(
            "[{}] 开始转码（ffmpeg.exe → 16kHz/mono/16bit WAV）...",
            file_name
        );
        if let Err(e) = bin.convert_to_wav(input_path, &out, cancel) {
            // 转码失败出口立即清理半成品（尽力而为，忽略失败）
            let _ = std::fs::remove_file(&out);
            error!("[{}] 转码失败: {}", file_name, e);
            return Err(AudioError::FfmpegExeError(e));
        }
        info!("[{}] 转码完成", file_name);
        out
    };

    // 获取时长
    let total_duration = wav_codec::get_duration(&wav_path)?;
    info!("[{}] 音频时长: {:.1}s", file_name, total_duration);

    if total_duration <= 0.0 {
        error!("[{}] 音频时长为 0", file_name);
        return Err(AudioError::Encoding("音频时长为 0".to_string()));
    }

    // 动态切割
    let max_chunk = _slice_seconds
        .unwrap_or(constants::MAX_SLICE_SECONDS)
        .clamp(constants::MIN_SLICE_SECONDS, constants::MAX_SLICE_SECONDS);
    info!("[{}] 开始分片... (切片时长 {}s)", file_name, max_chunk);
    // 估算总分片数（用于前端进度条）
    let estimated_total = ((total_duration / max_chunk as f64).ceil() as u32).max(1);
    let mut current_time = 0.0;
    let mut chunk_sec = max_chunk;
    let mut chunk_index = 0;

    while current_time < total_duration {
        // B-05：取消标志置位时立即终止切片循环
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            info!("[{}] 切片任务收到取消信号，提前终止", file_name);
            break;
        }
        let remaining = total_duration - current_time;
        if remaining < 0.2 {
            break;
        }
        let mut this_sec = (chunk_sec as f64).min(remaining);

        info!(
            "[{}] 分片 {}: {:.0}s-{:.0}s ({:.1}s)",
            file_name,
            chunk_index + 1,
            current_time,
            current_time + this_sec,
            this_sec
        );

        // 切片文件保留在任务目录（供暂停/续传复用），此处只需字节内容
        let log_tag = format!("[{}] 分片 {}", file_name, chunk_index);
        let (_, audio_bytes, used_sec) = slice_chunk_with_shrink(
            &wav_path,
            temp_dir,
            current_time,
            remaining,
            chunk_sec,
            constants::MAX_BASE64_BYTES,
            &log_tag,
        )?;
        this_sec = used_sec;
        let b64_len = base64::base64_encoded_len(audio_bytes.len());

        let base64_str = base64::base64_encode(&audio_bytes);
        let data_url = format!("data:audio/wav;base64,{}", base64_str);
        // 立即通过回调交出 data_url；回调返回 false（channel 已关闭/取消）则停止切片
        if !callback(chunk_index + 1, estimated_total, data_url) {
            info!("[{}] 切片回调请求停止，提前终止切片循环", file_name);
            drop(audio_bytes);
            break;
        }
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            info!(
                "[{}] 分片 {} 交付后收到取消信号，停止后续分片",
                file_name,
                chunk_index + 1
            );
            break;
        }
        // 释放原始音频字节
        drop(audio_bytes);

        info!(
            "[{}] 分片 {} 完成: {:.0}KB",
            file_name,
            chunk_index + 1,
            b64_len as f64 / 1024.0
        );

        current_time += this_sec;
        chunk_index += 1;
        chunk_sec = max_chunk;
    }

    info!(
        "[{}] convert_and_slice 完成: {} 个分片",
        file_name, chunk_index
    );

    Ok(chunk_index as usize)
}

/// 分片超限收缩重试（B-06 残留修复）。
///
/// 从 `min(max_chunk, remaining)` 开始切片；若切片 Base64 长度超过 `max_base64_bytes`，
/// **先删除该起点刚产出的超限切片文件**再按 CHUNK_SHRINK_STEP 收缩重切。
/// 若不删除，旧 `slice_{start}_{old_end}.wav` 会与新切片并存，`collect_slice_files`
/// 按起始秒排序后会把同一起点转写两遍。
///
/// `max_base64_bytes` 注入以便单元测试构造收缩场景；生产一律传
/// `constants::MAX_BASE64_BYTES`。`log_tag` 仅用于日志/错误文案前缀。
/// 返回 (最终切片路径, 切片字节, 实际使用的秒数)。
pub(crate) fn slice_chunk_with_shrink(
    wav_path: &Path,
    temp_dir: &Path,
    start_time: f64,
    remaining: f64,
    max_chunk: u32,
    max_base64_bytes: usize,
    log_tag: &str,
) -> Result<(PathBuf, Vec<u8>, f64), AudioError> {
    let mut this_sec = (max_chunk as f64).min(remaining);
    loop {
        let slice_path = wav_codec::slice_wav(wav_path, temp_dir, start_time, this_sec)?;
        let audio_bytes = std::fs::read(&slice_path)?;
        let b64_len = base64::base64_encoded_len(audio_bytes.len());

        if b64_len <= max_base64_bytes {
            return Ok((slice_path, audio_bytes, this_sec));
        }

        // B-06：收缩前删除该起点已产出的超限切片，避免同起点重复文件
        if let Err(e) = std::fs::remove_file(&slice_path) {
            warn!("{} 删除超限切片失败: {}", log_tag, e);
        }

        this_sec -= constants::CHUNK_SHRINK_STEP as f64;
        if this_sec < constants::MIN_SLICE_SECONDS as f64 {
            error!(
                "{} 缩至 {:.1}s 仍超限 ({:.1}MB)",
                log_tag,
                this_sec,
                b64_len as f64 / 1024.0 / 1024.0
            );
            return Err(AudioError::Encoding(format!(
                "缩至 {:.1}s 仍超限 ({:.1}MB)",
                this_sec,
                b64_len as f64 / 1024.0 / 1024.0
            )));
        }
        warn!(
            "{} Base64={:.1}MB 超限，缩短至 {:.1}s",
            log_tag,
            b64_len as f64 / 1024.0 / 1024.0,
            this_sec
        );
    }
}

/// 清理任务临时目录。
/// B-10/P4-04：本函数被 async 命令路径调用，递归删除统一放入
/// `spawn_blocking`（调用方负责），故此处可安全地保留一次失败重试
/// （Windows 句柄延迟释放）。返回 `Result<(), String>`：清理失败时返回错误，
/// 残留目录由启动时 cleanup_legacy_temp 统一回收，并记录结构化清理 issue。
pub fn cleanup_temp(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    match std::fs::remove_dir_all(path) {
        Ok(_) => {
            info!("临时目录已清理");
            Ok(())
        }
        Err(_) => {
            // Windows 句柄延迟释放，重试一次
            std::thread::sleep(std::time::Duration::from_millis(200));
            match std::fs::remove_dir_all(path) {
                Ok(_) => {
                    info!("临时目录已清理（重试）");
                    Ok(())
                }
                Err(e2) => {
                    warn!("临时目录清理失败（留待启动清理回收）: {}", e2);
                    Err(format!("临时目录清理失败: {}", e2))
                }
            }
        }
    }
}

/// 统计目录中的切片文件数量
pub fn count_slice_files(temp_dir: &std::path::Path) -> u32 {
    collect_slice_files(temp_dir).len() as u32
}

/// 扫描目录中的切片文件，返回 (起始秒数, 路径) 按起始时间排序
pub fn collect_slice_files(temp_dir: &std::path::Path) -> Vec<(f64, std::path::PathBuf)> {
    let mut files: Vec<(f64, std::path::PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(temp_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // 匹配 slice_{start}_{end}.wav
            if let Some(params) = name
                .strip_prefix("slice_")
                .and_then(|s| s.strip_suffix(".wav"))
            {
                if let Some(start_str) = params.split('_').next() {
                    if let Ok(start) = start_str.parse::<f64>() {
                        files.push((start, entry.path()));
                    }
                }
            }
        }
    }
    files.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    files
}

/// 读取切片 WAV 文件并编码为 data URL
pub fn encode_slice_to_data_url(slice_path: &std::path::Path) -> Result<String, AudioError> {
    let audio_bytes = std::fs::read(slice_path)?;
    let base64_str = base64::base64_encode(&audio_bytes);
    Ok(format!("data:audio/wav;base64,{}", base64_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化访问共享 app_temp_root 的测试（create_task_temp_dir 相关），
    /// 避免并发枚举快照互相干扰。
    static TEMP_ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 生成 16kHz/mono/16bit 静音 WAV（hound），返回路径
    fn write_silent_wav(path: &Path, seconds: u32) -> PathBuf {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("创建测试 WAV 失败");
        for _ in 0..(seconds * 16000) {
            writer.write_sample(0i16).expect("写入采样失败");
        }
        writer.finalize().expect("收尾 WAV 失败");
        path.to_path_buf()
    }

    /// B-06 验收：首片超限后收缩重试，旧切片必须被删除，
    /// count_slice_files 与实际分片数一致、无同起点重复文件
    #[test]
    fn shrink_retry_deletes_stale_slice_no_duplicate_start() {
        let root = test_root();
        let wav = write_silent_wav(&root.join("source.wav"), 20);
        let slices_dir = root.join("slices");
        std::fs::create_dir_all(&slices_dir).unwrap();

        // 20s 切片 ≈ 640KB → Base64 ≈ 853KB；上限 600KB 迫使收缩 20s → 10s（≈427KB 达标）
        let (_, bytes, used_sec) =
            slice_chunk_with_shrink(&wav, &slices_dir, 0.0, 20.0, 150, 600_000, "test")
                .expect("收缩重试后应成功");

        assert_eq!(used_sec, 10.0, "应收缩至 10s");
        assert!(!bytes.is_empty());
        // 关键断言：同起点只剩 1 个切片，旧的 slice_0.0_20.0.wav 已被删除
        let files = collect_slice_files(&slices_dir);
        assert_eq!(files.len(), 1, "同起点不得残留旧超限切片");
        assert_eq!(count_slice_files(&slices_dir), 1);
        assert_eq!(files[0].0, 0.0);
        assert!(
            !slices_dir.join("slice_0.0_20.0.wav").exists(),
            "超限旧切片必须被删除"
        );
        assert!(files[0].1.ends_with("slice_0.0_10.0.wav"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 未超限时不收缩、不误删：一次命中即返回
    #[test]
    fn shrink_retry_no_shrink_when_within_limit() {
        let root = test_root();
        let wav = write_silent_wav(&root.join("source.wav"), 20);
        let slices_dir = root.join("slices");
        std::fs::create_dir_all(&slices_dir).unwrap();

        let (_, _, used_sec) = slice_chunk_with_shrink(
            &wav,
            &slices_dir,
            0.0,
            20.0,
            150,
            constants::MAX_BASE64_BYTES,
            "test",
        )
        .expect("未超限应直接成功");
        assert_eq!(used_sec, 20.0, "未超限不得收缩");
        assert_eq!(count_slice_files(&slices_dir), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 收缩到 MIN_SLICE_SECONDS 以下仍超限：报错且不留同起点重复文件
    #[test]
    fn shrink_retry_gives_up_below_min_and_cleans_up() {
        let root = test_root();
        let wav = write_silent_wav(&root.join("source.wav"), 10);
        let slices_dir = root.join("slices");
        std::fs::create_dir_all(&slices_dir).unwrap();

        // 上限 1 字节：任何切片都超限，收缩到 < MIN_SLICE_SECONDS 后应报错
        let r = slice_chunk_with_shrink(&wav, &slices_dir, 0.0, 10.0, 150, 1, "test");
        assert!(matches!(r, Err(AudioError::Encoding(_))));
        // 失败路径上每次收缩前都已删除旧切片，不得残留同起点重复文件
        let files = collect_slice_files(&slices_dir);
        assert!(
            files.len() <= 1,
            "失败后不得残留多个同起点切片: {:?}",
            files
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 测试专用临时根（避免触碰真实应用根）
    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mimo_guard_test_{}_{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("创建测试根失败");
        root
    }

    #[test]
    fn guard_rejects_relative_and_dotdot_paths() {
        let root = test_root();
        assert!(verify_task_dir(Path::new("relative/dir"), &root).is_err());
        assert!(verify_task_dir(&root.join("../escape"), &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn guard_rejects_root_itself_and_outside_dirs() {
        let root = test_root();
        // 根目录本身拒绝
        assert!(verify_task_dir(&root, &root).is_err());
        // 根外普通目录拒绝
        let outside = std::env::temp_dir().join(format!(
            "mimo_guard_outside_{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        assert!(verify_task_dir(&outside, &root).is_err());
        // 伪造同名前缀目录（根外）同样拒绝
        let forged = std::env::temp_dir().join(format!("mimo_{}forged", std::process::id()));
        std::fs::create_dir_all(&forged).unwrap();
        assert!(verify_task_dir(&forged, &root).is_err());
        // 系统目录拒绝
        #[cfg(target_os = "windows")]
        assert!(verify_task_dir(Path::new("C:/Windows"), &root).is_err());
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&forged);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn guard_accepts_registered_task_dir() {
        let root = test_root();
        let task_dir = root.join("mimo_task_ok");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("slice_0.0_10.0.wav"), b"x").unwrap();
        let canon = verify_task_dir(&task_dir, &root).expect("真实任务目录应通过");
        assert!(canon.starts_with(root.canonicalize().unwrap()));
        assert_ne!(canon, root.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn guard_rejects_nonexistent_path() {
        let root = test_root();
        assert!(verify_task_dir(&root.join("not_created"), &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Windows 专项：根内 junction 指向根外目录，守卫必须拒绝且哨兵文件存活。
    /// junction 通过 mklink /J 创建（无需管理员）；创建失败时跳过并提示手工验证命令。
    #[cfg(target_os = "windows")]
    #[test]
    fn guard_rejects_junction_pointing_outside_root() {
        let root = test_root();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let outside = std::env::temp_dir().join(format!("mimo_junc_target_{}", stamp));
        std::fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("sentinel.txt");
        std::fs::write(&sentinel, b"sentinel").unwrap();

        let link = root.join(format!("mimo_junc_link_{}", stamp));
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output();
        let created = out.map(|o| o.status.success()).unwrap_or(false);
        if !created {
            eprintln!(
                "无法创建 junction（需管理员/开发者模式），手工验证命令: mklink /J {:?} {:?}",
                link, outside
            );
            let _ = std::fs::remove_dir_all(&outside);
            let _ = std::fs::remove_dir_all(&root);
            return;
        }

        // 守卫必须拒绝，绝不能跟随 junction 删除根外目录
        assert!(verify_task_dir(&link, &root).is_err());
        assert!(remove_task_dir_guarded_with_root(&link, &root).is_err());
        assert!(sentinel.exists(), "根外哨兵文件必须保持存在");

        // 手工清理：junction 本身用 remove_dir（不跟随）
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 内部测试用：指定根的守卫式删除
    fn remove_task_dir_guarded_with_root(dir: &Path, root: &Path) -> Result<(), String> {
        let canon = verify_task_dir(dir, root)?;
        std::fs::remove_dir_all(&canon).map_err(|e| e.to_string())
    }

    #[test]
    fn create_task_temp_dir_writes_meta() {
        let _guard = TEMP_ROOT_LOCK.lock().unwrap();
        let dir = create_task_temp_dir().expect("创建任务临时目录失败");
        assert!(dir.is_dir());
        assert!(dir.starts_with(app_temp_root()));
        let meta = read_task_meta(&dir).expect("元数据应存在");
        assert_eq!(meta.pid, std::process::id());
        assert!(meta.created_ms > 0);
        assert_eq!(meta.schema_version, SCHEMA_VERSION);
        assert_eq!(meta.task_generation, 0);
        assert_eq!(meta.run_id, "");
        // 守卫校验应通过
        assert!(verify_app_task_dir(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P4-03：旧版（无 schema_version/task_generation/run_id）元数据应可被
    /// 启动清理正常读取，且新字段取默认值（向后兼容）。
    #[test]
    fn read_task_meta_tolerates_legacy_without_new_fields() {
        let root = test_root();
        let dir = root.join("legacy_task");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TASK_META_FILE), r#"{"pid":123,"created_ms":456}"#).unwrap();
        let meta = read_task_meta(&dir).expect("旧元数据应可解析");
        assert_eq!(meta.pid, 123);
        assert_eq!(meta.created_ms, 456);
        assert_eq!(meta.schema_version, SCHEMA_VERSION);
        assert_eq!(meta.task_generation, 0);
        assert_eq!(meta.run_id, "");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 统计应用临时根下的任务目录名（用于断言失败后无残留）。
    fn app_temp_dir_names() -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(app_temp_root()) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    names.push(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
        names.sort();
        names
    }

    /// P4-03：元数据原子写任一阶段失败时，create_task_temp_dir 必须回滚——
    /// 返回错误且不残留刚创建的目录。复用 atomic 域故障注入（WriteAll/Flush/
    /// SyncAll/ReplaceMain 各验证一次）。
    #[test]
    fn create_task_temp_dir_removes_dir_on_meta_write_failure() {
        use crate::atomic::FaultPoint;
        let _guard = TEMP_ROOT_LOCK.lock().unwrap();
        for point in [
            FaultPoint::WriteAll,
            FaultPoint::Flush,
            FaultPoint::SyncAll,
            FaultPoint::ReplaceMain,
        ] {
            let before = app_temp_dir_names();
            crate::atomic::set_fail_at(point);
            assert!(
                create_task_temp_dir().is_err(),
                "元数据写失败（{:?}）应返回错误",
                point
            );
            let after = app_temp_dir_names();
            assert_eq!(
                after, before,
                "元数据写失败（{:?}）后不得残留刚创建的任务目录",
                point
            );
        }
    }
}

// base64 测试已随 B-10 迁移至 base64.rs 的 #[cfg(test)] 模块
