mod ffmpeg;
mod audio;
mod api;
mod prompt;
mod provider;
mod log;
mod dpapi;
mod rate_limiter;

use api::Emitter;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    AppHandle, Emitter as TauriEmitterTrait, Manager, Theme, WebviewUrl, WebviewWindow,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
    window::Color,
};

// ============================================================
// Tauri 事件发射器
// ============================================================

struct TauriEmitter {
    handle: AppHandle,
}

impl api::Emitter for TauriEmitter {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String> {
        self.handle.emit(event, payload).map_err(|e| e.to_string())
    }
}

impl api::Emitter for Arc<TauriEmitter> {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String> {
        self.handle.emit(event, payload).map_err(|e| e.to_string())
    }
}

/// 带 file_id 标签的事件发射器，所有 emit 自动携带 file_id
struct TaggedEmitter {
    handle: AppHandle,
    file_id: String,
}

impl api::Emitter for TaggedEmitter {
    fn emit(&self, event: &str, payload: &str) -> Result<(), String> {
        let tagged = serde_json::json!({ "file_id": self.file_id, "text": payload });
        self.handle.emit(event, &tagged.to_string()).map_err(|e| e.to_string())
    }
}

// ============================================================
// 任务取消机制
// ============================================================

struct CancelHandle {
    cancelled: Arc<AtomicBool>,
}

struct TaskRegistry {
    tasks: tokio::sync::Mutex<HashMap<String, CancelHandle>>,
}

#[tauri::command]
async fn cancel_task(handle: AppHandle, file_id: String, file_name: String) {
    let handle_opt = handle.state::<TaskRegistry>().tasks.lock().await.remove(&file_id);
    if let Some(ch) = handle_opt {
        ch.cancelled.store(true, Ordering::Release);
        info!("[{}] 任务已取消", file_name);
    }
}

// ============================================================
// 请求参数类型
// ============================================================

#[derive(Debug, Deserialize)]
pub struct ApiParams {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
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

// ============================================================
// 配置文件
// ============================================================

use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub transcription_model: String,
    pub polish_model: String,
    pub theme: String,
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
}

fn default_true() -> bool { true }

fn default_asr_provider() -> String { "mimo-api".to_string() }

fn default_polish_provider() -> String { "deepseek".to_string() }

fn default_polish_prompt() -> String {
    "给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，去掉口语化表达的语气词，要求只对每句话做修饰，每句话都要输出，不简略，根据原文的内容，智能切分长段落".to_string()
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
        }
    }
}

fn config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("config.json")
}

#[tauri::command]
fn load_config() -> AppConfig {
    let path = config_path();
    let config = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let mut config: AppConfig = serde_json::from_str(&content).unwrap_or_default();
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
                if config.api_key.starts_with("ENC:") {
                    match dpapi::decrypt_string(&config.api_key[4..]) {
                        Ok(decrypted) => config.api_key = decrypted,
                        Err(e) => {
                            error!("API Key 解密失败: {}", e);
                            config.api_key.clear();
                        }
                    }
                }
                // 解密 provider_keys
                for (_, value) in config.provider_keys.iter_mut() {
                    if value.starts_with("ENC:") {
                        match dpapi::decrypt_string(&value[4..]) {
                            Ok(decrypted) => *value = decrypted,
                            Err(e) => {
                                error!("Provider Key 解密失败: {}", e);
                                value.clear();
                            }
                        }
                    }
                }
                config
            }
            Err(_) => AppConfig::default(),
        }
    } else {
        AppConfig::default()
    };
    log::set_enabled(config.logging_enabled);
    config
}

#[tauri::command]
fn save_config(config: AppConfig) -> Result<(), String> {
    let path = config_path();
    let mut save = config.clone();
    // 向后兼容：同步 api_key 为当前活跃 provider 的 key
    if save.api_key.is_empty() {
        let active_key = save.provider_keys.get(&save.asr_provider)
            .or_else(|| save.provider_keys.get(&save.polish_provider))
            .cloned()
            .unwrap_or_default();
        save.api_key = active_key;
    }
    if !save.api_key.is_empty() {
        let encrypted = dpapi::encrypt_string(&save.api_key).map_err(|e| e.to_string())?;
        save.api_key = format!("ENC:{}", encrypted);
    }
    // 加密 provider_keys
    for (_, value) in save.provider_keys.iter_mut() {
        if !value.is_empty() {
            let encrypted = dpapi::encrypt_string(value).map_err(|e| e.to_string())?;
            *value = format!("ENC:{}", encrypted);
        }
    }
    let json = serde_json::to_string_pretty(&save).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    log::set_enabled(config.logging_enabled);
    Ok(())
}

