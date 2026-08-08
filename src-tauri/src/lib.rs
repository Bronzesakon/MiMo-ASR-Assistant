//! 应用入口（清单 B-11 重组）：本文件只保留 run()、builder、插件注册与
//! managed state。IPC 命令见 commands/，配置见 config.rs，提示词仓库见
//! prompt_store.rs，托盘与窗口见 tray.rs，魔法常量见 constants.rs。

// 清单 I-03 平台策略 A：运行与发布仅支持 Windows 10/11 x64。
// 非 Windows 构建在编译期给出明确的平台不支持错误，避免在链接阶段
// 以找不到 crypt32 等晦涩错误失败。平台支持矩阵见 README「平台支持」。
#[cfg(not(target_os = "windows"))]
compile_error!(
    "MiMo ASR Assistant 仅支持 Windows 平台（Windows 10/11 x64，平台策略 A）：\
     API Key 加密依赖 Windows DPAPI，音频转码依赖捆绑的精简 ffmpeg.exe。\
     非 Windows 平台不受支持，详见 README「平台支持」章节。"
);

mod api;
mod atomic;
mod audio;
mod base64;
mod cancel;
mod commands;
mod config;
mod config_store;
mod constants;
mod dpapi;
mod endpoint;
mod ffmpeg_exe;
mod log;
mod prompt;
mod prompt_store;
mod provider;
mod rate_limiter;
mod state;
#[cfg(test)]
mod task;
mod tray;
mod wav_codec;

use std::collections::HashMap;
use tauri::Manager;

use state::{FfmpegBinState, TaskRegistry};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window_handle(app);
        }))
        .manage(rate_limiter::create_asr_limiter())
        .manage(rate_limiter::create_chat_limiter())
        .manage(TaskRegistry {
            tasks: tokio::sync::Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            commands::capabilities::get_capabilities,
            commands::transcribe::process_and_transcribe,
            commands::transcribe::cancel_task,
            commands::transcribe::pause_task,
            commands::transcribe::resume_transcription,
            commands::transcribe::cleanup_file_temp,
            commands::chat::start_chat,
            commands::config::load_config,
            commands::config::set_theme,
            commands::config::set_slice_seconds,
            commands::config::set_asr_provider,
            commands::config::set_polish_selection,
            commands::config::set_provider_key,
            commands::config::upsert_custom_provider,
            commands::config::delete_custom_provider,
            commands::config::get_providers,
            commands::config::detect_provider,
            commands::config::get_default_prompt,
            commands::config::fetch_custom_models,
            commands::config::set_logging_enabled,
            commands::fs::get_audio_duration,
            commands::fs::export_file,
            commands::fs::get_log_status,
            commands::fs::open_log_dir,
            commands::prompts::load_prompts,
            commands::prompts::create_prompt,
            commands::prompts::update_prompt,
            commands::prompts::delete_prompt,
            commands::prompts::reorder_prompts,
            commands::prompts::select_prompt,
            commands::prompts::builtin_default_content,
            commands::prompts::open_prompt_dir,
            tray::hide_main_window,
            tray::show_main_window,
        ])
        .setup(|app| {
            // B-09：日志目录迁至 app_log_dir（setup 阶段才有 AppHandle）
            let log_dir = config::log_dir_for(app.handle());
            log::init_log(&log_dir);

            // H-05：检测旧安装目录下的旧日志目录，只提示可发现性，不自动迁移未知文件
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    let legacy_log = exe_dir.join("log");
                    if legacy_log != log_dir && legacy_log.is_dir() {
                        warn!("检测到旧安装目录下存在旧版日志目录，可从旧安装位置手动查看，本应用不会自动迁移");
                    }
                }
            }

            commands::fs::clear_webview2_cache();
            commands::fs::cleanup_legacy_temp();

            // 捆绑 ffmpeg.exe 一次性 resolve（含 SHA-256 完整性校验）。
            // 失败仅 warn 降级（转码/时长探测不可用），不阻断启动。
            let ffmpeg_bin = match ffmpeg_exe::FfmpegBin::resolve(app.handle()) {
                Ok(bin) => Some(bin),
                Err(e) => {
                    warn!(
                        "捆绑 ffmpeg.exe 不可用，转码与媒体时长探测功能将被禁用: {}",
                        e
                    );
                    None
                }
            };
            app.manage(FfmpegBinState(ffmpeg_bin));

            // F-06：提示词仓库（prompts.json）。目录复用 B-09 的 app_config_dir
            // 路径规则，不创建第二套目录规则；旧 config.json.polish_prompt 仅作为
            // 一次性迁移来源（prompts.json 存在时不再读取旧字段）。
            // P3：配置仓库单实例初始化（含迁移/损坏恢复/解密/只读判定），
            // 运行期间的配置事实来源为该仓库，不再重复读磁盘。
            let config_repo = config_store::ConfigRepository::init(app.handle());
            let legacy_prompt = config_repo.initial_polish_prompt();
            let prompts_dir = config::config_dir(app.handle());
            app.manage(config_repo);
            app.manage(prompt_store::PromptRepository::open(
                prompts_dir,
                &legacy_prompt,
            ));

            tray::setup_tray(app)?;
            tray::create_main_window(app)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application");
    // Warning 6：日志 static 句柄在退出时不执行 Drop，
    // 在 RunEvent::Exit 处显式 flush，避免丢失尾部日志
    app.run(|_handle, event| {
        if let tauri::RunEvent::Exit = event {
            log::flush();
        }
    });
}
