//! Overview and bootstrap Tauri v2 IPC command implementations (Task T024).

use crate::state::{AppState, TaskProgressEvent};
#[cfg(all(windows, feature = "store"))]
use greenc_core::config::STORE_STARTUP_TASK_ID;
use greenc_core::config::{
    APP_NAME, DEFAULT_RETENTION_DAYS, IS_STORE_BUILD, MICROSOFT_STORE_UPDATES_URL,
};
use greenc_core::error::Result;
use greenc_core::models::DriveInfo;
use greenc_core::task::TaskControl;
#[cfg(all(windows, feature = "store"))]
use greenc_platform::power_session::{NativePowerSessionApi, PowerSessionApi};
use greenc_platform::volume::{list_drives_native, recommend_drive};
use greenc_scanner::forecast::{predict_fullness, BurnRateForecast};
use greenc_scanner::space_lens::SpaceNode;
use greenc_scanner::walker::walk_directory_parallel;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapStateDto {
    pub app_name: String,
    pub app_version: String,
    pub language: String,
    pub is_rtl: bool,
    pub decimal_sep: String,
    pub theme_mode: String,
    pub needs_first_run: bool,
    pub drives: Vec<DriveInfo>,
    pub recommended_drive: Option<char>,
    pub current_data_root: Option<String>,
    pub retention_days: u32,
    pub autostart_enabled: bool,
    pub autostart_status: String,
    pub is_store_build: bool,
    pub store_updates_url: Option<String>,
    pub low_disk_shield_gb: u32,
    pub pause_on_battery_fullscreen: bool,
    pub translations: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimaryDataDriveDto {
    pub drive_letter: char,
    pub label: String,
    pub data_root: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReclaimableItemDto {
    pub app_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub category: String,
    pub recommendation: String,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewSummary {
    pub drive_letter: char,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub usage_ratio: f64,
    pub burn_rate_bps: i64,
    pub days_until_full: Option<u32>,
    pub days_until_90_pct: Option<u32>,
    pub health_status: String,
    pub primary_data_drive: Option<PrimaryDataDriveDto>,
    pub burn_rate_forecast: BurnRateForecast,
    pub reclaimable_bytes: u64,
    pub reclaimable_count: u32,
    pub reclaimable_proposals: Vec<ReclaimableItemDto>,
}

/// 1. get_bootstrap_state
/// Checks if first-run completed, evaluates host logical drives, detects recommended data drive,
/// and returns full bootstrap configuration.
#[tauri::command]
pub async fn get_bootstrap_state(
    state: State<'_, AppState>,
) -> std::result::Result<BootstrapStateDto, String> {
    let settings = state.settings();

    // Check first-run status
    let needs_first_run = settings.needs_first_run().map_err(|e| e.to_string())?;

    // Query host drives via Win32 volume enumeration
    let drives = list_drives_native().map_err(|e| e.to_string())?;
    let recommended_drive = recommend_drive(&drives).map(|d| d.drive_letter);

    // Localization & theme preferences
    let current_lang = settings
        .get_language()
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| greenc_i18n::format::match_system_language("en_US").to_string());
    let is_rtl = greenc_i18n::format::is_rtl(&current_lang);
    let decimal_sep = greenc_i18n::format::decimal_separator(&current_lang).to_string();
    let theme_mode = settings
        .get_theme_mode()
        .map_err(|e| e.to_string())?
        .as_str()
        .to_string();

    let current_data_root = settings
        .get_data_root()
        .map_err(|e| e.to_string())?
        .map(|p| p.to_string_lossy().to_string());
    let retention_days = settings.get_retention_days().map_err(|e| e.to_string())?;
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
    let translations = state.catalog.get_translations_for_locale(&current_lang);

    Ok(BootstrapStateDto {
        app_name: APP_NAME.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        language: current_lang,
        is_rtl,
        decimal_sep,
        theme_mode,
        needs_first_run,
        drives,
        recommended_drive,
        current_data_root,
        retention_days,
        autostart_enabled,
        autostart_status,
        is_store_build: IS_STORE_BUILD,
        store_updates_url: IS_STORE_BUILD.then(|| MICROSOFT_STORE_UPDATES_URL.to_string()),
        low_disk_shield_gb,
        pause_on_battery_fullscreen,
        translations,
    })
}

/// 2. complete_first_run
/// Persists the selected primary data drive and retention period, initializes target storage
/// directories ({data_root}\Vault, {data_root}\Apps, {data_root}\Moving), and unblocks normal app operation.
#[tauri::command]
pub async fn complete_first_run(
    state: State<'_, AppState>,
    selected_drive: Option<char>,
    data_root: Option<String>,
    retention_days: Option<u32>,
) -> std::result::Result<(), String> {
    let drives = list_drives_native().map_err(|e| e.to_string())?;

    // Determine target drive letter
    let target_drive_letter = if let Some(c) = selected_drive {
        c.to_ascii_uppercase()
    } else if let Some(ref path_str) = data_root {
        path_str
            .chars()
            .next()
            .ok_or_else(|| "Empty data_root path provided".to_string())?
            .to_ascii_uppercase()
    } else {
        recommend_drive(&drives)
            .map(|d| d.drive_letter)
            .unwrap_or('C')
    };

    // Validate target drive eligibility
    let drive_info = drives
        .iter()
        .find(|d| d.drive_letter == target_drive_letter)
        .ok_or_else(|| format!("Drive '{}:' not found", target_drive_letter))?;

    if !drive_info.is_eligible {
        return Err(format!(
            "Drive '{}:' is not eligible: {}",
            target_drive_letter,
            drive_info
                .ineligible_reason
                .as_deref()
                .unwrap_or("Unsupported volume format")
        ));
    }

    let root_path = if let Some(ref custom_root) = data_root {
        PathBuf::from(custom_root)
    } else if cfg!(windows) {
        PathBuf::from(format!("{}:\\GreenC", target_drive_letter))
    } else if drive_info.is_system {
        greenc_core::config::app_home()
    } else {
        drive_info.root.join(greenc_core::config::DATA_FOLDER_NAME)
    };

    // Initialize required subdirectories with error handling
    let vault_dir = root_path.join("Vault");
    let apps_dir = root_path.join("Apps");
    let moving_dir = root_path.join("Moving");

    std::fs::create_dir_all(&root_path).map_err(|e| {
        format!(
            "Failed to create data root directory {:?}: {}",
            root_path, e
        )
    })?;
    std::fs::create_dir_all(&vault_dir)
        .map_err(|e| format!("Failed to create Vault directory {:?}: {}", vault_dir, e))?;
    std::fs::create_dir_all(&apps_dir)
        .map_err(|e| format!("Failed to create Apps directory {:?}: {}", apps_dir, e))?;
    std::fs::create_dir_all(&moving_dir).map_err(|e| {
        format!(
            "Failed to create Moving temporary directory {:?}: {}",
            moving_dir, e
        )
    })?;

    // Persist settings in SQLite WAL
    let settings = state.settings();
    settings
        .set_data_root(&root_path)
        .map_err(|e| e.to_string())?;
    let days = retention_days.unwrap_or(DEFAULT_RETENTION_DAYS);
    settings
        .set_retention_days(days)
        .map_err(|e| e.to_string())?;

    tracing::info!(
        "First-run setup successfully completed. Data root: {:?}, retention: {} days",
        root_path,
        days
    );

    Ok(())
}

/// 3. get_overview_summary
/// Returns system drive usage stats, 14-day exponential/linear burn-rate forecast,
/// health status, primary data drive metadata, and reclaimable proposals.
#[tauri::command]
pub async fn get_overview_summary(
    state: State<'_, AppState>,
    drive: Option<char>,
) -> std::result::Result<OverviewSummary, String> {
    // Get current live disk capacity
    let drives = list_drives_native().map_err(|e| e.to_string())?;
    let drive_info = if let Some(letter) = drive {
        let target = letter.to_ascii_uppercase();
        drives
            .iter()
            .find(|d| d.drive_letter == target)
            .or_else(|| drives.iter().find(|d| d.is_system))
            .ok_or_else(|| format!("Drive '{}' not found", target))?
    } else {
        drives
            .iter()
            .find(|d| d.is_system)
            .or_else(|| drives.first())
            .ok_or_else(|| "No drives detected".to_string())?
    };
    let root_str = drive_info.root.to_string_lossy().into_owned();

    let total_bytes = drive_info.total_bytes;
    let free_bytes = drive_info.free_bytes;
    let used_bytes = drive_info.used_bytes();
    let usage_ratio = drive_info.usage_ratio();

    // Query 14-day snapshots from database
    let snapshots = state
        .db
        .get_daily_snapshots(&root_str, 14)
        .unwrap_or_default();

    // Calculate burn-rate forecast using the scanner's forecast engine
    let forecast = predict_fullness(&snapshots, free_bytes, total_bytes);

    // Determine health status:
    // "critical" if used >= 90% or days_until_full <= 14
    // "warning" if used >= 80% or days_until_full <= 30
    // "healthy" otherwise
    let health_status = if forecast.is_critical || usage_ratio >= 0.90 {
        "critical".to_string()
    } else if usage_ratio >= 0.80 || forecast.days_until_full.is_some_and(|d| d <= 30) {
        "warning".to_string()
    } else {
        "healthy".to_string()
    };

    // Primary data drive info
    let settings = state.settings();
    let primary_data_drive = if let Ok(Some(data_root)) = settings.get_data_root() {
        let p_drive_letter = data_root
            .to_string_lossy()
            .chars()
            .next()
            .unwrap_or('C')
            .to_ascii_uppercase();
        let p_drive_info = drives.iter().find(|d| d.drive_letter == p_drive_letter);
        Some(PrimaryDataDriveDto {
            drive_letter: p_drive_letter,
            label: p_drive_info.map(|d| d.label.clone()).unwrap_or_default(),
            data_root: data_root.to_string_lossy().to_string(),
            free_bytes: p_drive_info.map(|d| d.free_bytes).unwrap_or(0),
            total_bytes: p_drive_info.map(|d| d.total_bytes).unwrap_or(0),
        })
    } else {
        None
    };

    Ok(OverviewSummary {
        drive_letter: drive_info.drive_letter,
        total_bytes,
        free_bytes,
        used_bytes,
        usage_ratio,
        burn_rate_bps: forecast.daily_net_bps,
        days_until_full: forecast.days_until_full,
        days_until_90_pct: forecast.days_until_90_pct,
        health_status,
        primary_data_drive,
        burn_rate_forecast: forecast,
        reclaimable_bytes: 0,
        reclaimable_count: 0,
        reclaimable_proposals: Vec::new(),
    })
}

/// 4. scan_space_lens
/// Triggers parallel directory walker on the target drive, throttles real-time progress events
/// onto channel `greenc://task-progress` at <= 100ms, and builds the hierarchical SpaceNode tree.
#[tauri::command]
pub async fn scan_space_lens(
    state: State<'_, AppState>,
    app: AppHandle,
    drive: char,
    max_depth: Option<u8>,
) -> std::result::Result<SpaceNode, String> {
    let drive_letter = drive.to_ascii_uppercase();
    let drives_list = list_drives_native().unwrap_or_default();
    let matched_drive = drives_list.iter().find(|d| d.drive_letter == drive_letter);
    let root_path = if let Some(d) = matched_drive {
        d.root.to_string_lossy().into_owned()
    } else if cfg!(windows) {
        format!("{}:\\", drive_letter)
    } else {
        "/".to_string()
    };
    let task_id = format!(
        "space-lens-{}-{}",
        drive_letter,
        chrono::Utc::now().timestamp_millis()
    );

    let task_control = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_control));

    // Query host logical drives to estimate total used bytes for percentage calculation
    let total_used_bytes = matched_drive
        .map(|d| d.total_bytes.saturating_sub(d.free_bytes))
        .unwrap_or(0);

    // Progress throttle controller (<= 100ms interval)
    let last_emit = Arc::new(parking_lot::Mutex::new(
        Instant::now() - Duration::from_secs(1),
    ));
    let app_handle_clone = app.clone();
    let task_id_clone = task_id.clone();

    let emit_progress = {
        let last_emit = Arc::clone(&last_emit);
        let app_handle = app_handle_clone.clone();
        let task_id = task_id_clone.clone();
        Arc::new(
            move |percent: u32,
                  phase: &str,
                  title: &str,
                  detail: &str,
                  done_files: u64,
                  done_bytes: u64,
                  force: bool| {
                let mut guard = last_emit.lock();
                let now = Instant::now();
                if force || now.duration_since(*guard) >= Duration::from_millis(100) {
                    *guard = now;
                    let event = TaskProgressEvent {
                        task_id: task_id.clone(),
                        kind: "space_lens".to_string(),
                        state: "running".to_string(),
                        percent,
                        phase: phase.to_string(),
                        title: title.to_string(),
                        detail: detail.to_string(),
                        done_files,
                        total_files: 0,
                        done_bytes,
                        total_bytes: total_used_bytes,
                        throughput_bps: 0,
                        eta_seconds: 0,
                    };
                    let _ = app_handle.emit("greenc://task-progress", &event);
                }
            },
        )
    };

    // Emit initial phase event immediately
    emit_progress(
        0,
        "init",
        "Khởi tạo quét Space Lens",
        &root_path,
        0,
        0,
        true,
    );

    // Register progress reporting callback onto task_control
    {
        let emit_cb = Arc::clone(&emit_progress);
        task_control.set_walk_progress_callback(Arc::new(
            move |current_path: &Path, files: u64, bytes: u64| {
                let percent = if total_used_bytes > 0 {
                    ((bytes as f64 / total_used_bytes as f64) * 100.0).clamp(1.0, 99.0) as u32
                } else {
                    0
                };
                let path_str = current_path.to_string_lossy();
                emit_cb(
                    percent,
                    "scanning",
                    "Đang phân tích không gian đĩa...",
                    &path_str,
                    files,
                    bytes,
                    false, // Throttled to <= 100ms
                );
            },
        ));
    }

    let depth = max_depth.unwrap_or(4);
    let root_path_clone = root_path.clone();
    let task_ctrl_clone = Arc::clone(&task_control);

    let scan_result = tokio::task::spawn_blocking(move || -> Result<SpaceNode> {
        walk_directory_parallel(Path::new(&root_path_clone), &task_ctrl_clone, depth)
    })
    .await
    .map_err(|e| format!("Scan task failed: {}", e))?;

    // Unregister task
    state.unregister_task(&task_id);

    match scan_result {
        Ok(node) => {
            let final_event = TaskProgressEvent {
                task_id,
                kind: "space_lens".to_string(),
                state: "completed".to_string(),
                percent: 100,
                phase: "completed".to_string(),
                title: "Hoàn tất quét Space Lens".to_string(),
                detail: format!("Đã quét {} tệp ({})", node.file_count, node.physical_bytes),
                done_files: node.file_count,
                total_files: node.file_count,
                done_bytes: node.physical_bytes,
                total_bytes: node.physical_bytes,
                throughput_bps: 0,
                eta_seconds: 0,
            };
            let _ = app.emit("greenc://task-progress", &final_event);
            Ok(node)
        }
        Err(e) => {
            let error_event = TaskProgressEvent {
                task_id,
                kind: "space_lens".to_string(),
                state: "failed".to_string(),
                percent: 0,
                phase: "failed".to_string(),
                title: "Quét Space Lens thất bại".to_string(),
                detail: e.to_string(),
                done_files: 0,
                total_files: 0,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            };
            let _ = app.emit("greenc://task-progress", &error_event);
            Err(e.to_string())
        }
    }
}

