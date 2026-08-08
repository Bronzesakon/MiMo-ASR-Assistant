// ffmpeg_exe.rs — 捆绑精简 ffmpeg.exe 的运行时封装
//
// 该 ffmpeg.exe（FFmpeg 9.0 精简版）仅支持「输入容器 → 提取音轨 → 16kHz/mono/16bit WAV」，
// encoder/muxer 只有 pcm_s16le + wav，无 ffprobe，无网络能力。
//
// 本模块职责：
//   1. resolve：定位 exe（安装态 resource_dir 两级探测 + dev 模式回退），
//      并做运行时 SHA-256 完整性校验（自包含实现，不引任何新依赖）。
//   2. convert_to_wav：参数数组直传（无 shell），stderr 独立线程抽干（限长尾缓冲），
//      主线程轮询 try_wait + 取消/超时检查，Windows 上 taskkill /T 杀进程树。
//   3. probe_duration：无输出文件的探测调用，从 stderr 手写解析 Duration 行。
//
// 零 panic 纪律：全 Result、from_utf8_lossy / 字符边界安全截取、索引先查长度。
// 注意：本模块接线（tauri command 注册等）由后续任务完成，此处暂允许 dead_code。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ============================================================
// 错误类型
// ============================================================

/// ffmpeg.exe 封装错误（B-07：thiserror 统一错误体系，Display 文案不变）
#[derive(Debug, thiserror::Error)]
pub enum FfmpegExeError {
    /// exe 定位失败（所有候选路径都不存在 / 平台不支持）
    #[error("ffmpeg.exe 未找到: {0}")]
    BinMissing(String),
    /// 运行时 SHA-256 校验失败（文件损坏或被篡改）
    #[error("ffmpeg.exe 完整性校验失败: {0}")]
    BinCorrupt(String),
    /// 进程启动失败
    #[error("ffmpeg.exe 启动失败: {0}")]
    SpawnFailed(String),
    /// ffmpeg 非零退出
    #[error("ffmpeg 异常退出（退出码 {code}）: {stderr_tail}")]
    NonZeroExit { code: i32, stderr_tail: String },
    /// 输出 WAV 无效（不存在 / 过小 / 头错误 / 时长为 0）
    #[error("无效输出: {0}")]
    OutputInvalid(String),
    /// 执行超时
    #[error("ffmpeg 处理超时")]
    Timeout,
    /// 被取消
    #[error("任务已取消")]
    Cancelled,
    /// 输入格式不被支持（尽力而归一）
    #[error("不支持的输入格式: {0}")]
    UnsupportedFormat(String),
}

// ============================================================
// 公开 API
// ============================================================

/// 捆绑 ffmpeg.exe 的句柄
#[derive(Clone)]
pub struct FfmpegBin {
    path: PathBuf,
}

