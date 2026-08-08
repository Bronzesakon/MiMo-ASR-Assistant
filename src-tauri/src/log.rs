//! 应用日志（B-09/B-10 改造 + P7 日志可靠性与轮转）。
//!
//! - B-09：日志目录由 `init_log` 参数传入（Tauri `app_log_dir`），不再写 exe 目录；
//!   建目录/开文件失败时打 stderr 警告而非静默；时间戳带日期消除跨午夜歧义。
//! - B-10：`LOG_FILE` 从 `Mutex<Option<PathBuf>>`（每条日志 open/append/close）
//!   改为长生命期 `Mutex<Option<BufWriter<File>>>` 句柄缓存，每 64 条或每秒 flush，
//!   `info!/warn!/debug!` 宏先查 `is_enabled()`，关闭时不做 `format!`；`error!` 始终输出。
//! - Warning 6：static 句柄在进程退出时不执行 Drop（panic="abort" 更无），
//!   因此 `error!` 级别写入后立即 flush，并提供 `flush()` 供
//!   Tauri `RunEvent::Exit` 显式调用，避免退出时丢失尾部日志。
//! - P7-01：`LogWriter` 记录 `health`（Healthy/Failed）与 `reported_failure`，
//!   write/flush 失败通过 stderr（eprintln）只报告一次，失败后停止递增未刷盘计数；
//!   健康错误只走 stderr，避免递归调用日志宏。
//! - P7-02：大小轮转（单文件 5 MiB）+ 保留策略（最多 10 个文件、目录总量 ≤ 50 MiB）；
//!   启动时只扫描严格匹配 `mimo_YYYYMMDD_HHMMSS[_N].log` 的普通文件，不误删其他文件。

use chrono::Local;
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// 攒批 flush 阈值：每 64 条或距上次 flush 超过 1 秒
const FLUSH_EVERY: u32 = 64;
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// P7-02：单文件轮转阈值（5 MiB）
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// P7-02：同一日志目录最多保留的文件数
const MAX_FILES: usize = 10;
/// P7-02：日志目录总量上限（50 MiB）
const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;

/// 文件侧健康状态（P7-01）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Healthy,
    Failed,
}

struct LogWriter {
    writer: BufWriter<File>,
    unflushed: u32,
    last_flush: Instant,
    health: Health,
    /// 是否已就本次失败向 stderr 报告过一次（避免刷屏）
    reported_failure: bool,
    /// 日志目录（轮转时用于生成新文件路径）
    log_dir: PathBuf,
    /// 当前文件已写入字节数（用于轮转判断）
    current_size: u64,
}

static LOG_FILE: Mutex<Option<LogWriter>> = Mutex::new(None);
static LOGGING_ENABLED: AtomicBool = AtomicBool::new(true);

/// 设置日志开关
pub fn set_enabled(v: bool) {
    LOGGING_ENABLED.store(v, Ordering::Relaxed);
}

/// 日志开关查询（宏在 format! 之前先检查，关闭时零格式化开销）
pub fn is_enabled() -> bool {
    LOGGING_ENABLED.load(Ordering::Relaxed)
}

