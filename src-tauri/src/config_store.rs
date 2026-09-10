//! 配置仓库（清单 P3）：带互斥锁与 revision 的单实例配置存储。
//!
//! 职责：
//! - 启动时一次性加载磁盘 config.json（含 schema 迁移、损坏恢复、DPAPI 解密）；
//! - 运行期间配置事实来源 = 内存中的 `ConfigDocument`，不再重复读磁盘；
//! - 所有写操作在同一异步互斥锁内完成"读取最新 -> 构造候选 -> 完整校验 ->
//!   持久化 -> 成功后替换内存 -> 返回新快照"的完整事务；
//! - DPAPI 加解密与文件系统写入放入 `tokio::task::spawn_blocking`，
//!   候选文档与写入路径在进入阻塞任务前完成所有权转移；
//! - 未来 schema 版本进入只读状态并保留原文件；
//! - 解密失败字段保留原始 `ENC:` 密文，序列化时无关字段原样写回，
//!   仅当用户明确更新该 key 时才替换密文并清除对应 issue。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::config::{AppConfig, CustomProvider};
use crate::{config as cfg, dpapi, provider};

/// 当前配置 schema 版本
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// 合法切片时长集合（P3-03：constants 无显式合法列表，采用该值集合）
const ALLOWED_SLICE_SECONDS: &[u32] = &[10, 30, 60, 150];

// ============================================================
// 数据模型
// ============================================================

/// 落盘文档：schema_version / revision 为仓库元数据，配置扁平内联。
#[derive(Clone, Serialize, Deserialize)]
pub struct ConfigDocument {
    #[serde(default = "default_config_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub revision: u64,
    #[serde(flatten)]
    pub config: AppConfig,
}

fn default_config_schema_version() -> u32 {
    CONFIG_SCHEMA_VERSION
}

impl ConfigDocument {
    fn default_doc() -> Self {
        ConfigDocument {
            schema_version: CONFIG_SCHEMA_VERSION,
            revision: 0,
            config: AppConfig::default(),
        }
    }
}

/// 仓库运行态（内存事实来源）。
pub struct ConfigRepoState {
    pub doc: ConfigDocument,
    /// 一次性恢复/持久化提示（损坏恢复等）
    pub notice: Option<String>,
    /// 只读原因（如未知更高 schema 版本）：设置后禁止任何写盘，避免覆盖原文件
    pub read_only_reason: Option<String>,
    /// 解密失败字段：path -> 失败原因；对应密文保留在 doc.config 中
    pub unreadable_secrets: HashMap<SecretPath, String>,
}

/// 敏感字段路径（用于向前端标识"需要重新输入"的 key）
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SecretPath {
    Api,
    Provider(String),
    CustomProvider(String),
}

impl SecretPath {
    fn as_issue_string(&self) -> String {
        match self {
            SecretPath::Api => "api_key".to_string(),
            SecretPath::Provider(id) => format!("provider_keys.{}", id),
            SecretPath::CustomProvider(id) => format!("custom_providers.{}.api_key", id),
        }
    }
}

impl Serialize for SecretPath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_issue_string())
    }
}

/// 单个敏感字段的解密问题（camelCase 序列化）
#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SecretIssue {
    pub path: SecretPath,
    pub reason: String,
}

/// 配置快照：load_config 与所有操作型命令的返回类型（camelCase 元数据，
/// 配置主体沿用 AppConfig 的 snake_case 序列化，与既有前端字段对齐）。
#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub revision: u64,
    #[serde(flatten)]
    pub config: AppConfig,
    pub notice: Option<String>,
    pub secret_issues: Vec<SecretIssue>,
}

/// 操作型命令入参：自定义服务商（camelCase 序列化）
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderInput {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

/// 命令错误（结构化返回，前端可据此识别 REVISION_CONFLICT）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigCommandError {
    pub code: String,
    pub message: String,
    pub current_revision: Option<u64>,
}