impl FfmpegBin {
    /// 定位捆绑的 ffmpeg.exe 并完成运行时完整性校验。
    ///
    /// 候选顺序：
    ///   1. resource_dir()/resources/ffmpeg.exe（Tauri v2 安装态 resources 子目录）
    ///   2. resource_dir()/ffmpeg.exe（安装态平铺兜底）
    ///   3. CARGO_MANIFEST_DIR/resources/ffmpeg.exe（dev 模式回退，编译期路径）
    pub fn resolve(app: &tauri::AppHandle) -> Result<Self, FfmpegExeError> {
        #[cfg(target_os = "windows")]
        {
            use tauri::Manager;

            // 候选路径附带来源标签：成功日志只输出标签，不输出绝对资源路径（T-07）
            let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
            if let Ok(rd) = app.path().resource_dir() {
                candidates.push((
                    rd.join("resources").join("ffmpeg.exe"),
                    "resource_dir/resources",
                ));
                candidates.push((rd.join("ffmpeg.exe"), "resource_dir"));
            }
            // 仅 debug 构建允许从源码目录回退；release 安装态不得依赖 checkout 绝对路径
            #[cfg(debug_assertions)]
            candidates.push((
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("resources")
                    .join("ffmpeg.exe"),
                "dev source",
            ));

            let (exe, source) = candidates
                .iter()
                .find(|(p, _)| p.is_file())
                .cloned()
                .ok_or_else(|| {
                    FfmpegExeError::BinMissing(format!(
                        "已尝试: {}",
                        candidates
                            .iter()
                            .map(|(p, _)| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("; ")
                    ))
                })?;

            verify_integrity(&exe)?;
            crate::info!("ffmpeg.exe 定位成功并通过完整性校验（来源: {}）", source);
            Ok(FfmpegBin { path: exe })
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = app;
            Err(FfmpegExeError::BinMissing(
                "捆绑 ffmpeg.exe 仅支持 Windows 平台".to_string(),
            ))
        }
    }

    /// exe 绝对路径
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 将输入文件转换为 16kHz / mono / 16bit WAV。
    ///
    /// cancel 命中或超时（60s + 1s/MB 输入大小，封顶 600s）时杀进程树并返回对应错误。
    pub fn convert_to_wav(
        &self,
        input: &Path,
        output: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), FfmpegExeError> {
        if let Some(parent) = output.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut cmd = Command::new(&self.path);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
            .arg(input)
            .args([
                "-vn",
                "-map",
                "0:a:0",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "pcm_s16le",
                "-map_metadata",
                "-1",
            ])
            .arg(output)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x08000000);

        let mut child = cmd
            .spawn()
            .map_err(|e| FfmpegExeError::SpawnFailed(e.to_string()))?;

        // 独立线程持续抽干 stderr，保留尾部约 8KB（防管道写满死锁 + 防内存膨胀）
        let stderr_pipe = child.stderr.take();
        let drainer = std::thread::spawn(move || drain_stream(stderr_pipe, CONVERT_STDERR_CAP));

        let timeout = convert_timeout(input);
        let start = Instant::now();

        let status = loop {
            if cancel.load(Ordering::Acquire) {
                kill_tree(&mut child);
                let _ = drainer.join();
                cleanup_output(output);
                return Err(FfmpegExeError::Cancelled);
            }
            if start.elapsed() > timeout {
                kill_tree(&mut child);
                let _ = drainer.join();
                cleanup_output(output);
                return Err(FfmpegExeError::Timeout);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(e) => {
                    kill_tree(&mut child);
                    let _ = drainer.join();
                    cleanup_output(output);
                    return Err(FfmpegExeError::SpawnFailed(format!(
                        "等待 ffmpeg 进程失败: {}",
                        e
                    )));
                }
            }
        };

        let stderr_bytes: Vec<u8> = drainer.join().unwrap_or_default();
        if !stderr_bytes.is_empty() {
            crate::info!(
                "ffmpeg 转码 stderr: {}",
                stderr_tail(&stderr_bytes, CONVERT_STDERR_CAP)
            );
        }

        let code = status.code().unwrap_or(-1);
        if code != 0 {
            cleanup_output(output);
            let tail = stderr_tail(&stderr_bytes, STDERR_TAIL_REPORT);
            if looks_unsupported(&stderr_bytes) {
                return Err(FfmpegExeError::UnsupportedFormat(tail));
            }
            return Err(FfmpegExeError::NonZeroExit {
                code,
                stderr_tail: tail,
            });
        }

        // 退出码 0 不代表产物可用（无音轨视频会产出空 WAV），必须结构校验
        // OutputInvalid 出口同步清理半成品输出，保持所有 Err 出口清理职责自洽
        if let Err(e) = validate_wav_output(output) {
            cleanup_output(output);
            return Err(e);
        }
        Ok(())
    }

    /// 探测输入媒体时长（秒）。
    ///
    /// 运行 `ffmpeg -hide_banner -i <input>`（无输出文件，必然非零退出，属正常现象），
    /// 从 stderr 解析 `Duration: HH:MM:SS.xx`。独立超时 10s。
    pub fn probe_duration(&self, input: &Path) -> Result<f64, FfmpegExeError> {
        let mut cmd = Command::new(&self.path);
        cmd.args(["-hide_banner", "-nostdin", "-i"])
            .arg(input)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x08000000);

        let mut child = cmd
            .spawn()
            .map_err(|e| FfmpegExeError::SpawnFailed(e.to_string()))?;

        let stderr_pipe = child.stderr.take();
        let drainer = std::thread::spawn(move || drain_stream(stderr_pipe, PROBE_STDERR_CAP));