#[tauri::command]
fn set_logging_enabled(enabled: bool) {
    log::set_enabled(enabled);
}

// ============================================================
// Tauri 命令
// ============================================================

/// 获取音频时长
#[tauri::command]
fn get_audio_duration(file_path: String) -> Result<f64, String> {
    let path = PathBuf::from(&file_path);
    ffmpeg::get_duration(&path).map_err(|e| e.to_string())
}

/// 处理音频并逐片转写（合并 process_audio + start_transcription）
/// 每个分片切完立即发送 API 并 drop，不全部存内存
#[tauri::command]
async fn process_and_transcribe(
    handle: AppHandle,
    file_id: String,
    file_path: String,
    api_params: ApiParams,
) -> Result<(), String> {
    let path = PathBuf::from(&file_path);
    let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| file_id.clone());
    info!("[{}] process_and_transcribe 开始", file_name);

    let config: api::ApiConfig = api_params.into();
    let tagged_emitter = TaggedEmitter { handle: handle.clone(), file_id: file_id.clone() };
    let plain_emitter = TauriEmitter { handle: handle.clone() };
    let limiter = handle.state::<rate_limiter::AsrRateLimiter>();

    // 注册取消句柄
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.insert(file_id.clone(), CancelHandle { cancelled: cancelled.clone() });
    }

    if !path.exists() {
        let err_msg = format!("文件不存在: {}", file_path);
        let _ = plain_emitter.emit("transcription-error", &serde_json::json!({
            "file_id": file_id, "error": err_msg
        }).to_string());
        return Err(err_msg);
    }

    // 发射"转码中"状态
    let _ = plain_emitter.emit("process-status", &serde_json::json!({
        "file_id": file_id, "stage": "转码中"
    }).to_string());

    // channel：blocking task → async context，缓冲 8 个分片
    let (tx, rx) = tokio::sync::mpsc::channel::<(u32, u32, String)>(8);
    let mut rx = Some(rx);

    // 临时目录由本函数管理，确保取消时也能清理
    let temp_dir = audio::unique_temp_dir();

    // 在阻塞线程中执行切片，每切完一片通过 channel 发送
    let slice_temp_dir = temp_dir.clone();
    let blocking_task = tokio::task::spawn_blocking(move || {
        audio::convert_and_slice(&path, &slice_temp_dir, None, &|index, total, data_url| {
            let _ = tx.blocking_send((index, total, data_url));
        })
    });

    // 发射"分片中"状态
    let _ = plain_emitter.emit("process-status", &serde_json::json!({
        "file_id": file_id, "stage": "分片中"
    }).to_string());

    let start_time = std::time::Instant::now();

    // 逐片接收并立即发送 API
    while let Some((index, total, data_url)) = rx.as_mut().unwrap().recv().await {
        // 检查是否已取消
        if cancelled.load(Ordering::Acquire) {
            // 立即关闭 channel，通知 blocking task 停止切片
            rx.take();
            break;
        }

        // 发射分片进度
        let progress_json = serde_json::json!({
            "file_id": file_id, "current": index, "total": total,
        });
        let _ = plain_emitter.emit("transcription-progress", &progress_json.to_string());

        // 获取速率许可
        let _permit = limiter.0.acquire_permit().await;

        let slice_start = std::time::Instant::now();
        let mut last_error = None;

        for attempt in 1..=3u32 {
            if attempt > 1 {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            match api::stream_transcription(&data_url, &config, &tagged_emitter, &cancelled).await {
                Ok(_) => { last_error = None; break; }
                Err(e) => {
                    warn!("[{}] 片段 {} 尝试 {} 失败: {}", file_name, index, attempt, e);
                    last_error = Some(e);
                }
            }
        }
        // data_url 在此 drop，释放 ~9MB 内存

        if let Some(e) = last_error {
            error!("[{}] 片段 {} 最终失败: {}", file_id, index, e);
            let err_json = serde_json::json!({ "file_id": file_id, "error": e.to_string() });
            let _ = plain_emitter.emit("transcription-error", &err_json.to_string());
            return Err(e.to_string());
        }

        let slice_ms = slice_start.elapsed().as_millis();
        let elapsed_s = start_time.elapsed().as_secs_f64();
        let upload_speed = if slice_ms > 0 { (data_url.len() as f64 / slice_ms as f64) * 1000.0 } else { 0.0 };
        let status_json = serde_json::json!({
            "file_id": file_id, "slice": index, "total": total,
            "elapsed": elapsed_s, "upload_speed": upload_speed,
        });
        let _ = plain_emitter.emit("status-update", &status_json.to_string());
    }

    // 等待 blocking task 完成
    let _ = blocking_task.await;

    // 清理取消句柄
    let is_cancelled = cancelled.load(Ordering::Acquire);
    {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.remove(&file_id);
    }

    if is_cancelled {
        audio::cleanup_temp(&temp_dir);
        info!("[{}] process_and_transcribe 已取消", file_name);
        return Ok(());
    }

    let complete_json = serde_json::json!({ "file_id": file_id });
    let _ = plain_emitter.emit("transcription-complete", &complete_json.to_string());

    // 转写完成，清理临时目录
    audio::cleanup_temp(&temp_dir);

    info!("[{}] process_and_transcribe 完成", file_name);
    Ok(())
}

