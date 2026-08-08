//! 配置域（清单 B-11 自 lib.rs 拆出）：AppConfig、路径规则、加解密持久化。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

use crate::{api, dpapi, prompt_store};

// ============================================================
// 数据结构
// ============================================================

#[derive(Serialize, Deserialize, Clone)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

// 手写 Debug（B-03）：api_key 脱敏
impl std::fmt::Debug for CustomProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomProvider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &api::redact_secret(&self.api_key))
            .field("models", &self.models)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub transcription_model: String,
    pub polish_model: String,
    pub theme: String,
    /// F-06：旧存储模型，仅为反序列化迁移兼容保留；
    /// 运行时提示词事实来源 = prompt_store（prompts.json）。
    #[serde(default = "default_polish_prompt")]
    pub polish_prompt: String,
    #[serde(default)]
    pub provider_keys: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub logging_enabled: bool,
    #[serde(default = "default_asr_provider")]
    pub asr_provider: String,
    #[serde(default = "default_polish_provider")]
    pub polish_provider: String,
    #[serde(default = "default_slice_seconds")]
    pub slice_seconds: u32,
    #[serde(default)]
    pub custom_providers: Vec<CustomProvider>,
}

// 手写 Debug（B-03）：api_key、provider_keys 中各 provider 的密钥均脱敏；
// custom_providers 内的密钥由其自身脱敏 Debug 处理
impl std::fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted_keys: HashMap<&String, String> = self
            .provider_keys
            .iter()
            .map(|(k, v)| (k, api::redact_secret(v)))
            .collect();
        f.debug_struct("AppConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &api::redact_secret(&self.api_key))
            .field("transcription_model", &self.transcription_model)
            .field("polish_model", &self.polish_model)
            .field("theme", &self.theme)
            // H-04：Debug 只输出提示词长度，不输出提示词正文
            .field(
                "polish_prompt",
                &format!("<{} chars>", self.polish_prompt.chars().count()),
            )
            .field("provider_keys", &redacted_keys)
            .field("logging_enabled", &self.logging_enabled)
            .field("asr_provider", &self.asr_provider)
            .field("polish_provider", &self.polish_provider)
            .field("slice_seconds", &self.slice_seconds)
            .field("custom_providers", &self.custom_providers)
            .finish()
    }
}

fn default_true() -> bool {
    true
}

fn default_asr_provider() -> String {
    "mimo-api".to_string()
}

fn default_polish_provider() -> String {
    "deepseek".to_string()
}

fn default_slice_seconds() -> u32 {
    crate::constants::DEFAULT_SLICE_SECONDS
}

/// 旧默认提示词（F-06 后仅为旧 JSON 反序列化兼容；
/// 比较基准常量保存在 prompt_store 供迁移使用）
fn default_polish_prompt() -> String {
    prompt_store::LEGACY_DEFAULT_POLISH_PROMPT.to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: "mimo-api".to_string(),
            base_url: "https://api.xiaomimimo.com/v1".to_string(),
            api_key: String::new(),
            transcription_model: "mimo-v2.5-asr".to_string(),
            polish_model: "mimo-v2.5".to_string(),
            theme: "system".to_string(),
            polish_prompt: default_polish_prompt(),
            provider_keys: HashMap::new(),
            logging_enabled: true,
            asr_provider: "mimo-api".to_string(),
            polish_provider: "deepseek".to_string(),
            slice_seconds: crate::constants::DEFAULT_SLICE_SECONDS,
            custom_providers: Vec::new(),
        }
    }
}

// ============================================================
// 配置路径与原子写（B-09）
// ============================================================

/// 旧版配置路径（exe 目录），仅供一次性迁移兼容读取
pub fn legacy_config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("config.json")
}

/// 新版配置目录：Tauri 标准 app_config_dir；获取失败时回退 exe 目录
pub fn config_dir(handle: &AppHandle) -> PathBuf {
    handle.path().app_config_dir().unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

pub fn config_path(handle: &AppHandle) -> PathBuf {
    config_dir(handle).join("config.json")
}

/// 日志目录：Tauri 标准 app_log_dir；获取失败时回退 exe 目录/log
pub fn log_dir_for(handle: &AppHandle) -> PathBuf {
    handle.path().app_log_dir().unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("log")
    })
}

/// 原子写（B-09/T-01）：写同目录 .tmp → 覆盖前把现有文件复制为 .bak → 原子替换主文件。
/// 任一阶段失败都不会损坏现有文件。Windows 上经 MoveFileExW 实现可证明的原子替换。
pub fn atomic_write_text(path: &Path, content: &str) -> std::io::Result<()> {
    crate::atomic::atomic_write_text_with_backup(path, content)
}