        let start = Instant::now();
        loop {
            if start.elapsed() > PROBE_TIMEOUT {
                kill_tree(&mut child);
                let _ = drainer.join();
                return Err(FfmpegExeError::Timeout);
            }
            match child.try_wait() {
                // 探测调用必然非零退出（没有输出文件），这里不关心退出码
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(e) => {
                    kill_tree(&mut child);
                    let _ = drainer.join();
                    return Err(FfmpegExeError::SpawnFailed(format!(
                        "等待 ffmpeg 进程失败: {}",
                        e
                    )));
                }
            }
        }

        let stderr_bytes: Vec<u8> = drainer.join().unwrap_or_default();
        let text = String::from_utf8_lossy(&stderr_bytes);
        match parse_duration_str(&text) {
            Some(d) if d > 0.0 => Ok(d),
            _ => {
                if looks_unsupported(&stderr_bytes) {
                    Err(FfmpegExeError::UnsupportedFormat(stderr_tail(
                        &stderr_bytes,
                        STDERR_TAIL_REPORT,
                    )))
                } else {
                    Err(FfmpegExeError::OutputInvalid(
                        "无法从 ffmpeg 输出中解析媒体时长".to_string(),
                    ))
                }
            }
        }
    }
}

// ============================================================
// 常量
// ============================================================

/// convert stderr 尾缓冲上限（约 8KB）
const CONVERT_STDERR_CAP: usize = 8 * 1024;
/// probe stderr 缓冲上限（64KB）
const PROBE_STDERR_CAP: usize = 64 * 1024;
/// 错误报告中附带的 stderr 尾部长度
const STDERR_TAIL_REPORT: usize = 500;
/// try_wait 轮询间隔（清单 B-11：集中至 constants）
const POLL_INTERVAL: Duration = Duration::from_millis(crate::constants::FFMPEG_POLL_INTERVAL_MS);
/// probe 独立超时
const PROBE_TIMEOUT: Duration = Duration::from_secs(crate::constants::FFMPEG_PROBE_TIMEOUT_SECS);

// ============================================================
// 完整性校验（运行时 SHA-256，自包含实现）
// ============================================================

/// 对 exe 做运行时 SHA-256 完整性校验。
///
/// fail-closed（T-07）：基准文件缺失、不可读、格式非法、哈希不匹配均返回错误，
/// 不允许在无法证明 exe 完整性时继续使用。
fn verify_integrity(exe: &Path) -> Result<(), FfmpegExeError> {
    let sum_path = match exe.file_name() {
        Some(name) => {
            let mut sum_name = name.to_os_string();
            sum_name.push(".sha256");
            match exe.parent() {
                Some(dir) => dir.join(sum_name),
                None => PathBuf::from(sum_name),
            }
        }
        None => {
            return Err(FfmpegExeError::BinMissing(
                "ffmpeg.exe 路径无文件名，无法执行完整性校验".to_string(),
            ));
        }
    };

    let expected = match fs::read_to_string(&sum_path) {
        Ok(text) => match extract_hex64(&text) {
            Some(h) => h.to_ascii_uppercase(),
            None => {
                return Err(FfmpegExeError::BinCorrupt(format!(
                    "sha256 基准文件无有效 64 位十六进制 token: {}",
                    sum_path.display()
                )));
            }
        },
        Err(e) => {
            return Err(FfmpegExeError::BinMissing(format!(
                "sha256 基准文件缺失或不可读: {} ({})",
                sum_path.display(),
                e
            )));
        }
    };

    let actual = sha256_file(exe).map_err(|e| {
        FfmpegExeError::BinCorrupt(format!("无法计算 {} 的哈希: {}", exe.display(), e))
    })?;

    if actual != expected {
        return Err(FfmpegExeError::BinCorrupt(format!(
            "{} 哈希不匹配（期望 {}，实际 {}），文件可能损坏或被篡改",
            exe.display(),
            expected,
            actual
        )));
    }
    Ok(())
}

/// 扫描文本，返回第一个长度恰为 64 的连续十六进制串。
fn extract_hex64(text: &str) -> Option<String> {
    let mut run = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_hexdigit() {
            run.push(ch);
        } else if !run.is_empty() {
            if run.len() == 64 {
                return Some(run);
            }
            run.clear();
        }
    }
    None
}

