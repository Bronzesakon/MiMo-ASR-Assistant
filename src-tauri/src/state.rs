//! 共享运行时状态（清单 B-11 自 lib.rs 拆出）：事件发射器、任务注册表、ffmpeg 句柄。
//! P2-01：统一结构化事件信封（TaskEvent）与任务终结辅助（finish_task）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter as TauriEmitterTrait};

use crate::{api, cancel, ffmpeg_exe};

// ============================================================
// 统一事件信封（P2-01，参考清单 1.1）
// ============================================================

/// 任务操作类型（转写 / 规整）。
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskOperation {
    Transcription,
    Polish,
}

/// 任务事件类型。
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskEventKind {
    Started,
    Stage,
    Progress,
    Chunk,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

/// 转写错误事件载荷（P2-04：与规整错误分离）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionErrorPayload {
    pub error: String,
    pub retryable: bool,
    pub checkpoint_valid: bool,
}

/// 规整错误事件载荷。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolishErrorPayload {
    pub error: String,
    pub retryable: bool,
}

/// 恢复断点（P1-04：后端权威保存与校验）。
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PauseCheckpoint {
    pub last_completed: u32,
    pub total_slices: u32,
}

impl PauseCheckpoint {
    pub fn is_valid(&self) -> bool {
        self.last_completed <= self.total_slices
    }
}

// ============================================================
// Tauri 事件发射器
// ============================================================

/// 带 file_id 标签的事件发射器，所有 emit 自动携带 file_id。
/// P2-01/P2-03：携带 operation + runId + generation，chunk 事件输出统一信封
/// （含单调递增 sequence），前端按 fileId+operation+runId+generation 路由并幂等。
pub struct TaggedEmitter {
    pub handle: AppHandle,
    pub file_id: String,
    pub operation: TaskOperation,
    pub run_id: String,
    pub generation: u64,
    pub output: Option<Arc<std::sync::Mutex<String>>>,
}

impl TaggedEmitter {
    pub fn new(
        handle: AppHandle,
        file_id: impl Into<String>,
        operation: TaskOperation,
        run_id: impl Into<String>,
        generation: u64,
    ) -> Self {
        Self {
            handle,
            file_id: file_id.into(),
            operation,
            run_id: run_id.into(),
            generation,
            output: None,
        }
    }

    pub fn with_output(mut self, output: Arc<std::sync::Mutex<String>>) -> Self {
        self.output = Some(output);
        self
    }

    fn next_sequence(&self) -> u64 {
        next_task_event_sequence()
    }
}

pub fn next_task_event_sequence() -> u64 {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl api::Emitter for TaggedEmitter {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String> {
        // token-update：非任务事件，保持原有 { text } 载荷（前端按事件名单独解析）
        if event == "token-update" {
            let json = serde_json::json!({ "file_id": self.file_id, "text": payload });
            return self
                .handle
                .emit(event, &json.to_string())
                .map_err(|e| e.to_string());
        }
        // chunk 事件：统一信封，payload 为 { text }
        let seq = self.next_sequence();
        let envelope = task_event_json(
            &self.file_id,
            self.operation,
            &self.run_id,
            self.generation,
            seq,
            TaskEventKind::Chunk,
            &serde_json::json!({ "text": payload }),
        );
        self.handle
            .emit(event, &envelope)
            .map_err(|e| e.to_string())?;
        if let Some(output) = &self.output {
            output
                .lock()
                .map_err(|_| "任务输出锁已损坏".to_string())?
                .push_str(payload);
        }
        Ok(())
    }
}

// ============================================================
// 任务注册表
// ============================================================

pub struct TaskEntry {
    /// B-08：组合式取消原语（替代 Arc<AtomicBool>）；阻塞域经
    /// `token.atomic()` 驱动 ffmpeg_exe 转码轮询，接口保持不变。
    pub cancelled: cancel::CancelToken,
    pub pause_flag: Arc<AtomicBool>,
    /// T-02：任务代际。后端创建任务时生成，用于让迟到的清理/旧任务退出
    /// 只能影响其持有的任务，无法移除新代际的注册条目。
    pub generation: u64,
    /// B-04：后端登记的任务临时目录。前端回传的 temp_dir 字符串不再作为
    /// 删除/继续转写的授权依据，必须通过 file_id 在注册表中找回真实路径。
    /// 活跃任务创建 temp 目录后置 Some；暂停任务保留条目以供 resume/cleanup 核对。
    pub temp_dir: Option<PathBuf>,
    /// P2-03：任务操作类型（转写/规整）。
    pub operation: TaskOperation,
    /// P2-03：前端发起操作时提交的运行标识。
    pub run_id: String,
    /// P1-06：任务运行阶段（Panel 由 task.rs 的 TaskPhase 提供，此处保留最小运行态）。
    pub phase: TaskPhase,
    /// P1-04：后端权威保存的恢复断点（暂停时写入，resume 时校验）。
    pub checkpoint: Option<PauseCheckpoint>,
    /// P2-06：流式发射 chunk 的同时累积的完整输出，终态时用于校准。
    pub output: Arc<std::sync::Mutex<String>>,
}

/// 任务运行阶段（P1 收敛，与 task.rs 的 TaskPhase 对齐）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskPhase {
    Starting,
    Running,
    PauseRequested,
    Paused,
    CancelRequested,
}

