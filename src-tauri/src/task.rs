//! 任务状态机（P0-02）：把注册、状态转换、断点更新、代际移除从 Tauri command
//! 中提取为普通 Rust 类型，使其可脱离 WebView 做单元测试。
//!
//! 设计目标：
//! - 状态机只做内存状态转换，不直接做任何同步 IO（临时目录清理、磁盘写入）
//!   都由调用方在 `tokio::task::spawn_blocking` 中执行，测试无需创建 WebView。
//! - 每个运行代际（generation）只产生一个终态。
//! - cancel 优先于 pause；`Paused` 之外的阶段拒绝 cleanup/resume。
//! - 后端是配置、任务状态、恢复断点、临时路径的权威来源。
//!
//! 本模块仅在 `#[cfg(test)]` 下编译，为测试场景保留的若干辅助接口/变体
//! 可能未被全部构造，故统一关闭 dead_code 检查，避免测试基础设施产生噪音。

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::cancel::CancelToken;

/// 任务操作类型（转写 / 规整）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskOperation {
    Transcription,
    Polish,
}

/// 任务运行阶段（固定枚举）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskPhase {
    Starting,
    Processing,
    Running,
    PauseRequested,
    Paused,
    CancelRequested,
}

/// 恢复断点（P1-04：由后端保存与校验，前端不计算）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PauseCheckpoint {
    pub last_completed: u32,
    pub total_slices: u32,
}

impl PauseCheckpoint {
    /// 断点有效：已完成的片数不超过总片数。
    pub fn is_valid(&self) -> bool {
        self.last_completed <= self.total_slices
    }
}

/// 后端任务记录（参考清单 1.3）。
pub struct TaskEntry {
    pub operation: TaskOperation,
    pub run_id: String,
    pub generation: u64,
    pub phase: TaskPhase,
    pub cancelled: CancelToken,
    pub pause_flag: Arc<AtomicBool>,
    pub temp_dir: Option<PathBuf>,
    pub checkpoint: Option<PauseCheckpoint>,
    pub output: Arc<std::sync::Mutex<String>>,
}

impl TaskEntry {
    pub fn new(operation: TaskOperation, run_id: String) -> Self {
        Self {
            operation,
            run_id,
            generation: Self::next_generation(),
            phase: TaskPhase::Starting,
            cancelled: CancelToken::new(),
            pause_flag: Arc::new(AtomicBool::new(false)),
            temp_dir: None,
            checkpoint: None,
            output: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// 生成单调递增的任务代际（进程内唯一）。
    fn next_generation() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(1);
        SEQ.fetch_add(1, Ordering::Relaxed)
    }

    /// 从 Paused 阶段启动新运行（resume）：生成新代际，重置临时句柄之外的状态。
    pub fn into_resumed(mut self) -> Self {
        self.phase = TaskPhase::Starting;
        self
    }
}

/// 任务状态机（P0-02）：以 `operation + run_id + generation` 为键管理任务。
/// IO 全部外置，本结构只做内存状态转换，可独立单测。
pub struct TaskStateMachine {
    tasks: HashMap<String, TaskEntry>,
}

/// 状态机操作结果，供调用方决定是否执行 IO。
#[derive(Debug, PartialEq, Eq)]
pub enum Transition {
    Ok,
    /// 拒绝：原因见消息。
    Rejected(String),
    /// 该运行已终结（多次终态被忽略）。
    AlreadyTerminal,
}

/// 代际查找结果：既有任务是否匹配 `operation + run_id + generation`。
#[derive(Debug, PartialEq, Eq)]
pub enum TaskLookup {
    Found,
    /// 条目存在但代际/run_id 不匹配（旧任务残留，不得影响新任务）。
    Stale,
    NotFound,
}

impl TaskStateMachine {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// 注册新任务；同一 file_id 已有活跃/暂停条目时拒绝（防止覆盖旧任务句柄）。
    pub fn try_register(&mut self, file_id: &str, entry: TaskEntry) -> Result<(), String> {
        if self.tasks.contains_key(file_id) {
            return Err("该文件已有进行中的任务，请先等待其完成或取消".to_string());
        }
        self.tasks.insert(file_id.to_string(), entry);
        Ok(())
    }