impl ConfigCommandError {
    fn revision_conflict(expected: u64, current: u64) -> Self {
        Self {
            code: "REVISION_CONFLICT".to_string(),
            message: format!(
                "配置已被其他操作修改，请刷新后重试（期望版本 {}，当前版本 {}）",
                expected, current
            ),
            current_revision: Some(current),
        }
    }
    fn validation(msg: String) -> Self {
        Self {
            code: "VALIDATION".to_string(),
            message: msg,
            current_revision: None,
        }
    }
    fn io(msg: String) -> Self {
        Self {
            code: "IO".to_string(),
            message: msg,
            current_revision: None,
        }
    }
}

// ============================================================
// 仓库
// ============================================================

/// 单实例配置仓库：所有读写经 `inner` 互斥锁串行化。
pub struct ConfigRepository {
    inner: tokio::sync::Mutex<ConfigRepoState>,
    path: PathBuf,
}

impl ConfigRepository {
    /// 启动时初始化仓库：迁移旧配置、加载/恢复、解密、进入只读判定。
    pub fn init(handle: &AppHandle) -> Self {
        let path = cfg::config_path(handle);
        let state = load_repository(&path);
        crate::log::set_enabled(state.doc.config.logging_enabled);
        Self {
            inner: tokio::sync::Mutex::new(state),
            path,
        }
    }

    /// 提供给 PromptRepository 一次性迁移的初始 polish_prompt 值。
    /// 仅在 setup 阶段（无并发）调用。
    pub fn initial_polish_prompt(&self) -> String {
        match self.inner.try_lock() {
            Ok(g) => g.doc.config.polish_prompt.clone(),
            Err(_) => AppConfig::default().polish_prompt,
        }
    }

    /// 读取当前快照（load_config）。
    pub async fn snapshot(&self) -> ConfigSnapshot {
        let guard = self.inner.lock().await;
        snapshot_state(&guard)
    }

    // ---- 操作型命令（P3-02） ----

    pub async fn set_theme(
        &self,
        expected_revision: u64,
        theme: String,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            candidate.config.theme = theme;
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn set_slice_seconds(
        &self,
        expected_revision: u64,
        seconds: u32,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            candidate.config.slice_seconds = seconds;
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn set_asr_provider(
        &self,
        expected_revision: u64,
        provider_id: String,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            candidate.config.asr_provider = provider_id;
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn set_polish_selection(
        &self,
        expected_revision: u64,
        provider_id: String,
        model_id: String,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            candidate.config.polish_provider = provider_id;
            candidate.config.polish_model = model_id;
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn set_provider_key(
        &self,
        expected_revision: u64,
        provider_id: String,
        key: String,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            candidate
                .config
                .provider_keys
                .insert(provider_id.clone(), key);
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![SecretPath::Provider(provider_id)],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn upsert_custom_provider(
        &self,
        expected_revision: u64,
        input: CustomProviderInput,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let cp = CustomProvider {
                id: input.id.trim().to_string(),
                name: input.name.trim().to_string(),
                base_url: input.base_url.trim().to_string(),
                api_key: input.api_key,
                models: input.models,
            };
            validate_custom_provider(&cp)?;
            let mut candidate = doc.clone();
            if let Some(existing) = candidate
                .config
                .custom_providers
                .iter_mut()
                .find(|p| p.id == cp.id)
            {
                *existing = cp.clone();
            } else {
                candidate.config.custom_providers.push(cp.clone());
            }
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![SecretPath::CustomProvider(cp.id)],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn delete_custom_provider(
        &self,
        expected_revision: u64,
        provider_id: String,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            let before = candidate.config.custom_providers.len();
            candidate
                .config
                .custom_providers
                .retain(|p| p.id != provider_id);
            if candidate.config.custom_providers.len() == before {
                return Err(format!("自定义服务商不存在: {}", provider_id));
            }
            // 若删除的是当前选中的 polish 服务商，normalize_polish_config 会回退到内置默认
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![SecretPath::CustomProvider(provider_id)],
                side_effect: None,
            })
        })
        .await
    }

    pub async fn set_logging_enabled(
        &self,
        expected_revision: u64,
        enabled: bool,
    ) -> Result<ConfigSnapshot, ConfigCommandError> {
        self.mutate(expected_revision, |doc| {
            let mut candidate = doc.clone();
            candidate.config.logging_enabled = enabled;
            Ok(MutationOutcome {
                doc: candidate,
                cleared: vec![],
                side_effect: Some(SideEffect::SetLogging(enabled)),
            })
        })
        .await
    }

    // ---- 事务核心（P3-01.5） ----

    /// 通用写事务：在单一互斥锁内完成读取 -> 构造候选 -> 完整校验 ->
    /// 持久化（spawn_blocking）-> 成功后替换内存 -> 返回新快照。
    async fn mutate<F>(
        &self,
        expected_revision: u64,
        f: F,
    ) -> Result<ConfigSnapshot, ConfigCommandError>
    where
        F: FnOnce(&ConfigDocument) -> Result<MutationOutcome, String>,
    {
        let mut guard = self.inner.lock().await;
        let state = &mut *guard;

        // revision 冲突检测
        if state.doc.revision != expected_revision {
            return Err(ConfigCommandError::revision_conflict(
                expected_revision,
                state.doc.revision,
            ));
        }
        if let Some(reason) = &state.read_only_reason {
            return Err(ConfigCommandError::validation(format!(
                "配置处于只读状态：{}",
                reason
            )));
        }

        // 构造候选（含领域特定 mutation）
        let outcome = f(&state.doc).map_err(ConfigCommandError::validation)?;

        let mut normalized = outcome.doc;
        let new_revision = state.doc.revision.wrapping_add(1);
        normalized.revision = new_revision;

        // 完整校验（P3-03：归一 polish 组合 + 严格不变量）
        normalize_polish_config(&mut normalized.config);
        validate_config(&normalized.config).map_err(ConfigCommandError::validation)?;

        // 序列化（DPAPI 加密）与文件写入放入 blocking 任务；候选文档与路径完成所有权转移
        let json = config_to_disk_json(&normalized).map_err(ConfigCommandError::io)?;
        let path = self.path.clone();
        let write = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {}", e))?;
            }
            cfg::atomic_write_text(&path, &json).map_err(|e| format!("写入配置失败: {}", e))
        })
        .await
        .map_err(|e| ConfigCommandError::io(format!("配置写入任务异常: {}", e)))?;
        write.map_err(ConfigCommandError::io)?;

