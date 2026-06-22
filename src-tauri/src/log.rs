use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use chrono::Local;

static LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// 初始化日志系统
pub fn init_log() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let log_dir = exe_dir.join("log");
    let _ = fs::create_dir_all(&log_dir);

    let now = Local::now();
    let filename = format!("mimo_{}.log", now.format("%Y%m%d_%H%M%S"));
    let log_path = log_dir.join(filename);

    let mut guard = LOG_FILE.lock().unwrap();
    *guard = Some(log_path);
}

/// 写入日志
pub fn log(level: &str, msg: &str) {
    let now = Local::now();
    let timestamp = now.format("%H:%M:%S%.3f");
    let line = format!("[{}] [{}] {}\n", timestamp, level, msg);

    // 输出到控制台
    print!("{}", line);

    // 输出到文件
    let guard = LOG_FILE.lock().unwrap();
    if let Some(path) = guard.as_ref() {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// info 级别日志
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::log::log("INFO", &format!($($arg)*));
    };
}

/// error 级别日志
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::log::log("ERROR", &format!($($arg)*));
    };
}

/// warn 级别日志
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::log::log("WARN", &format!($($arg)*));
    };
}

/// debug 级别日志
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::log::log("DEBUG", &format!($($arg)*));
    };
}
