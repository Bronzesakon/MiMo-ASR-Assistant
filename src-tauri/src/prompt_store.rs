//! 提示词分条管理存储模块（清单 F-06）。
//!
//! 职责：`prompts.json` 的路径、schema、校验、旧配置迁移、原子读写与损坏恢复。
//! 运行时提示词唯一事实来源 = 本模块维护的 `PromptDocument`；
//! 旧 `config.json.polish_prompt` 仅用于首次迁移兼容。
//!
//! 日志纪律：只记录操作、条目 ID、条目数量与错误类别；
//! 禁止记录标题、正文或整份 JSON。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ============================================================
// 常量与数据模型
// ============================================================

/// 当前 schema 版本
pub const SCHEMA_VERSION: u32 = 1;

/// 内置默认条目固定 ID
pub const BUILTIN_DEFAULT_ID: &str = "builtin-default";

/// 内置默认条目固定标题
pub const BUILTIN_DEFAULT_TITLE: &str = "默认";

/// 条目总数上限
pub const MAX_PROMPTS: usize = 8;

/// 标题长度上限（Unicode 字符计数）
pub const TITLE_MAX_CHARS: usize = 24;

/// 正文长度上限（Unicode 字符计数）
pub const CONTENT_MAX_CHARS: usize = 4000;

/// 迁移出的旧自定义提示词标题
const MIGRATED_CUSTOM_TITLE: &str = "用户自定义";

/// 新默认规整提示词（F-06.2：全应用唯一一份，前端不得复制）
pub const DEFAULT_POLISH_PROMPT: &str = "你是一名专业的语音转写文本编辑。请对用户提供的语音转写结果进行忠实规整，并严格遵循以下要求：\n\n1. 结合上下文修正明显的语音识别错误、错别字、同音误写以及专业术语错误。\n2. 删除无实际语义的语气词、口头禅和明显的口语重复，但必须保留所有有效信息、观点、事实、数字、专有名词和逻辑关系。\n3. 优化标点、断句和段落结构，使文本自然、清晰、易读。\n4. 不得新增原文没有的事实，不得总结、概括、缩写或省略有效内容，不得改变原意。\n5. 使用规范的 Markdown 格式输出；根据内容合理使用标题、段落、列表和引用，但不要为了格式而改变内容结构。\n6. 只输出规整后的完整正文，不要解释修改过程，不要添加前言、结语或免责声明。";

/// 旧默认提示词（迁移专用比较基准，内容与 F-06 前版本逐字节一致）
pub const LEGACY_DEFAULT_POLISH_PROMPT: &str = "给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，优化每句话的口语化表达的语气词、重复字等，要求只做修饰，每句话都要输出，不可简略，不可概括，根据原文的内容，智能切分长段落";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptEntry {
    pub id: String,
    pub title: String,
    pub content: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptDocument {
    pub schema_version: u32,
    pub selected_prompt_id: String,
    pub prompts: Vec<PromptEntry>,
}

fn default_document() -> PromptDocument {
    PromptDocument {
        schema_version: SCHEMA_VERSION,
        selected_prompt_id: BUILTIN_DEFAULT_ID.to_string(),
        prompts: vec![PromptEntry {
            id: BUILTIN_DEFAULT_ID.to_string(),
            title: BUILTIN_DEFAULT_TITLE.to_string(),
            content: DEFAULT_POLISH_PROMPT.to_string(),
            is_default: true,
        }],
    }
}

/// 自定义条目 ID：时间纳秒 + 进程内自增序号，不可预测且文档内唯一
fn new_prompt_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("custom-{:x}-{:x}", nanos, seq)
}

// ============================================================
// 校验（F-06.1.7：权威校验全部在后端）
// ============================================================

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn validate_title(title: &str) -> Result<String, String> {
    let t = title.trim();
    if t.is_empty() {
        return Err("提示词标题不能为空".to_string());
    }
    if char_len(t) > TITLE_MAX_CHARS {
        return Err(format!("提示词标题不能超过 {} 个字符", TITLE_MAX_CHARS));
    }
    Ok(t.to_string())
}

fn validate_content(content: &str) -> Result<String, String> {
    let c = content.trim();
    if c.is_empty() {
        return Err("提示词正文不能为空".to_string());
    }
    if char_len(c) > CONTENT_MAX_CHARS {
        return Err(format!("提示词正文不能超过 {} 个字符", CONTENT_MAX_CHARS));
    }
    Ok(c.to_string())
}

/// 校验自定义标题在文档内唯一（trim 后、大小写敏感、不做 Unicode 折叠；
/// 与固定标题“默认”的比较包含在内，因此自定义项无法占用该标题）
fn check_title_unique(
    doc: &PromptDocument,
    title: &str,
    exclude_id: Option<&str>,
) -> Result<(), String> {
    let dup = doc
        .prompts
        .iter()
        .any(|p| Some(p.id.as_str()) != exclude_id && p.title == title);
    if dup {
        return Err(format!("已存在同名提示词「{}」，请换一个标题", title));
    }
    Ok(())
}

