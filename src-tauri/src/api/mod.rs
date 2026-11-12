use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use crate::cancel::CancelToken;

/// 统一重试次数（B-07：分片与 chat 两处统一为 3 次尝试）
const MAX_ATTEMPTS: u32 = 3;
/// 退避基数（秒）：2s/4s/8s 指数增长
const BACKOFF_BASE_SECS: u64 = 2;
/// 退避随机抖动上限（毫秒）
const BACKOFF_JITTER_MAX_MS: u64 = 500;
/// Retry-After 上限（秒）：防止恶意/异常服务端让客户端停等过久
const RETRY_AFTER_CAP_SECS: u64 = 60;
/// HttpStatus 错误携带的响应正文上限（B-07：500 字节，字符边界安全）
const HTTP_STATUS_BODY_LIMIT: usize = 500;

/// 全局共享 HTTP 客户端（连接池复用，减少并发场景下 TCP 握手开销）。
/// S-02：禁用自动重定向，防止初始校验后的 URL 被 302 到受限地址；
/// 服务端返回 3xx 由调用方显式拒绝。
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(10)
            .connect_timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build HTTP client")
    })
}

/// S-02：三条请求路径共用的地址校验入口。
/// 验证 base_url（scheme/host/端口/DNS/IP 受限检查）并在每次真实请求前
/// 重新解析 DNS（防 DNS rebinding），用 URL path API 拼接 chat/completions。
async fn prepare_request_url(config: &ApiConfig) -> Result<url::Url, ApiError> {
    let ep = crate::endpoint::validate_endpoint(
        &config.base_url,
        crate::endpoint::EndpointPolicy::DEFAULT,
    )
    .map_err(|e| ApiError::Config(e.to_string()))?;
    crate::endpoint::recheck_endpoint(&ep)
        .await
        .map_err(|e| ApiError::Config(e.to_string()))?;
    let mut u = ep.url().clone();
    let p = u.path().trim_end_matches('/').to_string();
    u.set_path(&format!("{}/chat/completions", p));
    u.set_query(None);
    u.set_fragment(None);
    Ok(u)
}

/// 按 UTF-8 字符边界安全截取：返回不超过 `max_bytes` 字节、且完整落在字符边界上的最长前缀。
/// 用于日志等场景，避免按字节索引切分 `String` 时在多字节字符内部 panic
/// （release 下 `panic = "abort"`，该 panic 会直接终止整个进程）。
/// H-04：日志不再记录 SSE 解析失败正文，本函数仅保留给登录 URL/错误片段截取测试使用。
#[cfg(test)]
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    &s[..end]
}

/// 将 API Key 等敏感字符串脱敏：超过 4 个字符时仅保留前 4 位并以 `***` 代替，
/// 否则（含空串）一律显示 `***`，避免短密钥被完整还原。
pub(crate) fn redact_secret(secret: &str) -> String {
    if secret.chars().count() <= 4 {
        return "***".to_string();
    }
    let visible: String = secret.chars().take(4).collect();
    format!("{}***", visible)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
}

// 手写 Debug 实现（B-03）：api_key 脱敏，防止任何 {:?} 打印泄露明文密钥
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &redact_secret(&self.api_key))
            .field("model", &self.model)
            .field("language", &self.language)
            .finish()
    }
}

impl ApiConfig {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self.api_key.is_empty() {
            return Err(ApiError::Config("API Key 不能为空".to_string()));
        }
        if self.base_url.is_empty() {
            return Err(ApiError::Config("Base URL 不能为空".to_string()));
        }
        if self.model.is_empty() {
            return Err(ApiError::Config("模型名称不能为空".to_string()));
        }
        Ok(())
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.xiaomimimo.com/v1".to_string(),
            api_key: String::new(),
            model: "mimo-v2.5-asr".to_string(),
            language: Some("auto".to_string()),
        }
    }
}

// 变体名不带统一的 "Error" 后缀（修复 clippy::enum_variant_names），
// 类型名 ApiError 已表达错误语义。
// B-07：thiserror 统一错误体系，保留原有 Display 文案（前端错误信息不变），
// source() 链路完整；新增结构化 HttpStatus 变体（状态码 + 截断到 500 字节的响应体）
// 与 Cancelled（B-08 取消专用）。
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("配置错误: {0}")]
    Config(String),
    #[error("HTTP 错误: {0}")]
    Http(String),
    #[error("HTTP 错误: {status}: {body}")]
    HttpStatus {
        status: u16,
        body: String,
        retry_after: Option<u64>,
    },
    // 注：原 Parse 变体随 Critical 修复删除——SSE 解析坏行仅记日志不报错，
    // 生产代码已无任何构造点（死分支清理）
    #[error("请求超时")]
    Timeout,
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("任务已取消")]
    Cancelled,
    /// 流式传输中途失败且已有部分内容发射到前端。
    /// 重试会让服务端从头重发，前端 += 追加将造成文本重复，
    /// 因此该错误被改写为不可重试（快速失败）。
    #[error("传输中断，已接收部分内容")]
    PartialStream,
    /// T-05：流在收到 `[DONE]` 之前干净结束（EOF）。连接被截断但
    /// 未触发网络错误；应视为不完整响应。已输出部分文本时由
    /// CountingEmitter 改写为 PartialStream，禁止从头重试。
    #[error("流式响应在收到结束标记前中断")]
    UnexpectedEof,
}

impl ApiError {
    /// B-07 错误分级：是否可重试。
    /// 可重试：网络错误、超时、408、429、5xx；
    /// 不可重试（快速失败）：400/401/403/404 等其余状态码、配置错误、PartialStream；
    /// 取消一律终止，不重试。
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::Http(_) | ApiError::Timeout => true,
            ApiError::HttpStatus { status, .. } => {
                *status == 408 || *status == 429 || (500..600).contains(status)
            }
            ApiError::Config(_)
            | ApiError::Io(_)
            | ApiError::Cancelled
            | ApiError::PartialStream
            | ApiError::UnexpectedEof => false,
        }
    }

    /// 429 限流响应携带的 Retry-After 秒数
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            ApiError::HttpStatus { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, ApiError::Cancelled)
    }
}

/// reqwest 发送失败归一：超时 → Timeout，其余网络错误 → Http（均可重试）
fn send_error(e: reqwest::Error) -> ApiError {
    if e.is_timeout() {
        ApiError::Timeout
    } else {
        ApiError::Http(e.to_string())
    }
}

/// 非成功响应统一构造结构化 HttpStatus 错误：
/// 429 时读取 Retry-After（秒数）供退避使用；正文按 500 字节字符边界安全截断。
async fn status_error(response: reqwest::Response) -> ApiError {
    let status = response.status();
    let retry_after = if status.as_u16() == 429 {
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|s| s.min(RETRY_AFTER_CAP_SECS))
    } else {
        None
    };
    let body =
        match crate::endpoint::read_body_limited(response, HTTP_STATUS_BODY_LIMIT as u64).await {
            Ok(text) => crate::endpoint::safe_error_body_limited(&text, HTTP_STATUS_BODY_LIMIT),
            Err(_) => "(响应正文超限已丢弃)".to_string(),
        };
    ApiError::HttpStatus {
        status: status.as_u16(),
        body,
        retry_after,
    }
}

/// 无 rand crate 的轻量随机抖动（0~BACKOFF_JITTER_MAX_MS）：
/// 复用工程内既有的 RandomState 哈希随机源模式（audio/mod.rs 同款）
fn jitter_ms() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_usize(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as usize,
    );
    hasher.finish() % (BACKOFF_JITTER_MAX_MS + 1)
}

/// B-07 退避策略：attempt 为已完成尝试次数（1-based）。
/// 429 优先采用 Retry-After；否则 2s/4s/8s 指数退避 + 0~500ms 抖动。
fn retry_sleep_duration(err: &ApiError, attempt: u32) -> std::time::Duration {
    if let Some(secs) = err.retry_after_secs() {
        if secs > 0 {
            return std::time::Duration::from_secs(secs);
        }
    }
    let shift = (attempt.saturating_sub(1)).min(6);
    let base = BACKOFF_BASE_SECS.saturating_mul(1u64 << shift);
    std::time::Duration::from_millis(base * 1000 + jitter_ms())
}