/// 判断文件名是否严格匹配应用日志命名规则 `mimo_YYYYMMDD_HHMMSS[_N].log`。
/// 用于保留策略扫描，保证只处理本应用生成的轮转文件，不误删目录内其他文件。
fn is_application_log_name(name: &str) -> bool {
    let Some(stem_ext) = name.strip_prefix("mimo_") else {
        return false;
    };
    let Some(stem) = stem_ext.strip_suffix(".log") else {
        return false;
    };
    let parts: Vec<&str> = stem.split('_').collect();
    // 合法形态：YYYYMMDD_HHMMSS（2 段）或 YYYYMMDD_HHMMSS_N（仅轮转序号 1 段）
    match parts.len() {
        2 => {}
        3 => {
            if parts[2].is_empty() || !parts[2].chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }
        _ => return false,
    }
    if parts[0].len() != 8 || !parts[0].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if parts[1].len() != 6 || !parts[1].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

/// 生成一个不冲突的下一个日志文件路径（`mimo_已存在则追加 _N` 序号）
fn next_log_path(log_dir: &Path) -> PathBuf {
    let now = Local::now();
    let base = format!("mimo_{}", now.format("%Y%m%d_%H%M%S"));
    let mut candidate = log_dir.join(format!("{}.log", base));
    let mut n = 1;
    while candidate.exists() {
        candidate = log_dir.join(format!("{}_{}.log", base, n));
        n += 1;
    }
    candidate
}

/// 打开（或创建追加）日志文件并构造 LogWriter；`log_dir` 取自父目录。
fn open_new_writer(path: &Path) -> std::io::Result<LogWriter> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let current_size = file.metadata()?.len();
    Ok(LogWriter {
        writer: BufWriter::new(file),
        unflushed: 0,
        last_flush: Instant::now(),
        health: Health::Healthy,
        reported_failure: false,
        log_dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
        current_size,
    })
}

/// 是否需要轮转（P7-02：当前文件 ≥ 5 MiB）
fn should_rotate(lw: &LogWriter) -> bool {
    lw.current_size >= MAX_FILE_BYTES
}

/// 处理一次 write 的结果（P7-01）：成功递增未刷盘计数并累计字节数；
/// 失败标记 Failed，仅向 stderr 报告一次，不递增未刷盘计数。
fn handle_write_result(lw: &mut LogWriter, line_len: u64, result: std::io::Result<()>) {
    match result {
        Ok(()) => {
            lw.current_size += line_len;
            lw.unflushed += 1;
        }
        Err(e) => {
            lw.health = Health::Failed;
            if !lw.reported_failure {
                lw.reported_failure = true;
                eprintln!(
                    "[Mimo 日志] 写入日志文件失败: {}，后续日志仅输出到控制台",
                    e
                );
            }
        }
    }
}

/// 处理一次 flush 的结果（P7-01）：成功才清零未刷盘计数并更新 last_flush；
/// 失败保留计数并仅向 stderr 报告一次。
fn handle_flush_result(lw: &mut LogWriter, result: std::io::Result<()>) {
    match result {
        Ok(()) => {
            lw.unflushed = 0;
            lw.last_flush = Instant::now();
        }
        Err(e) => {
            lw.health = Health::Failed;
            if !lw.reported_failure {
                lw.reported_failure = true;
                eprintln!("[Mimo 日志] 刷新日志文件失败: {}，未刷盘日志可能丢失", e);
            }
        }
    }
}

/// 关闭当前文件并开新文件（P7-02 轮转）。新文件复用 `mimo_YYYYMMDD_HHMMSS[_N].log` 规则。
fn rotate_to_new_file(lw: &mut LogWriter) {
    let _ = lw.writer.flush();
    let new_path = next_log_path(&lw.log_dir);
    match open_new_writer(&new_path) {
        Ok(nw) => *lw = nw,
        Err(e) => {
            lw.health = Health::Failed;
            if !lw.reported_failure {
                lw.reported_failure = true;
                eprintln!("[Mimo 日志] 轮转日志文件 {:?} 失败: {}", new_path, e);
            }
        }
    }
}

