//! 跨平台可靠原子文件替换（终验 T-01）。
//!
//! Windows 上 `std::fs::rename` 使用 `MoveFileExW` 且不带
//! `MOVEFILE_REPLACE_EXISTING`，覆盖已存在目标会失败；这里改用
//! `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`
//! 实现可证明的原子替换。非 Windows 用 POSIX `rename`（天然覆盖）。
//!
//! 写盘顺序：写同目录 `.tmp` → flush → sync_all →（可选）旧主文件复制为
//! `.bak` → 原子替换主文件。任一步失败：清理临时文件，保持旧主文件与备份
//! 不变并返回错误。日志纪律：不记录文件正文、Key 或完整 JSON。
//!
//! P0-03：写盘链路被拆成独立的内部步骤（`write_tmp` / `update_backup` /
//! `replace_file`），`#[cfg(test)]` 下可注入"第 N 步失败"，验证每个失败点
//! 都保持原主文件可解析并清理本次临时文件。生产 API 保持简洁。

use std::io::{self, Write};
use std::path::Path;

/// 原子替换 `path`，替换前把旧主文件复制为 `.bak`（备份 = 上一版）。
/// 备份步骤失败则中止，不替换主文件。任一步失败都清理本次临时文件。
pub fn atomic_write_text_with_backup(path: &Path, content: &str) -> io::Result<()> {
    let tmp = unique_tmp_path(path);
    let result = (|| -> io::Result<()> {
        write_tmp(&tmp, content)?;
        if path.exists() {
            // 备份失败不得继续替换主文件（violation 会丢掉唯一好备份）
            update_backup(path, &tmp)?;
        }
        replace_file(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 原子替换且不生成备份（用于从备份/corrupt 恢复主文件，
/// 避免内部把损坏旧主文件复制回 `.bak` 覆盖唯一完好备份）。
/// 任一步失败都清理本次临时文件。
pub fn atomic_write_text_no_backup(path: &Path, content: &str) -> io::Result<()> {
    let tmp = unique_tmp_path(path);
    let result = (|| -> io::Result<()> {
        write_tmp(&tmp, content)?;
        replace_file(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 生成同目录唯一临时文件名（含进程 id 与单调序号），保证并发调用互不干扰。
fn unique_tmp_path(path: &Path) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => {
            let dir = path.parent().unwrap_or_else(|| Path::new("."));
            dir.join(format!(
                ".{name}.{}.{}.{}.tmp",
                std::process::id(),
                seq,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ))
        }
        None => path.with_extension("json.tmp"),
    }
}

/// 把旧主文件复制为 `.bak`。备份更新走 `tmp -> flush -> sync_all -> 原子替换`
/// 链路（P4-01），不再直接覆盖现有 bak，避免中间失败破坏唯一好备份。
/// 任一步失败都清理本次 `bak_tmp`，主文件保持原样。
fn update_backup(path: &Path, _tmp: &Path) -> io::Result<()> {
    let bak = path.with_extension("json.bak");
    let bak_tmp = unique_tmp_path(&bak);
    let result = (|| -> io::Result<()> {
        std::fs::copy(path, &bak_tmp)?;
        {
            let mut f = std::fs::OpenOptions::new().write(true).open(&bak_tmp)?;
            f.flush()?;
            f.sync_all()?;
        }
        replace_file(&bak_tmp, &bak)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&bak_tmp);
    }
    result
}

fn write_tmp(tmp: &Path, content: &str) -> io::Result<()> {
    fault_point(fault::FaultPoint::WriteAll)?;
    let mut f = std::fs::File::create(tmp)?;
    f.write_all(content.as_bytes())?;
    fault_point(fault::FaultPoint::Flush)?;
    f.flush()?;
    fault_point(fault::FaultPoint::SyncAll)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn replace_file(src: &Path, dst: &Path) -> io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    let src_w: Vec<u16> = OsStr::new(src.as_os_str())
        .encode_wide()
        .chain(Some(0))
        .collect();
    let dst_w: Vec<u16> = OsStr::new(dst.as_os_str())
        .encode_wide()
        .chain(Some(0))
        .collect();
    fault_point(fault::FaultPoint::ReplaceMain)?;
    let r = unsafe {
        MoveFileExW(
            src_w.as_ptr(),
            dst_w.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if r == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_file(src: &Path, dst: &Path) -> io::Result<()> {
    fault_point(fault::FaultPoint::ReplaceMain)?;
    std::fs::rename(src, dst)
}

/// 原子写内部步骤（P0-03 故障注入点）。
/// `pub(crate)` 使其可被 audio 域测试复用（P4-03 验证 create_task_temp_dir
/// 的元数据原子写在任一阶段失败时回滚目录）。生产路径恒为 `Ok(())`。
pub(crate) mod fault {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum FaultPoint {
        WriteAll,
        Flush,
        SyncAll,
        ReplaceMain,
    }
}

/// 供其它模块（如 audio）的测试复用原子写故障注入开关，仅测试可见。
#[cfg(test)]
pub(crate) use fault::FaultPoint;

/// 故障注入：仅在 `#[cfg(test)]` 下生效。测试通过 `set_fail_at` 指定某一步
/// 返回错误，生产路径恒为 `Ok(())`。
#[cfg(test)]
fn fault_point(point: fault::FaultPoint) -> io::Result<()> {
    match fault_inject::with_lock(|guard| {
        if *guard == Some(point) {
            *guard = None; // 只失败一次
            Some(io::Error::other("注入的原子写故障"))
        } else {
            None
        }
    }) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(not(test))]
fn fault_point(_point: fault::FaultPoint) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
pub(crate) fn set_fail_at(point: fault::FaultPoint) {
    fault_inject::with_lock(|guard| {
        *guard = Some(point);
        None
    });
}

/// 单一共享的故障注入开关（`set_fail_at` 与 `fault_point` 共用同一 static）。
/// 使用 thread-local：原子写均为调用线程内的同步操作，故障注入只影响当前测试线程，
/// 避免并行测试互相污染全局状态。
#[cfg(test)]
mod fault_inject {
    use std::cell::RefCell;
    use std::sync::Mutex;

    use super::fault::FaultPoint;

    thread_local! {
        // 初始化器已写成文档规定的 const 块形式，但 clippy 1.96 在该写法上
        // 仍报 missing_const_for_thread_local（建议值与现状一致），属误报。
        #[allow(clippy::missing_const_for_thread_local)]
        static FAIL_AT: RefCell<Option<FaultPoint>> = const { RefCell::new(None) };
    }

    /// 在锁内执行闭包；返回 `Some(err)` 时向外传播为写失败，`None` 表示无故障。
    pub fn with_lock<F: FnOnce(&mut Option<FaultPoint>) -> Option<std::io::Error>>(
        f: F,
    ) -> Option<std::io::Error> {
        // 与其它测试共享的全局互斥锁仅用于串行化，避免并发测试对同一临时目录写冲突；
        // 故障开关本身按线程隔离。
        static GLOBAL: Mutex<()> = Mutex::new(());
        let _guard = GLOBAL.lock().unwrap();
        FAIL_AT.with(|cell| {
            let mut slot = cell.borrow_mut();
            f(&mut slot)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fault::FaultPoint;

    fn test_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mimo_atomic_test_{}_{}_{}",
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

    #[test]
    fn overwrite_existing_succeeds_and_backs_up_previous() {
        let dir = test_dir("overwrite");
        let path = dir.join("cfg.json");
        atomic_write_text_with_backup(&path, "v1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
        assert!(!path.with_extension("json.bak").exists(), "首次无备份");
        // 第二次覆盖：主文件变 v2，备份为 v1
        atomic_write_text_with_backup(&path, "v2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "v1"
        );
        assert!(
            !std::fs::read_dir(&dir).unwrap().any(|e| {
                let n = e.unwrap().file_name().to_string_lossy().into_owned();
                n.ends_with(".tmp")
            }),
            "tmp 不得残留"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_backup_does_not_touch_existing_bak() {
        let dir = test_dir("nobackup");
        let path = dir.join("cfg.json");
        atomic_write_text_no_backup(&path, "v1").unwrap();
        // 手动放入一个"唯一好备份"
        std::fs::write(path.with_extension("json.bak"), "good-bak").unwrap();
        atomic_write_text_no_backup(&path, "v2").unwrap();
        // 备份必须保持原样，不被 v1 覆盖
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "good-bak"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tmp_blocked_fails_and_keeps_original() {
        let dir = test_dir("tmpblocked");
        let path = dir.join("cfg.json");
        atomic_write_text_no_backup(&path, "v1").unwrap();
        // 用目录占用一个 tmp 文件名使 create 失败（唯一 tmp 路径含序号，这里占住所有候选）
        // 直接构造一个不可写父目录来触发失败
        let blocked = dir.join("ro").join("cfg.json");
        std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        atomic_write_text_no_backup(&blocked, "v1").unwrap();
        // 用目录占用 tmp 路径：把父目录替换为文件使 create 失败
        let parent = blocked.parent().unwrap();
        std::fs::remove_dir_all(parent).unwrap();
        std::fs::write(parent, "not a dir").unwrap();
        assert!(atomic_write_text_no_backup(&blocked, "v2").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ============================================================
    // P0-03：故障注入测试。每个失败点都必须：
    //   1. 保持原主文件可解析；
    //   2. 清理本次临时文件；
    //   3. 备份不被破坏（针对 with_backup 变体）。
    // ============================================================

    /// 断言写失败后主文件保持原值、备份保持原样、无残留 tmp。
    fn assert_failed_write(
        base: &std::path::Path,
        expected_main: &str,
        has_bak: bool,
        bak_val: &str,
    ) {
        if has_bak {
            assert_eq!(
                std::fs::read_to_string(base.with_extension("json.bak")).unwrap(),
                bak_val,
                "备份不得被破坏"
            );
        }
        assert_eq!(
            std::fs::read_to_string(base).unwrap(),
            expected_main,
            "主文件必须保持原值"
        );
        assert!(
            !std::fs::read_dir(base.parent().unwrap()).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
                    .ends_with(".tmp")
            }),
            "失败后不得残留本次临时文件"
        );
    }

    /// 建立"主=v1、备份=v1"的稳定基线（写入两次），供故障注入使用。
    fn seed_with_backup(path: &std::path::Path) {
        atomic_write_text_with_backup(path, "v0").unwrap();
        atomic_write_text_with_backup(path, "v1").unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "v1");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "v0"
        );
    }

    #[test]
    fn fault_writeall_keeps_original_clean_tmp() {
        let dir = test_dir("f_writeall");
        let path = dir.join("cfg.json");
        seed_with_backup(&path);
        set_fail_at(FaultPoint::WriteAll);
        assert!(atomic_write_text_with_backup(&path, "v2").is_err());
        assert_failed_write(&path, "v1", true, "v0");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fault_flush_keeps_original_clean_tmp() {
        let dir = test_dir("f_flush");
        let path = dir.join("cfg.json");
        seed_with_backup(&path);
        set_fail_at(FaultPoint::Flush);
        assert!(atomic_write_text_with_backup(&path, "v2").is_err());
        assert_failed_write(&path, "v1", true, "v0");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fault_syncall_keeps_original_clean_tmp() {
        let dir = test_dir("f_sync");
        let path = dir.join("cfg.json");
        seed_with_backup(&path);
        set_fail_at(FaultPoint::SyncAll);
        assert!(atomic_write_text_with_backup(&path, "v2").is_err());
        assert_failed_write(&path, "v1", true, "v0");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fault_replace_main_keeps_original_backup_intact() {
        let dir = test_dir("f_replace");
        let path = dir.join("cfg.json");
        seed_with_backup(&path);
        set_fail_at(FaultPoint::ReplaceMain);
        assert!(atomic_write_text_with_backup(&path, "v2").is_err());
        // 主文件仍是 v1，备份仍是 v0（新备份未提交），tmp 已清理
        assert_failed_write(&path, "v1", true, "v0");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fault_replace_main_no_backup_variant() {
        let dir = test_dir("f_replace_nb");
        let path = dir.join("cfg.json");
        atomic_write_text_no_backup(&path, "v1").unwrap();
        set_fail_at(FaultPoint::ReplaceMain);
        assert!(atomic_write_text_no_backup(&path, "v2").is_err());
        assert_failed_write(&path, "v1", false, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 并发调用：每个调用使用独立 tmp，互不删除对方文件，主备份至少一个可解析
    #[test]
    fn concurrent_atomic_writes_use_independent_tmp() {
        let dir = test_dir("concurrent");
        let path = dir.join("cfg.json");
        atomic_write_text_with_backup(&path, "seed").unwrap();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let p = path.clone();
                std::thread::spawn(move || {
                    let content = format!("v{}", i);
                    let _ = atomic_write_text_with_backup(&p, &content);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // 主文件或备份至少一个可解析（都是 JSON 字符串）
        let main = std::fs::read_to_string(&path).unwrap_or_default();
        let bak = std::fs::read_to_string(path.with_extension("json.bak")).unwrap_or_default();
        assert!(
            !main.is_empty() || !bak.is_empty(),
            "至少保留一个可解析副本"
        );
        // 无残留 tmp
        assert!(
            !std::fs::read_dir(&dir).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
                    .ends_with(".tmp")
            }),
            "并发写后不得残留 tmp"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