/// 5. reveal_in_explorer
/// Safely reveals the target file or directory inside the OS file manager
/// (Windows Explorer, macOS Finder, or Linux xdg-open).
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> std::result::Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let clean_path = path.trim().replace('/', "\\");
        let target = Path::new(&clean_path);
        let mut cmd = std::process::Command::new("explorer.exe");
        if target.is_file() {
            cmd.arg(format!("/select,{}", clean_path));
        } else if target.is_dir() {
            cmd.arg(&clean_path);
        } else if let Some(parent) = target.parent() {
            if parent.exists() {
                cmd.arg(parent.to_string_lossy().to_string());
            } else {
                return Err(format!(
                    "Target path and parent directory do not exist: {}",
                    clean_path
                ));
            }
        } else {
            return Err(format!("Target path does not exist: {}", clean_path));
        }
        cmd.spawn()
            .map_err(|e| format!("Failed to spawn explorer: {}", e))?;
        Ok(true)
    }

    #[cfg(target_os = "macos")]
    {
        let clean_path = path.trim();
        let target = Path::new(clean_path);
        let mut cmd = std::process::Command::new("open");
        if target.is_file() {
            cmd.args(["-R", clean_path]);
        } else if target.is_dir() {
            cmd.arg(clean_path);
        } else if let Some(parent) = target.parent().filter(|p| p.exists()) {
            cmd.arg(parent);
        } else {
            return Err(format!("Target path does not exist: {}", clean_path));
        }
        cmd.spawn()
            .map_err(|e| format!("Failed to spawn Finder: {}", e))?;
        Ok(true)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let clean_path = path.trim();
        let target = Path::new(clean_path);
        let open_target = if target.is_dir() {
            target.to_path_buf()
        } else if let Some(parent) = target.parent().filter(|p| p.exists()) {
            parent.to_path_buf()
        } else {
            return Err(format!("Target path does not exist: {}", clean_path));
        };
        std::process::Command::new("xdg-open")
            .arg(&open_target)
            .spawn()
            .map_err(|e| format!("Failed to spawn xdg-open: {}", e))?;
        Ok(true)
    }
}