/// 流式转写音频（携带 file_id 做事件隔离，使用速率控制器）
#[tauri::command]
async fn start_transcription(
    handle: AppHandle,
    file_id: String,
    file_name: String,
    audio_data_urls: Vec<String>,
    api_params: ApiParams,
) -> Result<(), String> {
    let total = audio_data_urls.len();
    info!("[{}] start_transcription 开始: {} 个片段", file_name, total);

    let config: api::ApiConfig = api_params.into();
    let tagged_emitter = TaggedEmitter { handle: handle.clone(), file_id: file_id.clone() };
    let plain_emitter = TauriEmitter { handle: handle.clone() };
    let limiter = handle.state::<rate_limiter::AsrRateLimiter>();

    // 注册取消句柄
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.insert(file_id.clone(), CancelHandle { cancelled: cancelled.clone() });
    }

    if total == 0 {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.remove(&file_id);
        return Ok(());
    }

    let start_time = std::time::Instant::now();

    for (i, data_url) in audio_data_urls.iter().enumerate() {
        if cancelled.load(Ordering::Acquire) { break; }

        let progress_json = serde_json::json!({
            "file_id": file_id,
            "current": i + 1,
            "total": total,
        });
        let _ = plain_emitter.emit("transcription-progress", &progress_json.to_string());

        let slice_start = std::time::Instant::now();

        // 获取速率许可（等待并发槽位 + RPM 余量）
        let _permit = limiter.0.acquire_permit().await;

        let mut last_error = None;
        let max_retries = 3;

        for attempt in 1..=max_retries {
            if attempt > 1 {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }

            match api::stream_transcription(data_url, &config, &tagged_emitter, &cancelled).await {
                Ok(_) => {
                    last_error = None;
                    break;
                }
                Err(e) => {
                    warn!("[{}] 片段 {} 尝试 {} 失败: {}", file_name, i + 1, attempt, e);
                    last_error = Some(e);
                }
            }
        }

        if let Some(e) = last_error {
            error!("[{}] 片段 {} 最终失败 (已重试 {} 次): {}", file_id, i + 1, max_retries, e);
            let err_json = serde_json::json!({ "file_id": file_id, "error": e.to_string() });
            let _ = plain_emitter.emit("transcription-error", &err_json.to_string());
            return Err(e.to_string());
        }

        let slice_ms = slice_start.elapsed().as_millis();
        let elapsed_s = start_time.elapsed().as_secs_f64();
        let upload_bytes = data_url.len();
        let upload_speed = if slice_ms > 0 {
            (upload_bytes as f64 / slice_ms as f64) * 1000.0
        } else {
            0.0
        };
        let status_json = serde_json::json!({
            "file_id": file_id,
            "slice": i + 1,
            "total": total,
            "elapsed": elapsed_s,
            "upload_speed": upload_speed,
        });
        let _ = plain_emitter.emit("status-update", &status_json.to_string());
    }

    let is_cancelled = cancelled.load(Ordering::Acquire);
    {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.remove(&file_id);
    }

    if is_cancelled {
        info!("[{}] start_transcription 已取消", file_name);
        return Ok(());
    }

    let complete_json = serde_json::json!({ "file_id": file_id });
    let _ = plain_emitter.emit("transcription-complete", &complete_json.to_string());
    info!("[{}] start_transcription 全部完成", file_name);
    Ok(())
}

