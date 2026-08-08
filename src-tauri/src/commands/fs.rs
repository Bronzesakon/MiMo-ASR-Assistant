//! 文件系统/媒体域命令（清单 B-11 自 lib.rs 拆出）：
//! get_audio_duration、export_file、cleanup_file_temp、open_log_dir。

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::state::FfmpegBinState;
use crate::{audio, wav_codec};

/// 获取音频时长
///
/// 优先走 Rust 纯解析（任意 PCM WAV，无需 ffmpeg）；非 WAV 或解析失败时
/// 回退到捆绑 ffmpeg.exe 探测（state 为 None 或探测失败返回 Err，前端兜底 duration=0）。
#[tauri::command]
pub async fn get_audio_duration(handle: AppHandle, file_path: String) -> Result<f64, String> {
    let path = PathBuf::from(&file_path);
    // WAV 快速路径：fmt/data chunk 直读精确计算（覆盖目标 16kHz WAV 与其他 PCM WAV）
    if let Ok(d) = wav_codec::get_duration(&path) {
        return Ok(d);
    }
    // 非 WAV（视频/压缩音频等）：从 managed state 取 ffmpeg 句柄探测
    let bin = match handle.state::<FfmpegBinState>().0.clone() {
        Some(b) => b,
        None => return Err("ffmpeg 组件不可用，无法获取媒体时长".to_string()),
    };
    // probe_duration 会拉起子进程（最长 10s），放阻塞线程避免卡 UI
    tokio::task::spawn_blocking(move || bin.probe_duration(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// 导出格式枚举（B-04：格式必须由结构化枚举决定，仅允许 .txt / .md）
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Txt,
    Md,
}

impl ExportFormat {
    fn ext(&self) -> &'static str {
        match self {
            ExportFormat::Txt => "txt",
            ExportFormat::Md => "md",
        }
    }
}

/// 判断文件名是否为 Windows 保留设备名（H-02）。
/// 规则：去扩展名并 trim 尾部点/空格后的 stem 做 ASCII 大写，精确匹配
/// CON/PRN/AUX/NUL/CLOCK$、COM1..COM9、LPT1..LPT9（含 CON.txt 这类带扩展名形式）。
/// 普通包含这些字串但不完全相等的名称（如 conic.txt、COM10）应允许。
fn is_reserved_device_name(file_name: &str) -> bool {
    let stem = match file_name.rsplit_once('.') {
        Some((s, _)) => s,
        None => file_name,
    }
    .trim()
    .trim_end_matches([' ', '.']);
    if stem.is_empty() {
        return true;
    }
    const RESERVED: [&str; 23] = [
        "CON", "PRN", "AUX", "NUL", "CLOCK$", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
        "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
        "LPT9",
    ];
    RESERVED.contains(&stem.to_ascii_uppercase().as_str())
}

/// 校验并归一化建议文件名：只保留文件名部分，拒绝空名/非法字符/保留设备名，强制目标扩展名
fn sanitize_export_name(suggested: &str, format: &ExportFormat) -> Result<String, String> {
    let file_part = Path::new(suggested)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let stem = match file_part.rsplit_once('.') {
        Some((s, _)) => s,
        None => file_part,
    }
    .trim();
    if stem.is_empty() || stem == "." || stem == ".." {
        return Err("建议文件名无效".to_string());
    }
    if stem.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return Err("建议文件名含非法字符".to_string());
    }
    if is_reserved_device_name(file_part) {
        return Err("建议文件名是系统保留设备名".to_string());
    }
    Ok(format!("{}.{}", stem, format.ext()))
}