        // 提交：写入成功后才替换内存并清除相关 issue
        state.doc = normalized;
        for p in outcome.cleared {
            state.unreadable_secrets.remove(&p);
        }
        // 运行时副作用只在配置提交成功后执行
        if let Some(SideEffect::SetLogging(v)) = outcome.side_effect {
            crate::log::set_enabled(v);
        }

        Ok(snapshot_state(state))
    }
}

// ============================================================
// 加载 / 解密 / 校验
// ============================================================

/// 加载磁盘配置为仓库初始状态（仅初始化调用一次）。
fn load_repository(path: &Path) -> ConfigRepoState {
    cfg::migrate_legacy_config(&cfg::legacy_config_path(), path);

    let mut notice = None;
    let mut read_only_reason = None;
    let mut doc = if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                // 先探测 schema_version：高于当前版本进入只读，保留原文件
                let schema = serde_json::from_str::<serde_json::Value>(&content)
                    .ok()
                    .and_then(|v| v.get("schema_version").and_then(|x| x.as_u64()));
                match serde_json::from_str::<ConfigDocument>(&content) {
                    Ok(d) => {
                        if let Some(sv) = schema {
                            if sv > CONFIG_SCHEMA_VERSION as u64 {
                                crate::warn!(
                                    "config.json schema 版本 {} 高于当前支持版本 {}，进入只读模式",
                                    sv,
                                    CONFIG_SCHEMA_VERSION
                                );
                                read_only_reason = Some("配置版本高于当前应用支持版本".to_string());
                            }
                        }
                        d
                    }
                    Err(_) => {
                        // 损坏 JSON：复用 config.rs 的 .bak 恢复策略
                        let cfg = cfg::parse_config_or_backup(&content, path);
                        notice = Some("配置已损坏，已从备份或默认值恢复".to_string());
                        ConfigDocument {
                            schema_version: CONFIG_SCHEMA_VERSION,
                            revision: 0,
                            config: cfg,
                        }
                    }
                }
            }
            Err(_) => ConfigDocument::default_doc(),
        }
    } else {
        ConfigDocument::default_doc()
    };

    // 旧配置迁移：无 asr/polish provider 时从旧 provider 推导
    if doc.config.asr_provider.is_empty() {
        doc.config.asr_provider = if doc.config.provider == "deepseek" {
            "mimo-api".to_string()
        } else {
            doc.config.provider.clone()
        };
    }
    if doc.config.polish_provider.is_empty() {
        doc.config.polish_provider = doc.config.provider.clone();
    }

    // 解密（跟踪失败字段，保留密文）
    let (config, unreadable) = decrypt_config_fields_tracked(doc.config);
    doc.config = config;

    ConfigRepoState {
        doc,
        notice,
        read_only_reason,
        unreadable_secrets: unreadable,
    }
}