/// 流式计算文件 SHA-256，返回大写十六进制字符串。
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("无法打开 {}: {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("读取 {} 失败: {}", path.display(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02X}")).collect())
}

// ---- 内联纯 Rust SHA-256 实现（自包含，不引依赖；与 build.rs 构建期实现同源） ----

struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Sha256 {
            state: [
                0x6A09_E667,
                0xBB67_AE85,
                0x3C6E_F372,
                0xA54F_F53A,
                0x510E_527F,
                0x9B05_688C,
                0x1F83_D9AB,
                0x5BE0_CD19,
            ],
            buf: [0; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;
        // 先把内部缓冲区填满一个完整块
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        // 整块压缩
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        // 尾部余数追加到缓冲区（保留已有缓冲内容）
        self.buf[self.buf_len..self.buf_len + data.len()].copy_from_slice(data);
        self.buf_len += data.len();
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        let pad = if self.buf_len <= 56 {
            56 - self.buf_len
        } else {
            64 + 56 - self.buf_len
        };
        let zeros = vec![0u8; pad];
        self.update(&zeros);
        self.update(&bit_len.to_be_bytes());
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a_2f98,
            0x7137_4491,
            0xb5c0_fbcf,
            0xe9b5_dba5,
            0x3956_c25b,
            0x59f1_11f1,
            0x923f_82a4,
            0xab1c_5ed5,
            0xd807_aa98,
            0x1283_5b01,
            0x2431_85be,
            0x550c_7dc3,
            0x72be_5d74,
            0x80de_b1fe,
            0x9bdc_06a7,
            0xc19b_f174,
            0xe49b_69c1,
            0xefbe_4786,
            0x0fc1_9dc6,
            0x240c_a1cc,
            0x2de9_2c6f,
            0x4a74_84aa,
            0x5cb0_a9dc,
            0x76f9_88da,
            0x983e_5152,
            0xa831_c66d,
            0xb003_27c8,
            0xbf59_7fc7,
            0xc6e0_0bf3,
            0xd5a7_9147,
            0x06ca_6351,
            0x1429_2967,
            0x27b7_0a85,
            0x2e1b_2138,
            0x4d2c_6dfc,
            0x5338_0d13,
            0x650a_7354,
            0x766a_0abb,
            0x81c2_c92e,
            0x9272_2c85,
            0xa2bf_e8a1,
            0xa81a_664b,
            0xc24b_8b70,
            0xc76c_51a3,
            0xd192_e819,
            0xd699_0624,
            0xf40e_3585,
            0x106a_a070,
            0x19a4_c116,
            0x1e37_6c08,
            0x2748_774c,
            0x34b0_bcb5,
            0x391c_0cb3,
            0x4ed8_aa4a,
            0x5b9c_ca4f,
            0x682e_6ff3,
            0x748f_82ee,
            0x78a5_636f,
            0x84c8_7814,
            0x8cc7_0208,
            0x90be_fffa,
            0xa450_6ceb,
            0xbef9_a3f7,
            0xc671_78f2,
        ];
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
            self.state[0],
            self.state[1],
            self.state[2],
            self.state[3],
            self.state[4],
            self.state[5],
            self.state[6],
            self.state[7],
        );
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, val) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(val);
        }
    }
}

// ============================================================
// 进程管理辅助
// ============================================================

/// 杀进程树并回收子进程。
///
/// Windows 上 child.kill() 只杀直接子进程，用 taskkill /F /T /PID 兜底杀整棵树，
/// 随后 kill()（幂等兜底）+ wait() 回收。
fn kill_tree(child: &mut Child) {
    let pid = child.id();
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 独立线程读取管道流到限长尾缓冲（保留最后 cap 字节，防管道满死锁）。
fn drain_stream(mut stream: Option<std::process::ChildStderr>, cap: usize) -> Vec<u8> {
    let mut buf: VecDeque<u8> = VecDeque::new();
    if let Some(ref mut s) = stream {
        let mut tmp = [0u8; 4096];
        loop {
            match s.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend(&tmp[..n]);
                    if buf.len() > cap {
                        let excess = buf.len() - cap;
                        buf.drain(..excess);
                    }
                }
                Err(_) => break,
            }
        }
    }
    buf.make_contiguous().to_vec()
}