/// 保留策略（P7-02）：只扫描严格匹配命名规则的普通文件。
/// 1. 先按时间删除超出 `MAX_FILES` 的旧文件；
/// 2. 再按总字节数继续删除最旧文件，直到目录总量 ≤ `MAX_TOTAL_BYTES`。
/// 清理失败写 stderr，但不阻断启动。
fn cleanup_old_logs(log_dir: &Path) {
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !is_application_log_name(&name) || !path.is_file() {
                continue;
            }
            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((path, meta.len(), mtime));
        }
    }
    // 按修改时间升序（最旧在前）
    files.sort_by_key(|(_, _, t)| *t);

    // 步骤 1：超出数量上限时删除最旧文件
    while files.len() > MAX_FILES {
        let (path, _, _) = files.remove(0);
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!(
                "[Mimo 日志] 清理旧日志文件 {:?} 失败: {}（不阻断启动）",
                path, e
            );
        }
    }

    // 步骤 2：总量超限时继续删除最旧文件，直到 ≤ 上限
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    while total > MAX_TOTAL_BYTES && !files.is_empty() {
        let (path, len, _) = files.remove(0);
        match std::fs::remove_file(&path) {
            Ok(()) => total -= len,
            Err(e) => {
                eprintln!(
                    "[Mimo 日志] 清理旧日志文件 {:?} 失败: {}（不阻断启动）",
                    path, e
                );
            }
        }
    }
}

/// 初始化日志系统（B-09：目录由调用方传入，通常为 app.path().app_log_dir()）。
/// 建目录或打开文件失败时打一条 stderr 警告，不静默、不 panic。
/// P7-02：启动时顺带执行保留策略清理。
pub fn init_log(log_dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(log_dir) {
        eprintln!(
            "[Mimo 日志] 创建日志目录 {:?} 失败: {}，日志将仅输出到控制台",
            log_dir, e
        );
        return;
    }

    cleanup_old_logs(log_dir);

    let log_path = next_log_path(log_dir);
    match open_new_writer(&log_path) {
        Ok(lw) => {
            let mut guard = LOG_FILE.lock().unwrap();
            *guard = Some(lw);
        }
        Err(e) => {
            eprintln!(
                "[Mimo 日志] 打开日志文件 {:?} 失败: {}，日志将仅输出到控制台",
                log_path, e
            );
        }
    }
}

/// 写入日志
pub fn log(level: &str, msg: &str) {
    // ERROR 级别始终输出；其他级别受开关控制
    if level != "ERROR" && !is_enabled() {
        return;
    }

    let now = Local::now();
    // B-09：时间戳含日期（%Y-%m-%d %H:%M:%S），消除跨午夜歧义
    let line = format!(
        "[{}] [{}] {}\n",
        now.format("%Y-%m-%d %H:%M:%S%.3f"),
        level,
        msg
    );

    // 输出到控制台
    print!("{}", line);

    // 输出到文件（长生命期句柄 + 攒批 flush）
    let mut guard = LOG_FILE.lock().unwrap();
    let Some(lw) = guard.as_mut() else {
        return;
    };
    // 文件侧已失效时仅控制台输出，不再尝试写文件
    if lw.health == Health::Failed {
        return;
    }
    // P7-02：写入前按当前文件长度判断是否轮转
    if should_rotate(lw) {
        rotate_to_new_file(lw);
        if lw.health == Health::Failed {
            return; // 轮转失败，降级为仅控制台
        }
    }
    let result = lw.writer.write_all(line.as_bytes());
    handle_write_result(lw, line.len() as u64, result);
    // Warning 6：ERROR 级别写入后立即 flush，防止退出时丢失关键日志
    let force = level == "ERROR";
    if force || lw.unflushed >= FLUSH_EVERY || lw.last_flush.elapsed() >= FLUSH_INTERVAL {
        let fr = lw.writer.flush();
        handle_flush_result(lw, fr);
    }
}

/// 显式 flush 缓冲区（Warning 6）：static 句柄不执行 Drop，
/// 由 Tauri `RunEvent::Exit` 回调显式调用，确保退出前尾部日志落盘
pub fn flush() {
    let mut guard = LOG_FILE.lock().unwrap();
    if let Some(lw) = guard.as_mut() {
        let fr = lw.writer.flush();
        handle_flush_result(lw, fr);
    }
}