/// 流式聊天文本（携带 file_id 做事件隔离，含日志）
#[tauri::command]
async fn start_chat(
    handle: AppHandle,
    file_id: String,
    system_prompt: String,
    user_text: String,
    audio_data_url: Option<String>,
    api_params: ApiParams,
    response_format: Option<String>,
    file_name: String,
) -> Result<(), String> {
    info!("[{}] start_chat 开始: 模型={}, 文本长度={}", file_name, api_params.model, user_text.len());

    let config: api::ApiConfig = api_params.into();
    let plain_emitter = TauriEmitter { handle: handle.clone() };
    let limiter = handle.state::<rate_limiter::ChatRateLimiter>();

    // 注册取消句柄
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.insert(file_id.clone(), CancelHandle { cancelled: cancelled.clone() });
    }

    // 获取 Chat 速率许可（等待并发槽位 + RPM 余量）
    let _permit = limiter.0.acquire_permit().await;

    // 使用 build_prompt 构造系统提示词（模板扩展：system + prefix + suffix）
    let template = prompt::PromptTemplate {
        system: system_prompt,
        user_prefix: String::new(),
        user_suffix: String::new(),
    };
    let final_system_prompt = prompt::build_prompt(&template, "");

    let max_retries = 2;
    let mut last_error = None;

    for attempt in 0..=max_retries {
        if cancelled.load(Ordering::Acquire) { break; }

        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }

        // 每次重试需要新的 emitter（SSE 流式状态不可复用）
        let retry_emitter = TaggedEmitter { handle: handle.clone(), file_id: file_id.clone() };

        match api::stream_chat(
            &final_system_prompt,
            &user_text,
            audio_data_url.as_deref(),
            &config,
            response_format.as_deref(),
            &retry_emitter,
            &cancelled,
        ).await {
            Ok(()) => {
                let is_cancelled = cancelled.load(Ordering::Acquire);
                {
                    let registry = handle.state::<TaskRegistry>();
                    registry.tasks.lock().await.remove(&file_id);
                }
                if is_cancelled {
                    info!("[{}] start_chat 已取消", file_name);
                    return Ok(());
                }
                let complete_json = serde_json::json!({ "file_id": file_id });
                let _ = plain_emitter.emit("chat-complete", &complete_json.to_string());
                info!("[{}] start_chat 完成", file_name);
                return Ok(());
            }
            Err(e) => {
                warn!("[{}] start_chat 尝试 {} 失败: {}", file_name, attempt + 1, e);
                last_error = Some(e);
            }
        }
    }

    // 清理取消句柄
    {
        let registry = handle.state::<TaskRegistry>();
        registry.tasks.lock().await.remove(&file_id);
    }

    if cancelled.load(Ordering::Acquire) {
        info!("[{}] start_chat 已取消", file_name);
        return Ok(());
    }

    let e = last_error.unwrap();
    error!("[{}] start_chat 最终失败 (已重试 {} 次): {} (模型={}, 文本长度={})", file_id, max_retries, e, config.model, user_text.len());
    let err_json = serde_json::json!({ "file_id": file_id, "error": e.to_string() });
    let _ = plain_emitter.emit("transcription-error", &err_json.to_string());
    Err(e.to_string())
}