/// convert 超时：60s + 1s/MB(输入文件大小)，封顶 600s。
fn convert_timeout(input: &Path) -> Duration {
    let bytes = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    Duration::from_secs(timeout_secs_for_size(bytes))
}

/// 超时秒数折算（可单测）：基础值 + MB 数，封顶。常量见 constants（清单 B-11）
fn timeout_secs_for_size(bytes: u64) -> u64 {
    const MB: u64 = 1024 * 1024;
    (crate::constants::FFMPEG_CONVERT_TIMEOUT_BASE_SECS + bytes / MB)
        .min(crate::constants::FFMPEG_CONVERT_TIMEOUT_CAP_SECS)
}

/// 失败路径清理半成品输出文件（尽力而为）。
fn cleanup_output(output: &Path) {
    let _ = fs::remove_file(output);
}

/// stderr 中是否含典型「格式无效/不支持」关键字（尽力归一为 UnsupportedFormat）。
fn looks_unsupported(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr);
    let lower = text.to_ascii_lowercase();
    [
        "invalid data found",
        "not supported",
        "unrecognized file type",
        "moov atom not found",
        "invalid file format",
        "did not contain any streams",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

/// 字符边界安全的 stderr 尾部截取：先 from_utf8_lossy，再按字符边界回退取尾。
fn stderr_tail(bytes: &[u8], max_len: usize) -> String {
    let lossy = String::from_utf8_lossy(bytes);
    if lossy.len() <= max_len {
        return lossy.into_owned();
    }
    let mut start = lossy.len() - max_len;
    while start < lossy.len() && !lossy.is_char_boundary(start) {
        start += 1;
    }
    lossy[start..].to_string()
}

// ============================================================
// Duration 解析（手写查找，不引 regex）
// ============================================================

/// 从 ffmpeg stderr 文本中解析 `Duration: HH:MM:SS.xx`。
/// `N/A`、缺失、格式异常一律返回 None。
fn parse_duration_str(text: &str) -> Option<f64> {
    let idx = text.find("Duration:")?;
    let rest = &text[idx + "Duration:".len()..];
    let rest = rest.trim_start();
    if rest.starts_with("N/A") {
        return None;
    }
    // 收集连续的数字 / ':' / '.' 字符
    let token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':' || *c == '.')
        .collect();
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let s: f64 = parts[2].parse().ok()?;
    if m >= 60.0 || s >= 60.0 {
        return None;
    }
    let total = h * 3600.0 + m * 60.0 + s;
    if total.is_finite() && total > 0.0 {
        Some(total)
    } else {
        None
    }
}

// ============================================================
// 输出 WAV 校验
// ============================================================

