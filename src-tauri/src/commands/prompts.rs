//! 提示词域命令（清单 F-06.3）：操作型 IPC，文档守护全部在 prompt_store 后端完成。
//! 禁止整份文档覆盖型命令；默认 ID、schema、数量与排序由后端守护。

use tauri::{AppHandle, Emitter as TauriEmitterTrait, Manager};

use crate::prompt_store::{PromptDocument, PromptRepository};

/// 加载提示词文档；若存在一次性恢复/持久化提示，经事件转发给前端展示
#[tauri::command]
pub async fn load_prompts(handle: AppHandle) -> Result<PromptDocument, String> {
    let repo = handle.state::<PromptRepository>();
    if let Some(notice) = repo.take_notice().await {
        // H-01：发射结构化 JSON，前端统一 JSON.parse 解析，避免普通字符串解析失败
        let _ = handle.emit(
            "prompts-recovery-notice",
            &serde_json::json!({ "message": notice }),
        );
    }
    repo.load().await
}

/// 新建条目（插入首位；达上限返回可展示错误）
#[tauri::command]
pub async fn create_prompt(
    handle: AppHandle,
    title: String,
    content: String,
) -> Result<PromptDocument, String> {
    handle
        .state::<PromptRepository>()
        .create(&title, &content)
        .await
}

/// 更新条目；默认项忽略标题修改
#[tauri::command]
pub async fn update_prompt(
    handle: AppHandle,
    id: String,
    title: Option<String>,
    content: String,
) -> Result<PromptDocument, String> {
    handle
        .state::<PromptRepository>()
        .update(&id, title.as_deref(), &content)
        .await
}

/// 删除条目；默认项不可删；删除最近项同事务回退 builtin-default
#[tauri::command]
pub async fn delete_prompt(handle: AppHandle, id: String) -> Result<PromptDocument, String> {
    handle.state::<PromptRepository>().delete(&id).await
}

/// 重排序；ordered_ids 必须与当前 ID 集合完全相等
#[tauri::command]
pub async fn reorder_prompts(
    handle: AppHandle,
    ordered_ids: Vec<String>,
) -> Result<PromptDocument, String> {
    handle
        .state::<PromptRepository>()
        .reorder(&ordered_ids)
        .await
}

/// 选择最近项；先原子持久化成功再返回新文档
#[tauri::command]
pub async fn select_prompt(handle: AppHandle, id: String) -> Result<PromptDocument, String> {
    handle.state::<PromptRepository>().select(&id).await
}

/// 返回内置默认提示词正文（F-06.5.9「恢复默认」专用）。
/// 默认正文常量全应用仅此一份（prompt_store::DEFAULT_POLISH_PROMPT），前端不得复制。
#[tauri::command]
pub fn builtin_default_content() -> String {
    crate::prompt_store::DEFAULT_POLISH_PROMPT.to_string()
}

/// 打开提示词目录（只打开目录，不提供导入/导出/合并）
#[tauri::command]
pub fn open_prompt_dir(handle: AppHandle) -> Result<(), String> {
    let dir = handle.state::<PromptRepository>().dir().to_path_buf();
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("open")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