/// 日志状态快照（P7-01 只读查询，供设置页展示）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogStatus {
    /// 文件侧健康状态（Healthy 为 true）
    pub healthy: bool,
    /// 目录内匹配应用命名规则的文件数
    pub file_count: usize,
    /// 目录内匹配应用命名规则的文件总字节数
    pub total_bytes: u64,
}

/// 计算日志目录状态（P7-01）：健康状态取自当前 writer，文件数与字节数扫描目录。
pub fn log_status(log_dir: &Path) -> LogStatus {
    let healthy = {
        let guard = LOG_FILE.lock().unwrap();
        match guard.as_ref() {
            Some(lw) => lw.health == Health::Healthy,
            None => false, // 未初始化或打开失败视为不健康
        }
    };
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !is_application_log_name(&name) || !path.is_file() {
                continue;
            }
            if let Ok(m) = std::fs::metadata(&path) {
                total_bytes += m.len();
                file_count += 1;
            }
        }
    }
    LogStatus {
        healthy,
        file_count,
        total_bytes,
    }
}

/// info 级别日志（开关关闭时不做 format!）
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {{
        if $crate::log::is_enabled() {
            $crate::log::log("INFO", &format!($($arg)*));
        }
    }};
}

/// error 级别日志（始终输出）
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::log::log("ERROR", &format!($($arg)*));
    };
}

/// warn 级别日志（开关关闭时不做 format!）
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {{
        if $crate::log::is_enabled() {
            $crate::log::log("WARN", &format!($($arg)*));
        }
    }};
}

