//! 托盘与窗口域（清单 B-11 自 lib.rs 拆出）：
//! 系统托盘菜单、程序化主窗口创建、窗口显示命令与兜底显示。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
    window::Color,
    App, AppHandle, Manager, Theme, WebviewUrl, WebviewWindow,
};

use crate::constants;

/// 显示主窗口并聚焦（托盘菜单/托盘图标点击/单实例唤起共用）
pub fn show_main_window_handle(app: &AppHandle) {
    if let Some(window) = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_iter().next().map(|(_, w)| w))
    {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 隐藏主窗口（最小化到托盘）
#[tauri::command]
pub fn hide_main_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// 显示主窗口（由前端 rAF 回调，确认第一帧已绘制后调用）
#[tauri::command]
pub fn show_main_window(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

/// 系统托盘图标与菜单
pub fn setup_tray(app: &App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut tray_builder = TrayIconBuilder::new()
        .tooltip(format!("Mimo v{}", app.package_info().version))
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                show_main_window_handle(app);
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
                show_main_window_handle(app);
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(icon.clone());
    }

    let _tray = tray_builder.build(app)?;
    Ok(())
}

/// 程序化创建主窗口（替代 tauri.conf.json 声明式配置）
pub fn create_main_window(app: &mut App) -> tauri::Result<()> {
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
            let _ = webview.eval(
                r#"
                requestAnimationFrame(() => {
                    requestAnimationFrame(() => {
                        try {
                            window.__TAURI_INTERNALS__.invoke('show_main_window');
                        } catch(e) {
                            console.error('[JS] invoke failed:', e);
                        }
                    });
                });
            "#,
            );
        }
    })
    .build()?;

    // 检测系统主题，设置 WebView2 原生背景色
    let bg_color = match window.theme() {
        Ok(Theme::Light) => Color(248, 248, 247, 255),
        _ => Color(17, 17, 17, 255),
    };
    let _ = window.set_background_color(Some(bg_color));

    // 兜底：超时后窗口仍不可见时才强制显示（B-11：已正常显示则不抢焦点）
    let win_fallback = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(
            constants::WINDOW_FALLBACK_SECS,
        ));
        if matches!(win_fallback.is_visible(), Ok(false)) {
            let _ = win_fallback.show();
            let _ = win_fallback.set_focus();
        }
    });

    app.manage(window);
    Ok(())
}
