//! Tauri v2 desktop application backend library for GreenC v2.

#![deny(unsafe_code)]

pub mod commands;
pub mod guardian;
pub mod state;
pub mod tray;

pub use state::*;

use greenc_core::error::Result;
use greenc_platform::GreenCAppMutex;
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
    // If another instance of GreenC is already running, exit gracefully.
    let _app_mutex = match acquire_single_instance() {
        Ok(mutex) => mutex,
        Err(err) => {
            eprintln!(
                "GreenC is already running or could not acquire single-instance mutex: {err}"
            );
            tracing::warn!("Aborting desktop application startup: {err}");
            return;
        }
    };

    let db_path = greenc_core::config::default_db_path();
    let _ = auto_migrate_legacy_db(&db_path);
    let app_state = AppState::new(db_path).expect("Failed to initialize AppState");
    // Re-point Vault archives whose ZIP moved (project rename, data-root change, legacy import)
    // and mark the ones whose ZIP is gone, before the UI or the watchdog look at them.
    if let Err(err) = greenc_storage::vault_repair::repair_vault_paths_default(&app_state.db) {
        tracing::warn!("Vault path repair skipped: {err}");
    }

    let start_minimized = std::env::args().any(|a| a == "--minimized" || a == "--autostart");

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
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
        ])
        .setup(move |app| {
            let state = app.state::<AppState>().inner().clone();
            state.set_app_handle(app.handle().clone());

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
        .run(tauri::generate_context!())
        .expect("failed to run GreenC Tauri application");
}
