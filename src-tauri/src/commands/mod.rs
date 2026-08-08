//! IPC 命令按域拆分（清单 B-11）：transcribe / chat / config / fs / prompts。
//! 本模块承载跨命令共享的请求/响应 DTO。

pub mod capabilities;
pub mod chat;
pub mod config;
pub mod fs;
pub mod prompts;
pub mod transcribe;

use serde::{Deserialize, Serialize};

use crate::{api, provider};

// ============================================================
// 请求参数类型
// ============================================================

#[derive(Deserialize)]
pub struct ApiParams {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
}

// 手写 Debug（B-03）：api_key 脱敏，避免 {:?} 打印泄露明文密钥
impl std::fmt::Debug for ApiParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiParams")
            .field("base_url", &self.base_url)
            .field("api_key", &api::redact_secret(&self.api_key))
            .field("model", &self.model)
            .field("language", &self.language)
            .finish()
    }
}

impl From<ApiParams> for api::ApiConfig {
    fn from(params: ApiParams) -> Self {
        api::ApiConfig {
            base_url: params.base_url,
            api_key: params.api_key,
            model: params.model,
            language: params.language,
        }
    }
}

/// start_chat IPC 请求（B-11：聚合命令参数，删除临时 Clippy allow）。
/// 字段名与前端 camelCase 对齐（serde rename_all）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChatRequest {
    pub file_id: String,
    pub system_prompt: String,
    pub user_text: String,
    pub audio_data_url: Option<String>,
    pub api_params: ApiParams,
    pub response_format: Option<String>,
    pub file_name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_prefix: String,
    pub asr_models: Vec<String>,
    pub chat_models: Vec<String>,
}

impl From<provider::ProviderInfo> for ProviderInfo {
    fn from(info: provider::ProviderInfo) -> Self {
        ProviderInfo {
            id: info.id,
            name: info.name,
            base_url: info.base_url,
            key_prefix: info.key_prefix,
            asr_models: info.asr_models,
            chat_models: info.chat_models,
        }
    }
}