/// 事件发射器 trait
/// 事件发射器抽象。
/// B-11：要求 Send + Sync，保证聚合请求结构（ChatRequest）可跨 await 传递。
pub trait Emitter: Send + Sync {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String>;
}

/// 包装发射器：记录是否已发射过任何事件（B-02 fallback 防重复发射用）
struct CountingEmitter<'a, E: Emitter + ?Sized> {
    inner: &'a E,
    emitted_any: &'a AtomicBool,
}

impl<E: Emitter + ?Sized> Emitter for CountingEmitter<'_, E> {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String> {
        self.emitted_any
            .store(true, std::sync::atomic::Ordering::Release);
        self.inner.emit(event, payload)
    }
}

// ============================================================
// ASR 语音识别（严格按照 API 接口规范文档）
// 模型：mimo-v2.5-asr
// 输入：仅音频（data URL 格式）
// 不支持 system prompt、不支持文本输入
// 注：非流式 transcribe 路径随 Critical 修复删除——其唯一调用方是
// 流式 Parse 降级死分支（parse_sse_stream_inner 从不返回 Parse）
// ============================================================

/// 流式 ASR 转写（单次尝试，重试逻辑由调用方控制）
pub async fn stream_transcription(
    audio_data_url: &str,
    config: &ApiConfig,
    emitter: &impl Emitter,
    cancel: &CancelToken,
) -> Result<(), ApiError> {
    config.validate()?;

    let url = prepare_request_url(config).await?;

    let body = serde_json::json!({
        "model": config.model,
        "messages": [{
            "role": "user",
            "content": [{
                "type": "input_audio",
                "input_audio": {
                    "data": audio_data_url
                }
            }]
        }],
        "asr_options": {
            "language": config.language.as_deref().unwrap_or("auto")
        },
        "stream": true
    });

    let client = http_client();
    let response = client
        .post(url.as_str())
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&body)
        .send()
        .await
        .map_err(send_error)?;

    if response.status().is_redirection() {
        return Err(ApiError::Http(format!(
            "{}: 服务端返回重定向，已被安全策略拒绝",
            response.status()
        )));
    }

    if !response.status().is_success() {
        return Err(status_error(response).await);
    }

    // CountingEmitter 记录是否已发射过任何 chunk：中途失败时若已有部分输出，
    // 错误被改写为不可重试的 PartialStream，避免重试从头重发导致前端文本重复拼接
    let emitted_any = AtomicBool::new(false);
    let counting = CountingEmitter {
        inner: emitter,
        emitted_any: &emitted_any,
    };
    // 注：parse_sse_stream_inner 从不返回 Parse 错误（坏行仅记日志），
    // 原 Parse 降级非流式分支为死分支，已随 Critical 修复一并清理
    match parse_sse_stream(
        response,
        &counting,
        "transcription-chunk",
        "transcription-complete",
        cancel,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(e) => {
            if emitted_any.load(std::sync::atomic::Ordering::Acquire) && !e.is_cancelled() {
                crate::warn!("SSE 流中途失败且已有部分输出，改写为不可重试: {}", e);
                return Err(ApiError::PartialStream);
            }
            Err(e)
        }
    }
}

// ============================================================
// 流式聊天文本
// 模型：mimo-v2.6-flash 等（非 ASR 模型）
// 支持 system prompt + 文本 + 可选音频
// ============================================================

/// 流式聊天文本（支持 system prompt + 用户文本 + 可选音频）
// B-11：emitter 接受 trait object，支持 ChatRequest 上下文结构复用
pub async fn stream_chat(
    system_prompt: &str,
    user_text: &str,
    audio_data_url: Option<&str>,
    config: &ApiConfig,
    response_format: Option<&str>,
    emitter: &dyn Emitter,
    cancel: &CancelToken,
) -> Result<(), ApiError> {
    config.validate()?;

    let url = prepare_request_url(config).await?;

    let mut messages = Vec::new();

    // system prompt
    if !system_prompt.is_empty() {
        messages.push(serde_json::json!({
            "role": "system",
            "content": system_prompt
        }));
    }

    // user content parts
    let mut content_parts = Vec::new();

    if !user_text.is_empty() {
        content_parts.push(serde_json::json!({
            "type": "text",
            "text": user_text
        }));
    }

    if let Some(audio) = audio_data_url {
        if !audio.is_empty() {
            content_parts.push(serde_json::json!({
                "type": "input_audio",
                "input_audio": {
                    "data": audio
                }
            }));
        }
    }

    if content_parts.is_empty() {
        return Err(ApiError::Config("消息内容不能为空".to_string()));
    }

    messages.push(serde_json::json!({
        "role": "user",
        "content": content_parts
    }));

    let mut body = serde_json::json!({
        "model": config.model,
        "messages": messages,
        "stream": true
    });

    if let Some(format) = response_format {
        body["response_format"] = serde_json::json!({ "type": format });
    }

    let client = http_client();
    let response = client
        .post(url.as_str())
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.api_key))
        .json(&body)
        .send()
        .await
        .map_err(send_error)?;

    if response.status().is_redirection() {
        return Err(ApiError::Http(format!(
            "{}: 服务端返回重定向，已被安全策略拒绝",
            response.status()
        )));
    }

    if !response.status().is_success() {
        return Err(status_error(response).await);
    }

    // 与 stream_transcription 同款计数：中途失败且已有输出 → 不可重试的 PartialStream
    let emitted_any = AtomicBool::new(false);
    let counting = CountingEmitter {
        inner: emitter,
        emitted_any: &emitted_any,
    };
    match parse_sse_stream(response, &counting, "chat-chunk", "chat-complete", cancel).await {
        Ok(()) => Ok(()),
        Err(e) => {
            if emitted_any.load(std::sync::atomic::Ordering::Acquire) && !e.is_cancelled() {
                crate::warn!("chat 流中途失败且已有部分输出，改写为不可重试: {}", e);
                return Err(ApiError::PartialStream);
            }
            Err(e)
        }
    }
}

// ============================================================
// 重试包装器（供命令层调用，统一重试策略）
// ============================================================

/// 退避等待（B-07/B-08）：等待期间纳入 select，取消即时打断并返回 Cancelled。
async fn backoff_sleep(err: &ApiError, attempt: u32, cancel: &CancelToken) -> Result<(), ApiError> {
    let duration = retry_sleep_duration(err, attempt);
    tokio::select! {
        _ = tokio::time::sleep(duration) => Ok(()),
        _ = cancel.cancelled() => Err(ApiError::Cancelled),
    }
}

/// 每次真实请求前的限流准备（B-07 记账修正 + B-08 可取消）
async fn prepare_request_slot(
    limiter: Option<&crate::rate_limiter::RateLimiter>,
    cancel: &CancelToken,
) -> Result<(), ApiError> {
    if let Some(limiter) = limiter {
        limiter
            .wait_rpm_slot(cancel)
            .await
            .map_err(|_| ApiError::Cancelled)?;
    }
    Ok(())
}

/// 重试内核（B-07）：统一的重试分级/退避/取消/记账循环，
/// 分片与 chat 两条路径共用；`attempt_fn` 为一次完整尝试。
/// 语义：重试意味着整个请求从头重发；因此流式路径在已发射过任何
/// 内容 chunk 后必须把错误改写为不可重试的 `ApiError::PartialStream`，
/// 否则服务端重发 + 前端追加会造成文本重复。
/// 单测可直接注入返回指定错误的闭包验证重试策略。
async fn retry_stream<F, Fut>(
    mut attempt_fn: F,
    cancel: &CancelToken,
    limiter: Option<&crate::rate_limiter::RateLimiter>,
    context: &str,
) -> Result<(), ApiError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), ApiError>>,
{
    let mut last_error = None;
    for attempt in 1..=MAX_ATTEMPTS {
        if cancel.is_cancelled() {
            return Err(ApiError::Cancelled);
        }
        prepare_request_slot(limiter, cancel).await?;
        match attempt_fn().await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if e.is_cancelled() {
                    return Err(e);
                }
                crate::warn!("{} 尝试 {} 失败: {}", context, attempt, e);
                let retryable = e.is_retryable() && attempt < MAX_ATTEMPTS;
                last_error = Some(e);
                if !retryable {
                    break;
                }
                backoff_sleep(last_error.as_ref().unwrap(), attempt, cancel).await?;
            }
        }
    }
    Err(last_error.unwrap())
}