/// 校验输出 WAV：存在、>44 字节、RIFF/WAVE 头、fmt/data chunk 可算出时长 >0。
fn validate_wav_output(path: &Path) -> Result<(), FfmpegExeError> {
    let meta = fs::metadata(path).map_err(|_| {
        FfmpegExeError::OutputInvalid(format!("输出文件不存在: {}", path.display()))
    })?;
    if meta.len() <= 44 {
        return Err(FfmpegExeError::OutputInvalid(format!(
            "输出 WAV 过小（{} 字节），可能无音轨",
            meta.len()
        )));
    }

    let mut f = File::open(path)
        .map_err(|e| FfmpegExeError::OutputInvalid(format!("无法打开输出文件: {}", e)))?;

    // 前 12 字节：RIFF ???? WAVE
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr)
        .map_err(|_| FfmpegExeError::OutputInvalid("输出文件头部不完整".to_string()))?;
    if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
        return Err(FfmpegExeError::OutputInvalid(
            "输出文件不是有效的 RIFF/WAVE".to_string(),
        ));
    }

    // 逐 chunk 扫描 fmt / data
    let mut byte_rate: Option<u32> = None;
    let mut iterations = 0usize;
    loop {
        iterations += 1;
        if iterations > 4096 {
            return Err(FfmpegExeError::OutputInvalid(
                "WAV chunk 数量异常".to_string(),
            ));
        }
        let mut ch_hdr = [0u8; 8];
        match f.read_exact(&mut ch_hdr) {
            Ok(_) => {}
            Err(_) => break,
        }
        let chunk_size = u32::from_le_bytes([ch_hdr[4], ch_hdr[5], ch_hdr[6], ch_hdr[7]]) as u64;

        if &ch_hdr[0..4] == b"fmt " {
            // fmt 体：audio_format(2) channels(2) sample_rate(4) byte_rate(4) ...
            if (16..=4096).contains(&chunk_size) {
                let mut body = vec![0u8; chunk_size as usize];
                if f.read_exact(&mut body).is_ok() && body.len() >= 12 {
                    byte_rate = Some(u32::from_le_bytes([body[8], body[9], body[10], body[11]]));
                }
            } else {
                // 尺寸异常，跳过 chunk 体
                let skip = chunk_size + chunk_size % 2;
                if f.seek(SeekFrom::Current(skip as i64)).is_err() {
                    break;
                }
            }
        } else if &ch_hdr[0..4] == b"data" {
            let rate = byte_rate.unwrap_or(0);
            if rate == 0 {
                return Err(FfmpegExeError::OutputInvalid(
                    "WAV 缺少有效 fmt 信息（byte_rate=0）".to_string(),
                ));
            }
            let duration = chunk_size as f64 / rate as f64;
            if duration > 0.0 && duration.is_finite() {
                return Ok(());
            }
            return Err(FfmpegExeError::OutputInvalid(
                "输出 WAV 音轨时长为 0（输入可能无音轨）".to_string(),
            ));
        } else {
            let skip = chunk_size + chunk_size % 2;
            if f.seek(SeekFrom::Current(skip as i64)).is_err() {
                break;
            }
        }
    }

    Err(FfmpegExeError::OutputInvalid(
        "输出 WAV 中未找到 fmt/data 块".to_string(),
    ))
}