/// 旧配置一次性迁移兼容（B-09）：新位置无配置而 exe 目录旧 config.json 存在时，
/// 把旧配置复制到新位置；成功后不删旧文件（避免误删），新位置已存在则不覆盖。
pub fn migrate_legacy_config(legacy: &Path, new_path: &Path) -> bool {
    if new_path.exists() || !legacy.exists() {
        return false;
    }
    let content = match std::fs::read_to_string(legacy) {
        Ok(c) => c,
        Err(e) => {
            crate::warn!("旧配置迁移失败（读取 {:?}）: {}", legacy, e);
            return false;
        }
    };
    if let Some(parent) = new_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::warn!("旧配置迁移失败（创建目录 {:?}）: {}", parent, e);
            return false;
        }
    }
    match atomic_write_text(new_path, &content) {
        Ok(()) => {
            crate::info!("已将旧配置迁移至 {:?}（保留旧文件）", new_path);
            true
        }
        Err(e) => {
            crate::warn!("旧配置迁移失败（写入 {:?}）: {}", new_path, e);
            false
        }
    }
}

// ============================================================
// 加载（含解密）与磁盘 polish_prompt 读取
// ============================================================

/// 解析配置内容；JSON 损坏时的恢复策略（Warning 4 修复，B-12 可测化）：
/// 1. 损坏内容写入独立的 `.corrupt` 文件留证，绝不触碰完好的 `.bak`；
/// 2. 若 `.bak` 存在且可解析，用其恢复（回写主文件）并返回备份配置；
/// 3. 都失败再回退默认配置。
pub fn parse_config_or_backup(content: &str, path: &Path) -> AppConfig {
    if let Ok(c) = serde_json::from_str(content) {
        return c;
    }
    crate::error!("config.json 解析失败，尝试从备份恢复");
    // 损坏内容留证（独立 .corrupt，不覆盖 .bak）
    let corrupt = path.with_extension("json.corrupt");
    let _ = std::fs::write(&corrupt, content);

    // 尝试从 .bak 恢复
    let bak = path.with_extension("json.bak");
    if let Ok(bak_text) = std::fs::read_to_string(&bak) {
        match serde_json::from_str::<AppConfig>(&bak_text) {
            Ok(cfg) => {
                // 回写主文件：走不带备份步骤的原子替换，避免其备份步骤把
                // 损坏内容复制回 .bak（覆盖唯一好备份），且不回退为直接写
                if let Err(e) = crate::atomic::atomic_write_text_no_backup(path, &bak_text) {
                    crate::warn!("用备份回写主文件失败: {}", e);
                } else {
                    crate::info!("config.json 已从备份 {:?} 恢复", bak);
                }
                return cfg;
            }
            Err(_) => crate::warn!("备份 {:?} 同样无法解析，回退默认配置", bak),
        }
    }
    AppConfig::default()
}

/// 解密配置中所有 `ENC:` 前缀字段（api_key / provider_keys / custom_providers）。
/// 单项解密失败时清空该项并记录日志，不阻断整体加载。
pub fn decrypt_config_fields(config: &mut AppConfig) {
    if config.api_key.starts_with("ENC:") {
        match dpapi::decrypt_string(&config.api_key[4..]) {
            Ok(decrypted) => config.api_key = decrypted,
            Err(e) => {
                crate::error!("API Key 解密失败: {}", e);
                config.api_key.clear();
            }
        }
    }
    for value in config.provider_keys.values_mut() {
        if value.starts_with("ENC:") {
            match dpapi::decrypt_string(&value[4..]) {
                Ok(decrypted) => *value = decrypted,
                Err(e) => {
                    crate::error!("Provider Key 解密失败: {}", e);
                    value.clear();
                }
            }
        }
    }
    for cp in config.custom_providers.iter_mut() {
        if cp.api_key.starts_with("ENC:") {
            match dpapi::decrypt_string(&cp.api_key[4..]) {
                Ok(decrypted) => cp.api_key = decrypted,
                Err(e) => {
                    crate::error!("[{}] 自定义服务商 Key 解密失败: {}", cp.name, e);
                    cp.api_key.clear();
                }
            }
        }
    }
}