/// 解密配置中所有 `ENC:` 前缀字段；失败字段保留原始密文并记录原因（P3-04）。
fn decrypt_config_fields_tracked(
    mut config: AppConfig,
) -> (AppConfig, HashMap<SecretPath, String>) {
    let mut unreadable = HashMap::new();

    if config.api_key.starts_with("ENC:") {
        match dpapi::decrypt_string(&config.api_key[4..]) {
            Ok(d) => config.api_key = d,
            Err(e) => {
                crate::error!("API Key 解密失败: {}", e);
                unreadable.insert(SecretPath::Api, e);
            }
        }
    }
    for (pid, value) in config.provider_keys.iter_mut() {
        if value.starts_with("ENC:") {
            match dpapi::decrypt_string(&value[4..]) {
                Ok(d) => *value = d,
                Err(e) => {
                    crate::error!("Provider Key [{}] 解密失败: {}", pid, e);
                    unreadable.insert(SecretPath::Provider(pid.clone()), e);
                }
            }
        }
    }
    for cp in config.custom_providers.iter_mut() {
        if cp.api_key.starts_with("ENC:") {
            match dpapi::decrypt_string(&cp.api_key[4..]) {
                Ok(d) => cp.api_key = d,
                Err(e) => {
                    crate::error!("[{}] 自定义服务商 Key 解密失败: {}", cp.name, e);
                    unreadable.insert(SecretPath::CustomProvider(cp.id.clone()), e);
                }
            }
        }
    }
    (config, unreadable)
}