/// 隐藏主窗口（最小化到托盘）
#[tauri::command]
fn hide_main_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// 显示主窗口（由前端 rAF 回调，确认第一帧已绘制后调用）
#[tauri::command]
fn show_main_window(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

/// 写入文件内容到指定路径
#[tauri::command]
fn save_file(content: String, path: String) -> Result<(), String> {
    std::fs::write(&path, &content).map_err(|e| e.to_string())?;
    Ok(())
}

/// 打开日志目录
#[tauri::command]
fn open_log_dir() -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let log_dir = exe_dir.join("log");
    if !log_dir.exists() {
        std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    { std::process::Command::new("explorer").arg(&log_dir).spawn().map_err(|e| e.to_string())?; }
    #[cfg(not(target_os = "windows"))]
    { std::process::Command::new("open").arg(&log_dir).spawn().map_err(|e| e.to_string())?; }
    Ok(())
}

/// 获取可用服务商列表
#[tauri::command]
fn get_providers() -> Vec<ProviderInfo> {
    provider::get_providers().into_iter().map(Into::into).collect()
}

/// 根据 API Key 前缀自动检测服务商并返回配置
#[tauri::command]
fn detect_provider(api_key: String) -> ProviderInfo {
    let svc = provider::detect_provider(&api_key);
    let info = provider::get_providers()
        .into_iter()
        .find(|p| match (&svc, p.id.as_str()) {
            (provider::ServiceProvider::MimoApi, "mimo-api") => true,
            (provider::ServiceProvider::MimoTokenPlan, "mimo-token-plan") => true,
            _ => false,
        })
        .unwrap_or_else(|| provider::get_providers().into_iter().next().unwrap());
    info.into()
}

/// 获取默认提示词模板
#[tauri::command]
fn get_default_prompt(template_type: String) -> Result<String, String> {
    let tmpl = match template_type.as_str() {
        "transcription" => prompt::default_transcription_template(),
        "punctuation" => prompt::default_punctuation_template(),
        _ => return Err(format!("未知模板类型: {}", template_type)),
    };
    Ok(serde_json::to_string(&tmpl).unwrap_or_default())
}

// ============================================================
// WebView2 缓存清理
// ============================================================

fn clear_webview2_cache() {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let cache_dir = exe_dir.join(".mimo_cache");
            if cache_dir.exists() {
                let _ = std::fs::remove_dir_all(&cache_dir);
            }
        }
    }
    let temp_audio = std::env::temp_dir().join("mimo_audio");
    if temp_audio.exists() {
        let _ = std::fs::remove_dir_all(&temp_audio);
    }
}

// ============================================================
// 应用入口
// ============================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    log::init_log();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main")
                .or_else(|| app.webview_windows().into_iter().next().map(|(_, w)| w))
            {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(rate_limiter::create_asr_limiter())
        .manage(rate_limiter::create_chat_limiter())
        .manage(TaskRegistry { tasks: tokio::sync::Mutex::new(HashMap::new()) })
        .invoke_handler(tauri::generate_handler![
            process_and_transcribe,
            start_transcription,
            start_chat,
            cancel_task,
            save_file,
            get_providers,
            detect_provider,
            get_default_prompt,
            get_audio_duration,
            load_config,
            save_config,
            set_logging_enabled,
            hide_main_window,
            show_main_window,
            open_log_dir,
        ])
        .setup(|app| {
            clear_webview2_cache();

            // 系统托盘图标
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::new()
                .tooltip(format!("Mimo v{}", app.package_info().version))
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main")
                            .or_else(|| app.webview_windows().into_iter().next().map(|(_, w)| w))
                        {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main")
                            .or_else(|| app.webview_windows().into_iter().next().map(|(_, w)| w))
                        {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }

            let _tray = tray_builder.build(app)?;

            // 程序化创建主窗口（替代 tauri.conf.json 声明式配置）
            let window = tauri::webview::WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
            .title("MiMo ASR Assistant")
            .inner_size(1200.0, 800.0)
            .min_inner_size(960.0, 640.0)
            .decorations(false)
            .center()
            .visible(false)
            .on_page_load(|webview, payload| {
                if payload.event() == PageLoadEvent::Finished {
                    let _ = webview.eval(r#"
                        requestAnimationFrame(() => {
                            requestAnimationFrame(() => {
                                try {
                                    window.__TAURI_INTERNALS__.invoke('show_main_window');
                                } catch(e) {
                                    console.error('[JS] invoke failed:', e);
                                }
                            });
                        });
                    "#);
                }
            })
            .build()?;

            // 检测系统主题，设置 WebView2 原生背景色
            let bg_color = match window.theme() {
                Ok(Theme::Light) => Color(248, 248, 247, 255),
                _ => Color(17, 17, 17, 255),
            };
            let _ = window.set_background_color(Some(bg_color));

            // 兜底：5 秒后无论如何显示窗口
            let win_fallback = window.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let _ = win_fallback.show();
                let _ = win_fallback.set_focus();
            });

            app.manage(window);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