pub fn load_config_inner(handle: &AppHandle) -> AppConfig {
    // B-09：首次启动时把 exe 目录旧配置一次性迁到 app_config_dir
    let path = config_path(handle);
    migrate_legacy_config(&legacy_config_path(), &path);
    let config = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let mut config = parse_config_or_backup(&content, &path);
                // 迁移旧配置：无 asr_provider 时从旧 provider 推导
                if config.asr_provider.is_empty() {
                    config.asr_provider = if config.provider == "deepseek" {
                        "mimo-api".to_string()
                    } else {
                        config.provider.clone()
                    };
                }
                if config.polish_provider.is_empty() {
                    config.polish_provider = config.provider.clone();
                }
                decrypt_config_fields(&mut config);
                config
            }
            Err(_) => AppConfig::default(),
        }
    } else {
        AppConfig::default()
    };
    crate::log::set_enabled(config.logging_enabled);
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用隔离目录
    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mimo_cfg_test_{}_{}_{}",
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
    fn migrate_legacy_config_copies_and_keeps_old_file() {
        let dir = test_dir("migrate");
        let legacy = dir.join("old").join("config.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"provider":"mimo-api"}"#).unwrap();
        let new_path = dir.join("new").join("config.json");

        assert!(migrate_legacy_config(&legacy, &new_path), "首次应执行迁移");
        assert!(new_path.exists(), "新位置应有配置");
        assert_eq!(
            std::fs::read_to_string(&new_path).unwrap(),
            r#"{"provider":"mimo-api"}"#
        );
        assert!(legacy.exists(), "旧文件不得被删除");
        // 重复调用幂等：新位置已存在时不再迁移
        assert!(!migrate_legacy_config(&legacy, &new_path));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_legacy_config_noop_when_legacy_missing_or_new_exists() {
        let dir = test_dir("migrate_noop");
        let legacy = dir.join("no_such").join("config.json");
        let new_path = dir.join("config.json");
        // 旧配置不存在：不迁移
        assert!(!migrate_legacy_config(&legacy, &new_path));
        assert!(!new_path.exists());
        // 新位置已存在：不覆盖
        std::fs::write(&new_path, "{}").unwrap();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"x":1}"#).unwrap();
        assert!(!migrate_legacy_config(&legacy, &new_path));
        assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_creates_file_and_backup_on_overwrite() {
        let dir = test_dir("atomic");
        let path = dir.join("config.json");
        // 首次写入：无 .bak（无旧文件可备份）
        atomic_write_text(&path, "v1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
        assert!(!path.with_extension("json.bak").exists());
        assert!(!path.with_extension("json.tmp").exists(), "tmp 不得残留");
        // 覆盖写入：旧内容进 .bak
        atomic_write_text(&path, "v2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "v1"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_fails_cleanly_on_missing_parent() {
        let dir = test_dir("atomic_fail");
        let path = dir.join("no_such_dir").join("config.json");
        assert!(atomic_write_text(&path, "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 构造包含全部敏感字段的配置（配置回环测试用）
    fn sample_config() -> AppConfig {
        let mut cfg = AppConfig {
            provider: "mimo-api".to_string(),
            base_url: "https://api.xiaomimimo.com/v1".to_string(),
            api_key: "sk-roundtrip-secret-key".to_string(),
            transcription_model: "mimo-v2.5-asr".to_string(),
            polish_model: "deepseek-v4-pro".to_string(),
            theme: "dark".to_string(),
            polish_prompt: "自定义提示词内容".to_string(),
            provider_keys: HashMap::new(),
            logging_enabled: false,
            asr_provider: "mimo-token-plan".to_string(),
            polish_provider: "deepseek".to_string(),
            slice_seconds: 30,
            custom_providers: vec![CustomProvider {
                id: "cp1".to_string(),
                name: "自建服务".to_string(),
                base_url: "https://example.com/v1".to_string(),
                api_key: "sk-custom-secret".to_string(),
                models: vec!["m1".to_string(), "m2".to_string()],
            }],
        };
        cfg.provider_keys
            .insert("mimo-api".to_string(), "sk-provider-key-1".to_string());
        cfg.provider_keys
            .insert("deepseek".to_string(), "sk-provider-key-2".to_string());
        cfg
    }

    /// B-12 配置回环：序列化 → 加密 → 解密 → 反序列化一致
    #[test]
    fn config_roundtrip_serialize_encrypt_decrypt_deserialize() {
        let original = sample_config();

        // 序列化
        let json = serde_json::to_string(&original).expect("序列化失败");

        // 模拟保存链路：敏感字段 DPAPI 加密后加 ENC: 前缀
        let mut stored: serde_json::Value = serde_json::from_str(&json).unwrap();
        let enc_key = format!("ENC:{}", dpapi::encrypt_string(&original.api_key).unwrap());
        stored["api_key"] = serde_json::Value::String(enc_key);
        for (prov, key) in &original.provider_keys {
            let enc = format!("ENC:{}", dpapi::encrypt_string(key).unwrap());
            stored["provider_keys"][prov] = serde_json::Value::String(enc);
        }
        for cp in stored["custom_providers"].as_array_mut().unwrap() {
            let enc = format!("ENC:{}", dpapi::encrypt_string("sk-custom-secret").unwrap());
            cp["api_key"] = serde_json::Value::String(enc);
        }
        let stored_json = stored.to_string();
        assert!(
            !stored_json.contains("sk-roundtrip-secret-key"),
            "落盘内容不得含明文密钥"
        );

        // 反序列化 + 解密（与 load_config_inner 同一链路）
        let mut restored: AppConfig = serde_json::from_str(&stored_json).expect("反序列化失败");
        decrypt_config_fields(&mut restored);

        assert_eq!(restored.api_key, original.api_key);
        assert_eq!(restored.provider_keys, original.provider_keys);
        assert_eq!(restored.custom_providers[0].api_key, "sk-custom-secret");
        assert_eq!(restored.provider, original.provider);
        assert_eq!(restored.base_url, original.base_url);
        assert_eq!(restored.polish_prompt, original.polish_prompt);
        assert_eq!(restored.logging_enabled, original.logging_enabled);
        assert_eq!(restored.slice_seconds, original.slice_seconds);
        assert_eq!(
            restored.custom_providers[0].models,
            original.custom_providers[0].models
        );
    }

    /// B-12 配置回环：损坏 JSON 的恢复策略（Warning 4）：
    /// 损坏内容进独立 .corrupt 留证，.bak 不被触碰；能恢复则用 .bak，否则默认
    #[test]
    fn corrupted_config_recovery_behaviour() {
        let dir = test_dir("corrupt");
        let path = dir.join("config.json");
        let corrupted = "{\"provider\": \"mimo-api\", 损坏的 JSON 尾巴";
        let good_bak = r#"{"provider":"deepseek","base_url":"https://api.deepseek.com","api_key":"sk-x","transcription_model":"m","polish_model":"p","theme":"dark","slice_seconds":60}"#;

        // 场景 1：损坏 + 完好 .bak → 用备份恢复并回写主文件
        std::fs::write(path.with_extension("json.bak"), good_bak).unwrap();
        let cfg = parse_config_or_backup(corrupted, &path);
        assert_eq!(cfg.provider, "deepseek", "应从备份恢复配置");
        assert_eq!(cfg.slice_seconds, 60);
        // 损坏内容进 .corrupt，.bak 保持完好
        let corrupt = path.with_extension("json.corrupt");
        assert!(corrupt.exists(), ".corrupt 留证文件必须生成");
        assert_eq!(std::fs::read_to_string(&corrupt).unwrap(), corrupted);
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            good_bak,
            "完好备份不得被覆盖"
        );
        // 主文件已被备份内容回写
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good_bak);

        // 场景 2：损坏 + 无 .bak → 回退默认配置，.corrupt 留证
        let path2 = dir.join("case2").join("config.json");
        std::fs::create_dir_all(path2.parent().unwrap()).unwrap();
        let cfg = parse_config_or_backup(corrupted, &path2);
        assert_eq!(cfg.provider, AppConfig::default().provider);
        assert!(path2.with_extension("json.corrupt").exists());
        assert!(
            !path2.with_extension("json.bak").exists(),
            "不得把损坏内容写进 .bak"
        );

        // 场景 3：损坏 + .bak 也损坏 → 回退默认配置
        let path3 = dir.join("case3").join("config.json");
        std::fs::create_dir_all(path3.parent().unwrap()).unwrap();
        std::fs::write(path3.with_extension("json.bak"), "{bad").unwrap();
        let cfg = parse_config_or_backup(corrupted, &path3);
        assert_eq!(cfg.provider, AppConfig::default().provider);
        assert_eq!(
            std::fs::read_to_string(path3.with_extension("json.bak")).unwrap(),
            "{bad",
            "损坏备份保持原样"
        );

        // 合法 JSON 不走兑底：不生成 .bak/.corrupt，字段按文件内容解析（必填字段需齐全）
        let good_path = dir.join("good.json");
        let cfg2 = parse_config_or_backup(good_bak, &good_path);
        assert_eq!(cfg2.provider, "deepseek");
        assert_eq!(cfg2.slice_seconds, 60);
        assert!(!good_path.with_extension("json.bak").exists());
        assert!(!good_path.with_extension("json.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // H-04：Debug 输出不得包含提示词正文、API Key 或完整密钥
    #[test]
    fn debug_output_redacts_prompt_body_and_secrets() {
        let secret = "sk-TEST-SENTINEL-SECRET-42";
        let prompt_body = "这是不应出现在日志里的提示词正文 SENTINEL_PROMPT";
        let mut cfg = AppConfig::default();
        cfg.api_key = secret.to_string();
        cfg.provider_keys
            .insert("deepseek".to_string(), secret.to_string());
        cfg.polish_prompt = prompt_body.to_string();
        cfg.custom_providers = vec![CustomProvider {
            id: "cp".to_string(),
            name: "custom".to_string(),
            base_url: "https://example.com".to_string(),
            api_key: secret.to_string(),
            models: vec!["m".to_string()],
        }];

        let debug = format!("{:?}", cfg);
        assert!(!debug.contains(secret), "Debug 不得输出 API Key");
        assert!(
            !debug.contains("SENTINEL_PROMPT"),
            "Debug 不得输出提示词正文"
        );
    }
}
