//! Tauri v2 desktop application backend library for GreenC v2.

#![deny(unsafe_code)]

pub mod commands;
pub mod guardian;
pub mod state;
pub mod tray;

pub use state::*;

use greenc_core::error::Result;
use greenc_platform::{notify_already_running, GreenCAppMutex};
#[cfg(not(feature = "store"))]
use greenc_storage::legacy_migration::auto_migrate_legacy_db;
use std::sync::Arc;
use tauri::Manager;

/// Attempts to acquire the global single-instance application mutex.
/// Returns `Ok(GreenCAppMutex)` on success, or `Err(GreenCError::PreconditionBlocked)` if another instance is running.
pub fn acquire_single_instance() -> Result<GreenCAppMutex> {
    GreenCAppMutex::acquire()
}

pub fn run() {
    // Acquire single-instance mutex lock before launching desktop shell.
    // If another instance of GreenC (including an EXE or Microsoft Store edition) is already
    // running, inform the user and exit gracefully.
    let _app_mutex = match acquire_single_instance() {
        Ok(mutex) => mutex,
        Err(err) => {
            tracing::warn!("Aborting desktop application startup: {err}");
            notify_already_running(
                "GreenC đang chạy (có thể ở khay hệ thống hoặc phiên bản cài đặt khác).\nHãy mở biểu tượng GreenC ở khay hệ thống hoặc thoát phiên bản đang chạy trước.",
            );
            return;
        }
    };

    let db_path = greenc_core::config::default_db_path();
    let app_dir = db_path.parent().unwrap_or(&db_path).to_path_buf();
    let diagnostics = state::open_diagnostics_for_app_dir(&app_dir);

    // Install panic hook early so unexpected panics leave a privacy-safe crash marker
    // without locking internal diagnostics state, then delegate to the previous hook.
    let prev_hook = std::panic::take_hook();
    let diag_for_panic = Arc::clone(&diagnostics);
    std::panic::set_hook(Box::new(move |info| {
        if let Some(loc) = info.location() {
            diag_for_panic.record_panic(loc.file(), loc.line());
        } else {
            diag_for_panic.record_panic("unknown", 0);
        }
        prev_hook(info);
    }));

    #[cfg(not(feature = "store"))]
    let _ = auto_migrate_legacy_db(&db_path);
    let app_state =
        AppState::new_with_diagnostics(db_path, diagnostics).expect("Failed to initialize AppState");
    // Re-point Vault archives whose ZIP moved (project rename, data-root change, legacy import)
    // and mark the ones whose ZIP is gone, before the UI or the watchdog look at them.
    if let Err(err) = greenc_storage::vault_repair::repair_vault_paths_default(&app_state.db) {
        tracing::warn!("Vault path repair skipped: {err}");
    }

    let start_minimized = std::env::args().any(|a| a == "--minimized" || a == "--autostart");
    app_state.set_start_minimized(start_minimized);

    let builder = tauri::Builder::default();
    // Store (MSIX) builds manage autostart exclusively via `desktop:StartupTask`, never via
    // `tauri-plugin-autostart` (which writes `HKCU\...\Run`).
    #[cfg(not(all(windows, feature = "store")))]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec!["--minimized"]),
    ));

    builder
        .plugin(tauri_plugin_notification::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            // Bootstrap & Overview (User Story 1)
            commands::overview::get_bootstrap_state,
            commands::overview::complete_first_run,
            commands::overview::get_overview_summary,
            commands::overview::scan_space_lens,
            commands::overview::reveal_in_explorer,
            // Cleanup & Vault (User Story 2)
            commands::cleanup::scan_cleanup_candidates,
            commands::cleanup::execute_cleanup,
            commands::cleanup::control_task,
            commands::vault::get_vault_archives,
            commands::vault::restore_vault_archive,
            commands::vault::delete_vault_archive,
            commands::vault::inspect_vault_archive,
            commands::vault::restore_vault_files_selective,
            // Relocation (User Story 3)
            commands::relocate::scan_relocation_candidates,
            commands::relocate::relocate_directory,
            commands::relocate::get_known_folders,
            commands::relocate::relocate_known_folder,
            commands::relocate::recover_interrupted_relocations,
            commands::relocate::list_relocation_history,
            commands::relocate::undo_relocation,
            commands::relocate::repair_junction_target,
            // Zalo PC Management (Discovery, Safe Clean & Auto Move)
            commands::zalo::get_zalo_status,
            commands::zalo::clean_zalo_now,
            commands::zalo::move_zalo_data,
            commands::zalo::purge_zalo_retained_original,
            commands::zalo::restore_zalo_retained_original,
            // In-Place Compaction (User Story 4)
            commands::compact::scan_compaction_candidates,
            commands::compact::compact_files,
            commands::compact::uncompact_files,
            commands::compact::compact_vhdx,
            commands::compact::list_compaction_history,
            // Daily Delta Timeline & Process/Service Attribution (User Story 5)
            commands::history::scan_drive_history,
            commands::history::get_daily_drilldown,
            // Rolling Quotas, Settings, 50-Locale Live Switch & Sandbox Demo (User Stories 6 & 7)
            commands::rules_settings::get_supported_languages,
            commands::rules_settings::get_app_settings,
            commands::rules_settings::update_settings,
            commands::rules_settings::change_data_root,
            commands::rules_settings::list_quota_rules,
            commands::rules_settings::save_quota_rule,
            commands::rules_settings::delete_quota_rule,
            commands::drive_quota::list_drive_quotas,
            commands::drive_quota::save_drive_quota,
            commands::drive_quota::delete_drive_quota,
            commands::rules_settings::run_sandbox_demo,
            commands::rules_settings::hide_to_tray,
            commands::updater::check_for_updates,
            commands::updater::download_and_install_update,
            // Opt-in Diagnostics & Local Log
            commands::diagnostics::get_diagnostics_consent,
            commands::diagnostics::set_diagnostics_consent,
            commands::diagnostics::dismiss_diagnostics_prompt,
            commands::diagnostics::preview_diagnostics,
            commands::diagnostics::diagnostics_ui_ready,
            commands::diagnostics::diagnostics_screen,
            commands::diagnostics::export_diagnostics_log,
            commands::diagnostics::clear_diagnostics_log,
        ])
        .setup(move |app| {
            let state = app.state::<AppState>().inner().clone();
            state.set_app_handle(app.handle().clone());

            #[cfg(not(feature = "store"))]
            commands::diagnostics::spawn_diagnostics_sender(Arc::clone(&state.diagnostics));

            // Setup System Tray (`Mở GreenC` / `Thoát`)
            if let Err(err) = tray::setup_system_tray(app.handle(), &state) {
                tracing::warn!("System tray setup warning: {err}");
            }

            // Attach WebView2 unload-on-close (`< 15 MB` idle RAM) or unload immediately if `--minimized`
            if let Some(main_win) = app.get_webview_window(tray::MAIN_WINDOW_LABEL) {
                tray::attach_close_to_tray_handler(&main_win);
                if start_minimized {
                    let _ = main_win.destroy();
                }
            }

            // Spawn autonomous 45-second BackgroundGuardian
            let guardian = Arc::new(guardian::BackgroundGuardian::new(state));
            let guardian_token = guardian.spawn();
            app.manage(guardian_token);

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build GreenC Tauri application")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                // Keep the background daemon alive in the system tray when the main window
                // is closed or unloaded via "Chạy ngầm". Explicit `app_handle.exit(0)` passes `Some(0)`.
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
