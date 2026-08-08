//! 配置/服务商域命令（清单 B-11 自 lib.rs 拆出）：
//! load_config、操作型配置命令（P3-02）、get_providers、detect_provider、
//! fetch_custom_models、get_default_prompt。
//!
//! P3：配置持久化统一经由单实例 `ConfigRepository`（config_store.rs），
//! 所有写操作均为带 expectedRevision 的操作型命令，返回完整的 `ConfigSnapshot`；
//! revision 不匹配返回 REVISION_CONFLICT 与最新 revision。

use tauri::State;

use crate::config_store::{
    ConfigCommandError, ConfigRepository, ConfigSnapshot, CustomProviderInput,
};
use crate::{api, endpoint, prompt, provider};

use super::ProviderInfo;

/// 加载当前配置快照（含 revision、notice、解密问题列表）
#[tauri::command]
pub async fn load_config(
    state: State<'_, ConfigRepository>,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    Ok(state.snapshot().await)
}

/// 设置主题（system/light/dark）
#[tauri::command(rename_all = "camelCase")]
pub async fn set_theme(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    theme: String,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state.set_theme(expected_revision, theme).await
}

/// 设置切片时长（秒，须属于合法值集合）
#[tauri::command(rename_all = "camelCase")]
pub async fn set_slice_seconds(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    seconds: u32,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state.set_slice_seconds(expected_revision, seconds).await
}

/// 设置 ASR 服务商（须存在且拥有 ASR model）
#[tauri::command(rename_all = "camelCase")]
pub async fn set_asr_provider(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    provider_id: String,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state.set_asr_provider(expected_revision, provider_id).await
}

/// 设置 polish 服务商与模型（模型须属于其 chat models）
#[tauri::command(rename_all = "camelCase")]
pub async fn set_polish_selection(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    provider_id: String,
    model_id: String,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state
        .set_polish_selection(expected_revision, provider_id, model_id)
        .await
}

/// 设置某服务商的 API Key（替换解密失败密文并清除对应 issue）
#[tauri::command(rename_all = "camelCase")]
pub async fn set_provider_key(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    provider_id: String,
    key: String,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state
        .set_provider_key(expected_revision, provider_id, key)
        .await
}

/// 新增或更新自定义服务商（id/name/baseUrl/models 校验 + id 唯一）
#[tauri::command(rename_all = "camelCase")]
pub async fn upsert_custom_provider(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    provider: CustomProviderInput,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state
        .upsert_custom_provider(expected_revision, provider)
        .await
}

/// 删除自定义服务商（删除当前选中的 polish 服务商时回退内置默认）
#[tauri::command(rename_all = "camelCase")]
pub async fn delete_custom_provider(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    provider_id: String,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state
        .delete_custom_provider(expected_revision, provider_id)
        .await
}

/// 设置日志开关（运行时副作用仅在配置提交成功后执行）
#[tauri::command(rename_all = "camelCase")]
pub async fn set_logging_enabled(
    state: State<'_, ConfigRepository>,
    expected_revision: u64,
    enabled: bool,
) -> Result<ConfigSnapshot, ConfigCommandError> {
    state.set_logging_enabled(expected_revision, enabled).await
}

/// 获取可用服务商列表
#[tauri::command]
pub fn get_providers() -> Vec<ProviderInfo> {
    provider::get_providers()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// 根据 API Key 前缀自动检测服务商并返回配置
#[tauri::command]
pub fn detect_provider(api_key: String) -> ProviderInfo {
    let svc = provider::detect_provider(&api_key);
    let info = provider::get_providers()
        .into_iter()
        .find(|p| {
            matches!(
                (&svc, p.id.as_str()),
                (provider::ServiceProvider::MimoApi, "mimo-api")
                    | (provider::ServiceProvider::MimoTokenPlan, "mimo-token-plan")
            )
        })
        .unwrap_or_else(|| provider::get_providers().into_iter().next().unwrap());
    info.into()
}

/// 获取默认提示词模板
#[tauri::command]
pub fn get_default_prompt(template_type: String) -> Result<String, String> {
    let tmpl = match template_type.as_str() {
        "transcription" => prompt::default_transcription_template(),
        "punctuation" => prompt::default_punctuation_template(),
        _ => return Err(format!("未知模板类型: {}", template_type)),
    };
    Ok(serde_json::to_string(&tmpl).unwrap_or_default())
}

/// 从自定义 OpenAI 兼容服务商获取可用模型列表（S-02：SSRF 防护 + 响应上限）
#[tauri::command]
pub async fn fetch_custom_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    if api_key.is_empty() {
        return Err("API Key 不能为空".to_string());
    }

    // S-02 单一校验入口：结构化 URL 解析 + scheme/host/端口 + DNS/IP 检查
    let ep = endpoint::validate_endpoint(&base_url, endpoint::EndpointPolicy::DEFAULT)
        .map_err(|e| e.to_string())?;
    // 日志只记录 scheme/host/port/path，不含 userinfo/query/凭据
    crate::info!("[fetch_custom_models] 校验通过: {}", ep.safe_display());

    // 用 URL path API 拼接候选端点（禁止字符串拼接产生双斜杠/覆盖 origin）
    let candidates = ep.model_candidates();

    let client = api::http_client();
    let mut last_err: Option<String> = None;

    for url in &candidates {
        // 防 DNS rebinding：每次真实请求前重新解析并检查全部结果
        endpoint::recheck_endpoint(&ep)
            .await
            .map_err(|e| e.to_string())?;

        let response = match client
            .get(url.as_str())
            .header("Authorization", format!("Bearer {}", api_key))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return Err(format!("请求失败: {}", e)),
        };

        let status = response.status();

        // 客户端已禁用重定向：任何 3xx 直接拒绝，不跟随
        if status.is_redirection() {
            return Err(format!(
                "HTTP {}: 服务端返回重定向，已被安全策略拒绝",
                status
            ));
        }

        if status.is_success() {
            // 响应体上限 2 MiB，防止恶意服务返回超大正文
            let body_text = endpoint::read_body_limited(response, endpoint::MODEL_LIST_BODY_LIMIT)
                .await
                .map_err(|e| format!("响应读取失败: {}", e))?;
            let body: serde_json::Value =
                serde_json::from_str(&body_text).map_err(|e| format!("解析响应失败: {}", e))?;

            let mut models: Vec<String> = Vec::new();
            if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                for entry in data {
                    if let Some(id) = entry.get("id").and_then(|v| v.as_str()) {
                        // 单个模型 ID 长度上限
                        if id.chars().count() > endpoint::MAX_MODEL_ID_CHARS {
                            continue;
                        }
                        models.push(id.to_string());
                    }
                }
            }
            if models.len() > endpoint::MAX_MODELS {
                return Err(format!(
                    "模型列表超过 {} 条上限，已拒绝",
                    endpoint::MAX_MODELS
                ));
            }
            models.sort();
            return Ok(models);
        }

        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            last_err = Some(format!("HTTP {}", status));
            continue;
        }

        // 错误正文最多保留 4 KiB（UTF-8 字符边界安全），不把超大正文带入错误/日志
        let err_body = endpoint::read_body_limited(response, endpoint::ERROR_BODY_LIMIT as u64)
            .await
            .unwrap_or_else(|_| "(响应正文超限已丢弃)".to_string());
        return Err(format!(
            "HTTP {}: {}",
            status,
            endpoint::safe_error_body(&err_body)
        ));
    }

    Err(format!(
        "获取模型列表失败: {}",
        last_err.unwrap_or_else(|| "无候选端点".to_string())
    ))
}