impl TaskEntry {
    pub fn new(cancelled: cancel::CancelToken, pause_flag: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            pause_flag,
            generation: Self::next_generation(),
            temp_dir: None,
            operation: TaskOperation::Transcription,
            run_id: String::new(),
            phase: TaskPhase::Starting,
            checkpoint: None,
            output: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// 生成单调递增的任务代际（进程内唯一）
    fn next_generation() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(1);
        SEQ.fetch_add(1, Ordering::Relaxed)
    }
}

pub struct TaskRegistry {
    pub tasks: tokio::sync::Mutex<std::collections::HashMap<String, TaskEntry>>,
}

/// 命令错误（清单 1.2）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub checkpoint_valid: bool,
}

impl CommandError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
            checkpoint_valid: true,
        }
    }
}

impl TaskRegistry {
    /// 注册新任务；同一 file_id 已存在活跃或暂停条目时拒绝，防止覆盖旧任务句柄。
    /// 返回 Err 时旧任务句柄保持原样，调用方不得继续启动。
    pub async fn try_register(&self, file_id: &str, entry: TaskEntry) -> Result<(), String> {
        let mut tasks = self.tasks.lock().await;
        match tasks.get(file_id) {
            Some(_) => Err("该文件已有进行中的任务，请先等待其完成或取消".to_string()),
            None => {
                tasks.insert(file_id.to_string(), entry);
                Ok(())
            }
        }
    }

    /// 条件式移除：仅当当前条目代际与给定代际一致时移除（旧任务退出不得移除新任务）。
    pub async fn remove_if_generation(&self, file_id: &str, generation: u64) -> bool {
        let mut tasks = self.tasks.lock().await;
        let same = tasks
            .get(file_id)
            .map(|e| e.generation == generation)
            .unwrap_or(false);
        if same {
            tasks.remove(file_id);
        }
        same
    }

    /// 登记临时目录到指定代际任务（P4-03：仅在元数据落盘成功后调用）。
    pub async fn register_temp_dir(&self, file_id: &str, generation: u64, dir: PathBuf) {
        let mut tasks = self.tasks.lock().await;
        if let Some(e) = tasks.get_mut(file_id) {
            if e.generation == generation {
                e.temp_dir = Some(dir);
            }
        }
    }

    /// 读取指定代际任务的临时目录（受信任路径）。
    pub async fn temp_dir_of(&self, file_id: &str, generation: u64) -> Option<PathBuf> {
        self.tasks.lock().await.get(file_id).and_then(|e| {
            if e.generation == generation {
                e.temp_dir.clone()
            } else {
                None
            }
        })
    }

    /// 条件式更新任务阶段（仅当代际匹配时）。
    pub async fn set_phase(&self, file_id: &str, generation: u64, phase: TaskPhase) {
        let mut tasks = self.tasks.lock().await;
        if let Some(e) = tasks.get_mut(file_id) {
            if e.generation == generation {
                e.phase = phase;
            }
        }
    }