/// 流式 ASR 转写（含重试：B-07 统一为最多 3 次尝试，按错误分级重试，
/// 2s/4s 指数退避 + 0~500ms 抖动，429 优先用 Retry-After；
/// 每次真实请求前重新做 RPM 记账）
pub async fn transcribe_with_retry(
    audio_data_url: &str,
    config: &ApiConfig,
    emitter: &impl Emitter,
    cancel: &CancelToken,
    limiter: Option<&crate::rate_limiter::RateLimiter>,
) -> Result<(), ApiError> {
    retry_stream(
        || stream_transcription(audio_data_url, config, emitter, cancel),
        cancel,
        limiter,
        "分片",
    )
    .await
}

/// 聊天请求上下文（B-11：聚合 chat_with_retry 参数，删除临时 Clippy allow）
pub struct ChatRequest<'a> {
    pub system_prompt: &'a str,
    pub user_text: &'a str,
    pub audio_data_url: Option<&'a str>,
    pub config: &'a ApiConfig,
    pub response_format: Option<&'a str>,
    pub emitter: &'a dyn Emitter,
    pub cancel: &'a CancelToken,
    pub limiter: Option<&'a crate::rate_limiter::RateLimiter>,
}

/// 流式聊天（含重试：与分片同策略，最多 3 次尝试）
pub async fn chat_with_retry(req: ChatRequest<'_>) -> Result<(), ApiError> {
    retry_stream(
        || {
            stream_chat(
                req.system_prompt,
                req.user_text,
                req.audio_data_url,
                req.config,
                req.response_format,
                req.emitter,
                req.cancel,
            )
        },
        req.cancel,
        req.limiter,
        "chat",
    )
    .await
}

// ============================================================
// 标签过滤器（流式，处理跨 chunk 的不完整标签）
// ============================================================

/// 过滤模式
#[derive(PartialEq)]
enum FilterMode {
    /// 正常输出
    Normal,
    /// 跳过内容（在 <think> 块内）
    SkipContent,
}

/// 流式标签过滤器，处理 `<think>...</think>` 完全跳过，
/// 其他标签如 `<chinese>` 只去除标签本身，保留内部文本。
/// 同时处理 API 返回的无 `<` 前缀的 `think>` 文本块。
struct TagFilter {
    mode: FilterMode,
    /// 标签缓冲区（处理跨 chunk 的不完整标签）
    tag_buf: String,
    /// 是否正在收集标签
    in_tag: bool,
    /// 是否正在收集闭合标签
    in_close_tag: bool,
    /// 等待的闭合标签名（如 "think"）
    skip_tag: String,
    /// SkipContent 模式下已跳过的字符数（防止未闭合标签吞掉全部后续文本）
    skip_chars: usize,
    /// 逐字符匹配 `think>` 的状态机位置
    think_match_pos: usize,
    /// 正在收集的潜在 think> 文本（匹配失败时回吐到输出）
    think_pending: String,
    /// 是否在无标签 think 块中跳过
    in_think_skip: bool,
}

const MAX_SKIP_CHARS: usize = 5000;
const THINK_PATTERN: &[u8] = b"think>";

impl TagFilter {
    fn new() -> Self {
        Self {
            mode: FilterMode::Normal,
            tag_buf: String::new(),
            in_tag: false,
            in_close_tag: false,
            skip_tag: String::new(),
            skip_chars: 0,
            think_match_pos: 0,
            think_pending: String::new(),
            in_think_skip: false,
        }
    }

    /// 处理一段文本，返回过滤后的文本
    fn process(&mut self, input: &str) -> String {
        let mut output = String::with_capacity(input.len());

        for ch in input.chars() {
            // 先处理无标签 think> 匹配（仅在 Normal 模式且不在 HTML 标签收集中）
            if self.mode == FilterMode::Normal && !self.in_tag {
                if self.in_think_skip {
                    // 在 think> 跳过模式中，检查是否遇到下一个 think>（结束标记）
                    self.think_pending.push(ch);
                    // B-01：按字符数取尾部 6 个字符比较，禁止裸字节减法
                    // （think_pending 含模型任意输出，字节偏移不保证落在字符边界上）
                    if self.think_pending.chars().count() >= 6 {
                        let tail: String = self
                            .think_pending
                            .chars()
                            .rev()
                            .take(6)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect();
                        if tail == "think>" {
                            // 遇到下一个 think>，结束跳过
                            self.in_think_skip = false;
                            self.think_pending.clear();
                            self.think_match_pos = 0;
                            continue;
                        }
                    }
                    // 跳过模式中，丢弃字符
                    self.skip_chars += 1;
                    if self.skip_chars > MAX_SKIP_CHARS {
                        self.in_think_skip = false;
                        self.think_pending.clear();
                        self.think_match_pos = 0;
                        self.skip_chars = 0;
                        output.push(ch);
                    }
                    continue;
                }

                // 逐字符匹配 "think>"
                let expected = THINK_PATTERN[self.think_match_pos] as char;
                if ch == expected {
                    self.think_match_pos += 1;
                    self.think_pending.push(ch);
                    if self.think_match_pos == THINK_PATTERN.len() {
                        // 匹配成功，进入跳过模式
                        self.in_think_skip = true;
                        self.think_pending.clear();
                        self.think_match_pos = 0;
                        self.skip_chars = 0;
                        continue;
                    }
                    // 还在匹配中，继续下一个字符
                    continue;
                } else if self.think_match_pos > 0 {
                    // 匹配失败，回吐已收集的字符到输出
                    self.think_match_pos = 0;
                    output.push_str(&self.think_pending);
                    self.think_pending.clear();
                    // 当前字符继续走正常的 tag 处理逻辑
                }
            }

            match self.mode {
                FilterMode::Normal => {
                    if ch == '<' {
                        // 开始收集标签
                        self.tag_buf.clear();
                        self.tag_buf.push(ch);
                        self.in_tag = true;
                        self.in_close_tag = false;
                    } else if self.in_tag {
                        self.tag_buf.push(ch);
                        if ch == '>' {
                            // 标签收集完成
                            self.in_tag = false;
                            let tag = self.tag_buf.clone();
                            if let Some(name) = Self::parse_tag(&tag, false) {
                                if name == "think" {
                                    // 进入跳过模式
                                    self.mode = FilterMode::SkipContent;
                                    self.skip_tag = name;
                                    self.tag_buf.clear();
                                    self.skip_chars = 0;
                                } else {
                                    // 内容标签：丢弃标签本身，保留后续内容
                                    self.tag_buf.clear();
                                }
                            } else if let Some(name) = Self::parse_tag(&tag, true) {
                                // 闭合标签（正常模式下遇到闭合标签，忽略）
                                self.tag_buf.clear();
                                let _ = name;
                            } else {
                                // 不是有效标签，输出缓冲区内容
                                output.push_str(&self.tag_buf);
                                self.tag_buf.clear();
                            }
                        }
                        // 标签还在收集中，不输出
                    } else {
                        output.push(ch);
                    }
                }
                FilterMode::SkipContent => {
                    if ch == '<' {
                        self.tag_buf.clear();
                        self.tag_buf.push(ch);
                        self.in_tag = true;
                        self.in_close_tag = false;
                    } else if self.in_tag {
                        self.tag_buf.push(ch);
                        if ch == '/' && self.tag_buf == "</" {
                            self.in_close_tag = true;
                        } else if ch == '>' && self.in_close_tag {
                            // 闭合标签收集完成
                            self.in_tag = false;
                            if let Some(name) = Self::parse_tag(&self.tag_buf, true) {
                                if name == self.skip_tag {
                                    // 匹配的闭合标签，退出跳过模式
                                    self.mode = FilterMode::Normal;
                                    self.skip_tag.clear();
                                    self.skip_chars = 0;
                                }
                            }
                            self.tag_buf.clear();
                        }
                    } else {
                        // SkipContent 模式下，普通字符直接丢弃，但计数
                        self.skip_chars += 1;
                        if self.skip_chars > MAX_SKIP_CHARS {
                            // 未闭合标签超限，强制退出跳过模式
                            self.mode = FilterMode::Normal;
                            self.skip_tag.clear();
                            self.skip_chars = 0;
                            output.push(ch);
                        }
                    }
                }
            }
        }

        // 如果标签还在收集中（跨 chunk），不输出 tag_buf，等待下一个 chunk
        // 但如果不在标签收集中，也不应该有残留
        if !self.in_tag && !self.tag_buf.is_empty() {
            // 不应该到这里，安全兜底
            self.tag_buf.clear();
        }

        output
    }

