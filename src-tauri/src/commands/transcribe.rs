//! 转写域命令（清单 B-11 自 lib.rs 拆出）：
//! process_and_transcribe、pause/cancel/resume、临时目录清理。
//! P1：暂停分支错误不吞、统一终结、后端权威断点；P2：统一事件信封 + generation。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::api::{self};
use crate::state::{
    finish_task, CommandError, PauseCheckpoint, TaggedEmitter, TaskEntry, TaskOperation, TaskPhase,
    TaskRegistry,
};
use crate::{audio, cancel, constants, rate_limiter};

use super::ApiParams;

// ============================================================
// 统一事件包装（P2-01）
// ============================================================

/// 在 TauriEmitter 之上发射统一事件信封。
struct Envelope {
    handle: AppHandle,
    file_id: String,
    operation: TaskOperation,
    run_id: String,
    generation: u64,
}

impl Envelope {
    fn new(
        handle: AppHandle,
        file_id: &str,
        operation: TaskOperation,
        run_id: &str,
        generation: u64,
    ) -> Self {
        Self {
            handle,
            file_id: file_id.to_string(),
            operation,
            run_id: run_id.to_string(),
            generation,
        }
    }

    fn next(&self) -> u64 {
        crate::state::next_task_event_sequence()
    }

    fn emit<T: serde::Serialize>(
        &self,
        event: &str,
        kind: crate::state::TaskEventKind,
        payload: &T,
    ) {
        let seq = self.next();
        let envelope = crate::state::task_event_json(
            &self.file_id,
            self.operation,
            &self.run_id,
            self.generation,
            seq,
            kind,
            payload,
        );
        crate::state::emit_task_event(
            &self.handle,
            event,
            &envelope,
            &self.run_id,
            self.generation,
        );
    }
}

// ============================================================
// 取消 / 暂停命令
// ============================================================