/// debug 级别日志（开关关闭时不做 format!）
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        if $crate::log::is_enabled() {
            $crate::log::log("DEBUG", &format!($($arg)*));
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用隔离目录
    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mimo_log_test_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).expect("创建测试目录失败");
        d
    }

    /// 构造一个指向真实文件的 LogWriter（供轮转/保留策略测试使用）
    fn writer_in(dir: &Path) -> LogWriter {
        let path = next_log_path(dir);
        open_new_writer(&path).expect("打开测试日志文件失败")
    }

    // ---- P7-01：write/flush 失败的健康状态与计数 ----

    #[test]
    fn write_failure_marks_failed_and_does_not_count() {
        let dir = test_dir("write_fail");
        let mut lw = writer_in(&dir);
        let err = std::io::Error::new(std::io::ErrorKind::Other, "注入的写入失败");

        // 写入成功：递增未刷盘计数
        handle_write_result(&mut lw, 10, Ok(()));
        assert_eq!(lw.health, Health::Healthy);
        assert_eq!(lw.unflushed, 1);
        assert_eq!(lw.current_size, 10);

        // 写入失败：健康变 Failed、不递增计数、报告一次
        handle_write_result(&mut lw, 99, Err(err));
        assert_eq!(lw.health, Health::Failed);
        assert_eq!(lw.unflushed, 1, "失败后不得递增未刷盘计数");
        assert_eq!(lw.current_size, 10, "失败不累计字节数");
        assert!(lw.reported_failure, "应报告一次失败");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flush_success_resets_count_but_failure_keeps_it() {
        let dir = test_dir("flush_fail");
        let mut lw = writer_in(&dir);
        lw.unflushed = 5;
        let err = std::io::Error::new(std::io::ErrorKind::Other, "注入的刷新失败");

        // flush 成功：清零计数并更新 last_flush，健康保持
        handle_flush_result(&mut lw, Ok(()));
        assert_eq!(lw.unflushed, 0, "flush 成功才清零未刷盘计数");
        assert_eq!(lw.health, Health::Healthy);

        // 再次攒一些计数后 flush 失败：保留计数、健康变 Failed
        lw.unflushed = 3;
        handle_flush_result(&mut lw, Err(err));
        assert_eq!(lw.unflushed, 3, "flush 失败应保留未刷盘计数");
        assert_eq!(lw.health, Health::Failed);
        assert!(lw.reported_failure);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- P7-02：命名规则匹配 ----

    #[test]
    fn application_log_name_matches_strict_rule() {
        for name in [
            "mimo_20260101_000000.log",
            "mimo_20261231_235959.log",
            "mimo_20260101_000000_1.log",
            "mimo_20260101_000000_12.log",
        ] {
            assert!(is_application_log_name(name), "{name:?} 应匹配");
        }
    }

    #[test]
    fn application_log_name_rejects_others() {
        for name in [
            "mimo.log",
            "mimo_2026.log",
            "mimo_20260101.log",
            "mimo_20260101_0000.log",
            "mimo_20260101_000000_N.log",
            "mimo_20260101_000000_1.txt",
            "important.txt",
            "notes.md",
            "other_mimo_20260101_000000.log",
            "",
        ] {
            assert!(!is_application_log_name(name), "{name:?} 不应匹配");
        }
    }

    // ---- P7-02：超过 5 MiB 创建新文件 ----

    #[test]
    fn rotation_creates_new_file_over_threshold() {
        let dir = test_dir("rotate");
        let mut lw = writer_in(&dir);
        assert_eq!(lw.current_size, 0);
        assert!(!should_rotate(&lw));

        // 模拟当前文件已到轮转阈值（≥ 5 MiB）
        lw.current_size = MAX_FILE_BYTES;
        assert!(should_rotate(&lw), "达到 5 MiB 应触发轮转");

        let count_before = std::fs::read_dir(&dir).unwrap().count();
        rotate_to_new_file(&mut lw);
        let count_after = std::fs::read_dir(&dir).unwrap().count();

        assert_eq!(count_after, count_before + 1, "轮转应创建新文件");
        assert_eq!(lw.current_size, 0, "新文件字节数归零");
        assert_eq!(lw.health, Health::Healthy, "轮转成功后健康状态保持");
        assert!(!lw.reported_failure);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- P7-02：非应用文件保持原样 ----

    #[test]
    fn cleanup_keeps_non_application_files() {
        let dir = test_dir("keep_other");
        // 生成 12 个应用日志文件（超数量上限，且每个 5 MiB，总量超 50 MiB）
        for i in 0..12 {
            let name = format!("mimo_20260101_{:06}_{}.log", i, i);
            let path = dir.join(name);
            std::fs::write(&path, vec![b'x'; (MAX_FILE_BYTES as usize) - 1]).unwrap();
        }
        // 非应用文件：应保持原样
        let keep = dir.join("important.txt");
        std::fs::write(&keep, "请勿删除").unwrap();
        let sub = dir.join("sub_mimo_20260101_000000.log");
        std::fs::write(&sub, "子目录非应用文件").unwrap();

        cleanup_old_logs(&dir);

        assert!(keep.exists(), "非应用文件不得被删除");
        assert_eq!(std::fs::read_to_string(&keep).unwrap(), "请勿删除");
        assert!(sub.exists(), "子目录中的非应用文件不得被删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- P7-02：应用日志满足文件数与总量上限 ----

    #[test]
    fn cleanup_enforces_file_count_and_total_byte_limits() {
        let dir = test_dir("limits");
        // 12 个文件，每个略小于 5 MiB，总量约 60 MiB（超 50 MiB 上限）
        for i in 0..12 {
            let name = format!("mimo_20260101_{:06}_{}.log", i, i);
            let path = dir.join(name);
            std::fs::write(&path, vec![b'x'; (MAX_FILE_BYTES as usize) - 2]).unwrap();
        }

        cleanup_old_logs(&dir);

        // 只统计匹配命名规则的应用日志文件
        let mut count = 0usize;
        let mut total = 0u64;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_application_log_name(&name) {
                count += 1;
                total += std::fs::metadata(entry.path()).unwrap().len();
            }
        }
        assert!(count <= MAX_FILES, "文件数应 ≤ 10，实际 {count}");
        assert!(total <= MAX_TOTAL_BYTES, "总量应 ≤ 50 MiB，实际 {total}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