    /// 解析标签名，返回 (tag_name) 或 None
    /// close=true 时解析闭合标签 </xxx>
    fn parse_tag(tag: &str, close: bool) -> Option<String> {
        let content = if close {
            // </xxx>
            tag.strip_prefix("</")?.strip_suffix('>')?
        } else {
            // <xxx> 或 <xxx ...>
            let inner = tag.strip_prefix('<')?.strip_suffix('>')?;
            // 去掉属性（取第一个空格前的部分）
            inner.split_whitespace().next()?
        };
        let name = content.trim().to_lowercase();
        if name.is_empty() {
            return None;
        }
        Some(name)
    }

    /// 收尾（T-06）：流结束/[DONE] 前调用，把尚未构成完整过滤标记的
    /// 普通文本回吐（如尾部 `t/th/thi/thin/think`、未闭合普通标签）；
    /// 仍处于 think 跳过态的内容则丢弃。
    fn finish(&mut self) -> String {
        let mut out = String::new();
        // Normal 模式下，未完成 think> 匹配的待定字符是普通正文，需回吐
        if self.mode == FilterMode::Normal {
            if !self.think_pending.is_empty() && !self.in_think_skip {
                out.push_str(&self.think_pending);
            }
            // 未闭合的普通标签（非 think）回吐为普通文本
            if self.in_tag && !self.tag_buf.is_empty() {
                let is_think_open = Self::parse_tag(&self.tag_buf, false)
                    .map(|n| n == "think")
                    .unwrap_or(false);
                if !is_think_open {
                    out.push_str(&self.tag_buf);
                }
            }
        }
        // 重置状态，避免重复回吐
        self.think_pending.clear();
        self.tag_buf.clear();
        self.in_tag = false;
        out
    }
}

// ============================================================
// SSE 解析（共用）
// ============================================================

/// 从字节缓冲中提取所有以 `\n` 结尾的完整行（行尾 `\r` 一并去除），
/// 并将 `consumed` 推进到已消费字节数。
///
/// B-02：缓冲为字节级 `Vec<u8>`，只对完整行做 UTF-8 转换。
/// UTF-8  continuation 字节范围为 0x80–0xBF，不可能与 `\n`（0x0A）混淆，
/// 因此按字节查找行边界是安全的；跨 chunk 的半字符/半行保留在缓冲尾部。
fn extract_complete_lines(buf: &[u8], consumed: &mut usize) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(pos) = buf[*consumed..].iter().position(|&b| b == b'\n') {
        let mut line_bytes = &buf[*consumed..*consumed + pos];
        if line_bytes.last() == Some(&b'\r') {
            line_bytes = &line_bytes[..line_bytes.len() - 1];
        }
        lines.push(String::from_utf8_lossy(line_bytes).into_owned());
        *consumed += pos + 1;
    }
    lines
}

/// SSE 单行/未换行缓冲上限（S-02）：防恶意服务无限发送无换行数据
const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;

/// SSE 空闲超时（秒）：无新数据视为连接中断（清单 B-11：值集中至 constants）
pub const SSE_IDLE_TIMEOUT_SECS: u64 = crate::constants::SSE_IDLE_TIMEOUT_SECS;

async fn parse_sse_stream(
    response: reqwest::Response,
    emitter: &impl Emitter,
    chunk_event: &str,
    _complete_event: &str,
    cancel: &CancelToken,
) -> Result<(), ApiError> {
    let stream = response.bytes_stream();
    parse_sse_stream_inner(
        stream,
        emitter,
        chunk_event,
        cancel,
        std::time::Duration::from_secs(SSE_IDLE_TIMEOUT_SECS),
    )
    .await
}

/// SSE 字节流解析核心逻辑。泛型化 Stream 以便单元测试用
/// `futures::stream::iter` 构造任意 chunk 切分方式的 mock 流（B-02/B-12 验收需要）。
/// chunk 元素只需可视为字节切片（reqwest 的 `Bytes`、测试用 `Vec<u8>` 均满足），
/// 不直接引用 `bytes` crate，避免引入新依赖。
///
/// B-08：`tokio::select!` 同时等待下一个 chunk、空闲超时与取消信号，
/// 取消分支立即返回 `ApiError::Cancelled`，不再最坏等满一个空闲超时周期。
async fn parse_sse_stream_inner<S, T, E>(
    mut stream: S,
    emitter: &impl Emitter,
    chunk_event: &str,
    cancel: &CancelToken,
    idle_timeout: std::time::Duration,
) -> Result<(), ApiError>
where
    S: futures::Stream<Item = Result<T, E>> + Unpin,
    T: AsRef<[u8]>,
    E: std::fmt::Display,
{
    use futures::StreamExt;

    // B-02：字节级行缓冲。跨 chunk 拼接原始字节后再按完整行做 UTF-8 转换，
    // 避免多字节汉字被拆到两个 chunk 时被替换成 U+FFFD
    let mut buf: Vec<u8> = Vec::new();
    let mut consumed = 0usize;
    let mut tag_filter = TagFilter::new();
    // T-05：只有收到 `data: [DONE]` 才视为协议完整成功
    let mut saw_done = false;
    let mut emitted_chunks = 0u64;

    // T-05 修复：收到 `[DONE]` 后必须同时跳出内层行循环与外层流循环，
    // 否则 `[DONE]` 之后的行仍会被处理（如 "不应出现" 被误发射）。
    'sse: loop {
        if cancel.is_cancelled() {
            return Err(ApiError::Cancelled);
        }
        let chunk_result = tokio::select! {
            _ = cancel.cancelled() => return Err(ApiError::Cancelled),
            r = tokio::time::timeout(idle_timeout, stream.next()) => r,
        };
        let chunk_result = match chunk_result {
            Ok(opt) => match opt {
                Some(result) => result,
                None => break, // 流结束，交由循环后统一判断是否完整（T-05）
            },
            Err(_) => {
                crate::error!("SSE 流式响应中断: {}s 无新数据", idle_timeout.as_secs());
                return Err(ApiError::Http(format!(
                    "流式响应中断: {}s 无新数据",
                    idle_timeout.as_secs()
                )));
            }
        };

        let chunk = chunk_result.map_err(|e| ApiError::Http(e.to_string()))?;
        buf.extend_from_slice(chunk.as_ref());

        // 处理缓冲中全部完整行
        for line in extract_complete_lines(&buf, &mut consumed) {
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            // B-12：SSE 规范允许 "data:" 后无空格，同时接受 "data: " 与 "data:"
            if let Some(rest) = line.strip_prefix("data:") {
                let data = rest.strip_prefix(' ').unwrap_or(rest);
                if data == "[DONE]" {
                    // T-06：收尾回吐未闭合普通标签/未完成 think> 待定文本
                    let tail = tag_filter.finish();
                    if !tail.is_empty() {
                        emitter.emit(chunk_event, &tail).map_err(ApiError::Http)?;
                        emitted_chunks += 1;
                    }
                    saw_done = true;
                    break 'sse;
                }

                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                    // 发射文本片段（经过标签过滤）
                    if let Some(text) = extract_stream_text(&value) {
                        let filtered = tag_filter.process(text);
                        if !filtered.is_empty() {
                            emitter
                                .emit(chunk_event, &filtered)
                                .map_err(ApiError::Http)?;
                            emitted_chunks += 1;
                        }
                    }
                    // 发射 token 统计（通常在最后一个 chunk）
                    if let Some(tokens) = value
                        .get("usage")
                        .and_then(|usage| usage.get("total_tokens"))
                        .and_then(|tokens| tokens.as_u64())
                    {
                        emitter
                            .emit("token-update", &tokens.to_string())
                            .map_err(ApiError::Http)?;
                    }
                } else if !data.trim().is_empty() {
                    // H-04：解析失败的行可能是模型正文，只记录长度与错误类别，不记录内容
                    crate::warn!("SSE 数据解析失败（已丢弃）: 长度={} 字节", data.len());
                }
            }
        }

        // B-02：回收已消费缓冲，避免缓冲只增不减
        if consumed > 0 {
            buf.drain(..consumed);
            consumed = 0;
        }

        // S-02：提取完整行并回收后，剩余缓冲即"未完成行"（无换行）。
        // 仅当未完成行超过单行上限才终止，防止恶意服务无限发送无换行数据；
        // 不因一次喂入多行总长超限而误判（T-05 修复）。
        if buf.len() > MAX_SSE_LINE_BYTES {
            crate::error!(
                "SSE 单行数据超过 {} 字节上限，终止流式解析",
                MAX_SSE_LINE_BYTES
            );
            return Err(ApiError::Http(format!(
                "流式响应单行超过 {} MiB 上限",
                MAX_SSE_LINE_BYTES / 1024 / 1024
            )));
        }
    }

    // T-05：流结束（EOF）但未收到 `[DONE]` → 视为不完整响应。
    // 已输出部分文本时由 CountingEmitter 改写为 PartialStream 禁止重试；
    // 未输出任何文本时本错误不可重试（UnexpectedEof 归入不可重试）。
    if saw_done {
        if emitted_chunks == 0 {
            crate::warn!("SSE 流正常结束，但未提取到任何文本块");
        } else {
            crate::info!("SSE 流正常结束，共发射 {} 个文本块", emitted_chunks);
        }
        Ok(())
    } else {
        crate::warn!("SSE 流在收到 [DONE] 前结束，视为不完整响应");
        Err(ApiError::UnexpectedEof)
    }
}