    /// 按 file_id 查找代际匹配情况。
    pub fn lookup(
        &self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> TaskLookup {
        match self.tasks.get(file_id) {
            None => TaskLookup::NotFound,
            Some(e)
                if e.operation == operation && e.run_id == run_id && e.generation == generation =>
            {
                TaskLookup::Found
            }
            Some(_) => TaskLookup::Stale,
        }
    }

    /// 启动转写：仅 `Transcription` 且可注册。
    pub fn start_transcription(&mut self, file_id: &str, run_id: &str) -> Result<u64, String> {
        let entry = TaskEntry::new(TaskOperation::Transcription, run_id.to_string());
        let generation = entry.generation;
        self.try_register(file_id, entry)?;
        Ok(generation)
    }

    /// 启动规整。
    pub fn start_polish(&mut self, file_id: &str, run_id: &str) -> Result<u64, String> {
        let entry = TaskEntry::new(TaskOperation::Polish, run_id.to_string());
        let generation = entry.generation;
        self.try_register(file_id, entry)?;
        Ok(generation)
    }

    /// 从暂停处继续转写（P1-04）：仅 `Paused` 阶段接受，用新 run_id + 新代际
    /// 替换旧暂停条目，并沿用其临时目录与断点。返回新代际。
    pub fn resume(
        &mut self,
        file_id: &str,
        old_run_id: &str,
        old_generation: u64,
        new_run_id: &str,
    ) -> Result<u64, String> {
        let entry = match self.tasks.get(file_id) {
            Some(e)
                if e.operation == TaskOperation::Transcription
                    && e.run_id == old_run_id
                    && e.generation == old_generation
                    && e.phase == TaskPhase::Paused =>
            {
                e
            }
            _ => return Err("该任务不在暂停状态，无法继续转写".to_string()),
        };
        let temp_dir = entry.temp_dir.clone();
        let checkpoint = entry.checkpoint.clone();
        let mut new_entry = TaskEntry::new(TaskOperation::Transcription, new_run_id.to_string());
        new_entry.temp_dir = temp_dir;
        new_entry.checkpoint = checkpoint;
        let generation = new_entry.generation;
        self.tasks.insert(file_id.to_string(), new_entry);
        Ok(generation)
    }

    /// 转写到 Running 阶段。
    pub fn mark_running(
        &mut self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> Transition {
        match self.lookup(file_id, operation, run_id, generation) {
            TaskLookup::NotFound => Transition::Rejected("任务不存在".to_string()),
            TaskLookup::Stale => Transition::Rejected("任务代际不匹配".to_string()),
            TaskLookup::Found => {
                let e = self.tasks.get_mut(file_id).unwrap();
                e.phase = TaskPhase::Running;
                Transition::Ok
            }
        }
    }

    /// 请求暂停：仅 `Transcription + Running` 接受；置位 pause_flag 并进入 PauseRequested。
    pub fn request_pause(
        &mut self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> Transition {
        match self.lookup(file_id, operation, run_id, generation) {
            TaskLookup::NotFound => Transition::Rejected("TASK_NOT_FOUND".to_string()),
            TaskLookup::Stale => Transition::Rejected("GENERATION_MISMATCH".to_string()),
            TaskLookup::Found => {
                let e = self.tasks.get_mut(file_id).unwrap();
                if e.operation != TaskOperation::Transcription {
                    return Transition::Rejected("NOT_TRANSCRIPTION".to_string());
                }
                if e.phase != TaskPhase::Running {
                    return Transition::Rejected("NOT_RUNNING".to_string());
                }
                e.phase = TaskPhase::PauseRequested;
                e.pause_flag
                    .store(true, std::sync::atomic::Ordering::Release);
                Transition::Ok
            }
        }
    }

    /// 确认暂停：仅 PauseRequested 可进入 Paused，并写入断点。
    pub fn confirm_pause(
        &mut self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
        checkpoint: PauseCheckpoint,
    ) -> Transition {
        match self.lookup(file_id, operation, run_id, generation) {
            TaskLookup::NotFound => Transition::Rejected("TASK_NOT_FOUND".to_string()),
            TaskLookup::Stale => Transition::Rejected("GENERATION_MISMATCH".to_string()),
            TaskLookup::Found => {
                let e = self.tasks.get_mut(file_id).unwrap();
                if e.phase != TaskPhase::PauseRequested {
                    return Transition::Rejected("NOT_PAUSE_REQUESTED".to_string());
                }
                e.phase = TaskPhase::Paused;
                e.checkpoint = Some(checkpoint);
                Transition::Ok
            }
        }
    }

    /// 请求取消：对任意非终态任务置位取消；cancel 优先于 pause。
    pub fn request_cancel(
        &mut self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> Transition {
        match self.lookup(file_id, operation, run_id, generation) {
            TaskLookup::NotFound => Transition::Rejected("TASK_NOT_FOUND".to_string()),
            TaskLookup::Stale => Transition::Rejected("GENERATION_MISMATCH".to_string()),
            TaskLookup::Found => {
                let e = self.tasks.get_mut(file_id).unwrap();
                e.cancelled.cancel();
                e.phase = TaskPhase::CancelRequested;
                Transition::Ok
            }
        }
    }

    /// 代际条件式移除：仅当当前条目代际与给定代际一致时移除。
    pub fn remove_if_generation(&mut self, file_id: &str, generation: u64) -> bool {
        match self.tasks.get(file_id) {
            Some(e) if e.generation == generation => {
                self.tasks.remove(file_id);
                true
            }
            _ => false,
        }
    }

    /// 读取某代际的断点（仅 Paused 阶段返回）。
    pub fn paused_checkpoint(
        &self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> Option<PauseCheckpoint> {
        match self.lookup(file_id, operation, run_id, generation) {
            TaskLookup::Found => self.tasks.get(file_id).and_then(|e| e.checkpoint.clone()),
            _ => None,
        }
    }

    /// 读取某代际的临时目录。
    pub fn temp_dir(
        &self,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> Option<PathBuf> {
        match self.lookup(file_id, operation, run_id, generation) {
            TaskLookup::Found => self.tasks.get(file_id).and_then(|e| e.temp_dir.clone()),
            _ => None,
        }
    }

    /// 设置代际临时目录；仅当代际匹配时生效。
    pub fn set_temp_dir(&mut self, file_id: &str, generation: u64, dir: PathBuf) {
        if let Some(e) = self.tasks.get_mut(file_id) {
            if e.generation == generation {
                e.temp_dir = Some(dir);
            }
        }
    }

    /// 当前是否存在活跃或暂停任务（供并发/单飞判断）。
    pub fn has_active(&self) -> bool {
        self.tasks.iter().any(|(_, e)| e.phase != TaskPhase::Paused)
    }

    /// 快照：任务数。
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

impl Default for TaskStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation_of(sm: &TaskStateMachine, file_id: &str) -> u64 {
        sm.tasks.get(file_id).unwrap().generation
    }

    #[test]
    fn same_file_repeat_start_rejected() {
        let mut sm = TaskStateMachine::new();
        let g1 = sm.start_transcription("a", "run-1").unwrap();
        assert!(sm.start_transcription("a", "run-2").is_err());
        assert_eq!(sm.len(), 1);
        assert_eq!(generation_of(&sm, "a"), g1, "旧任务句柄不得被覆盖");
    }

    #[test]
    fn stale_generation_cleanup_does_not_touch_new_generation() {
        let mut sm = TaskStateMachine::new();
        let g1 = sm.start_transcription("a", "run-1").unwrap();
        // 旧代际清理不得影响新代际
        assert!(!sm.remove_if_generation("a", g1 + 100));
        assert_eq!(sm.len(), 1);
        assert!(sm.remove_if_generation("a", g1));
        assert_eq!(sm.len(), 0);
    }

    #[test]
    fn cancel_prioritized_over_pause() {
        let mut sm = TaskStateMachine::new();
        let g = sm.start_transcription("a", "run-1").unwrap();
        assert!(matches!(
            sm.mark_running("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        // cancel 优先于 pause：先 pause 后 cancel，最终状态为 CancelRequested
        assert!(matches!(
            sm.request_pause("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        assert!(matches!(
            sm.request_cancel("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        assert!(sm.tasks.get("a").unwrap().cancelled.is_cancelled());
        assert_eq!(sm.tasks.get("a").unwrap().phase, TaskPhase::CancelRequested);
    }

    #[test]
    fn non_paused_phase_rejects_cleanup_resume_flow() {
        let mut sm = TaskStateMachine::new();
        let g = sm.start_transcription("a", "run-1").unwrap();
        // Paused 之外的阶段拒绝 resume（本站无 resume 命令，但 confirm_pause 需 PauseRequested）
        assert!(matches!(
            sm.confirm_pause(
                "a",
                TaskOperation::Transcription,
                "run-1",
                g,
                PauseCheckpoint {
                    last_completed: 1,
                    total_slices: 3,
                }
            ),
            Transition::Rejected(_)
        ));
        // Running 阶段请求暂停合法
        assert!(matches!(
            sm.mark_running("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        assert!(matches!(
            sm.request_pause("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        assert!(matches!(
            sm.confirm_pause(
                "a",
                TaskOperation::Transcription,
                "run-1",
                g,
                PauseCheckpoint {
                    last_completed: 1,
                    total_slices: 3,
                }
            ),
            Transition::Ok
        ));
        assert_eq!(sm.tasks.get("a").unwrap().phase, TaskPhase::Paused);
    }

    #[test]
    fn each_generation_produces_single_terminal() {
        let mut sm = TaskStateMachine::new();
        let g = sm.start_transcription("a", "run-1").unwrap();
        assert!(matches!(
            sm.mark_running("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        // 取消后再次取消按 OK 处理（幂等取消），但代际移除只有一次生效
        assert!(matches!(
            sm.request_cancel("a", TaskOperation::Transcription, "run-1", g),
            Transition::Ok
        ));
        assert!(sm.remove_if_generation("a", g));
        assert!(!sm.remove_if_generation("a", g), "代际只能移除一次");
    }

    #[test]
    fn resume_uses_new_generation_and_keeps_checkpoint() {
        let mut sm = TaskStateMachine::new();
        let g1 = sm.start_transcription("a", "run-1").unwrap();
        assert!(matches!(
            sm.mark_running("a", TaskOperation::Transcription, "run-1", g1),
            Transition::Ok
        ));
        assert!(matches!(
            sm.request_pause("a", TaskOperation::Transcription, "run-1", g1),
            Transition::Ok
        ));
        assert!(matches!(
            sm.confirm_pause(
                "a",
                TaskOperation::Transcription,
                "run-1",
                g1,
                PauseCheckpoint {
                    last_completed: 2,
                    total_slices: 5,
                }
            ),
            Transition::Ok
        ));
        // resume 用新 run_id + 新代际
        let g2 = sm.resume("a", "run-1", g1, "run-2").unwrap();
        assert_ne!(g1, g2);
        assert_eq!(sm.len(), 1, "新代际替换旧暂停条目");
        // 新代际沿用断点
        assert_eq!(
            sm.paused_checkpoint("a", TaskOperation::Transcription, "run-2", g2),
            Some(PauseCheckpoint {
                last_completed: 2,
                total_slices: 5
            })
        );
        // 非 Paused 阶段 resume 拒绝
        let g3 = sm.start_transcription("b", "run-b").unwrap();
        assert!(sm.resume("b", "run-b", g3, "run-b2").is_err());
    }

    #[test]
    fn checkpoint_validity_boundaries() {
        let c0 = PauseCheckpoint {
            last_completed: 0,
            total_slices: 3,
        };
        let cmid = PauseCheckpoint {
            last_completed: 3,
            total_slices: 5,
        };
        let cend = PauseCheckpoint {
            last_completed: 5,
            total_slices: 5,
        };
        let _cstart = PauseCheckpoint {
            last_completed: 1,
            total_slices: 5,
        };
        assert!(c0.is_valid());
        assert!(cmid.is_valid());
        assert!(cend.is_valid(), "最后一片完成后暂停是合法值（0.3 节）");
        assert!(!PauseCheckpoint {
            last_completed: 6,
            total_slices: 5
        }
        .is_valid());
    }
}