// ============================================================
// 单元测试（不依赖真实 exe）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- SHA-256 KAT ----

    fn sha256_hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha256_kat_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_kat_empty() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_kat_multi_block() {
        // 跨块更新（>64 字节，分多次 update）
        let mut h = Sha256::new();
        h.update(b"a".repeat(100).as_slice());
        h.update(b"b".repeat(100).as_slice());
        let mut whole = Sha256::new();
        let mut all = vec![b'a'; 100];
        all.extend(std::iter::repeat_n(b'b', 100));
        whole.update(&all);
        assert_eq!(h.finalize(), whole.finalize());
    }

    #[test]
    fn extract_hex64_basic() {
        let text =
            "  BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD *ffmpeg.exe\n";
        assert_eq!(
            extract_hex64(text).map(|t| t.to_ascii_lowercase()),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string())
        );
        assert_eq!(extract_hex64("no hash here"), None);
        // 63 位不算
        assert_eq!(extract_hex64(&"a".repeat(63)), None);
    }

    // ---- verify_integrity fail-closed（T-07） ----

    /// 每个调用返回唯一临时目录，避免并行测试竞争同一路径互相覆盖。
    fn tmp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "mimo_ffmpeg_integrity_{}_{}_{}",
            std::process::id(),
            uniq,
            n
        ));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// 返回 (临时目录, exe 路径)，目录由各测试保存并用于清理。
    fn tmp_exe() -> (PathBuf, PathBuf) {
        let dir = tmp_dir();
        let exe = dir.join("ffmpeg.exe");
        fs::write(&exe, b"fake exe bytes").unwrap();
        (dir, exe)
    }

    #[test]
    fn verify_integrity_missing_baseline_is_error() {
        let (dir, exe) = tmp_exe();
        // 无 .sha256 基准 → fail-closed
        assert!(matches!(
            verify_integrity(&exe),
            Err(FfmpegExeError::BinMissing(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_integrity_invalid_baseline_is_error() {
        let (dir, exe) = tmp_exe();
        fs::write(dir.join("ffmpeg.exe.sha256"), "not-a-hash").unwrap();
        assert!(matches!(
            verify_integrity(&exe),
            Err(FfmpegExeError::BinCorrupt(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_integrity_hash_mismatch_is_error() {
        let (dir, exe) = tmp_exe();
        fs::write(
            dir.join("ffmpeg.exe.sha256"),
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        assert!(matches!(
            verify_integrity(&exe),
            Err(FfmpegExeError::BinCorrupt(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_integrity_match_is_ok() {
        let (dir, exe) = tmp_exe();
        let hash = sha256_file(&exe).unwrap();
        fs::write(dir.join("ffmpeg.exe.sha256"), format!("{hash}\n")).unwrap();
        assert!(verify_integrity(&exe).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- Duration 解析 ----

    #[test]
    fn parse_duration_normal() {
        let stderr = "Input #0, mp4, from 'a.mp4':\n  Duration: 00:01:23.45, start: 0.000000, bitrate: 128 kb/s\n";
        let d = parse_duration_str(stderr);
        assert!(d.is_some());
        assert!((d.unwrap() - 83.45).abs() < 1e-9);
    }

    #[test]
    fn parse_duration_na() {
        assert_eq!(
            parse_duration_str("Input #0, pipe: Duration: N/A, bitrate: N/A"),
            None
        );
    }

    #[test]
    fn parse_duration_missing() {
        assert_eq!(parse_duration_str("some random error output"), None);
        assert_eq!(parse_duration_str(""), None);
    }

    #[test]
    fn parse_duration_with_garbage_bytes() {
        // 含非法 UTF-8（中文乱码字节）→ 先 from_utf8_lossy 再解析
        let mut raw: Vec<u8> = vec![0xE4, 0xB8, 0xFF, 0xFE];
        raw.extend_from_slice("乱七八糟 Duration: 00:00:05.20, start: 0".as_bytes());
        let text = String::from_utf8_lossy(&raw);
        let d = parse_duration_str(&text);
        assert!(d.is_some());
        assert!((d.unwrap() - 5.20).abs() < 1e-9);
    }

    #[test]
    fn parse_duration_hours() {
        let d = parse_duration_str("Duration: 01:02:03.50, bitrate");
        assert!(d.is_some());
        assert!((d.unwrap() - 3723.5).abs() < 1e-9);
    }

    #[test]
    fn parse_duration_invalid_fields() {
        // 分钟/秒越界视为无效
        assert_eq!(parse_duration_str("Duration: 00:61:00.00"), None);
        assert_eq!(parse_duration_str("Duration: 00:00:60.00"), None);
    }

    // ---- stderr_tail 字符边界安全截取 ----

    #[test]
    fn stderr_tail_short_input() {
        let bytes = "短错误".as_bytes();
        assert_eq!(stderr_tail(bytes, 500), "短错误");
    }

    #[test]
    fn stderr_tail_chinese_boundary() {
        // 每汉字 3 字节；取尾部时不得在汉字中间截断
        let s: String = "中文错误信息".repeat(50); // 300 字符 = 900 字节
        let tail = stderr_tail(s.as_bytes(), 10);
        assert!(tail.len() <= 12); // 至多回退对齐到字符边界
        assert!(tail.len().is_multiple_of(3)); // 全是完整汉字
        assert_eq!(tail, "误信息"); // 从第 891 字节对齐后取尾 9 字节
        assert!(s.ends_with(&tail));
    }

    #[test]
    fn stderr_tail_invalid_utf8_no_panic() {
        let bytes: Vec<u8> = vec![0xFF; 1000];
        let tail = stderr_tail(&bytes, 500);
        assert!(!tail.is_empty());
    }

    // ---- 超时折算 ----

    #[test]
    fn timeout_small_file() {
        assert_eq!(timeout_secs_for_size(0), 60);
        assert_eq!(timeout_secs_for_size(512 * 1024), 60);
        assert_eq!(timeout_secs_for_size(1024 * 1024), 61);
    }

    #[test]
    fn timeout_scales_with_size() {
        assert_eq!(timeout_secs_for_size(100 * 1024 * 1024), 160);
    }

    #[test]
    fn timeout_capped_at_600() {
        assert_eq!(timeout_secs_for_size(1000 * 1024 * 1024), 600);
        assert_eq!(timeout_secs_for_size(u64::MAX / 2), 600);
    }
}