/// 文档级不变量校验（解析手改 JSON 与写盘前都必须通过）
fn validate_document(doc: &PromptDocument) -> Result<(), String> {
    if doc.schema_version != SCHEMA_VERSION {
        return Err("当前应用版本无法读取该提示词文件".to_string());
    }
    if doc.prompts.is_empty() || doc.prompts.len() > MAX_PROMPTS {
        return Err(format!("提示词条目数量必须为 1~{}", MAX_PROMPTS));
    }
    // ID 非空且唯一
    for (i, p) in doc.prompts.iter().enumerate() {
        if p.id.trim().is_empty() {
            return Err(format!("第 {} 条提示词缺少 ID", i + 1));
        }
        if doc.prompts.iter().filter(|q| q.id == p.id).count() > 1 {
            return Err("提示词 ID 重复".to_string());
        }
    }
    // 恰好一个默认项，且为固定 ID
    let defaults: Vec<&PromptEntry> = doc.prompts.iter().filter(|p| p.is_default).collect();
    if defaults.len() != 1 {
        return Err("必须恰好存在一个默认提示词".to_string());
    }
    if defaults[0].id != BUILTIN_DEFAULT_ID {
        return Err("默认提示词 ID 非法".to_string());
    }
    for p in &doc.prompts {
        if p.title != p.title.trim() || p.title.is_empty() || char_len(&p.title) > TITLE_MAX_CHARS {
            return Err("提示词标题格式非法".to_string());
        }
        if p.content != p.content.trim()
            || p.content.is_empty()
            || char_len(&p.content) > CONTENT_MAX_CHARS
        {
            return Err("提示词正文格式非法".to_string());
        }
    }
    // 标题两两不同（大小写敏感）
    for i in 0..doc.prompts.len() {
        for j in (i + 1)..doc.prompts.len() {
            if doc.prompts[i].title == doc.prompts[j].title {
                return Err("提示词标题重复".to_string());
            }
        }
    }
    Ok(())
}

/// 解析并校验主/备份文件内容；未知更高版本单独报错（不得按 v1 猜测解析）
fn parse_document(text: &str) -> Result<PromptDocument, ParseError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| ParseError::Corrupt)?;
    if let Some(v) = value.get("schema_version").and_then(|v| v.as_u64()) {
        if v > SCHEMA_VERSION as u64 {
            return Err(ParseError::UnsupportedVersion);
        }
    }
    let doc: PromptDocument = serde_json::from_str(text).map_err(|_| ParseError::Corrupt)?;
    validate_document(&doc).map_err(|_| ParseError::Corrupt)?;
    Ok(doc)
}

#[derive(Debug, PartialEq)]
enum ParseError {
    Corrupt,
    UnsupportedVersion,
}

// ============================================================
// 原子写与损坏恢复（F-06.3.9 / F-06.3.10）
// ============================================================

const MAIN_FILE: &str = "prompts.json";
const BAK_FILE: &str = "prompts.json.bak";

/// 原子写核心（T-01）：序列化 → 写同目录 .tmp → flush + sync_all →
/// （可选）旧主文件复制为 .bak → 原子替换主文件（Windows 经 MoveFileExW）。
fn write_document(dir: &Path, doc: &PromptDocument, backup: bool) -> std::io::Result<()> {
    let main = dir.join(MAIN_FILE);
    let json = serde_json::to_string_pretty(doc).map_err(std::io::Error::other)?;
    if backup {
        crate::atomic::atomic_write_text_with_backup(&main, &json)
    } else {
        crate::atomic::atomic_write_text_no_backup(&main, &json)
    }
}

/// 原子写：序列化 → 写同目录 .tmp → flush + sync_all → 旧主文件复制为 .bak → rename 替换
fn atomic_write_document(dir: &Path, doc: &PromptDocument) -> std::io::Result<()> {
    write_document(dir, doc, true)
}

/// 恢复专用原子写（Warning 5）：不带备份步骤（直接 tmp→sync→rename），
/// 避免从 .bak 恢复主文件时把（损坏的）主文件复制回 .bak 覆盖完好备份
fn atomic_write_document_no_backup(dir: &Path, doc: &PromptDocument) -> std::io::Result<()> {
    write_document(dir, doc, false)
}

/// 损坏证据保留：把损坏的主文件/备份重命名为带 UTC 时间戳的 corrupt 副本
fn preserve_corrupt_evidence(dir: &Path) {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let main = dir.join(MAIN_FILE);
    let bak = dir.join(BAK_FILE);
    if main.exists() {
        let target = dir.join(format!("prompts.{}.corrupt.json", ts));
        let _ = std::fs::rename(&main, &target);
    }
    if bak.exists() {
        let target = dir.join(format!("prompts.{}.bak.corrupt.json", ts));
        let _ = std::fs::rename(&bak, &target);
    }
}

/// 旧配置迁移（F-06.2.4）：空/仅空白/严格等于旧默认 → 只建默认项；
/// 其余 trim 后非空的值作为“用户自定义”放在默认项之后
fn migrate_from_legacy(legacy_prompt: &str) -> PromptDocument {
    let mut doc = default_document();
    let trimmed = legacy_prompt.trim();
    if !trimmed.is_empty() && legacy_prompt != LEGACY_DEFAULT_POLISH_PROMPT {
        doc.prompts.push(PromptEntry {
            id: new_prompt_id(),
            title: MIGRATED_CUSTOM_TITLE.to_string(),
            content: trimmed.to_string(),
            is_default: false,
        });
        crate::info!("[prompts] 已从旧配置迁移 1 条自定义提示词");
    }
    doc
}

// ============================================================
// PromptRepository（F-06.3.2：单写锁 + 事务式写操作）
// ============================================================

