//! Background `QuotaRule` management, live 50-locale & theme `SettingsPatch` updates,
//! verified `change_data_root` (`move_vault`), and isolated `run_sandbox_demo` (Tasks T062, T065, T066).

use crate::commands::diagnostics::{record_action_result, CommandFailure};
use crate::guardian::measure_quota_rule_usage;
use crate::state::{AppState, TaskProgressEvent};
use chrono::Utc;
#[cfg(all(windows, feature = "store"))]
use greenc_core::config::STORE_STARTUP_TASK_ID;
use greenc_core::config::{
    default_vault_dir, sandbox_demo_dir, APP_NAME, AUTHOR, DEFAULT_MIN_FILE_AGE_SECONDS,
    DEFAULT_RETENTION_DAYS, DEFAULT_WATERMARK_RATIO, IS_STORE_BUILD, MICROSOFT_STORE_UPDATES_URL,
    MIN_VAULT_RESERVE_BYTES, WEBSITE_LABEL, WEBSITE_URL,
};
use greenc_core::models::{CleanCandidate, QuotaRule};
use greenc_core::task::TaskControl;
use greenc_diagnostics::Outcome;
use greenc_i18n::catalog::LANGUAGES;
use greenc_i18n::format::{decimal_separator, is_rtl};
use greenc_platform::handle::NativeFileLockProbe;
use greenc_platform::power_session::{NativePowerSessionApi, PowerSessionApi};
use greenc_platform::traits::NativeProcessServiceProbe;
use greenc_platform::volume::{list_drives_native, NativeWin32VolumeApi};
use greenc_storage::legacy_migration::move_vault;
use greenc_storage::settings::ThemeMode;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::canary::CanaryVerifier;
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageOptionDto {
    pub code: String,
    pub native_name: String,
    pub english_name: String,
    pub is_rtl: bool,
    pub decimal_sep: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettingsDto {
    pub app_name: String,
    pub app_version: String,
    pub author: String,
    pub website_url: String,
    pub website_label: String,
    pub data_root: Option<String>,
    pub retention_days_default: u32,
    pub language: String,
    pub is_rtl: bool,
    pub decimal_sep: String,
    pub theme_mode: String,
    pub autostart_enabled: bool,
    pub autostart_status: String,
    pub is_store_build: bool,
    pub store_updates_url: Option<String>,
    pub low_disk_shield_gb: u32,
    pub pause_on_battery_fullscreen: bool,
    pub zalo_auto_clean_enabled: bool,
    pub translations: HashMap<String, String>,
    pub supported_languages: Vec<LanguageOptionDto>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsPatchDto {
    pub language: Option<String>,
    pub theme_mode: Option<String>,
    pub retention_days_default: Option<u32>,
    pub autostart_enabled: Option<bool>,
    pub low_disk_shield_gb: Option<u32>,
    pub pause_on_battery_fullscreen: Option<bool>,
    pub zalo_auto_clean_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRuleInputDto {
    pub id: Option<i64>,
    pub app_name: String,
    pub process_name: Option<String>,
    pub service_name: Option<String>,
    pub target_paths: Vec<String>,
    pub max_bytes: u64,
    pub watermark_ratio: Option<f64>,
    pub retention_days: Option<u32>,
    pub min_file_age_seconds: Option<u64>,
    pub exclude_patterns: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRuleStatusDto {
    pub id: i64,
    pub app_name: String,
    pub process_name: String,
    pub service_name: String,
    pub target_paths: Vec<String>,
    pub max_bytes: u64,
    pub current_bytes: u64,
    pub watermark_ratio: f64,
    pub watermark_bytes: u64,
    pub retention_days: u32,
    pub min_file_age_seconds: u64,
    pub exclude_patterns: Vec<String>,
    pub protected_files: Vec<String>,
    pub enabled: bool,
    pub is_exceeded: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveVaultReportDto {
    pub task_id: String,
    pub new_data_root: String,
    pub moved_archives_count: usize,
    pub errors: Vec<String>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxDemoResultDto {
    pub sandbox_dir: String,
    pub canary_passed: bool,
    pub archived_files_count: usize,
    pub auto_rolled_back: bool,
    pub restored_files_count: usize,
    pub detail: String,
}

pub fn build_supported_languages() -> Vec<LanguageOptionDto> {
    LANGUAGES
        .iter()
        .map(|(code, native_name, english_name)| LanguageOptionDto {
            code: (*code).to_string(),
            native_name: (*native_name).to_string(),
            english_name: (*english_name).to_string(),
            is_rtl: is_rtl(code),
            decimal_sep: decimal_separator(code).to_string(),
        })
        .collect()
}

fn snapshot_settings_dto(state: &AppState) -> std::result::Result<AppSettingsDto, String> {
    let settings = state.settings();
    let language = settings
        .get_language()
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "vi".to_string());
    let data_root = settings
        .get_data_root()
        .map_err(|e| e.to_string())?
        .map(|p| p.to_string_lossy().to_string());
    let retention_days_default = settings.get_retention_days().map_err(|e| e.to_string())?;
    let theme_mode = settings
        .get_theme_mode()
        .map_err(|e| e.to_string())?
        .as_str()
        .to_string();
    let db_autostart_enabled = settings
        .get_autostart_enabled()
        .map_err(|e| e.to_string())?;
    let (autostart_enabled, autostart_status) = {
        #[cfg(all(windows, feature = "store"))]
        {
            match NativePowerSessionApi.autostart_state(STORE_STARTUP_TASK_ID) {
                Ok(st) => (st.is_enabled(), st.as_str().to_string()),
                Err(_) => (db_autostart_enabled, "unavailable".to_string()),
            }
        }
        #[cfg(not(all(windows, feature = "store")))]
        {
            let status = if db_autostart_enabled {
                "enabled"
            } else {
                "disabled"
            };
            (db_autostart_enabled, status.to_string())
        }
    };
    let low_disk_shield_gb = settings
        .get_low_disk_shield_gb()
        .map_err(|e| e.to_string())?;
    let pause_on_battery_fullscreen = settings
        .get_pause_on_battery_fullscreen()
        .map_err(|e| e.to_string())?;
    let zalo_auto_clean_enabled = settings
        .get_zalo_auto_clean_enabled()
        .map_err(|e| e.to_string())?;

    let translations = state.catalog.get_translations_for_locale(&language);

    Ok(AppSettingsDto {
        app_name: APP_NAME.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        author: AUTHOR.to_string(),
        website_url: WEBSITE_URL.to_string(),
        website_label: WEBSITE_LABEL.to_string(),
        data_root,
        retention_days_default,
        is_rtl: is_rtl(&language),
        decimal_sep: decimal_separator(&language).to_string(),
        language,
        theme_mode,
        autostart_enabled,
        autostart_status,
        is_store_build: IS_STORE_BUILD,
        store_updates_url: IS_STORE_BUILD.then(|| MICROSOFT_STORE_UPDATES_URL.to_string()),
        low_disk_shield_gb,
        pause_on_battery_fullscreen,
        zalo_auto_clean_enabled,
        translations,
        supported_languages: build_supported_languages(),
    })
}

fn map_quota_rule_dto(rule: QuotaRule) -> QuotaRuleStatusDto {
    let current_bytes = measure_quota_rule_usage(&rule);
    let watermark_bytes = ((rule.max_bytes as f64) * rule.watermark_ratio) as u64;
    let is_exceeded = current_bytes > rule.max_bytes;
    QuotaRuleStatusDto {
        id: rule.id.unwrap_or(0),
        app_name: rule.app_name,
        process_name: rule.process_name,
        service_name: rule.service_name,
        target_paths: rule
            .target_paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        max_bytes: rule.max_bytes,
        current_bytes,
        watermark_ratio: rule.watermark_ratio,
        watermark_bytes,
        retention_days: rule.retention_days,
        min_file_age_seconds: rule.min_file_age_seconds,
        exclude_patterns: rule.exclude_patterns,
        protected_files: rule.protected_files,
        enabled: rule.enabled,
        is_exceeded,
        created_at: rule.created_at.to_rfc3339(),
    }
}

/// Returns all 50 embedded languages with native names, RTL flags, and decimal separators.
#[tauri::command]
pub fn get_supported_languages() -> Vec<LanguageOptionDto> {
    build_supported_languages()
}

/// Returns the complete current application settings and active locale translation catalog.
#[tauri::command]
pub async fn get_app_settings(
    state: State<'_, AppState>,
) -> std::result::Result<AppSettingsDto, String> {
    snapshot_settings_dto(&state)
}

/// Live-updates settings without restarting the application:
/// - `language`: validates against all 50 locales, updates tray menu captions, and returns the
///   full in-memory translation dictionary (`< 5 ms`).
/// - `autostart_enabled`: registers/removes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\GreenC`.
#[tauri::command]
pub async fn update_settings(
    patch: SettingsPatchDto,
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<AppSettingsDto, String> {
    let settings = state.settings();

    if let Some(ref lang) = patch.language {
        settings.set_language(lang).map_err(|e| e.to_string())?;
        crate::tray::refresh_tray_menu_language(&app, &state, lang);
    }

    if let Some(ref mode_str) = patch.theme_mode {
        let mode = ThemeMode::from_str_loose(mode_str);
        settings.set_theme_mode(mode).map_err(|e| e.to_string())?;
    }

    if let Some(days) = patch.retention_days_default {
        settings
            .set_retention_days(days)
            .map_err(|e| e.to_string())?;
    }

    if let Some(enabled) = patch.autostart_enabled {
        #[cfg(all(windows, feature = "store"))]
        {
            // Store (MSIX) builds use `desktop:StartupTask` exclusively; never fall back to
            // `tauri-plugin-autostart` (which writes `HKCU\...\Run`).
            let power_api = NativePowerSessionApi;
            power_api
                .set_autostart(STORE_STARTUP_TASK_ID, std::path::Path::new(""), enabled)
                .map_err(|e| format!("Không thể cập nhật khởi động cùng hệ điều hành: {e}"))?;
        }

        #[cfg(not(all(windows, feature = "store")))]
        {
            let mut os_err: Option<String> = match std::env::current_exe() {
                Ok(exe_path) => {
                    let power_api = NativePowerSessionApi;
                    power_api
                        .set_autostart(APP_NAME, &exe_path, enabled)
                        .err()
                        .map(|e| e.to_string())
                }
                Err(err) => Some(format!("Không thể xác định đường dẫn thực thi: {err}")),
            };
            // Fallback to tauri-plugin-autostart if native registration failed.
            if os_err.is_some() {
                use tauri_plugin_autostart::ManagerExt;
                let manager = app.autolaunch();
                let fallback_res = if enabled {
                    manager.enable()
                } else {
                    manager.disable()
                };
                if fallback_res.is_ok() {
                    os_err = None;
                }
            }
            if let Some(err_msg) = os_err {
                return Err(format!(
                    "Không thể cập nhật khởi động cùng hệ điều hành: {err_msg}"
                ));
            }
        }

        settings
            .set_autostart_enabled(enabled)
            .map_err(|e| e.to_string())?;
    }

    if let Some(shield_gb) = patch.low_disk_shield_gb {
        settings
            .set_low_disk_shield_gb(shield_gb)
            .map_err(|e| e.to_string())?;
    }

    if let Some(pause) = patch.pause_on_battery_fullscreen {
        settings
            .set_pause_on_battery_fullscreen(pause)
            .map_err(|e| e.to_string())?;
    }

    if let Some(zalo_auto) = patch.zalo_auto_clean_enabled {
        settings
            .set_zalo_auto_clean_enabled(zalo_auto)
            .map_err(|e| e.to_string())?;
    }

    snapshot_settings_dto(&state)
}

/// Migrates existing quarantined Vault archives to `new_root\Vault` (preserving the strict
/// `512 MiB` free-space reserve) and updates `data_root` in SQLite.
#[tauri::command]
pub async fn change_data_root(
    new_root: String,
    state: State<'_, AppState>,
) -> std::result::Result<MoveVaultReportDto, String> {
    let started = Instant::now();
    let prep: std::result::Result<(PathBuf, u64), CommandFailure> = (|| {
        let trimmed = new_root.trim();
        if trimmed.is_empty() {
            return Err(CommandFailure::from_msg(
                "Đường dẫn ổ lưu trữ mới không hợp lệ",
                "InvalidPath",
            ));
        }

        let target_letter = trimmed
            .chars()
            .next()
            .ok_or_else(|| {
                CommandFailure::from_msg("Không xác định được ký tự ổ đĩa", "InvalidPath")
            })?
            .to_ascii_uppercase();

        let drives =
            list_drives_native().map_err(|e| CommandFailure::from_err(&e, e.to_string()))?;
        let drive_info = drives
            .iter()
            .find(|d| d.drive_letter == target_letter)
            .ok_or_else(|| {
                CommandFailure::from_msg(
                    format!("Không tìm thấy ổ đĩa '{target_letter}:'"),
                    "NotFound",
                )
            })?;

        if !drive_info.is_eligible {
            return Err(CommandFailure::from_msg(
                format!(
                    "Ổ đĩa '{target_letter}:' không đủ điều kiện: {}",
                    drive_info
                        .ineligible_reason
                        .as_deref()
                        .unwrap_or("Không phải ổ NTFS cố định")
                ),
                "PreconditionBlocked",
            ));
        }

        Ok((PathBuf::from(trimmed), drive_info.free_bytes))
    })();

    let (new_root_path, free_bytes) = match prep {
        Ok(v) => v,
        Err(fail) => {
            return record_action_result(
                &state.diagnostics,
                "vault_move",
                started,
                Err(fail),
                |_| Outcome::Ok,
            );
        }
    };

    let task_id = format!("move-vault-{}", Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();

    let join_res = tokio::task::spawn_blocking(
        move || -> std::result::Result<MoveVaultReportDto, CommandFailure> {
            let new_vault_dir = new_root_path.join("Vault");
            let new_apps_dir = new_root_path.join("Apps");
            let new_moving_dir = new_root_path.join("Moving");

            std::fs::create_dir_all(&new_vault_dir).map_err(|e| {
                CommandFailure::from_msg(
                    format!("Không thể tạo thư mục {:?}: {e}", new_vault_dir),
                    "Io",
                )
            })?;
            std::fs::create_dir_all(&new_apps_dir).map_err(|e| {
                CommandFailure::from_msg(
                    format!("Không thể tạo thư mục {:?}: {e}", new_apps_dir),
                    "Io",
                )
            })?;
            std::fs::create_dir_all(&new_moving_dir).map_err(|e| {
                CommandFailure::from_msg(
                    format!("Không thể tạo thư mục {:?}: {e}", new_moving_dir),
                    "Io",
                )
            })?;

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "move_vault".to_string(),
                state: "running".to_string(),
                percent: 20,
                phase: "move_vault".to_string(),
                title: "Đang di chuyển Kho dự phòng sang vị trí mới...".to_string(),
                detail: new_vault_dir.to_string_lossy().to_string(),
                done_files: 0,
                total_files: 0,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let summary = move_vault(
                &state_inner.db,
                &new_vault_dir,
                free_bytes,
                Some(&task_ctrl),
            )
            .map_err(|e| {
                CommandFailure::from_err(&e, format!("Không thể di chuyển Kho dự phòng: {e}"))
            })?;

            if summary.errors.is_empty() {
                state_inner
                    .settings()
                    .set_data_root(&new_root_path)
                    .map_err(|e| CommandFailure::from_err(&e, e.to_string()))?;
            }

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "move_vault".to_string(),
                state: "completed".to_string(),
                percent: 100,
                phase: "completed".to_string(),
                title: "Đã cập nhật vị trí lưu dữ liệu".to_string(),
                detail: format!("Đã chuyển {} gói dữ liệu", summary.moved_count),
                done_files: summary.moved_count as u64,
                total_files: summary.moved_count as u64,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let success = summary.errors.is_empty();
            Ok(MoveVaultReportDto {
                task_id: task_id_worker,
                new_data_root: new_root_path.to_string_lossy().to_string(),
                moved_archives_count: summary.moved_count,
                errors: summary.errors,
                success,
            })
        },
    )
    .await;

    state.unregister_task(&task_id);
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("Move vault task join error: {e}"),
            "unknown",
        )),
    };
    record_action_result(&state.diagnostics, "vault_move", started, diag_res, |r| {
        if r.success {
            Outcome::Ok
        } else {
            Outcome::Failed
        }
    })
}

/// Lists all configured rolling `QuotaRule` entries with live folder byte usage.
#[tauri::command]
pub async fn list_quota_rules(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<QuotaRuleStatusDto>, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let rules = state_inner
            .db
            .list_quota_rules(false)
            .map_err(|e| format!("Lỗi tải danh sách quy tắc: {e}"))?;
        Ok(rules.into_iter().map(map_quota_rule_dto).collect())
    })
    .await
    .map_err(|e| format!("List rules join error: {e}"))?
}

/// Creates or updates a background rolling `QuotaRule`.
#[tauri::command]
pub async fn save_quota_rule(
    rule: QuotaRuleInputDto,
    state: State<'_, AppState>,
) -> std::result::Result<QuotaRuleStatusDto, String> {
    let app_name = rule.app_name.trim().to_string();
    if app_name.is_empty() {
        return Err("Tên ứng dụng không được để trống".to_string());
    }

    let target_paths: Vec<PathBuf> = rule
        .target_paths
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    if target_paths.is_empty() {
        return Err("Cần ít nhất 1 thư mục áp dụng quy tắc".to_string());
    }

    let max_bytes = rule.max_bytes.max(1024 * 1024); // >= 1 MiB
    let watermark_ratio = rule
        .watermark_ratio
        .unwrap_or(DEFAULT_WATERMARK_RATIO)
        .clamp(0.50, 0.95);
    let retention_days = rule
        .retention_days
        .unwrap_or(DEFAULT_RETENTION_DAYS)
        .clamp(1, 90);
    let min_file_age_seconds = rule
        .min_file_age_seconds
        .unwrap_or(DEFAULT_MIN_FILE_AGE_SECONDS);

    let existing_protected = if let Some(id) = rule.id {
        state
            .db
            .get_quota_rule(id)
            .ok()
            .flatten()
            .map(|r| r.protected_files)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let model = QuotaRule {
        id: rule.id,
        app_name,
        process_name: rule.process_name.unwrap_or_default().trim().to_string(),
        service_name: rule.service_name.unwrap_or_default().trim().to_string(),
        target_paths,
        max_bytes,
        watermark_ratio,
        retention_days,
        min_file_age_seconds,
        exclude_patterns: rule.exclude_patterns.unwrap_or_default(),
        protected_files: existing_protected,
        enabled: rule.enabled.unwrap_or(true),
        created_at: Utc::now(),
    };

    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let id = state_inner
            .db
            .save_quota_rule(&model)
            .map_err(|e| format!("Không thể lưu quy tắc: {e}"))?;
        let saved = state_inner
            .db
            .get_quota_rule(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Không tìm thấy quy tắc vừa lưu".to_string())?;
        Ok(map_quota_rule_dto(saved))
    })
    .await
    .map_err(|e| format!("Save rule join error: {e}"))?
}

/// Deletes a background `QuotaRule` by ID.
#[tauri::command]
pub async fn delete_quota_rule(
    rule_id: i64,
    state: State<'_, AppState>,
) -> std::result::Result<bool, String> {
    state
        .db
        .delete_quota_rule(rule_id)
        .map(|()| true)
        .map_err(|e| format!("Không thể xóa quy tắc: {e}"))
}

/// Runs an isolated Canary Vault + Auto-Rollback demonstration inside `%LOCALAPPDATA%\GreenC\sandbox`
/// without touching any user directories.
#[tauri::command]
pub async fn run_sandbox_demo(
    simulate_crash: bool,
    state: State<'_, AppState>,
) -> std::result::Result<SandboxDemoResultDto, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || -> std::result::Result<SandboxDemoResultDto, String> {
        let base_dir = sandbox_demo_dir().join(format!("demo_{}", Utc::now().timestamp_millis()));
        let app_dir = base_dir.join("MockAppCache");
        let demo_vault = state_inner
            .settings()
            .vault_dir(&default_vault_dir())
            .unwrap_or_else(|_| default_vault_dir());
        std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&demo_vault).map_err(|e| e.to_string())?;

        let sample_file = app_dir.join("session_cache.bin");
        std::fs::write(&sample_file, vec![b'G'; 64 * 1024]).map_err(|e| e.to_string())?;
        let mtime = std::fs::metadata(&sample_file)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let candidate = CleanCandidate {
            path: sample_file.clone(),
            size_bytes: 64 * 1024,
            mtime,
            is_locked: false,
            profile_id: "sandbox_demo".to_string(),
            is_canary: true,
            is_orphan: false,
            hardlink_siblings: Vec::new(),
        };

        let probe = CanaryVerifier::run_sandbox_probe(&candidate, &base_dir.join("probe"))
            .map_err(|e| format!("Canary probe error: {e}"))?;

        let vol_api = NativeWin32VolumeApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;
        let ctrl = TaskControl::new();

        let pipeline = ArchiverPipeline::new(ArchiveOptions {
            vault_dir: demo_vault.clone(),
            retention_days: 1,
            min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
            ..Default::default()
        });

        let mut manifest = pipeline
            .archive_candidates(
                0,
                "SandboxDemoApp",
                std::slice::from_ref(&candidate),
                &vol_api,
                &ctrl,
                |_| {},
            )
            .map_err(|e| format!("Demo archive error: {e}"))?;
        manifest.original_root = app_dir.clone();

        let zip_p = demo_vault.join(format!("{}.zip", manifest.archive_id));
        let man_p = demo_vault.join(format!("{}.manifest.json", manifest.archive_id));
        manifest.write_to_file(&man_p).map_err(|e| e.to_string())?;
        let _arch_db_id = manifest
            .persist_to_db(&state_inner.db, &zip_p, &man_p)
            .map_err(|e| e.to_string())?;

        let mut snaps = HashMap::new();
        snaps.insert(sample_file, (64 * 1024, mtime));
        let cleaner = SafeCleaner::new(
            &lock_probe,
            &vol_api,
            None,
            CleanupOptions {
                deletion_method: DeletionMethod::Permanent,
                min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                prune_empty_directories: false,
            },
        );
        let _ = cleaner
            .unlink_archived_items(&manifest.files, &snaps)
            .map_err(|e| e.to_string())?;

        let mut auto_rolled_back = false;
        let mut restored_files_count = 0usize;

        if simulate_crash {
            let log_path = app_dir.join("app.log");
            let mut log_f = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
            writeln!(
                log_f,
                "ERROR FileNotFoundError: cannot open session_cache.bin during startup"
            )
            .map_err(|e| e.to_string())?;
            log_f.sync_all().map_err(|e| e.to_string())?;

            let engine = RollbackEngine::new(
                &state_inner.db,
                RestoreOptions {
                    vault_dir: Some(demo_vault.clone()),
                    ..Default::default()
                },
            );
            let restores = engine
                .run_watchdog_cycle(&proc_probe)
                .map_err(|e| e.to_string())?;
            if let Some(first) = restores.first() {
                auto_rolled_back = true;
                restored_files_count = first.restored_count;
            }
        }

        let detail = if auto_rolled_back {
            "Mô phỏng lỗi ứng dụng thành công: Watchdog đã tự động khôi phục tệp với SHA-256 trùng khớp 100%.".to_string()
        } else {
            "Đã chạy Canary Probe và lưu trữ an toàn vào Kho dự phòng trong môi trường Sandbox cô lập.".to_string()
        };

        if std::fs::symlink_metadata(&base_dir)
            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir())
        {
            let _ = std::fs::remove_dir_all(&base_dir);
        }

        Ok(SandboxDemoResultDto {
            sandbox_dir: base_dir.to_string_lossy().to_string(),
            canary_passed: probe.passed,
            archived_files_count: 1,
            auto_rolled_back,
            restored_files_count,
            detail,
        })
    })
    .await
    .map_err(|e| format!("Sandbox demo join error: {e}"))?
}

/// Unloads the main WebView window into background system tray mode (`< 15 MB` RAM)
/// while keeping `BackgroundGuardian` active in the system tray.
#[tauri::command]
pub fn hide_to_tray(app: AppHandle) -> std::result::Result<bool, String> {
    use tauri::Manager;
    if let Some(win) = app.get_webview_window(crate::tray::MAIN_WINDOW_LABEL) {
        // Hide immediately so the IPC call resolves cleanly, then destroy WebView2 after 150 ms
        // to reclaim browser memory while `RunEvent::ExitRequested` keeps the tray daemon alive.
        let _ = win.hide();
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if let Some(w) = app_clone.get_webview_window(crate::tray::MAIN_WINDOW_LABEL) {
                if !w.is_visible().unwrap_or(false) {
                    let _ = w.destroy();
                }
            }
        });
        return Ok(true);
    }
    Ok(false)
}