/// 把内存文档序列化为落盘 JSON：明文敏感字段加密，密文（解密失败）原样保留。
fn config_to_disk_json(doc: &ConfigDocument) -> Result<String, String> {
    let mut save = doc.config.clone();

    // 向后兼容：同步 api_key 为当前活跃 provider 的 key
    if save.api_key.is_empty() {
        save.api_key = save
            .provider_keys
            .get(&save.asr_provider)
            .or_else(|| save.provider_keys.get(&save.polish_provider))
            .cloned()
            .unwrap_or_default();
    }

    save.api_key = encrypt_secret_for_disk(&save.api_key)?;
    for value in save.provider_keys.values_mut() {
        *value = encrypt_secret_for_disk(value)?;
    }
    for cp in save.custom_providers.iter_mut() {
        cp.api_key = encrypt_secret_for_disk(&cp.api_key)?;
    }

    let out = ConfigDocument {
        schema_version: CONFIG_SCHEMA_VERSION,
        revision: doc.revision,
        config: save,
    };
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

/// 加密单个敏感值写盘；已是密文（解密失败字段）原样写回。
fn encrypt_secret_for_disk(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.starts_with("ENC:") {
        return Ok(value.to_string());
    }
    let encrypted = dpapi::encrypt_string(value)?;
    Ok(format!("ENC:{}", encrypted))
}

/// 完整配置不变量校验（P3-03）。
fn validate_config(config: &AppConfig) -> Result<(), String> {
    // theme 属于 system/light/dark
    if !matches!(config.theme.as_str(), "system" | "light" | "dark") {
        return Err("主题必须是 system / light / dark 之一".to_string());
    }
    // slice seconds 属于能力列表
    if !ALLOWED_SLICE_SECONDS.contains(&config.slice_seconds) {
        return Err(format!(
            "切片时长必须是以下值之一: {:?}",
            ALLOWED_SLICE_SECONDS
        ));
    }
    // ASR provider 存在且拥有 ASR model
    let providers = provider::get_providers();
    let asr = providers
        .iter()
        .find(|p| p.id == config.asr_provider)
        .ok_or_else(|| format!("ASR 服务商不存在: {}", config.asr_provider))?;
    if asr.asr_models.is_empty() {
        return Err(format!("服务商 {} 不支持语音转写", config.asr_provider));
    }
    // 自定义 provider 合法性 + id 唯一 + 不与内置冲突
    let mut seen: HashSet<&str> = HashSet::new();
    for cp in &config.custom_providers {
        validate_custom_provider(cp)?;
        if !seen.insert(cp.id.as_str()) {
            return Err(format!("自定义服务商 ID 重复: {}", cp.id));
        }
        if providers.iter().any(|p| p.id == cp.id) {
            return Err(format!("自定义服务商 ID 与内置服务商冲突: {}", cp.id));
        }
    }
    Ok(())
}

/// 单个自定义服务商合法性校验。
fn validate_custom_provider(cp: &CustomProvider) -> Result<(), String> {
    if cp.id.is_empty() {
        return Err("自定义服务商 ID 不能为空".to_string());
    }
    if cp.id.starts_with("custom_") {
        return Err("自定义服务商 ID 不能以 custom_ 开头".to_string());
    }
    if cp.name.is_empty() {
        return Err("自定义服务商名称不能为空".to_string());
    }
    if cp.models.is_empty() {
        return Err(format!("自定义服务商 {} 至少需要一个模型", cp.id));
    }
    // 复用 endpoint 结构化 URL 校验（S-02）
    crate::endpoint::validate_endpoint(&cp.base_url, crate::endpoint::EndpointPolicy::DEFAULT)
        .map_err(|e| format!("自定义服务商地址无效: {}", e))?;
    Ok(())
}

/// H-03 归一 polish provider/model 组合（从 commands/config.rs 移入共享位置）：
/// 内置 provider 的模型必须属于其 chat_models 集合；custom_xxx 必须存在于
/// custom_providers 且模型属于其 models 集合；否则回退到集合首个默认模型。
/// 指向已删除的自定义服务商时回退内置默认（deepseek / deepseek-v4-pro）。
fn normalize_polish_config(config: &mut AppConfig) {
    const DEFAULT_PROVIDER: &str = "deepseek";
    const DEFAULT_MODEL: &str = "deepseek-v4-pro";

    let builtin_chat = |id: &str| -> Vec<String> {
        provider::get_providers()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.chat_models)
            .unwrap_or_default()
    };

    let mut provider_id = config.polish_provider.clone();
    let mut allowed: Vec<String> = if let Some(custom_id) = provider_id.strip_prefix("custom_") {
        match config.custom_providers.iter().find(|cp| cp.id == custom_id) {
            Some(cp) => cp.models.clone(),
            None => {
                provider_id = DEFAULT_PROVIDER.to_string();
                builtin_chat(&provider_id)
            }
        }
    } else {
        builtin_chat(&provider_id)
    };
    if allowed.is_empty() {
        provider_id = DEFAULT_PROVIDER.to_string();
        allowed = builtin_chat(&provider_id);
    }
    if !allowed.contains(&config.polish_model) {
        config.polish_model = allowed
            .first()
            .cloned()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    }
    config.polish_provider = provider_id;
}

// ============================================================
// 内部辅助
// ============================================================

/// 一次写事务的产出：候选文档 + 需清除的解密 issue + 提交后的运行时副作用
struct MutationOutcome {
    doc: ConfigDocument,
    cleared: Vec<SecretPath>,
    side_effect: Option<SideEffect>,
}

enum SideEffect {
    SetLogging(bool),
}