struct RepoState {
    doc: PromptDocument,
    /// 待向用户展示的恢复/持久化提示（一次性消费）
    notice: Option<String>,
    /// 只读原因（如未知更高 schema 版本）：设置后禁止任何写盘，避免覆盖原文件
    read_only_reason: Option<String>,
}

pub struct PromptRepository {
    dir: PathBuf,
    state: tokio::sync::Mutex<RepoState>,
}

impl PromptRepository {
    /// 打开仓库：加载 / 迁移 / 损坏恢复都在此完成。
    /// 永不失败——磁盘问题降级为内存文档 + 用户可见提示。
    pub fn open(dir: PathBuf, legacy_polish_prompt: &str) -> Self {
        let (doc, notice, read_only_reason) = Self::load_or_init(&dir, legacy_polish_prompt);
        Self {
            dir,
            state: tokio::sync::Mutex::new(RepoState {
                doc,
                notice,
                read_only_reason,
            }),
        }
    }

    fn load_or_init(
        dir: &Path,
        legacy_polish_prompt: &str,
    ) -> (PromptDocument, Option<String>, Option<String>) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            crate::warn!("[prompts] 创建目录失败，本会话修改可能无法持久化: {}", e);
            let reason = "提示词目录不可写，本次会话的修改无法保存".to_string();
            return (
                migrate_from_legacy(legacy_polish_prompt),
                Some(reason.clone()),
                Some(reason),
            );
        }
        let main = dir.join(MAIN_FILE);
        let bak = dir.join(BAK_FILE);

        if main.exists() {
            match std::fs::read_to_string(&main)
                .ok()
                .map(|t| parse_document(&t))
            {
                Some(Ok(doc)) => {
                    crate::info!("[prompts] 加载主文件成功，共 {} 条", doc.prompts.len());
                    return (doc, None, None);
                }
                Some(Err(ParseError::UnsupportedVersion)) => {
                    crate::warn!("[prompts] 主文件 schema 版本高于当前应用，进入只读模式");
                    return (
                        default_document(),
                        None,
                        Some("当前应用版本无法读取该提示词文件".to_string()),
                    );
                }
                Some(Err(ParseError::Corrupt)) | None => {
                    crate::warn!("[prompts] 主文件损坏，尝试备份");
                }
            }
            // 主文件损坏：尝试 .bak
            let bak_doc = std::fs::read_to_string(&bak)
                .ok()
                .and_then(|t| parse_document(&t).ok());
            if let Some(doc) = bak_doc {
                // 恢复主文件（失败不阻断使用，备份仍在）。
                // Warning 5：用不带备份步骤的变体写盘，否则内部会把
                // 损坏的主文件复制为 .bak，覆盖唯一完好的备份
                if atomic_write_document_no_backup(dir, &doc).is_err() {
                    crate::warn!("[prompts] 从备份恢复主文件写盘失败");
                }
                crate::info!("[prompts] 已从备份恢复，共 {} 条", doc.prompts.len());
                return (
                    doc,
                    Some("提示词文件已损坏，已从备份恢复".to_string()),
                    None,
                );
            }
            // 两者均损坏：保留证据后重建默认文档
            preserve_corrupt_evidence(dir);
            let doc = default_document();
            if atomic_write_document(dir, &doc).is_err() {
                crate::warn!("[prompts] 重建默认文档写盘失败");
                let reason = "提示词文件持久化失败，本次会话的修改无法保存".to_string();
                return (doc, Some(reason.clone()), Some(reason));
            }
            crate::info!("[prompts] 主文件与备份均损坏，已重建默认文档");
            return (
                doc,
                Some("提示词文件已损坏，已重建默认提示词".to_string()),
                None,
            );
        }

        // prompts.json 不存在：执行一次性迁移（F-06.2.2）
        let doc = migrate_from_legacy(legacy_polish_prompt);
        if let Err(e) = atomic_write_document(dir, &doc) {
            // 迁移写入失败：不创建空文件、不标记成功；会话内提供内存默认项，
            // 并进入只读（不可持久化）状态，避免用户误以为已保存
            crate::warn!("[prompts] 初始化写盘失败: {}", e);
            let reason = "提示词文件持久化失败，本次会话的修改无法保存".to_string();
            return (doc, Some(reason.clone()), Some(reason));
        }
        crate::info!("[prompts] 初始化完成，共 {} 条", doc.prompts.len());
        (doc, None, None)
    }

    fn ensure_writable(state: &RepoState) -> Result<(), String> {
        if let Some(reason) = &state.read_only_reason {
            return Err(reason.clone());
        }
        Ok(())
    }

    /// 事务式变更（F-06.3.2 / Critical 修复）：单次加锁内完成
    /// “读 state.doc → 校验 → 构造新文档 → 写盘 → 成功后替换内存”，
    /// 锁全程不中途释放，并发写由锁串行化，消除两段式加锁的丢更新窗口。
    async fn mutate<F>(&self, f: F) -> Result<PromptDocument, String>
    where
        F: FnOnce(&PromptDocument) -> Result<PromptDocument, String>,
    {
        let mut state = self.state.lock().await;
        Self::ensure_writable(&state)?;
        let new_doc = f(&state.doc)?;
        validate_document(&new_doc)?;
        atomic_write_document(&self.dir, &new_doc)
            .map_err(|e| format!("写入提示词文件失败: {}", e))?;
        state.doc = new_doc;
        Ok(state.doc.clone())
    }

    pub async fn load(&self) -> Result<PromptDocument, String> {
        let state = self.state.lock().await;
        if let Some(reason) = &state.read_only_reason {
            return Err(reason.clone());
        }
        Ok(state.doc.clone())
    }

    /// 取回一次性恢复/持久化提示（供命令层转发给前端）
    pub async fn take_notice(&self) -> Option<String> {
        self.state.lock().await.notice.take()
    }

    pub async fn create(&self, title: &str, content: &str) -> Result<PromptDocument, String> {
        let title = validate_title(title)?;
        let content = validate_content(content)?;
        let doc = self
            .mutate(|current| {
                if current.prompts.len() >= MAX_PROMPTS {
                    return Err(format!(
                        "最多保存 {} 条提示词，请先删除不需要的条目",
                        MAX_PROMPTS
                    ));
                }
                check_title_unique(current, &title, None)?;
                let mut doc = current.clone();
                doc.prompts.insert(
                    0,
                    PromptEntry {
                        id: new_prompt_id(),
                        title,
                        content,
                        is_default: false,
                    },
                );
                Ok(doc)
            })
            .await?;
        crate::info!("[prompts] 新建条目成功，共 {} 条", doc.prompts.len());
        Ok(doc)
    }

    /// 更新条目；默认项忽略标题修改（标题固定为“默认”）
    pub async fn update(
        &self,
        id: &str,
        title: Option<&str>,
        content: &str,
    ) -> Result<PromptDocument, String> {
        let content = validate_content(content)?;
        let title = match title {
            Some(t) => Some(validate_title(t)?),
            None => None,
        };
        let id = id.to_string();
        let doc = self
            .mutate(|current| {
                let mut doc = current.clone();
                let idx = doc
                    .prompts
                    .iter()
                    .position(|p| p.id == id)
                    .ok_or_else(|| "提示词不存在或已被删除".to_string())?;
                let is_default = doc.prompts[idx].id == BUILTIN_DEFAULT_ID;
                if !is_default {
                    if let Some(t) = &title {
                        check_title_unique(&doc, t, Some(&id))?;
                        doc.prompts[idx].title = t.clone();
                    }
                }
                doc.prompts[idx].content = content;
                Ok(doc)
            })
            .await;
        match &doc {
            Ok(d) => crate::info!("[prompts] 更新条目 id={}，共 {} 条", id, d.prompts.len()),
            Err(_) => crate::warn!("[prompts] 更新条目 id={} 写盘失败", id),
        }
        doc
    }

    pub async fn delete(&self, id: &str) -> Result<PromptDocument, String> {
        if id == BUILTIN_DEFAULT_ID {
            return Err("默认提示词不可删除".to_string());
        }
        let id = id.to_string();
        let doc = self
            .mutate(|current| {
                if !current.prompts.iter().any(|p| p.id == id) {
                    return Err("提示词不存在或已被删除".to_string());
                }
                let mut doc = current.clone();
                doc.prompts.retain(|p| p.id != id);
                // 删除的是最近选择项：同一事务内回退 builtin-default
                if doc.selected_prompt_id == id {
                    doc.selected_prompt_id = BUILTIN_DEFAULT_ID.to_string();
                }
                Ok(doc)
            })
            .await?;
        crate::info!("[prompts] 删除条目 id={}，共 {} 条", id, doc.prompts.len());
        Ok(doc)
    }

    /// 重排序：ordered_ids 必须与当前 ID 集合完全相等（不缺、不重、不加）
    pub async fn reorder(&self, ordered_ids: &[String]) -> Result<PromptDocument, String> {
        let ordered_ids = ordered_ids.to_vec();
        let doc = self
            .mutate(|current| {
                if ordered_ids.len() != current.prompts.len() {
                    return Err("排序列表与当前条目不一致".to_string());
                }
                let mut seen = std::collections::HashSet::new();
                for id in &ordered_ids {
                    if !current.prompts.iter().any(|p| &p.id == id) {
                        return Err("排序列表包含未知条目".to_string());
                    }
                    if !seen.insert(id.as_str()) {
                        return Err("排序列表包含重复条目".to_string());
                    }
                }
                let mut doc = current.clone();
                doc.prompts = ordered_ids
                    .iter()
                    .map(|id| doc.prompts.iter().find(|p| &p.id == id).cloned().unwrap())
                    .collect();
                Ok(doc)
            })
            .await?;
        crate::info!("[prompts] 排序更新，共 {} 条", doc.prompts.len());
        Ok(doc)
    }

    /// 选择最近项：必须先原子持久化成功才返回新文档
    pub async fn select(&self, id: &str) -> Result<PromptDocument, String> {
        let id = id.to_string();
        let doc = self
            .mutate(|current| {
                if !current.prompts.iter().any(|p| p.id == id) {
                    return Err("提示词不存在或已被删除".to_string());
                }
                let mut doc = current.clone();
                doc.selected_prompt_id = id;
                Ok(doc)
            })
            .await?;
        crate::info!("[prompts] 最近选择更新 id={}", doc.selected_prompt_id);
        Ok(doc)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mimo_prompts_test_{}_{}_{}",
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

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---------- 初始化 ----------

    #[test]
    fn init_creates_single_default_entry() {
        let dir = test_dir("init");
        let repo = PromptRepository::open(dir.clone(), "");
        let doc = tokio_block(repo.load()).unwrap();
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
        assert_eq!(doc.selected_prompt_id, BUILTIN_DEFAULT_ID);
        assert_eq!(doc.prompts.len(), 1);
        assert_eq!(doc.prompts[0].id, BUILTIN_DEFAULT_ID);
        assert_eq!(doc.prompts[0].title, BUILTIN_DEFAULT_TITLE);
        assert_eq!(doc.prompts[0].content, DEFAULT_POLISH_PROMPT);
        assert!(doc.prompts[0].is_default);
        assert!(dir.join(MAIN_FILE).exists(), "应已持久化");
        cleanup(&dir);
    }

    // ---------- 迁移（六类输入） ----------

    #[test]
    fn migration_empty_and_blank_create_default_only() {
        for legacy in ["", "   \n\t "] {
            let dir = test_dir("mig_empty");
            let repo = PromptRepository::open(dir.clone(), legacy);
            let doc = tokio_block(repo.load()).unwrap();
            assert_eq!(doc.prompts.len(), 1, "legacy={:?} 应只创建默认项", legacy);
            cleanup(&dir);
        }
    }

    #[test]
    fn migration_exact_legacy_default_creates_default_only() {
        let dir = test_dir("mig_exact");
        let repo = PromptRepository::open(dir.clone(), LEGACY_DEFAULT_POLISH_PROMPT);
        let doc = tokio_block(repo.load()).unwrap();
        assert_eq!(doc.prompts.len(), 1);
        cleanup(&dir);
    }

    #[test]
    fn migration_legacy_default_with_whitespace_becomes_custom() {
        let dir = test_dir("mig_ws");
        let legacy = format!("  {}  ", LEGACY_DEFAULT_POLISH_PROMPT);
        let repo = PromptRepository::open(dir.clone(), &legacy);
        let doc = tokio_block(repo.load()).unwrap();
        assert_eq!(doc.prompts.len(), 2, "仅首尾空白差异也应迁移为自定义");
        assert_eq!(doc.prompts[1].title, MIGRATED_CUSTOM_TITLE);
        assert_eq!(doc.prompts[1].content, LEGACY_DEFAULT_POLISH_PROMPT);
        assert_eq!(
            doc.selected_prompt_id, BUILTIN_DEFAULT_ID,
            "首次迁移不改变选择"
        );
        cleanup(&dir);
    }

    #[test]
    fn migration_custom_value_placed_after_default() {
        let dir = test_dir("mig_custom");
        let repo = PromptRepository::open(dir.clone(), "请按法律领域术语规整");
        let doc = tokio_block(repo.load()).unwrap();
        assert_eq!(doc.prompts.len(), 2);
        assert_eq!(doc.prompts[0].id, BUILTIN_DEFAULT_ID);
        assert_eq!(doc.prompts[1].content, "请按法律领域术语规整");
        cleanup(&dir);
    }

    #[test]
    fn migration_runs_only_once() {
        let dir = test_dir("mig_once");
        let repo = PromptRepository::open(dir.clone(), "旧自定义内容");
        drop(repo);
        // 第二次打开（文件已存在）不得再合并 config.json.polish_prompt
        let repo2 = PromptRepository::open(dir.clone(), "另一段不应出现的内容");
        let doc = tokio_block(repo2.load()).unwrap();
        assert_eq!(doc.prompts.len(), 2);
        assert!(!doc
            .prompts
            .iter()
            .any(|p| p.content.contains("另一段不应出现的内容")));
        cleanup(&dir);
    }

    // ---------- 校验 ----------

    #[tokio::test]
    async fn validation_rejects_bad_inputs() {
        let dir = test_dir("validate");
        let repo = PromptRepository::open(dir.clone(), "");
        assert!(repo.create("", "正文").await.is_err(), "空标题");
        assert!(repo.create("  ", "正文").await.is_err(), "仅空白标题");
        assert!(repo.create("标题", "  ").await.is_err(), "仅空白正文");
        assert!(
            repo.create(&"长".repeat(TITLE_MAX_CHARS + 1), "正文")
                .await
                .is_err(),
            "超长标题"
        );
        assert!(
            repo.create("标题", &"字".repeat(CONTENT_MAX_CHARS + 1))
                .await
                .is_err(),
            "超长正文"
        );
        repo.create("甲", "正文甲").await.expect("创建甲");
        assert!(repo.create("甲", "正文乙").await.is_err(), "重名");
        assert!(repo.create(" 甲 ", "正文乙").await.is_err(), "trim 后重名");
        assert!(
            repo.create(BUILTIN_DEFAULT_TITLE, "正文丙").await.is_err(),
            "不得占用固定标题"
        );
        cleanup(&dir);
    }

    #[test]
    fn document_invariants_rejected() {
        let mut doc = default_document();
        doc.prompts.clear();
        assert!(validate_document(&doc).is_err(), "0 条");
        let mut doc = default_document();
        for i in 0..8 {
            doc.prompts.push(PromptEntry {
                id: format!("c{}", i),
                title: format!("t{}", i),
                content: "c".into(),
                is_default: false,
            });
        }
        assert!(validate_document(&doc).is_err(), "9 条");
        let mut doc = default_document();
        doc.prompts[0].is_default = false;
        assert!(validate_document(&doc).is_err(), "无默认");
        let mut doc = default_document();
        doc.prompts.push(PromptEntry {
            id: "x".into(),
            title: "x".into(),
            content: "c".into(),
            is_default: true,
        });
        assert!(validate_document(&doc).is_err(), "多默认");
        let mut doc = default_document();
        doc.prompts.push(PromptEntry {
            id: "".into(),
            title: "x".into(),
            content: "c".into(),
            is_default: false,
        });
        assert!(validate_document(&doc).is_err(), "空 ID");
        let mut doc = default_document();
        doc.prompts.push(PromptEntry {
            id: BUILTIN_DEFAULT_ID.into(),
            title: "x".into(),
            content: "c".into(),
            is_default: false,
        });
        assert!(validate_document(&doc).is_err(), "重复 ID");
        let mut doc = default_document();
        doc.schema_version = 2;
        assert!(validate_document(&doc).is_err(), "未知版本");
    }

    #[test]
    fn unknown_higher_version_is_read_only_and_not_overwritten() {
        let dir = test_dir("version");
        let main = dir.join(MAIN_FILE);
        let foreign = r#"{"schema_version":99,"selected_prompt_id":"x","prompts":[]}"#;
        std::fs::write(&main, foreign).unwrap();
        let repo = PromptRepository::open(dir.clone(), "");
        assert!(tokio_block(repo.load()).is_err(), "加载应报无法读取");
        assert!(
            tokio_block(repo.create("新", "正文")).is_err(),
            "只读模式禁止写入"
        );
        assert_eq!(
            std::fs::read_to_string(&main).unwrap(),
            foreign,
            "不得覆盖原文件"
        );
        cleanup(&dir);
    }

    // ---------- CRUD ----------

    #[tokio::test]
    async fn create_inserts_at_front_and_enforces_cap() {
        let dir = test_dir("create");
        let repo = PromptRepository::open(dir.clone(), "");
        for i in 0..7 {
            repo.create(&format!("条目{}", i), &format!("正文{}", i))
                .await
                .expect("创建");
        }
        let doc = repo.load().await.unwrap();
        assert_eq!(doc.prompts.len(), MAX_PROMPTS);
        assert_eq!(doc.prompts[0].title, "条目6", "新建应插入首位");
        let err = repo
            .create("第9条", "正文")
            .await
            .expect_err("第 9 条应失败");
        assert!(err.contains("8"));
        assert_eq!(repo.load().await.unwrap().prompts.len(), MAX_PROMPTS);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn update_keeps_id_and_order_default_title_immutable() {
        let dir = test_dir("update");
        let repo = PromptRepository::open(dir.clone(), "");
        repo.create("自定义", "旧正文").await.unwrap();
        let doc = repo.load().await.unwrap();
        let custom_id = doc.prompts[0].id.clone();

        let doc = repo
            .update(BUILTIN_DEFAULT_ID, Some("改名尝试"), "新默认正文")
            .await
            .unwrap();
        assert_eq!(
            doc.prompts[1].title, BUILTIN_DEFAULT_TITLE,
            "默认项标题不可改"
        );
        assert_eq!(doc.prompts[1].content, "新默认正文", "默认项正文可编辑");

        let doc = repo
            .update(&custom_id, Some(" 新标题 "), " 新正文 ")
            .await
            .unwrap();
        assert_eq!(doc.prompts[0].id, custom_id, "ID 不变");
        assert_eq!(doc.prompts[0].title, "新标题", "标题 trim");
        assert_eq!(doc.prompts[0].content, "新正文", "正文 trim");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn delete_rules() {
        let dir = test_dir("delete");
        let repo = PromptRepository::open(dir.clone(), "");
        repo.create("甲", "正文甲").await.unwrap();
        let doc = repo.load().await.unwrap();
        let custom_id = doc.prompts[0].id.clone();

        assert!(repo.delete(BUILTIN_DEFAULT_ID).await.is_err(), "默认不可删");

        // 选择最近项后删除 → 回退 builtin-default
        repo.select(&custom_id).await.unwrap();
        let doc = repo.delete(&custom_id).await.unwrap();
        assert_eq!(doc.selected_prompt_id, BUILTIN_DEFAULT_ID);
        assert_eq!(doc.prompts.len(), 1);
        assert!(repo.delete(&custom_id).await.is_err(), "重复删除应失败");
        cleanup(&dir);
    }

    // ---------- 排序 ----------

    #[tokio::test]
    async fn reorder_rules() {
        let dir = test_dir("reorder");
        let repo = PromptRepository::open(dir.clone(), "");
        repo.create("甲", "正文甲").await.unwrap();
        repo.create("乙", "正文乙").await.unwrap();
        let doc = repo.load().await.unwrap();
        let ids: Vec<String> = doc.prompts.iter().map(|p| p.id.clone()).collect();

        // 合法：含默认项的任意顺序
        let mut reversed = ids.clone();
        reversed.reverse();
        let doc = repo.reorder(&reversed).await.unwrap();
        let now: Vec<String> = doc.prompts.iter().map(|p| p.id.clone()).collect();
        assert_eq!(now, reversed);

        // 缺 ID / 多 ID / 重复 ID / 未知 ID 均失败且旧顺序不变
        assert!(repo.reorder(&ids[..ids.len() - 1]).await.is_err(), "缺 ID");
        let mut extra = ids.clone();
        extra.push("unknown-id".into());
        assert!(repo.reorder(&extra).await.is_err(), "未知 ID");
        let mut dup = ids.clone();
        dup.push(ids[0].clone());
        assert!(repo.reorder(&dup).await.is_err(), "重复 ID");
        let after = repo.load().await.unwrap();
        let now: Vec<String> = after.prompts.iter().map(|p| p.id.clone()).collect();
        assert_eq!(now, reversed, "失败不得改变顺序");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn select_requires_existing_id_and_persists() {
        let dir = test_dir("select");
        let repo = PromptRepository::open(dir.clone(), "");
        repo.create("甲", "正文甲").await.unwrap();
        let doc = repo.load().await.unwrap();
        let custom_id = doc.prompts[0].id.clone();
        assert!(repo.select("no-such-id").await.is_err());
        repo.select(&custom_id).await.unwrap();
        drop(repo);
        // 重新打开验证已持久化
        let repo2 = PromptRepository::open(dir.clone(), "");
        assert_eq!(repo2.load().await.unwrap().selected_prompt_id, custom_id);
        cleanup(&dir);
    }

    // ---------- 持久化与损坏恢复 ----------

    #[tokio::test]
    async fn corrupt_main_recovered_from_backup() {
        let dir = test_dir("recover_bak");
        let repo = PromptRepository::open(dir.clone(), "");
        repo.create("甲", "正文甲").await.unwrap(); // 此次写入会生成 .bak（默认项版本）
        drop(repo);
        std::fs::write(dir.join(MAIN_FILE), "{not json").unwrap();
        let repo = PromptRepository::open(dir.clone(), "");
        let doc = repo.load().await.unwrap();
        assert_eq!(doc.prompts.len(), 1, "应恢复为备份版本");
        assert!(
            repo.take_notice().await.unwrap().contains("备份"),
            "应有恢复提示"
        );
        // 主文件已被恢复内容重写
        let text = std::fs::read_to_string(dir.join(MAIN_FILE)).unwrap();
        assert!(parse_document(&text).is_ok());
        // Warning 5：恢复写盘不得把损坏的主文件复制回 .bak 覆盖完好备份
        let bak_text = std::fs::read_to_string(dir.join(BAK_FILE)).unwrap();
        assert!(parse_document(&bak_text).is_ok(), ".bak 必须保持完好");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn both_corrupt_keeps_evidence_and_rebuilds() {
        let dir = test_dir("recover_both");
        let repo = PromptRepository::open(dir.clone(), "");
        drop(repo);
        std::fs::write(dir.join(MAIN_FILE), "{bad").unwrap();
        std::fs::write(dir.join(BAK_FILE), "{also bad").unwrap();
        let repo = PromptRepository::open(dir.clone(), "");
        let doc = repo.load().await.unwrap();
        assert_eq!(doc.prompts.len(), 1);
        assert!(repo.take_notice().await.unwrap().contains("重建"));
        let corrupts: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt"))
            .collect();
        assert_eq!(corrupts.len(), 2, "主文件与备份都应保留为 corrupt 副本");
        cleanup(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_tmp() {
        let dir = test_dir("atomic");
        let doc = default_document();
        atomic_write_document(&dir, &doc).unwrap();
        assert!(dir.join(MAIN_FILE).exists());
        assert!(!dir.join("prompts.json.tmp").exists(), "tmp 不得残留");
        atomic_write_document(&dir, &doc).unwrap();
        assert!(dir.join(BAK_FILE).exists(), "覆盖写应生成 .bak");
        // 父目录不存在时应干净失败
        let bad = dir.join("no_such");
        assert!(atomic_write_document(&bad, &doc).is_err());
        cleanup(&dir);
    }

    // ---------- 并发 ----------

    #[tokio::test]
    async fn concurrent_writes_serialize_without_lost_updates() {
        let dir = test_dir("concurrent");
        let repo = std::sync::Arc::new(PromptRepository::open(dir.clone(), ""));
        let mut handles = Vec::new();
        for i in 0..5 {
            let r = repo.clone();
            handles.push(tokio::spawn(async move {
                r.create(&format!("并发{}", i), &format!("正文{}", i)).await
            }));
        }
        let mut ok = 0;
        for h in handles {
            if h.await.unwrap().is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, 5, "5 次并发创建都应成功");
        let doc = repo.load().await.unwrap();
        assert_eq!(doc.prompts.len(), 6);
        // 磁盘 JSON 仍然合法
        let text = std::fs::read_to_string(dir.join(MAIN_FILE)).unwrap();
        assert!(parse_document(&text).is_ok());

        // 并发 select/reorder/update 混合不得损坏文档
        let mut handles = Vec::new();
        for i in 0..8 {
            let r = repo.clone();
            handles.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    r.select(BUILTIN_DEFAULT_ID).await.map(|_| ())
                } else {
                    r.update(BUILTIN_DEFAULT_ID, None, &format!("正文v{}", i))
                        .await
                        .map(|_| ())
                }
            }));
        }
        for h in handles {
            h.await.unwrap().expect("并发写不得失败");
        }
        let text = std::fs::read_to_string(dir.join(MAIN_FILE)).unwrap();
        assert!(parse_document(&text).is_ok());
        cleanup(&dir);
    }

    /// Critical（F-06.3.2）：持久化失败路径——注入不可写路径（目录占用 tmp
    /// 文件名使 File::create 失败），断言内存文档未替换、无 tmp 文件残留、
    /// 主文件保持原状
    #[tokio::test]
    async fn persistence_failure_keeps_memory_doc_intact() {
        let dir = test_dir("persist_fail");
        let repo = PromptRepository::open(dir.clone(), "");
        repo.create("甲", "正文甲").await.unwrap();
        let before = repo.load().await.unwrap();
        // P0-03 后原子写 tmp 名唯一，无法靠占用固定 tmp 名拦截写盘；
        // 改为把主文件设为只读，使 MoveFileExW REPLACE_EXISTING 替换失败 → 写盘失败，
        // 同时主文件仍可读（内存文档与磁盘都保持原状）。
        set_readonly(&dir.join(MAIN_FILE), true);
        let target = before.prompts[0].id.clone();
        let err = repo
            .update(&target, None, "新正文")
            .await
            .expect_err("写盘失败应报错");
        assert!(err.contains("写入提示词文件失败"), "错误文案: {}", err);
        // 内存文档未被替换
        assert_eq!(
            repo.load().await.unwrap(),
            before,
            "持久化失败不得替换内存文档"
        );
        // 无 tmp 文件残留
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "tmp 文件不得残留"
        );
        // 主文件保持原状
        let disk = parse_document(&std::fs::read_to_string(dir.join(MAIN_FILE)).unwrap()).unwrap();
        assert_eq!(disk, before);
        // 恢复可写后写入正常（失败不遗留坏状态）
        set_readonly(&dir.join(MAIN_FILE), false);
        repo.update(&target, None, "新正文")
            .await
            .expect("恢复后应写入成功");
        assert_eq!(repo.load().await.unwrap().prompts[0].content, "新正文");
        cleanup(&dir);
    }

    /// 设置 Windows 只读属性（Windows 下 MoveFileExW 无法覆盖只读文件 → 写盘失败）。
    #[cfg(target_os = "windows")]
    fn set_readonly(path: &std::path::Path, ro: bool) {
        let mut p = std::fs::metadata(path).unwrap().permissions();
        p.set_readonly(ro);
        std::fs::set_permissions(path, p).unwrap();
    }

    /// 非 Windows 用 chmod 模拟（当前产品为 Windows-only，主要是让测试可编译）。
    #[cfg(not(target_os = "windows"))]
    fn set_readonly(_path: &std::path::Path, _ro: bool) {}

    /// Critical（F-06.3.2）：并发 select/update/reorder 多轮交错，
    /// 持锁全程后断言最终一致、无丢更新
    #[tokio::test]
    async fn concurrent_select_update_reorder_no_lost_updates() {
        let dir = test_dir("concurrent_mix");
        let repo = std::sync::Arc::new(PromptRepository::open(dir.clone(), ""));
        repo.create("甲", "正文甲").await.unwrap();
        repo.create("乙", "正文乙").await.unwrap();
        let ids: Vec<String> = repo
            .load()
            .await
            .unwrap()
            .prompts
            .iter()
            .map(|p| p.id.clone())
            .collect();
        assert_eq!(ids.len(), 3);
        for round in 0..5usize {
            let r1 = repo.clone();
            let r2 = repo.clone();
            let r3 = repo.clone();
            let ids1 = ids.clone();
            let ids3 = ids.clone();
            let (sel, upd, ord) = tokio::join!(
                async move { r1.select(&ids1[round % ids1.len()]).await },
                async move {
                    r2.update(BUILTIN_DEFAULT_ID, None, &format!("正文v{}", round))
                        .await
                },
                async move {
                    let mut rev = ids3.clone();
                    rev.reverse();
                    r3.reorder(&rev).await
                }
            );
            // 每个操作都基于锁内最新文档，全部应成功且条目数一致（无丢更新/损坏）
            for r in [&sel, &upd, &ord] {
                let d = r.as_ref().expect("持锁全程后并发写不得失败");
                assert_eq!(d.prompts.len(), 3);
            }
        }
        let final_doc = repo.load().await.unwrap();
        assert_eq!(final_doc.prompts.len(), 3);
        // 最后一轮 update 不得被后续并发操作丢失
        let default_entry = final_doc
            .prompts
            .iter()
            .find(|p| p.id == BUILTIN_DEFAULT_ID)
            .unwrap();
        assert_eq!(default_entry.content, "正文v4", "最后一轮 update 不得丢失");
        // 最后一轮 reorder 生效（逆序）
        let mut rev = ids.clone();
        rev.reverse();
        let order: Vec<String> = final_doc.prompts.iter().map(|p| p.id.clone()).collect();
        assert_eq!(order, rev, "最后一轮 reorder 不得丢失");
        // 磁盘与内存一致
        let disk = parse_document(&std::fs::read_to_string(dir.join(MAIN_FILE)).unwrap()).unwrap();
        assert_eq!(disk, final_doc);
        cleanup(&dir);
    }

    // ---------- 日志/错误消息不含正文哨兵 ----------

    #[tokio::test]
    async fn error_messages_do_not_leak_content() {
        let dir = test_dir("leak");
        let repo = PromptRepository::open(dir.clone(), "");
        let sentinel = "SENTINEL_秘密正文_XYZ";
        let err = repo
            .update("no-such", None, sentinel)
            .await
            .expect_err("应失败");
        assert!(!err.contains(sentinel), "错误消息不得携带正文");
        let err = repo
            .create("标题", &"字".repeat(CONTENT_MAX_CHARS + 1))
            .await
            .expect_err("应失败");
        assert!(!err.contains(&"字".repeat(10)));
        cleanup(&dir);
    }

    // 测试辅助：无 tokio runtime 上下文时驱动 future
    fn tokio_block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }
}
