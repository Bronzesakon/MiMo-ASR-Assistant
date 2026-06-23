use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 全局共享 HTTP 客户端（连接池复用，减少并发场景下 TCP 握手开销）
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(10)
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
}

impl ApiConfig {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self.api_key.is_empty() {
            return Err(ApiError::ConfigError("API Key 不能为空".to_string()));
        }
        if self.base_url.is_empty() {
            return Err(ApiError::ConfigError("Base URL 不能为空".to_string()));
        }
        if self.model.is_empty() {
            return Err(ApiError::ConfigError("模型名称不能为空".to_string()));
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

#[derive(Debug)]
pub enum ApiError {
    ConfigError(String),
    HttpError(String),
    ParseError(String),
    TimeoutError,
    IoError(std::io::Error),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::ConfigError(msg) => write!(f, "配置错误: {}", msg),
            ApiError::HttpError(msg) => write!(f, "HTTP 错误: {}", msg),
            ApiError::ParseError(msg) => write!(f, "解析错误: {}", msg),
            ApiError::TimeoutError => write!(f, "请求超时"),
            ApiError::IoError(e) => write!(f, "IO 错误: {}", e),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::IoError(e)
    }
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    total_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<Message>,
    delta: Option<Delta>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
}

/// 事件发射器 trait
pub trait Emitter {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String>;
}

// ============================================================
// ASR 语音识别（严格按照 API 接口规范文档）
// 模型：mimo-v2.5-asr
// 输入：仅音频（data URL 格式）
// 不支持 system prompt、不支持文本输入
// ============================================================

/// 非流式 ASR 转写
pub async fn transcribe(
    audio_data_url: &str,
    config: &ApiConfig,
) -> Result<String, ApiError> {
    config.validate()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

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
        }
    });

    let client = http_client();
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.api_key))
        .timeout(std::time::Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ApiError::TimeoutError
            } else {
                ApiError::HttpError(e.to_string())
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::HttpError(format!("{}: {}", status, body)));
    }

    let result: ChatCompletionResponse = response
        .json()
        .await
        .map_err(|e| ApiError::ParseError(e.to_string()))?;

    let text = result.choices
        .first()
        .and_then(|c| c.message.as_ref())
        .and_then(|m| m.content.as_ref())
        .cloned()
        .unwrap_or_default();

    Ok(text)
}

/// 流式 ASR 转写（单次尝试，重试逻辑由调用方控制）
/// 如果 SSE 流解析失败，自动降级为非流式请求
pub async fn stream_transcription(
    audio_data_url: &str,
    config: &ApiConfig,
    emitter: &impl Emitter,
) -> Result<(), ApiError> {
    config.validate()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

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
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.api_key))
        .timeout(std::time::Duration::from_secs(120))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ApiError::TimeoutError
            } else {
                ApiError::HttpError(e.to_string())
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::HttpError(format!("{}: {}", status, body)));
    }

    // 尝试流式解析，失败则降级为非流式
    match parse_sse_stream(response, emitter, "transcription-chunk", "transcription-complete").await {
        Ok(()) => Ok(()),
        Err(ApiError::ParseError(msg)) => {
            log::warn!("SSE 流解析失败，降级为非流式请求: {}", msg);
            // 降级：发送完整文本作为单个 chunk
            let text = transcribe(audio_data_url, config).await?;
            let _ = emitter.emit("transcription-chunk", &text);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

// ============================================================
// 流式聊天文本
// 模型：mimo-v2.5 等（非 ASR 模型）
// 支持 system prompt + 文本 + 可选音频
// ============================================================

/// 流式聊天文本（支持 system prompt + 用户文本 + 可选音频）
pub async fn stream_chat(
    system_prompt: &str,
    user_text: &str,
    audio_data_url: Option<&str>,
    config: &ApiConfig,
    response_format: Option<&str>,
    emitter: &impl Emitter,
) -> Result<(), ApiError> {
    config.validate()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

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
        return Err(ApiError::ConfigError("消息内容不能为空".to_string()));
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
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", config.api_key))
        .timeout(std::time::Duration::from_secs(60))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                ApiError::TimeoutError
            } else {
                ApiError::HttpError(e.to_string())
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::HttpError(format!("{}: {}", status, body)));
    }

    parse_sse_stream(response, emitter, "chat-chunk", "chat-complete").await
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
}

impl TagFilter {
    fn new() -> Self {
        Self {
            mode: FilterMode::Normal,
            tag_buf: String::new(),
            in_tag: false,
            in_close_tag: false,
            skip_tag: String::new(),
        }
    }

    /// 处理一段文本，返回过滤后的文本
    fn process(&mut self, input: &str) -> String {
        let mut output = String::with_capacity(input.len());

        for ch in input.chars() {
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
                                }
                            }
                            self.tag_buf.clear();
                        }
                    }
                    // SkipContent 模式下，普通字符直接丢弃
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
}

// ============================================================
// SSE 解析（共用）
// ============================================================

async fn parse_sse_stream(
    response: reqwest::Response,
    emitter: &impl Emitter,
    chunk_event: &str,
    _complete_event: &str,
) -> Result<(), ApiError> {
    let stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut tag_filter = TagFilter::new();

    use futures::StreamExt;
    let mut stream = stream;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| ApiError::HttpError(e.to_string()))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Ok(());
                }

                if let Ok(resp) = serde_json::from_str::<ChatCompletionResponse>(data) {
                    // 发射文本片段（经过标签过滤）
                    if let Some(text) = resp.choices
                        .first()
                        .and_then(|c| c.delta.as_ref())
                        .and_then(|d| d.content.as_ref())
                    {
                        let filtered = tag_filter.process(text);
                        if !filtered.is_empty() {
                            let _ = emitter.emit(chunk_event, &filtered);
                        }
                    }
                    // 发射 token 统计（通常在最后一个 chunk）
                    if let Some(usage) = resp.usage {
                        if let Some(tokens) = usage.total_tokens {
                            let _ = emitter.emit("token-update", &tokens.to_string());
                        }
                    }
                } else if !data.trim().is_empty() {
                    log::warn!("SSE 数据解析失败: {}", &data[..data.len().min(200)]);
                }
            }
        }
    }

    Ok(())
}