/// 导出文件（B-04 方案 A）：保存对话框与文件写入都在后端完成。
/// 前端不再把绝对目标路径传给独立写文件命令；用户取消返回 Ok(None)，
/// 前端不创建文件也不报错。写入失败返回用户可理解的错误，不记录导出正文。
#[tauri::command]
pub async fn export_file(
    handle: AppHandle,
    content: String,
    suggested_name: String,
    format: ExportFormat,
) -> Result<Option<String>, String> {
    let file_name = sanitize_export_name(&suggested_name, &format)?;
    let ext = format.ext();

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<PathBuf>>();
    handle
        .dialog()
        .file()
        .set_title("导出文件")
        .set_file_name(&file_name)
        .add_filter("文本文件", &[ext])
        .save_file(move |path| {
            let _ = tx.send(path.and_then(|p| p.into_path().ok()));
        });
    let chosen = rx.await.map_err(|_| "保存对话框通信异常".to_string())?;
    let Some(path) = chosen else {
        // 用户取消：不创建文件且不报错
        return Ok(None);
    };

    // 对话框后二次校验：扩展名必须与格式枚举一致，拒绝目录路径（H-02：最终文件名不得是保留设备名）
    let path_ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !path_ext.eq_ignore_ascii_case(ext) {
        return Err(format!("仅允许导出 .{} 文件", ext));
    }
    if path.is_dir() {
        return Err("目标路径是目录，无法写入".to_string());
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if is_reserved_device_name(name) {
            return Err("目标文件名是系统保留设备名".to_string());
        }
    }

    std::fs::write(&path, &content).map_err(|e| format!("写入文件失败: {}", e))?;
    crate::info!("导出成功: .{} 格式, {} 字符", ext, content.chars().count());
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// 查询日志状态（P7-01 只读）：文件侧健康状态 + 目录内日志文件数与总字节数，
/// 供设置页展示（返回 `{ healthy, fileCount, totalBytes }`）。
#[tauri::command]
pub fn get_log_status(handle: AppHandle) -> crate::log::LogStatus {
    let log_dir = crate::config::log_dir_for(&handle);
    crate::log::log_status(&log_dir)
}

/// 打开日志目录（B-09：与日志写入目录同步，指向 app_log_dir）
#[tauri::command]
pub fn open_log_dir(handle: AppHandle) -> Result<(), String> {
    let log_dir = crate::config::log_dir_for(&handle);
    if !log_dir.exists() {
        std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&log_dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    // I-03 策略 A：非 Windows 不受支持，返回明确错误（实际构建会被 lib.rs 的
    // compile_error! 拦截，此分支仅为保持函数在任意平台语义自洽）
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &log_dir;
        return Err("平台不受支持：打开日志目录仅 Windows 可用".to_string());
    }
    Ok(())
}

// 供 lib.rs 启动清理复用的内部实现保留在音频域（audio::app_temp_root 等）。

/// Windows：检查指定 PID 是否存活（OpenProcess FFI，无新增依赖）。
/// ERROR_ACCESS_DENIED 说明进程存在但无权打开，同样视为存活。
#[cfg(target_os = "windows")]
fn is_pid_alive(pid: u32) -> bool {
    extern "system" {
        fn OpenProcess(
            desired_access: u32,
            inherit_handle: i32,
            process_id: u32,
        ) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn GetLastError() -> u32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_ACCESS_DENIED: u32 = 5;
    if pid == 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        CloseHandle(h);
        true
    }
}

#[cfg(not(target_os = "windows"))]
fn is_pid_alive(_pid: u32) -> bool {
    // 非 Windows 平台无轻量存活检查，仅凭创建时间判断（当前产品为 Windows-only）
    false
}

/// 启动时清理应用专属临时根中的遗留任务目录（崩溃/异常退出残留，B-04）。
/// 只扫描 app_temp_root，不再无差别删除系统 temp 的全部 mimo_* 目录；
/// 仅删除确认属于本应用（含元数据）、早于本次启动且不属于其它存活实例的目录。
/// T-04：删除前复用完整路径守卫（canonicalize + junction/reparse point 检查），
/// 禁止仅凭元数据/名称前缀直接递归删除。
pub fn cleanup_legacy_temp() {
    let root = audio::app_temp_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return, // 根不存在则无遗留可清
    };
    let startup_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let current_pid = std::process::id();
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta = match audio::read_task_meta(&path) {
            Some(m) => m,
            None => continue, // 无元数据无法确认属于本应用，不删除
        };
        if meta.pid == current_pid {
            continue;
        }
        if meta.created_ms >= startup_ms {
            continue; // 本次启动后创建，不属于遗留
        }
        if is_pid_alive(meta.pid) {
            continue; // 可能属于其它存活实例，不删除
        }
        // T-04：守卫式删除（完整路径守卫 + 删除），失败不阻断启动
        if let Err(e) = audio::remove_task_dir_guarded(&path) {
            crate::warn!("遗留临时目录清理失败: {}", e);
            continue;
        }
        removed += 1;
    }
    if removed > 0 {
        crate::info!("启动清理: 已删除 {} 个遗留任务临时目录", removed);
    }
}

/// WebView2 缓存清理
/// T-04：`.mimo_cache` 仅删除普通目录（拒绝 junction/符号链接）；
/// 旧版 `%TEMP%/mimo_audio` 无元数据可证归属，只记录迁移提示，不自动删除。
pub fn clear_webview2_cache() {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let cache_dir = exe_dir.join(".mimo_cache");
            if cache_dir.exists() && !audio::is_reparse_point_pub(&cache_dir) && cache_dir.is_dir()
            {
                let _ = std::fs::remove_dir_all(&cache_dir);
            }
        }
    }
    let temp_audio = std::env::temp_dir().join("mimo_audio");
    if temp_audio.exists() {
        crate::info!(
            "检测到旧版临时目录 %TEMP%/mimo_audio（无归属元数据），保留不删除，可手动清理"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // H-02：Windows 保留设备名表驱动测试
    #[test]
    fn reserved_device_names_rejected() {
        for name in [
            "CON",
            "con",
            "CON.txt",
            "con.md",
            "PRN",
            "AUX",
            "NUL",
            "NUL.txt",
            "CLOCK$",
            "COM1",
            "com3.txt",
            "COM9",
            "LPT1",
            "lpt7.md",
            "LPT9",
            "CON ",
            "CON.",
            "con. .txt",
        ] {
            assert!(is_reserved_device_name(name), "{name:?} 应被拒绝");
        }
    }

    #[test]
    fn ordinary_names_allowed() {
        for name in [
            "notes",
            "notes.txt",
            "conic.txt",
            "console",
            "COM10",
            "COM10.txt",
            "LPT10",
            "my_com1_file.txt",
            "normal 文件名.md",
            "a-con-b",
        ] {
            assert!(!is_reserved_device_name(name), "{name:?} 应被允许");
        }
    }

    #[test]
    fn sanitize_rejects_reserved_names() {
        let txt = ExportFormat::Txt;
        assert!(sanitize_export_name("CON", &txt).is_err());
        assert!(sanitize_export_name("con.txt", &txt).is_err());
        assert!(sanitize_export_name("lpt2.md", &txt).is_err());
        assert!(sanitize_export_name("com4", &txt).is_err());
        assert!(sanitize_export_name("aux.log", &txt).is_err());
    }

    #[test]
    fn sanitize_allows_ordinary_names() {
        let txt = ExportFormat::Txt;
        assert_eq!(
            sanitize_export_name("会议记录", &txt).unwrap(),
            "会议记录.txt"
        );
        assert_eq!(sanitize_export_name("notes", &txt).unwrap(), "notes.txt");
        assert_eq!(
            sanitize_export_name("conic.txt", &txt).unwrap(),
            "conic.txt"
        );
        assert_eq!(sanitize_export_name("COM10", &txt).unwrap(), "COM10.txt");
    }
}
