use std::{
    sync::Mutex,
    thread,
    time::Duration,
};

mod i18n;
mod webview2;

use clipboard_share::{AppStatus, SyncService, service::init_logging_global};
use i18n::{Locale, UiLocale, tr};
use tauri::{
    Emitter, Manager, State,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use uuid::Uuid;

struct AppState {
    sync: Mutex<SyncService>,
    locale: UiLocale,
}

fn map_err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command]
fn get_status() -> Result<AppStatus, String> {
    SyncService::status().map_err(map_err)
}

#[tauri::command]
fn start_sync(state: State<'_, AppState>) -> Result<AppStatus, String> {
    state.sync.lock().map_err(map_err)?.start().map_err(map_err)?;
    SyncService::status().map_err(map_err)
}

#[tauri::command]
fn stop_sync(state: State<'_, AppState>) -> Result<AppStatus, String> {
    let _ = state.sync.lock().map_err(map_err)?.stop().map_err(map_err)?;
    SyncService::status().map_err(map_err)
}

#[tauri::command]
async fn pair_listen(app: tauri::AppHandle, code: Option<String>) -> Result<String, String> {
    let resolved = code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(clipboard_share::config::random_pairing_code);
    clipboard_share::validate_code(&resolved).map_err(map_err)?;
    let _ = app.emit("pairing-started", &resolved);
    SyncService::pair_listen(Some(resolved.clone()))
        .await
        .map_err(map_err)?;
    let _ = app.emit("pairing-finished", &resolved);
    Ok(resolved)
}

#[tauri::command]
async fn pair_connect(address: String, code: String) -> Result<(), String> {
    SyncService::pair_connect(address, code)
        .await
        .map_err(map_err)
}

#[tauri::command]
fn unpair(device_id: String) -> Result<AppStatus, String> {
    let id = Uuid::parse_str(&device_id).map_err(map_err)?;
    let _ = SyncService::unpair(id).map_err(map_err)?;
    SyncService::status().map_err(map_err)
}

#[tauri::command]
fn set_locale(app: tauri::AppHandle, locale: String) -> Result<String, String> {
    let parsed = Locale::parse(&locale);
    let state = app.state::<AppState>();
    state.locale.set_locale(parsed);
    update_tray_tooltip(&app);
    Ok(parsed.as_str().to_owned())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn quit_app(app: &tauri::AppHandle, state: &AppState) {
    if let Ok(sync) = state.sync.lock() {
        let _ = sync.stop();
    }
    app.exit(0);
}

fn update_tray_tooltip(app: &tauri::AppHandle) {
    let Ok(status) = SyncService::status() else {
        return;
    };
    let locale = app.state::<AppState>().locale.current();
    let state = if status.running {
        if status.peers.is_empty() {
            tr(locale, "running_no_peers")
        } else {
            tr(locale, "running")
        }
    } else {
        tr(locale, "stopped")
    };
    let tip = format!("ClipboardShare — {state}");
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tip));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Windows: require WebView2. macOS uses built-in WKWebView. Linux is out of scope for v1.
    if !webview2::ensure_or_prompt() {
        return;
    }

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .manage(AppState {
            sync: Mutex::new(SyncService::new()),
            locale: UiLocale::new(),
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            start_sync,
            stop_sync,
            pair_listen,
            pair_connect,
            unpair,
            set_locale
        ])
        .setup(|app| {
            let _ = init_logging_global(false);

            let show = MenuItem::with_id(app, "show", tr(Locale::En, "show"), true, None::<&str>)?;
            let start =
                MenuItem::with_id(app, "start", tr(Locale::En, "start"), true, None::<&str>)?;
            let stop = MenuItem::with_id(app, "stop", tr(Locale::En, "stop"), true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", tr(Locale::En, "quit"), true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &start, &stop, &quit])?;
            app.state::<AppState>()
                .locale
                .set_menu_items(show, start, stop, quit);

            let icon = app
                .default_window_icon()
                .cloned()
                .expect("application icon must exist");

            TrayIconBuilder::with_id("main")
                .icon(icon)
                .tooltip("ClipboardShare")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "show" => show_main_window(app),
                        "start" => {
                            if let Ok(state) = app.state::<AppState>().sync.lock() {
                                let _ = state.start();
                            }
                            update_tray_tooltip(app);
                            let _ = app.emit("status-changed", ());
                        }
                        "stop" => {
                            if let Ok(state) = app.state::<AppState>().sync.lock() {
                                let _ = state.stop();
                            }
                            update_tray_tooltip(app);
                            let _ = app.emit("status-changed", ());
                        }
                        "quit" => {
                            let state = app.state::<AppState>();
                            quit_app(app, &state);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            if let Ok(state) = app.state::<AppState>().sync.lock() {
                match state.start() {
                    Ok(()) => {}
                    Err(error) if error.to_string().contains("已在运行") => {}
                    Err(error) => tracing::warn!(%error, "failed to auto-start sync"),
                }
            }
            update_tray_tooltip(app.handle());

            let app_handle = app.handle().clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(2));
                    update_tray_tooltip(&app_handle);
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!());

    if let Err(error) = result {
        webview2::prompt_runtime_failure(&error.to_string());
    }
}
