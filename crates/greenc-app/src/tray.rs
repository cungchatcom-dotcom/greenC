//! System Tray menu (`Mở GreenC` / `Thoát`), native toast notifications,
//! and `< 15 MB` RAM WebView2 unload-on-close / `< 120 ms` recreate-on-open (Task T061).

use crate::state::{AppState, WatchdogAlertEvent};
use greenc_core::config::APP_NAME;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_notification::NotificationExt;

pub const MAIN_WINDOW_LABEL: &str = "main";
pub const TRAY_ID: &str = "greenc-main-tray";
const MENU_ID_OPEN: &str = "tray_open_greenc";
const MENU_ID_QUIT: &str = "tray_quit_greenc";

/// Initializes the Windows system tray icon and localized context menu (`Mở GreenC` / `Thoát`).
pub fn setup_system_tray(app: &AppHandle, state: &AppState) -> tauri::Result<()> {
    let locale = state
        .settings()
        .get_language()
        .ok()
        .flatten()
        .unwrap_or_else(|| "vi".to_string());

    let open_label = state.catalog.tr(&locale, "Mở GreenC", &[]);
    let quit_label = state.catalog.tr(&locale, "Thoát", &[]);

    let open_item = MenuItem::with_id(app, MENU_ID_OPEN, open_label, true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, MENU_ID_QUIT, quit_label, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &sep, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(APP_NAME)
        .show_menu_on_left_click(false)
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            MENU_ID_OPEN => {
                let _ = show_or_recreate_main_window(app_handle);
            }
            MENU_ID_QUIT => {
                app_handle.exit(0);
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
                let _ = show_or_recreate_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    let _tray = builder.build(app)?;
    Ok(())
}

/// Refreshes the system tray context menu captions immediately when the user switches language.
pub fn refresh_tray_menu_language(app: &AppHandle, state: &AppState, locale: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let open_label = state.catalog.tr(locale, "Mở GreenC", &[]);
        let quit_label = state.catalog.tr(locale, "Thoát", &[]);
        if let (Ok(open_item), Ok(sep), Ok(quit_item)) = (
            MenuItem::with_id(app, MENU_ID_OPEN, open_label, true, None::<&str>),
            PredefinedMenuItem::separator(app),
            MenuItem::with_id(app, MENU_ID_QUIT, quit_label, true, None::<&str>),
        ) {
            if let Ok(menu) = Menu::with_items(app, &[&open_item, &sep, &quit_item]) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    }
}

/// Attaches the close-to-tray handler to a WebView2 window so closing the window destroys
/// the WebView2 surface (`msedgewebview2.exe` processes exit, dropping resident RAM `< 15 MB`)
/// while keeping `BackgroundGuardian` active in the system tray.
pub fn attach_close_to_tray_handler(window: &tauri::WebviewWindow) {
    let win_clone = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            // Destroy the WebView2 instance completely to reclaim ~80-150 MB of browser memory.
            if let Err(err) = win_clone.destroy() {
                tracing::warn!("WebView2 destroy fallback to hide(): {err}");
                let _ = win_clone.hide();
            }
        }
    });
}

/// Focuses the existing main window or recreates the WebView2 window from embedded assets
/// in `< 120 ms` after it was unloaded to the system tray.
pub fn show_or_recreate_main_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let recreated =
        WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::App("index.html".into()))
            .title(APP_NAME)
            .inner_size(1280.0, 820.0)
            .min_inner_size(960.0, 640.0)
            .resizable(true)
            .center()
            .build()?;

    attach_close_to_tray_handler(&recreated);
    let _ = recreated.set_focus();
    Ok(())
}

/// Dispatches a native Windows desktop toast notification for Auto-Rollback, Low-Disk Shield,
/// or Broken-Junction alerts while running silently in the system tray.
pub fn notify_watchdog_alert(app: &AppHandle, alert: &WatchdogAlertEvent) {
    let _ = app
        .notification()
        .builder()
        .title(&alert.title)
        .body(&alert.detail)
        .show();
}