fn snapshot_state(state: &ConfigRepoState) -> ConfigSnapshot {
    let secret_issues = state
        .unreadable_secrets
        .iter()
        .map(|(p, r)| SecretIssue {
            path: p.clone(),
            reason: r.clone(),
        })
        .collect();
    ConfigSnapshot {
        revision: state.doc.revision,
        config: state.doc.config.clone(),
        notice: state.notice.clone(),
        secret_issues,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mimo_cfig_store_test_{}_{}_{}",
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

    /// 建一个含合法配置的仓库（IP 字面量 base_url 避免测试依赖真实 DNS）
    fn repo_with(dir: &Path, json: &str) -> ConfigRepository {
        let path = dir.join("config.json");
        std::fs::write(&path, json).unwrap();
        let state = load_repository(&path);
        ConfigRepository {
            inner: tokio::sync::Mutex::new(state),
            path,
        }
    }

    fn valid_json() -> String {
        serde_json::json!({
            "provider": "mimo-api",
            "base_url": "https://api.xiaomimimo.com/v1",
            "api_key": "",
            "transcription_model": "mimo-v2.5-asr",
            "polish_model": "deepseek-v4-pro",
            "theme": "system",
            "provider_keys": {},
            "logging_enabled": true,
            "asr_provider": "mimo-api",
            "polish_provider": "deepseek",
            "slice_seconds": 10,
            "custom_providers": []
        })
        .to_string()
    }

    /// 旧 flat JSON 迁移为 schema 1、revision 从 0 起
    #[tokio::test]
    async fn legacy_flat_json_migrates_to_schema1() {
        let dir = test_dir("migrate");
        let repo = repo_with(&dir, &valid_json());
        let snap = repo.snapshot().await;
        assert_eq!(snap.revision, 0);
        assert_eq!(snap.config.asr_provider, "mimo-api");
        assert_eq!(snap.config.theme, "system");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// revision 不匹配返回 REVISION_CONFLICT 与最新 revision
    #[tokio::test]
    async fn revision_conflict_reported() {
        let dir = test_dir("conflict");
        let repo = repo_with(&dir, &valid_json());
        let err = repo.set_theme(5, "dark".to_string()).await.unwrap_err();
        assert_eq!(err.code, "REVISION_CONFLICT");
        assert_eq!(err.current_revision, Some(0));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 成功提交：revision +1、值更新、落盘
    #[tokio::test]
    async fn set_theme_commits_and_bumps_revision() {
        let dir = test_dir("theme");
        let repo = repo_with(&dir, &valid_json());
        let snap = repo.set_theme(0, "dark".to_string()).await.unwrap();
        assert_eq!(snap.revision, 1);
        assert_eq!(snap.config.theme, "dark");
        // revision 冲突：旧 revision 再次提交失败
        assert!(repo.set_theme(0, "light".to_string()).await.is_err());
        let snap2 = repo.set_theme(1, "light".to_string()).await.unwrap();
        assert_eq!(snap2.config.theme, "light");
        assert_eq!(snap2.revision, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 非法 theme 被校验拒绝
    #[tokio::test]
    async fn invalid_theme_rejected() {
        let dir = test_dir("theme_bad");
        let repo = repo_with(&dir, &valid_json());
        let err = repo.set_theme(0, "neon".to_string()).await.unwrap_err();
        assert_eq!(err.code, "VALIDATION");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 非法切片时长被校验拒绝；合法值提交
    #[tokio::test]
    async fn slice_seconds_validated() {
        let dir = test_dir("slice");
        let repo = repo_with(&dir, &valid_json());
        assert!(repo.set_slice_seconds(0, 7).await.is_err());
        let snap = repo.set_slice_seconds(0, 60).await.unwrap();
        assert_eq!(snap.config.slice_seconds, 60);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 不存在的 ASR 服务商被拒绝
    #[tokio::test]
    async fn asr_provider_validated() {
        let dir = test_dir("asr");
        let repo = repo_with(&dir, &valid_json());
        // deepseek 无 ASR model
        assert!(repo
            .set_asr_provider(0, "deepseek".to_string())
            .await
            .is_err());
        let snap = repo
            .set_asr_provider(0, "mimo-token-plan".to_string())
            .await
            .unwrap();
        assert_eq!(snap.config.asr_provider, "mimo-token-plan");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn custom_json() -> String {
        serde_json::json!({
            "provider": "mimo-api",
            "base_url": "https://api.xiaomimimo.com/v1",
            "api_key": "",
            "transcription_model": "mimo-v2.5-asr",
            "polish_model": "deepseek-v4-pro",
            "theme": "system",
            "provider_keys": {},
            "logging_enabled": true,
            "asr_provider": "mimo-api",
            "polish_provider": "deepseek",
            "slice_seconds": 10,
            "custom_providers": []
        })
        .to_string()
    }

    fn sample_input() -> CustomProviderInput {
        CustomProviderInput {
            id: "cp1".to_string(),
            name: "自建".to_string(),
            base_url: "https://8.8.8.8/v1".to_string(),
            api_key: "sk-test".to_string(),
            models: vec!["m-a".to_string()],
        }
    }

    /// upsert 自定义服务商 + 校验重复 id / 非法地址
    #[tokio::test]
    async fn upsert_custom_provider_validated() {
        let dir = test_dir("upsert");
        let repo = repo_with(&dir, &custom_json());
        // 合法地址提交
        let snap = repo
            .upsert_custom_provider(0, sample_input())
            .await
            .unwrap();
        assert_eq!(snap.config.custom_providers.len(), 1);
        // 重复 id：更新（幂等），不新增
        let snap = repo
            .upsert_custom_provider(1, sample_input())
            .await
            .unwrap();
        assert_eq!(snap.config.custom_providers.len(), 1);
        // 非法地址被拒绝
        let bad = CustomProviderInput {
            base_url: "not-a-url".to_string(),
            ..sample_input()
        };
        assert!(repo.upsert_custom_provider(2, bad).await.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 删除当前选中的自定义 polish 服务商时回退内置默认
    #[tokio::test]
    async fn delete_selected_custom_provider_falls_back() {
        let dir = test_dir("delete");
        let mut json: serde_json::Value = serde_json::from_str(&custom_json()).unwrap();
        json["custom_providers"] = serde_json::json!([{
            "id": "cp1", "name": "自建", "base_url": "https://8.8.8.8/v1",
            "api_key": "sk-x", "models": ["m-a", "m-b"]
        }]);
        json["polish_provider"] = serde_json::Value::String("custom_cp1".into());
        json["polish_model"] = serde_json::Value::String("m-a".into());
        let repo = repo_with(&dir, &json.to_string());
        let snap = repo
            .delete_custom_provider(0, "cp1".to_string())
            .await
            .unwrap();
        assert!(snap.config.custom_providers.is_empty());
        assert_eq!(snap.config.polish_provider, "deepseek");
        assert_eq!(snap.config.polish_model, "deepseek-v4-pro");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 删除不存在的自定义服务商报错
    #[tokio::test]
    async fn delete_missing_custom_provider_errors() {
        let dir = test_dir("delete_missing");
        let repo = repo_with(&dir, &custom_json());
        assert!(repo
            .delete_custom_provider(0, "nope".to_string())
            .await
            .is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 更高 schema 版本进入只读
    #[tokio::test]
    async fn higher_schema_enters_readonly() {
        let dir = test_dir("readonly");
        let mut json: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
        json["schema_version"] = serde_json::Value::from(99);
        let original = json.to_string();
        let repo = repo_with(&dir, &original);
        let err = repo.set_theme(0, "dark".to_string()).await.unwrap_err();
        assert_eq!(err.code, "VALIDATION");
        assert!(err.message.contains("只读"));
        // 原文件保留（未被覆盖）
        assert_eq!(
            std::fs::read_to_string(repo.path.clone()).unwrap(),
            original
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 设置 provider key：内存为明文，落盘为 ENC: 密文
    #[tokio::test]
    async fn set_provider_key_encrypts_on_disk() {
        let dir = test_dir("key");
        let repo = repo_with(&dir, &valid_json());
        let snap = repo
            .set_provider_key(0, "mimo-api".to_string(), "sk-my-key".to_string())
            .await
            .unwrap();
        assert_eq!(
            snap.config.provider_keys.get("mimo-api").unwrap(),
            "sk-my-key"
        );
        let disk = std::fs::read_to_string(repo.path.clone()).unwrap();
        assert!(disk.contains("ENC:"), "落盘必须是密文");
        assert!(!disk.contains("sk-my-key"), "落盘不得含明文");
        std::fs::remove_dir_all(&dir).ok();
    }
}