/// 取消任务（P1-02/P2-05）：
/// - 参数增加 operation/runId/generation，返回结构化确认；
/// - 未找到匹配任务返回 TASK_NOT_FOUND；
/// - 已暂停任务直接进入统一取消终结流程（清理注册项 + 临时目录）。
#[tauri::command]
pub async fn cancel_task(
    handle: AppHandle,
    file_id: String,
    file_name: String,
    operation: String,
    run_id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let registry = handle.state::<TaskRegistry>();
    let mut tasks = registry.tasks.lock().await;
    let entry = tasks.get_mut(&file_id);
    let Some(entry) = entry else {
        crate::info!("[{}] 取消请求时任务已结束，按已清理处理", file_name);
        return Ok(());
    };
    // 代际与 run_id 必须匹配，防止旧代际取消污染新任务
    if generation != 0 && entry.generation != generation
        || !entry.run_id.is_empty() && entry.run_id != run_id
    {
        return Err(CommandError::new(
            "GENERATION_MISMATCH",
            "任务代际不匹配，无法取消",
        ));
    }
    let op = TaskOperation::from_str(&operation);
    if entry.operation != op {
        return Err(CommandError::new("NOT_FOUND", "未找到匹配的操作类型任务"));
    }
    let paused = entry.phase == TaskPhase::Paused;
    let actual_generation = entry.generation;
    entry.cancelled.cancel();
    entry.phase = TaskPhase::CancelRequested;
    drop(tasks);
    crate::info!(
        "[{}] 任务已取消请求: operation={}, generation={}, phase={:?}",
        file_name,
        operation,
        actual_generation,
        if paused { "paused" } else { "running" }
    );

    // 已暂停任务：直接进入统一取消终结流程（清理注册项 + 临时目录）
    if paused {
        finish_task(&registry, &file_id, actual_generation, true).await;
    } else {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(CommandError::new(
                    "CANCEL_TIMEOUT",
                    "后台任务未能在限定时间内退出",
                ));
            }
            let finished = {
                let tasks = registry.tasks.lock().await;
                tasks
                    .get(&file_id)
                    .is_none_or(|task| task.generation != actual_generation)
            };
            if finished {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    Ok(())
}

/// 暂停任务（P1-03）：仅 Transcription + Running 接受；返回结构化确认。
#[tauri::command]
pub async fn pause_task(
    handle: AppHandle,
    file_id: String,
    file_name: String,
    run_id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let registry = handle.state::<TaskRegistry>();
    let mut tasks = registry.tasks.lock().await;
    let entry = tasks.get_mut(&file_id);
    let Some(entry) = entry else {
        return Err(CommandError::new("TASK_NOT_FOUND", "未找到匹配的任务"));
    };
    if generation != 0 && entry.generation != generation || entry.run_id != run_id {
        return Err(CommandError::new(
            "GENERATION_MISMATCH",
            "任务代际不匹配，无法暂停",
        ));
    }
    if entry.operation != TaskOperation::Transcription {
        return Err(CommandError::new("NOT_TRANSCRIPTION", "仅转写任务支持暂停"));
    }
    if entry.phase != TaskPhase::Running {
        return Err(CommandError::new(
            "NOT_RUNNING",
            "任务不在运行状态，无法暂停",
        ));
    }
    entry.phase = TaskPhase::PauseRequested;
    entry.pause_flag.store(true, Ordering::Release);
    let _ = &file_name;
    Ok(())
}

/// 清理暂停任务的临时目录（P1-05：仅 Paused 阶段接受，作为"放弃断点"命令）。
#[tauri::command]
pub async fn cleanup_file_temp(
    handle: AppHandle,
    file_id: String,
    run_id: String,
    generation: u64,
) -> Result<(), CommandError> {
    let registry = handle.state::<TaskRegistry>();
    let mut tasks = registry.tasks.lock().await;
    let entry = tasks.get_mut(&file_id);
    let Some(entry) = entry else {
        return Err(CommandError::new("TASK_NOT_FOUND", "未找到匹配的任务"));
    };
    if entry.generation != generation || entry.run_id != run_id {
        return Err(CommandError::new("GENERATION_MISMATCH", "任务代际不匹配"));
    }
    if entry.phase != TaskPhase::Paused {
        return Err(CommandError::new("NOT_PAUSED", "仅暂停任务可放弃断点"));
    }
    let temp_path = entry.temp_dir.clone();
    entry.phase = TaskPhase::CancelRequested;
    drop(tasks);

    let Some(temp_path) = temp_path else {
        return Err(CommandError::new("NO_TEMP", "任务无临时目录"));
    };
    audio::verify_app_task_dir(&temp_path).map_err(|e| CommandError::new("GUARD", e))?;
    let cleanup_path = temp_path.clone();
    let removed =
        tokio::task::spawn_blocking(move || audio::remove_task_dir_guarded(&cleanup_path))
            .await
            .map_err(|e| CommandError::new("IO", format!("清理任务异常: {}", e)))?
            .map_err(|e| CommandError::new("IO", e));
    if let Err(error) = removed {
        registry
            .set_phase(&file_id, generation, TaskPhase::Paused)
            .await;
        return Err(error);
    }
    // 删除成功后移除注册表条目（代际条件式）
    finish_task(&registry, &file_id, generation, false).await;
    Ok(())
}

// ============================================================
// 恢复转写（P1-04：后端权威断点）
// ============================================================

/// 从暂停处继续转写。
/// 删除 temp_dir 与 start_index 参数；后端从匹配的 paused TaskEntry 读取
/// temp_dir/checkpoint 并校验，下一片固定为 last_completed + 1。
#[tauri::command]
pub async fn resume_transcription(
    handle: AppHandle,
    file_id: String,
    file_name: String,
    operation: String,
    run_id: String,
    api_params: ApiParams,
) -> Result<u64, CommandError> {
    let registry = handle.state::<TaskRegistry>();
    let op = TaskOperation::from_str(&operation);

    // 读取暂停条目的 checkpoint 与 temp 路径（锁内克隆，不先 remove）
    let (temp_path, checkpoint, old_generation, previous_output) = {
        let tasks = registry.tasks.lock().await;
        match tasks.get(&file_id) {
            Some(e) if e.operation == op && e.phase == TaskPhase::Paused => (
                e.temp_dir.clone(),
                e.checkpoint.clone(),
                e.generation,
                e.output.clone(),
            ),
            _ => {
                return Err(CommandError::new(
                    "TASK_NOT_FOUND",
                    "无该文件的暂停数据，请重新转写",
                ))
            }
        }
    };
    let Some(checkpoint) = checkpoint else {
        return Err(CommandError::new(
            "NO_CHECKPOINT",
            "暂停数据缺少断点，请重新转写",
        ));
    };
    if !checkpoint.is_valid() {
        return Err(CommandError::new(
            "INVALID_CHECKPOINT",
            "暂停断点无效，请重新转写",
        ));
    }
    let Some(temp_path) = temp_path else {
        return Err(CommandError::new(
            "NO_TEMP",
            "暂停数据缺少临时目录，请重新转写",
        ));
    };
    if let Err(e) = audio::verify_app_task_dir(&temp_path) {
        crate::warn!("[{}] resume 路径守卫拒绝: {}", file_name, e);
        return Err(CommandError::new("GUARD", "暂停数据校验失败，请重新转写"));
    }
    if !temp_path.exists() {
        return Err(CommandError::new("NO_TEMP", "临时目录不存在，请重新转写"));
    }

    // 磁盘切片数量必须等于 total_slices
    let actual_total = audio::count_slice_files(&temp_path);
    if actual_total != checkpoint.total_slices {
        return Err(CommandError::new(
            "SLICE_MISMATCH",
            format!(
                "切片数量不一致（期望 {}，实际 {}），请重新转写",
                checkpoint.total_slices, actual_total
            ),
        ));
    }

    let config: api::ApiConfig = api_params.into();
    let limiter = handle.state::<rate_limiter::AsrRateLimiter>();

    // 生成新代际并条件式替换旧暂停条目（沿用 temp 与 checkpoint）
    let token = cancel::CancelToken::new();
    let pause_flag = Arc::new(AtomicBool::new(false));
    let mut entry = TaskEntry::new(token.clone(), pause_flag.clone());
    entry.operation = op;
    entry.run_id = run_id.clone();
    entry.temp_dir = Some(temp_path.clone());
    entry.phase = TaskPhase::Running;
    entry.output = previous_output;
    let output = entry.output.clone();
    let generation = entry.generation;
    {
        let mut tasks = registry.tasks.lock().await;
        let still_paused = tasks
            .get(&file_id)
            .map(|e| e.phase == TaskPhase::Paused && e.generation == old_generation)
            .unwrap_or(false);
        if !still_paused {
            return Err(CommandError::new("STALE", "暂停数据已失效，请重新转写"));
        }
        tasks.insert(file_id.clone(), entry);
    }

    let envelope = Envelope::new(handle.clone(), &file_id, op, &run_id, generation);

    // 下一片固定为 last_completed + 1
    let start_index = checkpoint.last_completed + 1;
    let total = checkpoint.total_slices;
    crate::info!(
        "[{}] resume_transcription 开始: {} 个切片, 从分片 {} 继续",
        file_name,
        total,
        start_index
    );

    let slice_files = audio::collect_slice_files(&temp_path);
    let start_time = std::time::Instant::now();

    for (i, (_start_sec, slice_path)) in slice_files.iter().enumerate() {
        let index = (i as u32) + 1; // 1-based
        if index < start_index {
            continue;
        }

        if token.is_cancelled() {
            crate::info!("[{}] resume_transcription 已取消", file_name);
            finish_task(&registry, &file_id, generation, true).await;
            envelope.emit(
                "transcription-cancelled",
                crate::state::TaskEventKind::Cancelled,
                &serde_json::json!({}),
            );
            return Ok(generation);
        }

        // 发射分片进度（统一信封）
        envelope.emit(
            "transcription-progress",
            crate::state::TaskEventKind::Progress,
            &serde_json::json!({ "current": index, "total": total }),
        );

        let data_url = match audio::encode_slice_to_data_url(slice_path) {
            Ok(d) => d,
            Err(e) => {
                let err_msg = format!("切片编码失败: {}", e);
                crate::error!("[{}] 片段 {} 编码失败: {}", file_id, index, e);
                finish_task(&registry, &file_id, generation, true).await;
                envelope.emit(
                    "transcription-error",
                    crate::state::TaskEventKind::Failed,
                    &crate::state::TranscriptionErrorPayload {
                        error: err_msg.clone(),
                        retryable: false,
                        checkpoint_valid: false,
                    },
                );
                return Err(CommandError::new("SLICE_ERR", err_msg));
            }
        };

        let _permit = match limiter.0.acquire_permit(&token).await {
            Ok(p) => p,
            Err(_) => {
                crate::info!("[{}] resume 排队等待许可时已取消", file_name);
                finish_task(&registry, &file_id, generation, true).await;
                envelope.emit(
                    "transcription-cancelled",
                    crate::state::TaskEventKind::Cancelled,
                    &serde_json::json!({}),
                );
                return Ok(generation);
            }
        };

        let tagged = TaggedEmitter::new(handle.clone(), &file_id, op, &run_id, generation)
            .with_output(output.clone());
        let slice_start = std::time::Instant::now();
        match api::transcribe_with_retry(&data_url, &config, &tagged, &token, Some(&limiter.0))
            .await
        {
            Ok(()) => {}
            Err(e) if e.is_cancelled() => {
                crate::info!(
                    "[{}] resume_transcription 已取消 (分片 {})",
                    file_name,
                    index
                );
                finish_task(&registry, &file_id, generation, true).await;
                envelope.emit(
                    "transcription-cancelled",
                    crate::state::TaskEventKind::Cancelled,
                    &serde_json::json!({}),
                );
                return Ok(generation);
            }
            Err(e) => {
                crate::error!("[{}] 片段 {} 最终失败: {}", file_id, index, e);
                finish_task(&registry, &file_id, generation, true).await;
                envelope.emit(
                    "transcription-error",
                    crate::state::TaskEventKind::Failed,
                    &crate::state::TranscriptionErrorPayload {
                        error: e.to_string(),
                        retryable: e.is_retryable(),
                        checkpoint_valid: false,
                    },
                );
                return Err(CommandError::new("SLICE_ERR", e.to_string()));
            }
        }

        let slice_ms = slice_start.elapsed().as_millis();
        let elapsed_s = start_time.elapsed().as_secs_f64();
        let upload_speed = if slice_ms > 0 {
            (data_url.len() as f64 / slice_ms as f64) * 1000.0
        } else {
            0.0
        };
        envelope.emit(
            "status-update",
            crate::state::TaskEventKind::Stage,
            &serde_json::json!({
                "slice": index, "total": total,
                "elapsed": elapsed_s, "upload_speed": upload_speed,
            }),
        );

        // 暂停检查：原子更新断点（P1-04），保留注册项与 temp
        if pause_flag.load(Ordering::Acquire) {
            let ck = PauseCheckpoint {
                last_completed: index,
                total_slices: total,
            };
            if !registry
                .commit_pause(&file_id, generation, ck.clone())
                .await
            {
                if token.is_cancelled() {
                    finish_task(&registry, &file_id, generation, true).await;
                    envelope.emit(
                        "transcription-cancelled",
                        crate::state::TaskEventKind::Cancelled,
                        &serde_json::json!({}),
                    );
                    return Ok(generation);
                }
                return Err(CommandError::new("PAUSE_RACE", "暂停状态已变化"));
            }
            envelope.emit(
                "transcription-paused",
                crate::state::TaskEventKind::Paused,
                &ck,
            );
            crate::info!(
                "[{}] resume_transcription 已暂停 (分片 {}/{})",
                file_name,
                index,
                total
            );
            return Ok(generation);
        }
    }

    if token.is_cancelled() {
        finish_task(&registry, &file_id, generation, true).await;
        envelope.emit(
            "transcription-cancelled",
            crate::state::TaskEventKind::Cancelled,
            &serde_json::json!({}),
        );
        return Ok(generation);
    }

    // 全部完成：清理注册项 + temp，返回完整文本作为 outcome
    let text = registry.take_output(&file_id, generation).await;
    finish_task(&registry, &file_id, generation, true).await;
    envelope.emit(
        "transcription-complete",
        crate::state::TaskEventKind::Completed,
        &serde_json::json!({ "text": text }),
    );
    crate::info!("[{}] resume_transcription 完成", file_name);
    let _ = text;
    Ok(generation)
}

// ============================================================
// 主转写流程（P1-01/P1-06）
// ============================================================

#[tauri::command]
pub async fn process_and_transcribe(
    handle: AppHandle,
    file_id: String,
    file_path: String,
    operation: String,
    run_id: String,
    api_params: ApiParams,
) -> Result<u64, CommandError> {
    let path = PathBuf::from(&file_path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_id.clone());
    crate::info!("[{}] process_and_transcribe 开始", file_name);

    let op = TaskOperation::from_str(&operation);
    let config: api::ApiConfig = api_params.into();
    let limiter = handle.state::<rate_limiter::AsrRateLimiter>();
    let registry = handle.state::<TaskRegistry>();

    // 注册任务（含 operation + run_id）
    let token = cancel::CancelToken::new();
    let pause_flag = Arc::new(AtomicBool::new(false));
    let mut entry = TaskEntry::new(token.clone(), pause_flag.clone());
    entry.operation = op;
    entry.run_id = run_id.clone();
    let output = entry.output.clone();
    let generation = entry.generation;
    registry.try_register(&file_id, entry).await.map_err(|e| {
        crate::warn!("[{}] process_and_transcribe 注册被拒绝: {}", file_name, e);
        CommandError::new("REGISTER", e)
    })?;

    let envelope = Envelope::new(handle.clone(), &file_id, op, &run_id, generation);

    if !path.exists() {
        let err_msg = format!("文件不存在: {}", file_path);
        envelope.emit(
            "transcription-error",
            crate::state::TaskEventKind::Failed,
            &crate::state::TranscriptionErrorPayload {
                error: err_msg.clone(),
                retryable: false,
                checkpoint_valid: false,
            },
        );
        finish_task(&registry, &file_id, generation, false).await;
        return Err(CommandError::new("NOT_FOUND", err_msg));
    }

    // "转码中"阶段
    envelope.emit(
        "process-status",
        crate::state::TaskEventKind::Stage,
        &serde_json::json!({ "stage": "转码中" }),
    );

    // 创建任务临时目录并登记（P4-03：元数据落盘成功后登记）
    let temp_dir = match audio::create_task_temp_dir() {
        Ok(t) => {
            registry
                .register_temp_dir(&file_id, generation, t.clone())
                .await;
            t
        }
        Err(e) => {
            let err_msg = format!("创建临时目录失败: {}", e);
            crate::error!("[{}] {}", file_name, err_msg);
            envelope.emit(
                "transcription-error",
                crate::state::TaskEventKind::Failed,
                &crate::state::TranscriptionErrorPayload {
                    error: err_msg.clone(),
                    retryable: false,
                    checkpoint_valid: false,
                },
            );
            finish_task(&registry, &file_id, generation, false).await;
            return Err(CommandError::new("TEMP_DIR", err_msg));
        }
    };

    let (tx, rx) =
        tokio::sync::mpsc::channel::<(u32, u32, String)>(constants::SLICE_CHANNEL_CAPACITY);
    let mut rx = Some(rx);

    let slice_sec = crate::config::load_config_inner(&handle).slice_seconds;

    let ffmpeg_bin = match handle.state::<crate::state::FfmpegBinState>().0.clone() {
        Some(bin) => bin,
        None => {
            let err_msg = "ffmpeg 组件不可用，无法转码音频，请检查安装完整性后重试".to_string();
            crate::error!("[{}] {}", file_name, err_msg);
            envelope.emit(
                "transcription-error",
                crate::state::TaskEventKind::Failed,
                &crate::state::TranscriptionErrorPayload {
                    error: err_msg.clone(),
                    retryable: false,
                    checkpoint_valid: false,
                },
            );
            finish_task(&registry, &file_id, generation, true).await;
            return Err(CommandError::new("FFMPEG", err_msg));
        }
    };

    registry
        .set_phase(&file_id, generation, TaskPhase::Running)
        .await;

    let slice_temp_dir = temp_dir.clone();
    let slice_cancel = token.atomic();
    let blocking_task = tokio::task::spawn_blocking(move || {
        audio::convert_and_slice(
            &path,
            &slice_temp_dir,
            Some(slice_sec),
            &slice_cancel,
            &ffmpeg_bin,
            &|index, total, data_url| {
                tx.blocking_send((index, total, data_url)).is_ok()
                    && !slice_cancel.load(std::sync::atomic::Ordering::Acquire)
            },
        )
    });

    // "分片中"阶段
    envelope.emit(
        "process-status",
        crate::state::TaskEventKind::Stage,
        &serde_json::json!({ "stage": "分片中" }),
    );

    // 逐片接收并立即发送 API
    while let Some(rx_inner) = rx.as_mut() {
        let Some((index, total, data_url)) = rx_inner.recv().await else {
            break;
        };
        if token.is_cancelled() {
            crate::info!("[{}] 消费循环检测到取消，停止接收后续分片", file_name);
            rx.take();
            break;
        }

        envelope.emit(
            "transcription-progress",
            crate::state::TaskEventKind::Progress,
            &serde_json::json!({ "current": index, "total": total }),
        );
        crate::info!(
            "[{}] 分片 {} 已进入上传流程 ({}/{})",
            file_name,
            index,
            index,
            total
        );

        let _rate_permit = match limiter.0.acquire_permit(&token).await {
            Ok(p) => p,
            Err(_) => {
                rx.take();
                break;
            }
        };

        let tagged = TaggedEmitter::new(handle.clone(), &file_id, op, &run_id, generation)
            .with_output(output.clone());
        match api::transcribe_with_retry(&data_url, &config, &tagged, &token, Some(&limiter.0))
            .await
        {
            Ok(()) => crate::info!(
                "[{}] 分片 {} 上传转写完成 ({}/{})",
                file_name,
                index,
                index,
                total
            ),
            Err(e) if e.is_cancelled() => {
                crate::info!("[{}] 分片 {} 上传期间收到取消", file_name, index);
                rx.take();
                break;
            }
            Err(e) => {
                crate::error!("[{}] 片段 {} 最终失败: {}", file_id, index, e);
                token.cancel();
                rx.take();
                let producer_result = blocking_task.await;
                crate::info!(
                    "[{}] 上传失败后分片生产者已退出: {:?}",
                    file_name,
                    producer_result
                );
                envelope.emit(
                    "transcription-error",
                    crate::state::TaskEventKind::Failed,
                    &crate::state::TranscriptionErrorPayload {
                        error: e.to_string(),
                        retryable: e.is_retryable(),
                        checkpoint_valid: false,
                    },
                );
                finish_task(&registry, &file_id, generation, true).await;
                return Err(CommandError::new("SLICE_ERR", e.to_string()));
            }
        }

        // 分片成功后检查暂停（P1-01：排空切片，等待 blocking_task，不吞错误）
        if pause_flag.load(Ordering::Acquire) {
            if let Some(rx_drain) = rx.as_mut() {
                while rx_drain.recv().await.is_some() {}
            }
            rx.take();

            // 完整匹配 blocking_task.await 结果（P1-01：不得静默丢弃）
            match blocking_task.await {
                Err(join_err) => {
                    crate::warn!("[{}] 暂停等待切片任务异常结束: {}", file_name, join_err);
                    let err_msg = format!("音频转码失败: {}", join_err);
                    envelope.emit(
                        "transcription-error",
                        crate::state::TaskEventKind::Failed,
                        &crate::state::TranscriptionErrorPayload {
                            error: err_msg.clone(),
                            retryable: false,
                            checkpoint_valid: false,
                        },
                    );
                    finish_task(&registry, &file_id, generation, true).await;
                    return Err(CommandError::new("SLICE_ERR", err_msg));
                }
                Ok(Err(convert_err)) => {
                    crate::error!("[{}] 转码/切片失败: {}", file_name, convert_err);
                    let err_msg = format!("音频转码失败: {}", convert_err);
                    envelope.emit(
                        "transcription-error",
                        crate::state::TaskEventKind::Failed,
                        &crate::state::TranscriptionErrorPayload {
                            error: err_msg.clone(),
                            retryable: false,
                            checkpoint_valid: false,
                        },
                    );
                    finish_task(&registry, &file_id, generation, true).await;
                    return Err(CommandError::new("SLICE_ERR", err_msg));
                }
                Ok(Ok(slice_count)) => {
                    // 取消已置位：执行取消终结，不发送暂停事件
                    if token.is_cancelled() {
                        finish_task(&registry, &file_id, generation, true).await;
                        envelope.emit(
                            "transcription-cancelled",
                            crate::state::TaskEventKind::Cancelled,
                            &serde_json::json!({}),
                        );
                        return Ok(generation);
                    }
                    // actual_total 必须等于成功切片任务返回值（P1-01）
                    let actual_total = audio::count_slice_files(&temp_dir);
                    if actual_total != slice_count as u32 {
                        crate::warn!(
                            "[{}] 切片数不一致: 任务返回 {} 实际 {}",
                            file_name,
                            slice_count,
                            actual_total
                        );
                    }
                    let ck = PauseCheckpoint {
                        last_completed: index,
                        total_slices: actual_total,
                    };
                    if !registry
                        .commit_pause(&file_id, generation, ck.clone())
                        .await
                    {
                        finish_task(&registry, &file_id, generation, true).await;
                        envelope.emit(
                            "transcription-cancelled",
                            crate::state::TaskEventKind::Cancelled,
                            &serde_json::json!({}),
                        );
                        return Ok(generation);
                    }
                    envelope.emit(
                        "transcription-paused",
                        crate::state::TaskEventKind::Paused,
                        &ck,
                    );
                    crate::info!("[{}] 已暂停 (分片 {}/{})", file_name, index, actual_total);
                    return Ok(generation);
                }
            }
        }
    }

    // 等待 blocking task 完成（P1-01：不得静默丢弃 Err）
    let task_result = blocking_task.await;

    let is_cancelled = token.is_cancelled();
    if is_cancelled {
        finish_task(&registry, &file_id, generation, true).await;
        envelope.emit(
            "transcription-cancelled",
            crate::state::TaskEventKind::Cancelled,
            &serde_json::json!({}),
        );
        crate::info!("[{}] process_and_transcribe 已取消", file_name);
        return Ok(generation);
    }

    match task_result {
        Err(join_err) => {
            crate::error!("[{}] 转码/切片阻塞任务异常终止: {}", file_name, join_err);
            let err_msg = format!("音频转码失败: {}", join_err);
            envelope.emit(
                "transcription-error",
                crate::state::TaskEventKind::Failed,
                &crate::state::TranscriptionErrorPayload {
                    error: err_msg.clone(),
                    retryable: false,
                    checkpoint_valid: false,
                },
            );
            finish_task(&registry, &file_id, generation, true).await;
            return Err(CommandError::new("SLICE_ERR", err_msg));
        }
        Ok(Err(convert_err)) => {
            crate::error!("[{}] 转码/切片失败: {}", file_name, convert_err);
            let err_msg = format!("音频转码失败: {}", convert_err);
            envelope.emit(
                "transcription-error",
                crate::state::TaskEventKind::Failed,
                &crate::state::TranscriptionErrorPayload {
                    error: err_msg.clone(),
                    retryable: false,
                    checkpoint_valid: false,
                },
            );
            finish_task(&registry, &file_id, generation, true).await;
            return Err(CommandError::new("SLICE_ERR", err_msg));
        }
        Ok(Ok(_slice_count)) => {}
    }

    if token.is_cancelled() {
        finish_task(&registry, &file_id, generation, true).await;
        envelope.emit(
            "transcription-cancelled",
            crate::state::TaskEventKind::Cancelled,
            &serde_json::json!({}),
        );
        return Ok(generation);
    }

    let text = registry.take_output(&file_id, generation).await;
    finish_task(&registry, &file_id, generation, true).await;
    envelope.emit(
        "transcription-complete",
        crate::state::TaskEventKind::Completed,
        &serde_json::json!({ "text": text }),
    );
    crate::info!("[{}] process_and_transcribe 完成", file_name);
    let _ = text;
    Ok(generation)
}

/// 从字符串解析任务操作类型（兼容前端传参）。
impl TaskOperation {
    pub fn from_str(s: &str) -> TaskOperation {
        if s == "polish" {
            TaskOperation::Polish
        } else {
            TaskOperation::Transcription
        }
    }
}
