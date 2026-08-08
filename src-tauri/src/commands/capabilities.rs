//! 能力元数据与约束单一来源（清单 P8-01）：
//! `get_capabilities` 导出一份统一能力文档（camelCase 序列化），前端据此
//! 渲染 provider 下拉、切片选项与导入/导出限制，避免前端散落魔法常量。
//!
//! 所有取值均来自 `constants`（导入/导出限制）、`provider`（provider/model
//! 元数据）与 `prompt_store`（提示词限制），本模块不重复定义业务常量。

use serde::Serialize;

use crate::constants;
use crate::{prompt_store, provider};

/// provider 能力元数据（camelCase，与前端 `ProviderInfo` 字段对齐）。
/// 独立于 commands::ProviderInfo（保持 snake_case 字段的既有 get_providers 使用方不变）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapability {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_prefix: String,
    pub asr_models: Vec<String>,
    pub chat_models: Vec<String>,
}

impl From<provider::ProviderInfo> for ProviderCapability {
    fn from(info: provider::ProviderInfo) -> Self {
        ProviderCapability {
            id: info.id,
            name: info.name,
            base_url: info.base_url,
            key_prefix: info.key_prefix,
            asr_models: info.asr_models,
            chat_models: info.chat_models,
        }
    }
}

/// 提示词编辑限制（camelCase）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptLimits {
    pub max_items: usize,
    pub title_max_chars: usize,
    pub content_max_chars: usize,
}

/// 导入限制（camelCase）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCapabilities {
    pub extensions: Vec<&'static str>,
    pub max_batch: usize,
    pub max_files: usize,
    pub probe_concurrency: usize,
}

/// 导出限制（camelCase）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCapabilities {
    pub formats: Vec<&'static str>,
    pub max_bytes: usize,
}

/// 统一能力文档（camelCase 序列化）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCapabilities {
    pub schema_version: u32,
    pub providers: Vec<ProviderCapability>,
    pub prompt_limits: PromptLimits,
    pub slice_seconds: Vec<u32>,
    pub import: ImportCapabilities,
    pub export: ExportCapabilities,
}

/// 导出统一能力文档（P8-01）
#[tauri::command]
pub fn get_capabilities() -> AppCapabilities {
    AppCapabilities {
        schema_version: 1,
        providers: provider::get_providers()
            .into_iter()
            .map(Into::into)
            .collect(),
        prompt_limits: PromptLimits {
            max_items: prompt_store::MAX_PROMPTS,
            title_max_chars: prompt_store::TITLE_MAX_CHARS,
            content_max_chars: prompt_store::CONTENT_MAX_CHARS,
        },
        slice_seconds: constants::SLICE_SECONDS_OPTIONS.to_vec(),
        import: ImportCapabilities {
            extensions: constants::IMPORT_EXTENSIONS.to_vec(),
            max_batch: constants::MAX_IMPORT_BATCH,
            max_files: constants::MAX_FILE_LIST_ITEMS,
            probe_concurrency: constants::PROBE_CONCURRENCY,
        },
        export: ExportCapabilities {
            formats: constants::EXPORT_FORMATS.to_vec(),
            max_bytes: constants::MAX_EXPORT_BYTES,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 序列化字段必须是 camelCase，与前端 AppCapabilities 对齐
    #[test]
    fn serializes_camel_case_and_sources_constants() {
        let caps = get_capabilities();
        let v = serde_json::to_value(&caps).unwrap();
        let obj = v.as_object().unwrap();

        assert!(obj.contains_key("schemaVersion"));
        assert!(obj.contains_key("promptLimits"));
        assert!(obj.contains_key("sliceSeconds"));
        assert!(obj.contains_key("import"));
        assert!(obj.contains_key("export"));
        // 拒绝 snake_case 顶层键
        assert!(!obj.contains_key("schema_version"));
        assert!(!obj.contains_key("prompt_limits"));
        assert!(!obj.contains_key("slice_seconds"));

        // provider 字段 camelCase
        let providers = obj["providers"].as_array().unwrap();
        assert!(!providers.is_empty());
        let p0 = providers[0].as_object().unwrap();
        for key in ["baseUrl", "keyPrefix", "asrModels", "chatModels"] {
            assert!(p0.contains_key(key), "缺少 camelCase 字段 {key}");
        }
        assert!(!p0.contains_key("base_url"));
        assert!(!p0.contains_key("asr_models"));

        // 限制取值来自唯一来源常量
        assert_eq!(
            obj["sliceSeconds"].as_array().unwrap().len(),
            constants::SLICE_SECONDS_OPTIONS.len()
        );
        assert_eq!(
            obj["promptLimits"]["maxItems"].as_u64().unwrap(),
            prompt_store::MAX_PROMPTS as u64
        );
        assert_eq!(
            obj["promptLimits"]["titleMaxChars"].as_u64().unwrap(),
            prompt_store::TITLE_MAX_CHARS as u64
        );
        assert_eq!(
            obj["promptLimits"]["contentMaxChars"].as_u64().unwrap(),
            prompt_store::CONTENT_MAX_CHARS as u64
        );
        assert_eq!(
            obj["import"]["maxBatch"].as_u64().unwrap(),
            constants::MAX_IMPORT_BATCH as u64
        );
        assert_eq!(
            obj["import"]["maxFiles"].as_u64().unwrap(),
            constants::MAX_FILE_LIST_ITEMS as u64
        );
        assert_eq!(
            obj["import"]["probeConcurrency"].as_u64().unwrap(),
            constants::PROBE_CONCURRENCY as u64
        );
        assert_eq!(
            obj["export"]["maxBytes"].as_u64().unwrap(),
            constants::MAX_EXPORT_BYTES as u64
        );
        assert_eq!(
            obj["export"]["formats"].as_array().unwrap().len(),
            constants::EXPORT_FORMATS.len()
        );
        let exts = obj["import"]["extensions"].as_array().unwrap();
        assert_eq!(exts.len(), constants::IMPORT_EXTENSIONS.len());
    }
}