    pub async fn commit_pause(
        &self,
        file_id: &str,
        generation: u64,
        checkpoint: PauseCheckpoint,
    ) -> bool {
        let mut tasks = self.tasks.lock().await;
        let Some(entry) = tasks.get_mut(file_id) else {
            return false;
        };
        if entry.generation != generation
            || entry.phase != TaskPhase::PauseRequested
            || entry.cancelled.is_cancelled()
        {
            return false;
        }
        entry.checkpoint = Some(checkpoint);
        entry.phase = TaskPhase::Paused;
        true
    }

    /// 读取完整输出（P2-06 终态校准）。
    pub async fn take_output(&self, file_id: &str, generation: u64) -> String {
        let mut tasks = self.tasks.lock().await;
        if let Some(e) = tasks.get_mut(file_id) {
            if e.generation == generation {
                if let Ok(mut out) = e.output.lock() {
                    let s = std::mem::take(&mut *out);
                    return s;
                }
            }
        }
        String::new()
    }
}

/// P1-06：统一任务终结与资源清理。
/// completed/failed/cancelled 都经此函数条件式移除注册项；返回该代际是否被移除。
/// transcription 的 completed/failed/cancelled 清理临时目录；paused 保留。
/// 递归目录删除统一放入 `tokio::task::spawn_blocking`。
///
/// `cleanup_temp` 为布尔：true=清理临时目录（非 paused 终态），false=保留。
pub async fn finish_task(
    registry: &TaskRegistry,
    file_id: &str,
    generation: u64,
    cleanup_temp: bool,
) -> bool {
    // 取受信任临时路径（spawn_blocking 前完成所有权转移）
    let temp_path = registry.temp_dir_of(file_id, generation).await;
    let removed = registry.remove_if_generation(file_id, generation).await;
    if cleanup_temp {
        if let Some(dir) = temp_path {
            let dir = dir.clone();
            // P4-04：cleanup_temp 现返回 Result；失败记录结构化清理 issue，
            // 残留目录由启动时 cleanup_legacy_temp 统一回收。
            match tokio::task::spawn_blocking(move || crate::audio::cleanup_temp(&dir)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => crate::warn!("任务终结清理临时目录失败: {}", e),
                Err(je) => crate::warn!("任务终结清理临时目录任务异常: {}", je),
            }
        }
    }
    removed
}

/// 事件发射辅助：构造统一信封并 emit；失败记录 event kind + runId + generation。
/// 终态发射失败时任务真实 outcome 保持原值，前端依靠 invoke outcome 或 snapshot 对账。
pub fn emit_task_event(
    handle: &AppHandle,
    event_name: &str,
    envelope: &str,
    run_id: &str,
    generation: u64,
) {
    if let Err(e) = handle.emit(event_name, envelope) {
        crate::warn!(
            "[event] 发射失败 kind={} runId={} generation={}: {}",
            event_name,
            run_id,
            generation,
            e
        );
    }
}

/// 构造统一任务事件信封的 JSON 字符串。
/// 信封字段固定为 `schemaVersion/fileId/operation/runId/generation/sequence/kind/payload`，
/// `payload` 按引用序列化，泛型仅需 `Serialize`，无需 `Clone`。
pub fn task_event_json<T: Serialize>(
    file_id: &str,
    operation: TaskOperation,
    run_id: &str,
    generation: u64,
    sequence: u64,
    kind: TaskEventKind,
    payload: &T,
) -> String {
    serde_json::to_string(&serde_json::json!({
        "schemaVersion": 1,
        "fileId": file_id,
        "operation": operation,
        "runId": run_id,
        "generation": generation,
        "sequence": sequence,
        "kind": kind,
        "payload": payload,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

// ============================================================
// 任务注册表
// ============================================================

/// 捆绑 ffmpeg.exe 句柄（setup 阶段一次性 resolve 并注入）。
/// None 表示降级态：转码/时长探测功能不可用，但应用正常运行。
pub struct FfmpegBinState(pub Option<ffmpeg_exe::FfmpegBin>);
