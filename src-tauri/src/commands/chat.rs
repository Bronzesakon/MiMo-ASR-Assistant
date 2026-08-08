//! 规整/聊天域命令（清单 B-11 自 lib.rs 拆出）。
//! P2-04：规整错误与转写错误分离；P2-05：取消有终态事件与确认结果。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter as TauriEmitterTrait, Manager};

use crate::api::{self};
use crate::state::{
    finish_task, CommandError, PolishErrorPayload, TaggedEmitter, TaskEntry, TaskOperation,
    TaskRegistry,
};
use crate::{cancel, prompt, rate_limiter};

use super::StartChatRequest;

/// 流式聊天文本（携带 file_id 做事件隔离，含日志）
#[tauri::command]
pub async fn start_chat(
    handle: AppHandle,
    request: StartChatRequest,
    run_id: String,
) -> Result<u64, CommandError> {
    let StartChatRequest {
        file_id,
        system_prompt,
        user_text,
        audio_data_url,
        api_params,
        response_format,
        file_name,
    } = request;
    crate::info!(
        "[{}] start_chat 开始: 模型={}, 文本长度={}",
        file_name,
        api_params.model,
        user_text.len()
    );

    let config: api::ApiConfig = api_params.into();
    let limiter = handle.state::<rate_limiter::ChatRateLimiter>();
    let registry = handle.state::<TaskRegistry>();

    // 注册规整任务，后端生成代际
    let token = cancel::CancelToken::new();
    let pause_flag = Arc::new(AtomicBool::new(false));
    let mut entry = TaskEntry::new(token.clone(), pause_flag.clone());
    entry.operation = TaskOperation::Polish;
    entry.run_id = run_id.clone();
    let generation = entry.generation;
    registry
        .try_register(&file_id, entry)
        .await
        .map_err(|e| CommandError::new("REGISTER", e))?;

    // 获取 Chat 并发许可
    let _permit = match limiter.0.acquire_permit(&token).await {
        Ok(p) => p,
        Err(_) => {
            finish_task(&registry, &file_id, generation, false).await;
            crate::info!("[{}] start_chat 排队等待许可时已取消", file_name);
            // 取消终态事件（P2-05）
            let _ = handle.emit(
                "chat-cancelled",
                &crate::state::task_event_json(
                    &file_id,
                    TaskOperation::Polish,
                    &run_id,
                    generation,
                    crate::state::next_task_event_sequence(),
                    crate::state::TaskEventKind::Cancelled,
                    &serde_json::json!({}),
                ),
            );
            return Ok(generation);
        }
    };

    let template = prompt::PromptTemplate {
        system: system_prompt,
        user_prefix: String::new(),
        user_suffix: String::new(),
    };
    let final_system_prompt = prompt::build_prompt(&template, "");

    let tagged_emitter = TaggedEmitter::new(
        handle.clone(),
        &file_id,
        TaskOperation::Polish,
        &run_id,
        generation,
    );

    match api::chat_with_retry(api::ChatRequest {
        system_prompt: &final_system_prompt,
        user_text: &user_text,
        audio_data_url: audio_data_url.as_deref(),
        config: &config,
        response_format: response_format.as_deref(),
        emitter: &tagged_emitter,
        cancel: &token,
        limiter: Some(&limiter.0),
    })
    .await
    {
        Ok(()) => {}
        Err(e) if e.is_cancelled() => {
            finish_task(&registry, &file_id, generation, false).await;
            crate::info!("[{}] start_chat 已取消", file_name);
            // 取消终态事件（P2-05）
            let _ = handle.emit(
                "chat-cancelled",
                &crate::state::task_event_json(
                    &file_id,
                    TaskOperation::Polish,
                    &run_id,
                    generation,
                    crate::state::next_task_event_sequence(),
                    crate::state::TaskEventKind::Cancelled,
                    &serde_json::json!({}),
                ),
            );
            return Ok(generation);
        }
        Err(e) => {
            finish_task(&registry, &file_id, generation, false).await;
            crate::error!(
                "[{}] start_chat 最终失败: {} (模型={}, 文本长度={})",
                file_id,
                e,
                config.model,
                user_text.len()
            );
            // P2-04：规整失败与转写失败分离
            let _ = handle.emit(
                "chat-error",
                &crate::state::task_event_json(
                    &file_id,
                    TaskOperation::Polish,
                    &run_id,
                    generation,
                    crate::state::next_task_event_sequence(),
                    crate::state::TaskEventKind::Failed,
                    &PolishErrorPayload {
                        error: e.to_string(),
                        retryable: e.is_retryable(),
                    },
                ),
            );
            return Err(CommandError::new("CHAT_ERR", e.to_string()));
        }
    }

    finish_task(&registry, &file_id, generation, false).await;

    if token.is_cancelled() {
        crate::info!("[{}] start_chat 已取消", file_name);
        return Ok(generation);
    }

    let _ = handle.emit(
        "chat-complete",
        &crate::state::task_event_json(
            &file_id,
            TaskOperation::Polish,
            &run_id,
            generation,
            crate::state::next_task_event_sequence(),
            crate::state::TaskEventKind::Completed,
            &serde_json::json!({}),
        ),
    );
    crate::info!("[{}] start_chat 完成", file_name);
    Ok(generation)
}