fn extract_stream_text(value: &serde_json::Value) -> Option<&str> {
    let choice = value.get("choices").and_then(|v| v.get(0));
    let candidates = [
        choice
            .and_then(|v| v.get("delta"))
            .and_then(|v| v.get("content")),
        choice.and_then(|v| v.get("text")),
        choice
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("content")),
        value.get("text"),
        value.get("transcript"),
    ];
    candidates.into_iter().flatten().find_map(content_text)
}

fn content_text(value: &serde_json::Value) -> Option<&str> {
    if let Some(text) = value.as_str() {
        return Some(text);
    }
    value.as_array().and_then(|parts| {
        parts.iter().find_map(|part| {
            part.get("text")
                .and_then(|text| text.as_str())
                .or_else(|| part.get("content").and_then(|text| text.as_str()))
        })
    })
}

// ============================================================
// 单元测试（B-01/B-02/B-03/B-12 相关）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn extract_stream_text_supports_asr_shapes() {
        let delta = serde_json::json!({ "choices": [{ "delta": { "content": "甲" } }] });
        let choice_text = serde_json::json!({ "choices": [{ "text": "乙" }] });
        let message = serde_json::json!({ "choices": [{ "message": { "content": "丙" } }] });
        let top_level = serde_json::json!({ "transcript": "丁" });
        let parts = serde_json::json!({ "choices": [{ "delta": { "content": [{ "type": "text", "text": "戊" }] } }] });

        assert_eq!(extract_stream_text(&delta), Some("甲"));
        assert_eq!(extract_stream_text(&choice_text), Some("乙"));
        assert_eq!(extract_stream_text(&message), Some("丙"));
        assert_eq!(extract_stream_text(&top_level), Some("丁"));
        assert_eq!(extract_stream_text(&parts), Some("戊"));
    }

    /// 记录所有 emit 调用的测试用发射器
    struct RecordingEmitter {
        events: Mutex<Vec<(String, String)>>,
    }

    impl RecordingEmitter {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        /// 拼接指定事件的全部 payload
        fn joined(&self, event: &str) -> String {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(e, _)| e == event)
                .map(|(_, p)| p.as_str())
                .collect::<Vec<_>>()
                .join("")
        }
    }

    impl Emitter for RecordingEmitter {
        fn emit(&self, event: &str, payload: &str) -> Result<(), String> {
            self.events
                .lock()
                .unwrap()
                .push((event.to_string(), payload.to_string()));
            Ok(())
        }
    }

    /// 构造一条携带 delta 文本的 SSE data 行
    fn sse_data_line(text: &str) -> String {
        let json = serde_json::json!({
            "choices": [{ "delta": { "content": text } }]
        });
        format!("data: {}\n\n", json)
    }

    /// 用 mock 字节流跑一遍 SSE 解析，返回发射器。
    /// 为保持既有"解析出文本"类断言的语义，自动追加 `data: [DONE]` 表示完整结束。
    async fn run_sse(chunks: Vec<Vec<u8>>) -> RecordingEmitter {
        let mut with_done = chunks;
        with_done.push(b"data: [DONE]\n\n".to_vec());
        let stream = futures::stream::iter(with_done.into_iter().map(Ok::<_, std::io::Error>));
        let emitter = RecordingEmitter::new();
        let cancel = CancelToken::new();
        parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &cancel,
            std::time::Duration::from_secs(90),
        )
        .await
        .expect("SSE 解析不应失败");
        emitter
    }

    // ---------- B-01：truncate_to_char_boundary ----------

    #[test]
    fn truncate_ascii_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_to_char_boundary(s, 5), "hello");
        assert_eq!(truncate_to_char_boundary(s, 100), s);
        assert_eq!(truncate_to_char_boundary(s, 0), "");
    }

    #[test]
    fn truncate_never_panics_inside_multibyte_char() {
        // 199 个 ASCII + "中"(3 字节)：第 200 字节恰落在 "中" 内部（原 panic 场景）
        let mut data = String::from("a").repeat(199);
        data.push_str("中文转写内容");
        let got = truncate_to_char_boundary(&data, 200);
        assert_eq!(got.len(), 199); // 回退到字符边界，不含半个 "中"
        assert!(got.ends_with('a'));
    }

    #[test]
    fn truncate_pure_multibyte_boundaries() {
        let s = "中文测试"; // 12 字节
        assert_eq!(truncate_to_char_boundary(s, 0), "");
        assert_eq!(truncate_to_char_boundary(s, 1), "");
        assert_eq!(truncate_to_char_boundary(s, 3), "中");
        assert_eq!(truncate_to_char_boundary(s, 4), "中");
        assert_eq!(truncate_to_char_boundary(s, 12), s);
        assert_eq!(truncate_to_char_boundary(s, 13), s);
        assert_eq!(truncate_to_char_boundary("", 10), "");
    }

    // ---------- B-01：TagFilter 中文 think_pending 不 panic ----------

    #[test]
    fn tag_filter_think_block_with_chinese_no_panic() {
        let mut f = TagFilter::new();
        // think_pending 中累积大量中文（字节偏移永远不在字符边界上的场景）
        let out = f.process("think>我正在思考中文内容，这里应该全部被跳过。think>正文保留");
        assert_eq!(out, "正文保留");
    }

    #[test]
    fn tag_filter_think_tag_normal_case() {
        let mut f = TagFilter::new();
        let out = f.process("<think>推理过程</think>结果文本");
        assert_eq!(out, "结果文本");
    }

    #[test]
    fn tag_filter_think_skip_max_limit_fallback() {
        let mut f = TagFilter::new();
        // 未闭合的 think> 块超过 MAX_SKIP_CHARS 后应回退输出，而非吞掉全部后续文本
        let content = "中".repeat(MAX_SKIP_CHARS + 1);
        let out = f.process(&format!("think>{}后续正文", content));
        assert!(out.ends_with("后续正文"));
        assert!(!out.is_empty());
    }

    // ---------- B-12：TagFilter 状态机补测 ----------

    #[test]
    fn tag_filter_normal_mode_passthrough_with_chinese() {
        let mut f = TagFilter::new();
        // Normal 模式：普通文本（含中文与标点）原样输出
        assert_eq!(
            f.process("你好，world 123！标点、：测试。"),
            "你好，world 123！标点、：测试。"
        );
        // 行尾孤立 '<'：进入标签缓冲等待下一 chunk，不误输出
        let mut f2 = TagFilter::new();
        assert_eq!(f2.process("价格 <"), "价格 ");
    }

    #[test]
    fn tag_filter_content_tag_stripped_content_kept() {
        let mut f = TagFilter::new();
        // 非 think 标签：只去标签本身，保留内部文本
        assert_eq!(f.process("<chinese>你好世界</chinese>"), "你好世界");
        assert_eq!(
            f.process("前缀<tag attr=\"x\">内容</tag>后缀"),
            "前缀内容后缀"
        );
    }

    #[test]
    fn tag_filter_think_prefix_match_fail_spits_back() {
        let mut f = TagFilter::new();
        // "thi" 是 "think>" 前缀但随后匹配失败，已收集字符必须回吐
        assert_eq!(f.process("thi正文"), "thi正文");
        assert_eq!(f.process("thinkX保留"), "thinkX保留");
    }

    #[test]
    fn tag_filter_tag_split_across_chunks() {
        let mut f = TagFilter::new();
        // 跨 chunk 的不完整标签：缓冲保留，不误输出
        assert_eq!(f.process("<thin"), "");
        assert_eq!(f.process("k>推理</think>保留"), "保留");

        let mut f2 = TagFilter::new();
        assert_eq!(f2.process("前文<th"), "前文");
        assert_eq!(f2.process("ink>思考</think>后文"), "后文");
    }

    #[test]
    fn tag_filter_skip_content_mode_max_limit_fallback() {
        let mut f = TagFilter::new();
        // <think> 未闭合：SkipContent 模式超过 MAX_SKIP_CHARS 后强制回退输出
        let content = "推".repeat(MAX_SKIP_CHARS + 1);
        let out = f.process(&format!("<think>{}尾部正文", content));
        assert!(out.ends_with("尾部正文"));
        assert!(!out.is_empty());
    }

    // ---------- B-12：SSE "data:" 无空格变体 ----------

    #[tokio::test]
    async fn sse_data_without_space_after_colon() {
        // 规范允许 data: 后无空格
        let json = serde_json::json!({ "choices": [{ "delta": { "content": "无空格前缀" } }] });
        let line = format!("data:{}\n\n", json);
        let emitter = run_sse(vec![line.into_bytes()]).await;
        assert_eq!(emitter.joined("chunk"), "无空格前缀");
    }

    #[tokio::test]
    async fn sse_done_without_space_after_colon() {
        let chunks = vec![
            sse_data_line("正文").into_bytes(),
            b"data:[DONE]\n\n".to_vec(),
            sse_data_line("不应出现").into_bytes(),
        ];
        let emitter = run_sse(chunks).await;
        assert_eq!(emitter.joined("chunk"), "正文");
    }

    // ---------- B-02：字节级行缓冲 ----------

    #[test]
    fn extract_lines_basic_and_crlf() {
        let buf = b"data: a\r\n\n\ndata: b\npartial";
        let mut consumed = 0usize;
        let lines = extract_complete_lines(buf, &mut consumed);
        assert_eq!(lines, vec!["data: a", "", "", "data: b"]);
        assert_eq!(consumed, buf.len() - b"partial".len());
    }

    #[test]
    fn extract_lines_partial_multibyte_tail_kept() {
        // 尾部是半个 "中"（3 字节中的前 2 字节），必须原样留在缓冲中
        let zh = "中".as_bytes();
        let mut buf = b"data: x\n".to_vec();
        buf.extend_from_slice(&zh[..2]);
        let mut consumed = 0usize;
        let lines = extract_complete_lines(&buf, &mut consumed);
        assert_eq!(lines, vec!["data: x"]);
        buf.drain(..consumed);
        assert_eq!(&buf[..], &zh[..2]);
    }

    #[tokio::test]
    async fn sse_multibyte_char_split_across_chunks() {
        // 汉字 "中" 的 3 个字节被拆到两个 chunk
        let payload = sse_data_line("中文转写");
        let bytes = payload.as_bytes();
        let split = bytes.iter().position(|&b| b == 0xE4).unwrap() + 1; // "中" 首字节后切开
        let emitter = run_sse(vec![bytes[..split].to_vec(), bytes[split..].to_vec()]).await;
        assert_eq!(emitter.joined("chunk"), "中文转写");
    }

    #[tokio::test]
    async fn sse_half_line_across_chunks() {
        let payload = sse_data_line("半行跨块测试");
        let bytes = payload.as_bytes();
        let split = 7; // 行中间任意位置
        let emitter = run_sse(vec![bytes[..split].to_vec(), bytes[split..].to_vec()]).await;
        assert_eq!(emitter.joined("chunk"), "半行跨块测试");
    }

    #[tokio::test]
    async fn sse_line_boundary_exactly_at_chunk_end() {
        let l1 = sse_data_line("第一");
        let l2 = sse_data_line("第二");
        let emitter = run_sse(vec![l1.into_bytes(), l2.into_bytes()]).await;
        assert_eq!(emitter.joined("chunk"), "第一第二");
    }

    #[tokio::test]
    async fn sse_done_terminates_stream() {
        let l1 = sse_data_line("正文");
        let chunks = vec![
            l1.into_bytes(),
            b"data: [DONE]\n\n".to_vec(),
            sse_data_line("不应出现").into_bytes(),
        ];
        let emitter = run_sse(chunks).await;
        assert_eq!(emitter.joined("chunk"), "正文");
    }

    #[tokio::test]
    async fn sse_crlf_line_endings() {
        let json = serde_json::json!({ "choices": [{ "delta": { "content": "CRLF文本" } }] });
        let line = format!("data: {}\r\n\r\n", json);
        let emitter = run_sse(vec![line.into_bytes()]).await;
        assert_eq!(emitter.joined("chunk"), "CRLF文本");
    }

    #[tokio::test]
    async fn sse_empty_stream_and_empty_lines() {
        let emitter = run_sse(Vec::new()).await;
        assert_eq!(emitter.joined("chunk"), "");
        let emitter = run_sse(vec![b"\n\n\n".to_vec()]).await;
        assert_eq!(emitter.joined("chunk"), "");
    }

    #[tokio::test]
    async fn sse_malformed_json_line_does_not_panic() {
        let chunks = vec![
            b"data: {broken json\n\n".to_vec(),
            sse_data_line("有效内容").into_bytes(),
        ];
        let emitter = run_sse(chunks).await;
        // 坏行只记日志，后续正常行继续解析
        assert_eq!(emitter.joined("chunk"), "有效内容");
    }

    #[tokio::test]
    async fn sse_malformed_json_with_multibyte_at_log_boundary() {
        // 构造超长坏行，使日志截取点（200 字节）落在多字节字符内部，不得 panic
        let mut line = String::from("data: {");
        for _ in 0..300 {
            line.push('中');
        }
        line.push_str("}\n\n");
        let emitter = run_sse(vec![line.into_bytes()]).await;
        assert_eq!(emitter.joined("chunk"), "");
    }

    #[tokio::test]
    async fn sse_usage_event_emitted() {
        let json = serde_json::json!({
            "choices": [{ "delta": {} }],
            "usage": { "total_tokens": 128 }
        });
        let line = format!("data: {}\n\n", json);
        let emitter = run_sse(vec![line.into_bytes()]).await;
        assert_eq!(emitter.joined("token-update"), "128");
    }

    #[tokio::test]
    async fn sse_long_input_buffer_is_reclaimed() {
        // 超长输入：逐字节喂入 1000 行，验证解析正确且 drain 后缓冲不累积
        let expected: String = (0..1000).map(|_| "长").collect();
        let payload: String = (0..1000).map(|_| sse_data_line("长")).collect();
        let bytes = payload.into_bytes();
        // 故意用与行/字符边界无关的碎块切分
        let mut chunks: Vec<Vec<u8>> = bytes.chunks(13).map(|c| c.to_vec()).collect();
        // T-05：流需以 `[DONE]` 结束才算完整，否则 UnexpectedEof
        chunks.push(b"data: [DONE]\n\n".to_vec());
        let stream = futures::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
        let emitter = RecordingEmitter::new();
        let cancel = CancelToken::new();

        // 手工驱动内层逻辑，以便检查缓冲容量回收
        // （直接复用 parse_sse_stream_inner 验证正确性）
        parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &cancel,
            std::time::Duration::from_secs(90),
        )
        .await
        .expect("超长输入解析不应失败");
        assert_eq!(emitter.joined("chunk"), expected);
    }

    // ---------- B-03：Debug 脱敏 ----------

    #[test]
    fn api_config_debug_redacts_api_key() {
        let config = ApiConfig {
            base_url: "https://example.com/v1".to_string(),
            api_key: "sk-very-secret-key-123456".to_string(),
            model: "mimo-v2.5-asr".to_string(),
            language: Some("auto".to_string()),
        };
        let dbg = format!("{:?}", config);
        assert!(!dbg.contains("sk-very-secret-key-123456"));
        assert!(!dbg.contains("secret"));
        assert!(dbg.contains("***"));
    }

    #[test]
    fn api_config_debug_default_and_short_key() {
        let dbg = format!("{:?}", ApiConfig::default());
        assert!(dbg.contains("***"));
        // 短密钥（<=4 字符）不允许原样出现
        let config = ApiConfig {
            api_key: "abcd".to_string(),
            ..ApiConfig::default()
        };
        let dbg = format!("{:?}", config);
        assert!(!dbg.contains("abcd"));
    }

    #[test]
    fn redact_secret_behaviour() {
        assert_eq!(redact_secret(""), "***");
        assert_eq!(redact_secret("abc"), "***");
        assert_eq!(redact_secret("abcd"), "***");
        assert_eq!(redact_secret("abcde"), "abcd***");
        assert_eq!(redact_secret("sk-very-secret"), "sk-v***");
    }

    // ---------- S-02：SSE 单行上限 ----------

    #[tokio::test]
    async fn sse_single_line_over_limit_terminates() {
        // 无换行的超大数据行：必须终止并报 Http 错误，不得无限缓冲
        let big = vec![b'a'; MAX_SSE_LINE_BYTES + 1];
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(big)]);
        let emitter = RecordingEmitter::new();
        let cancel = CancelToken::new();
        let r = parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &cancel,
            std::time::Duration::from_secs(90),
        )
        .await;
        assert!(
            matches!(r, Err(ApiError::Http(_))),
            "SSE 超限应报 Http 错误: {:?}",
            r.err()
        );
    }

    #[tokio::test]
    async fn sse_line_at_limit_boundary_ok() {
        // 恰好不超限的行正常解析（补上 [DONE] 表示完整结束，T-05）
        let mut line = vec![b'd'; MAX_SSE_LINE_BYTES - 8];
        line.extend_from_slice(b"\n\n");
        line.extend_from_slice(b"data: [DONE]\n\n");
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(line)]);
        let emitter = RecordingEmitter::new();
        let cancel = CancelToken::new();
        parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &cancel,
            std::time::Duration::from_secs(90),
        )
        .await
        .expect("边界内的行不应失败");
    }

    // ---------- S-02：禁重定向与响应上限（本地 mock HTTP 服务） ----------

    /// 启动一次性 mock HTTP 服务，返回地址与预置响应
    async fn mock_http_server(response: String) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定 mock 服务失败");
        let addr = listener.local_addr().expect("获取 mock 地址失败");
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn http_client_does_not_follow_redirects() {
        // mock 302 指向云元数据地址：客户端不得跟随，应原样返回 3xx
        let resp_body = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let addr = mock_http_server(resp_body.to_string()).await;
        let url = format!("http://{}/redirect", addr);
        let resp = http_client().get(&url).send().await.expect("请求不应失败");
        assert!(resp.status().is_redirection(), "重定向不得被自动跟随");
    }

    #[tokio::test]
    async fn read_body_limited_rejects_oversize_response() {
        // mock 超过上限的正文：必须稳定失败且不全量缓冲
        let limit: usize = 64 * 1024;
        let body = "x".repeat(limit * 2);
        let resp_body = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let addr = mock_http_server(resp_body).await;
        let url = format!("http://{}/big", addr);
        let resp = http_client().get(&url).send().await.expect("请求不应失败");
        let r = crate::endpoint::read_body_limited(resp, limit as u64).await;
        assert!(r.is_err(), "超限正文必须被拒绝");
    }

    #[tokio::test]
    async fn read_body_limited_allows_small_response() {
        let body = r#"{"data":[{"id":"m1"}]}"#;
        let resp_body = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let addr = mock_http_server(resp_body).await;
        let url = format!("http://{}/models", addr);
        let resp = http_client().get(&url).send().await.expect("请求不应失败");
        let r = crate::endpoint::read_body_limited(resp, 2 * 1024 * 1024).await;
        assert_eq!(r.expect("小响应应成功"), body);
    }

    // ---------- B-07：错误分级与 Display 文案 ----------

    fn http_status(status: u16, retry_after: Option<u64>) -> ApiError {
        ApiError::HttpStatus {
            status,
            body: "err".to_string(),
            retry_after,
        }
    }

    #[test]
    fn retryable_classification_by_status_code() {
        // 可重试：网络错误、超时、408、429、5xx
        assert!(ApiError::Http("connection reset".to_string()).is_retryable());
        assert!(ApiError::Timeout.is_retryable());
        assert!(http_status(408, None).is_retryable());
        assert!(http_status(429, Some(3)).is_retryable());
        assert!(http_status(500, None).is_retryable());
        assert!(http_status(503, None).is_retryable());
        // 不可重试：400/401/403/404、配置、解析、取消
        for code in [400u16, 401, 403, 404, 405, 422] {
            assert!(
                !http_status(code, None).is_retryable(),
                "{} 应快速失败",
                code
            );
        }
        assert!(!ApiError::Config("x".to_string()).is_retryable());
        assert!(!ApiError::Cancelled.is_retryable());
        assert!(
            !ApiError::PartialStream.is_retryable(),
            "部分内容已发射后不得重试"
        );
    }

    #[test]
    fn api_error_display_texts_unchanged() {
        // 前端错误文案必须与改造前一致
        assert_eq!(ApiError::Config("k".to_string()).to_string(), "配置错误: k");
        assert_eq!(ApiError::Http("h".to_string()).to_string(), "HTTP 错误: h");
        assert_eq!(http_status(401, None).to_string(), "HTTP 错误: 401: err");
        assert_eq!(ApiError::Timeout.to_string(), "请求超时");
        let io = ApiError::Io(std::io::Error::other("boom"));
        assert_eq!(io.to_string(), "IO 错误: boom");
        assert_eq!(
            ApiError::PartialStream.to_string(),
            "传输中断，已接收部分内容"
        );
        // source() 链路（B-07 统一错误体系）
        use std::error::Error;
        assert!(io.source().is_some(), "Io 必须携带 source");
        assert!(ApiError::Config("x".to_string()).source().is_none());
    }

    #[test]
    fn backoff_is_exponential_with_jitter_bounds() {
        for attempt in 1..=4u32 {
            let d = retry_sleep_duration(&http_status(503, None), attempt);
            let base = BACKOFF_BASE_SECS.saturating_mul(1u64 << (attempt - 1)) * 1000;
            let ms = d.as_millis() as u64;
            assert!(
                (base..=base + BACKOFF_JITTER_MAX_MS).contains(&ms),
                "attempt {} 退避 {}ms 应落在 [{}ms, {}ms]",
                attempt,
                ms,
                base,
                base + BACKOFF_JITTER_MAX_MS
            );
        }
        // attempt1/2/3 对应 2s/4s/8s 指数增长
        assert!(retry_sleep_duration(&http_status(500, None), 1).as_millis() >= 2000);
        assert!(retry_sleep_duration(&http_status(500, None), 2).as_millis() >= 4000);
        assert!(retry_sleep_duration(&http_status(500, None), 3).as_millis() >= 8000);
    }

    #[test]
    fn backoff_prefers_retry_after_on_429() {
        let d = retry_sleep_duration(&http_status(429, Some(7)), 1);
        assert_eq!(d, std::time::Duration::from_secs(7));
        // retry_after=0 回退指数退避
        let d = retry_sleep_duration(&http_status(429, Some(0)), 1);
        assert!(d.as_millis() >= 2000);
    }

    /// mock 401：不可重试，快速失败（验收标准：不重试）
    #[tokio::test(start_paused = true)]
    async fn retry_loop_401_fails_fast_without_retry() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let token = CancelToken::new();
        let r = retry_stream(
            move || {
                let c = c2.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(http_status(401, None))
                }
            },
            &token,
            None,
            "test",
        )
        .await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "401 不得重试"
        );
        assert!(matches!(r, Err(ApiError::HttpStatus { status: 401, .. })));
    }

    /// mock 503：可重试，共 3 次尝试（验收标准：重试 3 次）
    #[tokio::test(start_paused = true)]
    async fn retry_loop_503_retries_three_attempts() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let token = CancelToken::new();
        let r = retry_stream(
            move || {
                let c = c2.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(http_status(503, None))
                }
            },
            &token,
            None,
            "test",
        )
        .await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "503 应共尝试 3 次"
        );
        assert!(matches!(r, Err(ApiError::HttpStatus { status: 503, .. })));
    }

    /// 中途成功即停止，不多发请求
    #[tokio::test(start_paused = true)]
    async fn retry_loop_stops_on_first_success() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let token = CancelToken::new();
        let r = retry_stream(
            move || {
                let c = c2.clone();
                async move {
                    let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        Err(http_status(500, None))
                    } else {
                        Ok(())
                    }
                }
            },
            &token,
            None,
            "test",
        )
        .await;
        assert!(r.is_ok());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    // ---------- B-08：取消打断 ----------

    /// 重试退避 sleep 期间取消：立即返回 Cancelled，不等满退避时长
    #[tokio::test]
    async fn backoff_sleep_interrupted_by_cancel() {
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            t2.cancel();
        });
        let err = http_status(429, Some(30)); // 30s 退避
        let start = std::time::Instant::now();
        let r = backoff_sleep(&err, 1, &token).await;
        assert!(matches!(r, Err(ApiError::Cancelled)));
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    /// SSE 空闲等待期间取消：不等满 90s 空闲超时，立即返回 Cancelled
    #[tokio::test]
    async fn sse_idle_wait_interrupted_by_cancel() {
        let stream = futures::stream::pending::<Result<Vec<u8>, std::io::Error>>();
        let emitter = RecordingEmitter::new();
        let token = CancelToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            t2.cancel();
        });
        let start = std::time::Instant::now();
        let r = parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &token,
            std::time::Duration::from_secs(90),
        )
        .await;
        assert!(
            matches!(r, Err(ApiError::Cancelled)),
            "取消应返回 Cancelled: {:?}",
            r.err()
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "取消必须即时生效"
        );
    }

    /// 已进入取消态的 token：SSE 解析循环头立即退出
    #[tokio::test]
    async fn sse_returns_cancelled_when_token_pre_cancelled() {
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(
            sse_data_line("文本").into_bytes(),
        )]);
        let emitter = RecordingEmitter::new();
        let token = CancelToken::new();
        token.cancel();
        let r = parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &token,
            std::time::Duration::from_secs(90),
        )
        .await;
        assert!(matches!(r, Err(ApiError::Cancelled)));
        assert_eq!(emitter.joined("chunk"), "", "取消后不得再发射 chunk");
    }

    // ---------- Critical：中途失败已发射 chunk 时错误改写为 PartialStream ----------

    /// 重试内核收到 PartialStream 必须只尝试一次（不可重试）
    #[tokio::test(start_paused = true)]
    async fn retry_loop_partial_stream_fails_fast() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let token = CancelToken::new();
        let r = retry_stream(
            move || {
                let c = c2.clone();
                async move {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(ApiError::PartialStream)
                }
            },
            &token,
            None,
            "test",
        )
        .await;
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "PartialStream 不得重试"
        );
        assert!(matches!(r, Err(ApiError::PartialStream)));
    }

    // ---------- T-05：EOF 未收到 [DONE] 视为不完整响应 ----------

    #[tokio::test]
    async fn sse_eof_without_done_returns_unexpected_eof() {
        // 已解析出文本但流在 [DONE] 前干净结束 → UnexpectedEof（不可重试）
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(
            sse_data_line("半截正文").into_bytes(),
        )]);
        let emitter = RecordingEmitter::new();
        let cancel = CancelToken::new();
        let r = parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &cancel,
            std::time::Duration::from_secs(90),
        )
        .await;
        assert!(
            matches!(r, Err(ApiError::UnexpectedEof)),
            "无 [DONE] 的 EOF 应为 UnexpectedEof: {:?}",
            r.err()
        );
        // 已发射的文本保留（由调用方 PartialStream 语义决定重试与否）
        assert_eq!(emitter.joined("chunk"), "半截正文");
        assert!(!ApiError::UnexpectedEof.is_retryable(), "EOF 不可重试");
    }

    #[tokio::test]
    async fn sse_eof_with_done_is_success() {
        let stream = futures::stream::iter(vec![
            Ok::<_, std::io::Error>(sse_data_line("完整正文").into_bytes()),
            Ok::<_, std::io::Error>(b"data: [DONE]\n\n".to_vec()),
        ]);
        let emitter = RecordingEmitter::new();
        let cancel = CancelToken::new();
        let r = parse_sse_stream_inner(
            stream,
            &emitter,
            "chunk",
            &cancel,
            std::time::Duration::from_secs(90),
        )
        .await;
        assert!(r.is_ok(), "收到 [DONE] 应成功: {:?}", r.err());
        assert_eq!(emitter.joined("chunk"), "完整正文");
    }

    // ---------- T-06：TagFilter finish 收尾不丢字 ----------

    #[test]
    fn tag_filter_finish_flushes_think_prefix_tail() {
        let mut f = TagFilter::new();
        let out = f.process("I think");
        assert_eq!(out, "I "); // think 前缀暂存，未触发完整 think>
        let tail = f.finish();
        assert_eq!(tail, "think", "未完成的 think> 匹配字符应回吐");
    }

    #[test]
    fn tag_filter_finish_flushes_partial_plain_tag() {
        let mut f = TagFilter::new();
        let out = f.process("价格 <abc");
        assert_eq!(out, "价格 ");
        let tail = f.finish();
        assert_eq!(tail, "<abc", "未闭合普通标签应回吐");
    }

    #[test]
    fn tag_filter_finish_drops_unclosed_think_skip_content() {
        let mut f = TagFilter::new();
        let out = f.process("think>正在思考未结束");
        assert_eq!(out, "");
        // 仍处于 think 跳过态：finish 不得回吐思考内容
        assert_eq!(f.finish(), "");
    }

    #[test]
    fn tag_filter_finish_after_done_via_parser() {
        // 模拟 [DONE] 前尾部为 "think" 前缀：完整流应输出 "正文 I think"
        let mut f = TagFilter::new();
        let a = f.process("正文 I ");
        let b = f.process("think");
        assert_eq!(a, "正文 I ");
        assert_eq!(b, "");
        let tail = f.finish();
        assert_eq!(tail, "think");
        assert_eq!(format!("{}{}", a, tail), "正文 I think");
    }
}
