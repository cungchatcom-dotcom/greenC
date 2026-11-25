//! Tauri v2 IPC commands for Zalo PC status, safe cache/old-version cleanup, and
//! automatic chat-data relocation.

use crate::state::{AppState, TaskProgressEvent};
use greenc_core::config::default_vault_dir;
use greenc_core::models::{RegenerableCacheMode, RelocationStatus};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{AppLifecycleApi, NativeAppLifecycleApi};
use greenc_platform::traits::{
    NativeFileLockProbe, NativeProcessServiceProbe, NativeReparsePointApi, NativeShellFolderApi,
    NativeWin32VolumeApi, ProcessServiceProbe, ReparsePointApi,
};
use greenc_platform::volume::{list_drives_native, recommend_drive_info};
use greenc_relocate::recovery::RecoveryService;
use greenc_relocate::zalo_move::{
    discover_zalo_safety_copies, estimate_zalo_move_space, format_zalo_move_confirmation,
    is_zalo_auto_move_supported, zalo_auto_move_unsupported_reason, ZaloDeleteSafetyCopyOptions,
    ZaloMoveOptions, ZaloRelocator, ZaloSafetyCopyInfo, ZaloSpaceEstimate,
};
use greenc_scanner::zalo::{
    discover_zalo_installation, ZaloConfigStatus, ZaloDiscoveryOptions, ZALO_CACHE_MIN_AGE_SECS,
    ZALO_OLD_VERSION_MIN_AGE_SECS, ZALO_PROCESS_NAMES,
};
use greenc_vault::zalo_cleanup::{
    execute_zalo_cleanup, scan_zalo_cleanup_units, ZaloCleanupOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloSafetyCopyDto {
    pub copy_id: String,
    pub id: Option<i64>,
    pub relocation_id: Option<i64>,
    pub kind: String,
    pub path: String,
    pub source_path: Option<String>,
    pub target_path: Option<String>,
    pub size_bytes: u64,
    pub file_count: u64,
    pub created_at: Option<String>,
    pub purge_after: Option<String>,
    pub health_verified: bool,
    pub status: String,
    pub exists_on_disk: bool,
    pub restorable: bool,
    pub detail: Option<String>,
}

impl From<ZaloSafetyCopyInfo> for ZaloSafetyCopyDto {
    fn from(info: ZaloSafetyCopyInfo) -> Self {
        Self {
            copy_id: info.copy_id,
            id: info.id,
            relocation_id: info.relocation_id,
            kind: info.kind,
            path: info.path.to_string_lossy().to_string(),
            source_path: info.source_path.map(|p| p.to_string_lossy().to_string()),
            target_path: info.target_path.map(|p| p.to_string_lossy().to_string()),
            size_bytes: info.size_bytes,
            file_count: info.file_count,
            created_at: info.created_at.map(|d| d.to_rfc3339()),
            purge_after: info.purge_after.map(|d| d.to_rfc3339()),
            health_verified: info.health_verified,
            status: info.status,
            exists_on_disk: info.exists_on_disk,
            restorable: info.restorable,
            detail: info.detail,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloStatusDto {
    pub installed: bool,
    pub config_status: String,
    pub data_dir: Option<String>,
    pub data_drive: Option<String>,
    pub data_on_system_drive: bool,
    pub already_on_other_drive: bool,
    pub already_on_other_drive_message: Option<String>,
    pub active_relocation_id: Option<i64>,
    pub active_relocation_target: Option<String>,
    pub active_relocation_health: Option<String>,
    pub active_relocation_status: Option<String>,
    /// `true` when an active Zalo relocation exists in DB, but `source_path` is no longer a
    /// junction/reparse point and was replaced by a real directory (e.g. Zalo reinstalled).
    pub source_replaced_by_real_dir: bool,
    pub chat_data_bytes: u64,
    pub media_bytes: u64,
    pub message_bytes: u64,
    pub cache_bytes: u64,
    pub cache_reclaimable_bytes: u64,
    pub cache_locked_bytes: u64,
    pub old_versions_bytes: u64,
    pub old_versions_count: usize,
    pub total_reclaimable_bytes: u64,
    pub total_locked_bytes: u64,
    pub zalo_running: bool,
    pub locked_notice: Option<String>,
    pub auto_move_supported: bool,
    pub auto_move_unsupported_reason: Option<String>,
    pub recommended_drive: Option<String>,
    pub move_confirmation_text: Option<String>,
    pub retained_original_bytes: u64,
    pub safety_copies: Vec<ZaloSafetyCopyDto>,
    pub space_estimate: Option<ZaloSpaceEstimate>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloCleanupResultDto {
    pub dry_run: bool,
    pub mode: String,
    pub cleaned_units: usize,
    pub cleaned_files: u64,
    pub logical_freed_bytes: u64,
    pub vault_bytes: u64,
    pub net_freed_bytes: i64,
    pub vault_same_drive: bool,
    pub vault_archive_id: Option<String>,
    pub locked_skipped_files: u64,
    pub locked_skipped_bytes: u64,
    pub zalo_running: bool,
    pub locked_notice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloMoveResultDto {
    pub record_id: Option<i64>,
    pub dry_run: bool,
    pub already_on_other_drive: bool,
    pub source_path: String,
    pub target_path: String,
    pub target_drive: String,
    pub moved_bytes: u64,
    pub moved_files: u64,
    pub message_files_verified: u64,
    pub media_files_verified: u64,
    pub reopened_zalo: bool,
    pub health_verified: bool,
    pub auto_rolled_back: bool,
    pub retained_original_id: Option<i64>,
    pub retained_original_path: Option<String>,
    pub retained_original_bytes: u64,
    pub retained_purge_after: Option<String>,
    pub status: String,
    pub message: String,
}

struct ActiveZaloRelocation {
    id: i64,
    target_path: PathBuf,
    health: String,
    status: String,
    replaced_by_real_dir: bool,
}

fn known_zalo_relocations(state: &AppState) -> (HashSet<PathBuf>, Option<ActiveZaloRelocation>) {
    let mut known_sources = HashSet::new();
    let mut active = None;
    if let Ok(records) = state.db.list_relocations(None) {
        let reparse = NativeReparsePointApi;
        let healths = RecoveryService::check_junction_health(&records, &reparse);
        for (rec, health) in records.into_iter().zip(healths) {
            if rec.app_id == "zalo"
                && matches!(
                    rec.status,
                    RelocationStatus::Done
                        | RelocationStatus::NeedsReview
                        | RelocationStatus::Moving
                )
            {
                // Item 6: Only mark `source_path` as a known relocation when it is
                // ACTUALLY a reparse point on disk right now!
                let source_is_reparse = reparse.is_reparse_point(&rec.source_path);
                let replaced_by_real_dir = matches!(
                    rec.status,
                    RelocationStatus::Done | RelocationStatus::NeedsReview
                ) && !source_is_reparse
                    && std::fs::symlink_metadata(&rec.source_path)
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
                if source_is_reparse
                    && matches!(
                        rec.status,
                        RelocationStatus::Done | RelocationStatus::NeedsReview
                    )
                {
                    known_sources.insert(rec.source_path.clone());
                }
                if active.is_none() {
                    if let Some(id) = rec.id {
                        active = Some(ActiveZaloRelocation {
                            id,
                            target_path: rec.target_path,
                            health: health.as_str().to_string(),
                            status: rec.status.as_str().to_string(),
                            replaced_by_real_dir,
                        });
                    }
                }
            }
        }
    }
    (known_sources, active)
}

fn detect_zalo_running(
    lifecycle: &NativeAppLifecycleApi,
    proc_probe: &NativeProcessServiceProbe,
) -> (bool, HashSet<PathBuf>) {
    let exe_dirs = lifecycle.find_running_exe_dirs(ZALO_PROCESS_NAMES);
    let running = !exe_dirs.is_empty()
        || ZALO_PROCESS_NAMES
            .iter()
            .any(|p| proc_probe.is_process_running(p));
    (running, exe_dirs)
}

/// Inspects Zalo PC chat data, regenerable cache, obsolete versions, and relocation readiness.
#[tauri::command]
pub async fn get_zalo_status(
    target_drive: Option<String>,
    state: State<'_, AppState>,
) -> std::result::Result<ZaloStatusDto, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(recs) =
            RecoveryService::recover_interrupted(&state_inner.db, &NativeReparsePointApi)
        {
            if !recs.is_empty() {
                state_inner.push_startup_recoveries(recs);
            }
        }
        let lifecycle = NativeAppLifecycleApi;
        let proc_probe = NativeProcessServiceProbe;
        let lock_probe = NativeFileLockProbe;
        let (zalo_running, running_exe_dirs) = detect_zalo_running(&lifecycle, &proc_probe);
        let (known_relocated_sources, active_reloc) = known_zalo_relocations(&state_inner);

        let now = SystemTime::now();
        let opts = ZaloDiscoveryOptions {
            running_exe_dirs,
            known_relocated_sources,
            now,
            ..Default::default()
        };

        let drives = list_drives_native().unwrap_or_default();
        let rec_drive = target_drive
            .as_deref()
            .and_then(|s| s.chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .or_else(|| recommend_drive_info(&drives).map(|d| d.drive_letter));

        let auto_move_supported = is_zalo_auto_move_supported();
        let unsupported_reason = if auto_move_supported {
            None
        } else {
            Some(zalo_auto_move_unsupported_reason())
        };

        let disc_opt = discover_zalo_installation(&opts);
        let safety_copies_raw = discover_zalo_safety_copies(
            disc_opt.as_ref().and_then(|d| d.data_dir.as_deref()),
            &state_inner.db,
        );
        let retained_original_bytes: u64 = safety_copies_raw
            .iter()
            .filter(|c| c.exists_on_disk && c.kind == "original")
            .map(|c| c.size_bytes)
            .fold(0u64, u64::saturating_add);
        let safety_copies: Vec<ZaloSafetyCopyDto> =
            safety_copies_raw.into_iter().map(Into::into).collect();

        let Some(disc) = disc_opt else {
            return Ok(ZaloStatusDto {
                installed: false,
                config_status: "missing".to_string(),
                data_dir: None,
                data_drive: None,
                data_on_system_drive: false,
                already_on_other_drive: false,
                already_on_other_drive_message: None,
                active_relocation_id: active_reloc.as_ref().map(|a| a.id),
                active_relocation_target: active_reloc
                    .as_ref()
                    .map(|a| a.target_path.to_string_lossy().to_string()),
                active_relocation_health: active_reloc.as_ref().map(|a| a.health.clone()),
                active_relocation_status: active_reloc.as_ref().map(|a| a.status.clone()),
                source_replaced_by_real_dir: active_reloc
                    .as_ref()
                    .is_some_and(|a| a.replaced_by_real_dir),
                chat_data_bytes: 0,
                media_bytes: 0,
                message_bytes: 0,
                cache_bytes: 0,
                cache_reclaimable_bytes: 0,
                cache_locked_bytes: 0,
                old_versions_bytes: 0,
                old_versions_count: 0,
                total_reclaimable_bytes: 0,
                total_locked_bytes: 0,
                zalo_running,
                locked_notice: None,
                auto_move_supported,
                auto_move_unsupported_reason: unsupported_reason,
                recommended_drive: rec_drive.map(|c| c.to_string()),
                move_confirmation_text: None,
                retained_original_bytes,
                safety_copies,
                space_estimate: None,
                warnings: Vec::new(),
            });
        };

        let ctrl = TaskControl::new();
        let scan = scan_zalo_cleanup_units(
            &disc,
            now,
            Duration::from_secs(ZALO_CACHE_MIN_AGE_SECS),
            Duration::from_secs(ZALO_OLD_VERSION_MIN_AGE_SECS),
            &lock_probe,
            zalo_running,
            &ctrl,
        );

        let already_on_other_drive =
            disc.data_dir.is_some() && !disc.data_on_system_drive && !disc.data_is_known_relocation;
        let already_msg = if already_on_other_drive {
            let drv = disc
                .data_drive
                .clone()
                .unwrap_or_else(|| "khác".to_string());
            Some(format!("Đã ở ổ {drv}, không cần chuyển"))
        } else {
            None
        };

        let config_status = match disc.config_status {
            ZaloConfigStatus::Found => "found",
            ZaloConfigStatus::Missing => "missing",
            ZaloConfigStatus::Invalid { .. } => "invalid",
            ZaloConfigStatus::RejectedUnsafe { .. } => "rejected_unsafe",
        }
        .to_string();

        let confirm_text = rec_drive
            .filter(|_| disc.data_dir.is_some() && disc.data_on_system_drive)
            .map(|drv| format_zalo_move_confirmation(disc.chat_data_bytes, drv));

        let vol_api = NativeWin32VolumeApi;
        let space_estimate = if disc.data_dir.is_some() && disc.data_on_system_drive {
            let src_letter = disc
                .data_drive
                .as_deref()
                .and_then(|s| s.chars().next())
                .filter(|c| c.is_ascii_alphabetic())
                .unwrap_or('C');
            rec_drive.and_then(|drv| {
                estimate_zalo_move_space(src_letter, drv, disc.chat_data_bytes, &vol_api).ok()
            })
        } else {
            None
        };

        Ok(ZaloStatusDto {
            installed: disc.installed,
            config_status,
            data_dir: disc.data_dir.map(|p| p.to_string_lossy().to_string()),
            data_drive: disc.data_drive,
            data_on_system_drive: disc.data_on_system_drive,
            already_on_other_drive,
            already_on_other_drive_message: already_msg,
            active_relocation_id: active_reloc.as_ref().map(|a| a.id),
            active_relocation_target: active_reloc
                .as_ref()
                .map(|a| a.target_path.to_string_lossy().to_string()),
            active_relocation_health: active_reloc.as_ref().map(|a| a.health.clone()),
            active_relocation_status: active_reloc.as_ref().map(|a| a.status.clone()),
            source_replaced_by_real_dir: active_reloc
                .as_ref()
                .is_some_and(|a| a.replaced_by_real_dir),
            chat_data_bytes: disc.chat_data_bytes,
            media_bytes: disc.media_bytes,
            message_bytes: disc.message_bytes,
            cache_bytes: disc.cache_bytes,
            cache_reclaimable_bytes: scan.cache_reclaimable_bytes,
            cache_locked_bytes: scan.cache_locked_bytes,
            old_versions_bytes: disc.old_versions_bytes,
            old_versions_count: disc.old_version_dirs.len(),
            total_reclaimable_bytes: scan.total_reclaimable_bytes,
            total_locked_bytes: scan.total_locked_bytes,
            zalo_running,
            locked_notice: scan.locked_notice,
            auto_move_supported,
            auto_move_unsupported_reason: unsupported_reason,
            recommended_drive: rec_drive.map(|c| c.to_string()),
            move_confirmation_text: confirm_text,
            retained_original_bytes,
            safety_copies,
            space_estimate,
            warnings: disc.warnings,
        })
    })
    .await
    .map_err(|e| format!("Zalo status join error: {e}"))?
}

/// Safely cleans allowlisted Zalo caches and obsolete `Zalo-x.y.z` versions.
#[tauri::command]
pub async fn clean_zalo_now(
    enable_vault: Option<bool>,
    state: State<'_, AppState>,
) -> std::result::Result<ZaloCleanupResultDto, String> {
    let task_id = format!("zalo-clean-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();

    let res = tokio::task::spawn_blocking(move || {
        let lifecycle = NativeAppLifecycleApi;
        let proc_probe = NativeProcessServiceProbe;
        let lock_probe = NativeFileLockProbe;
        let vol_api = NativeWin32VolumeApi;
        let (zalo_running, running_exe_dirs) = detect_zalo_running(&lifecycle, &proc_probe);
        let (known_relocated_sources, _) = known_zalo_relocations(&state_inner);

        let now = SystemTime::now();
        let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
            running_exe_dirs,
            known_relocated_sources,
            now,
            ..Default::default()
        })
        .ok_or_else(|| "Không tìm thấy dữ liệu Zalo PC trên máy".to_string())?;

        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "zalo_clean".to_string(),
            state: "running".to_string(),
            percent: 20,
            phase: "cleaning".to_string(),
            title: "Đang dọn bộ đệm và bản cũ Zalo...".to_string(),
            detail: "Giữ nguyên tin nhắn, ảnh và đăng nhập".to_string(),
            done_files: 0,
            total_files: 0,
            done_bytes: 0,
            total_bytes: 0,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        let vault_dir = state_inner
            .settings()
            .vault_dir(&default_vault_dir())
            .unwrap_or_else(|_| default_vault_dir());
        let retention_days = state_inner.settings().get_retention_days().unwrap_or(14);
        let mode = if enable_vault.unwrap_or(true) {
            RegenerableCacheMode::Vault
        } else {
            RegenerableCacheMode::Direct
        };

        let report = execute_zalo_cleanup(
            &disc,
            &vault_dir,
            &state_inner.db,
            &vol_api,
            &lock_probe,
            zalo_running,
            &ZaloCleanupOptions {
                dry_run: false,
                mode,
                retention_days,
                now,
                ..Default::default()
            },
            &task_ctrl,
        )
        .map_err(|e| format!("Không dọn được Zalo: {e}"))?;

        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker,
            kind: "zalo_clean".to_string(),
            state: "completed".to_string(),
            percent: 100,
            phase: "done".to_string(),
            title: "Đã dọn xong Zalo".to_string(),
            detail: report.locked_notice.clone().unwrap_or_default(),
            done_files: report.cleaned_files,
            total_files: report.cleaned_files,
            done_bytes: report.logical_freed_bytes,
            total_bytes: report.logical_freed_bytes,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        Ok(ZaloCleanupResultDto {
            dry_run: report.dry_run,
            mode: report.mode.as_str().to_string(),
            cleaned_units: report.cleaned_units,
            cleaned_files: report.cleaned_files,
            logical_freed_bytes: report.logical_freed_bytes,
            vault_bytes: report.vault_bytes,
            net_freed_bytes: report.net_freed_bytes,
            vault_same_drive: report.vault_same_drive,
            vault_archive_id: report.vault_archive_id,
            locked_skipped_files: report.locked_skipped_files,
            locked_skipped_bytes: report.locked_skipped_bytes,
            zalo_running: report.zalo_running,
            locked_notice: report.locked_notice,
        })
    })
    .await
    .map_err(|e| format!("Zalo clean join error: {e}"))?;

    state.unregister_task(&task_id);
    res
}

/// Automatically moves Zalo chat data (`<ROOT>\Zalo Data`) to another fixed NTFS drive.
#[tauri::command]
pub async fn move_zalo_data(
    target_drive: String,
    reopen_zalo: Option<bool>,
    state: State<'_, AppState>,
) -> std::result::Result<ZaloMoveResultDto, String> {
    let task_id = format!("zalo-move-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();

    let res = tokio::task::spawn_blocking(move || {
        let drive_char = {
            let trimmed = target_drive.trim();
            let bytes = trimmed.as_bytes();
            let valid_shape = match bytes.len() {
                1 => bytes[0].is_ascii_alphabetic(),
                2 => bytes[0].is_ascii_alphabetic() && bytes[1] == b':',
                3 => {
                    bytes[0].is_ascii_alphabetic()
                        && bytes[1] == b':'
                        && (bytes[2] == b'\\' || bytes[2] == b'/')
                }
                _ => false,
            };
            if valid_shape {
                Some((bytes[0] as char).to_ascii_uppercase())
            } else {
                None
            }
        }
        .ok_or_else(|| "Chưa chọn ổ đĩa đích hợp lệ".to_string())?;

        let lifecycle = NativeAppLifecycleApi;
        let proc_probe = NativeProcessServiceProbe;
        let lock_probe = NativeFileLockProbe;
        let vol_api = NativeWin32VolumeApi;
        let reparse_api = NativeReparsePointApi;
        let shell_api = NativeShellFolderApi;

        let (_, running_exe_dirs) = detect_zalo_running(&lifecycle, &proc_probe);
        let (known_relocated_sources, _) = known_zalo_relocations(&state_inner);
        let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
            running_exe_dirs,
            known_relocated_sources,
            ..Default::default()
        })
        .ok_or_else(|| "Không tìm thấy cài đặt Zalo PC".to_string())?;

        let opts = ZaloMoveOptions {
            target_drive: drive_char,
            reopen_zalo: reopen_zalo.unwrap_or(true),
            ..Default::default()
        };

        let state_cb = state_inner.clone();
        let tid_cb = task_id_worker.clone();

        let report = ZaloRelocator::move_zalo_data(
            &disc,
            &opts,
            &state_inner.db,
            &vol_api,
            &reparse_api,
            &shell_api,
            &lock_probe,
            &proc_probe,
            &lifecycle,
            &task_ctrl,
            move |p| {
                state_cb.emit_progress(&TaskProgressEvent {
                    task_id: tid_cb.clone(),
                    kind: "zalo_move".to_string(),
                    state: if p.phase == greenc_relocate::zalo_move::ZaloMovePhase::Done {
                        "completed"
                    } else {
                        "running"
                    }
                    .to_string(),
                    percent: p.percent,
                    phase: p.phase.as_str().to_string(),
                    title: p.phase.label_vi().to_string(),
                    detail: p.detail,
                    done_files: p.done_files,
                    total_files: p.total_files,
                    done_bytes: p.done_bytes,
                    total_bytes: p.total_bytes,
                    throughput_bps: 0,
                    eta_seconds: 0,
                });
            },
        )
        .map_err(|e| e.to_string())?;

        Ok(ZaloMoveResultDto {
            record_id: report.record_id,
            dry_run: report.dry_run,
            already_on_other_drive: report.already_on_other_drive,
            source_path: report.source_path.to_string_lossy().to_string(),
            target_path: report.target_path.to_string_lossy().to_string(),
            target_drive: report.target_drive,
            moved_bytes: report.moved_bytes,
            moved_files: report.moved_files,
            message_files_verified: report.message_files_verified,
            media_files_verified: report.media_files_verified,
            reopened_zalo: report.reopened_zalo,
            health_verified: report.health_verified,
            auto_rolled_back: report.auto_rolled_back,
            retained_original_id: report.retained_original_id,
            retained_original_path: report
                .retained_original_path
                .map(|p| p.to_string_lossy().to_string()),
            retained_original_bytes: report.retained_original_bytes,
            retained_purge_after: report.retained_purge_after,
            status: report.status,
            message: report.message,
        })
    })
    .await
    .map_err(|e| format!("Zalo move join error: {e}"))?;

    state.unregister_task(&task_id);
    res
}

/// Explicitly deletes a temporarily retained Zalo original copy (`.greenc-original.<uuid>`).
#[tauri::command]
pub async fn purge_zalo_retained_original(
    retained_id: i64,
    state: State<'_, AppState>,
) -> std::result::Result<String, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let reparse = NativeReparsePointApi;
        let proc_probe = NativeProcessServiceProbe;
        let rec = ZaloRelocator::purge_retained_original_with_probe(
            retained_id,
            &state_inner.db,
            &reparse,
            &proc_probe,
        )
        .map_err(|e| e.to_string())?;
        Ok(rec
            .detail
            .unwrap_or_else(|| "Đã xoá bản gốc giữ tạm thành công.".to_string()))
    })
    .await
    .map_err(|e| format!("Lỗi tác vụ xoá bản gốc Zalo: {e}"))?
}

/// Explicitly restores a temporarily retained Zalo original copy back to its original path,
/// keeping any post-move target data preserved as a conflict copy so no messages are lost.
/// Refuses when target drive is offline unless `allow_offline_target == Some(true)`.
#[tauri::command]
pub async fn restore_zalo_retained_original(
    retained_id: i64,
    allow_offline_target: Option<bool>,
    state: State<'_, AppState>,
) -> std::result::Result<String, String> {
    let state_inner = state.inner().clone();
    let allow_offline = allow_offline_target.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        let reparse = NativeReparsePointApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;
        let lifecycle = NativeAppLifecycleApi;
        let rec = ZaloRelocator::restore_retained_original_with_options(
            retained_id,
            allow_offline,
            &state_inner.db,
            &reparse,
            &lock_probe,
            &proc_probe,
            &lifecycle,
            greenc_platform::app_lifecycle::DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
        )
        .map_err(|e| e.to_string())?;
        Ok(rec
            .detail
            .unwrap_or_else(|| "Đã khôi phục bản gốc Zalo về vị trí cũ an toàn.".to_string()))
    })
    .await
    .map_err(|e| format!("Lỗi tác vụ khôi phục bản gốc Zalo: {e}"))?
}

/// Restores a discovered Zalo safety copy (`original`, `conflict`, `moving`, or reconnected
/// `target`) strictly by its opaque `copy_id` returned from `discover_zalo_safety_copies`.
/// Accepts NO filesystem paths over IPC; source and copy paths are resolved solely from DB
/// records / Zalo discovery. Refuses when linked target is offline unless confirmed.
#[tauri::command]
pub async fn restore_zalo_safety_copy(
    copy_id: String,
    allow_offline_target: Option<bool>,
    state: State<'_, AppState>,
) -> std::result::Result<String, String> {
    let state_inner = state.inner().clone();
    let allow_offline = allow_offline_target.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        let reparse = NativeReparsePointApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;
        let lifecycle = NativeAppLifecycleApi;

        let (_, running_exe_dirs) = detect_zalo_running(&lifecycle, &proc_probe);
        let (known_relocated_sources, _) = known_zalo_relocations(&state_inner);
        let discovered_data_dir = discover_zalo_installation(&ZaloDiscoveryOptions {
            running_exe_dirs,
            known_relocated_sources,
            ..Default::default()
        })
        .and_then(|d| d.data_dir);

        ZaloRelocator::restore_safety_copy_by_id_with_options(
            &copy_id,
            allow_offline,
            discovered_data_dir.as_deref(),
            &state_inner.db,
            &reparse,
            &lock_probe,
            &proc_probe,
            &lifecycle,
            greenc_platform::app_lifecycle::DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Lỗi tác vụ khôi phục bản sao Zalo: {e}"))?
}

/// Permanently deletes ANY listed Zalo safety copy (`original`, `conflict`, `moving`, `restoring`,
/// `target`, or `purging`) strictly by its opaque `copy_id` after explicit user confirmation,
/// ONLY if the currently live Zalo data directory covers every message DB in that copy.
#[tauri::command]
pub async fn delete_zalo_safety_copy(
    copy_id: String,
    confirm_copy_may_be_newer: Option<bool>,
    force_delete_confirmation_text: Option<String>,
    state: State<'_, AppState>,
) -> std::result::Result<String, String> {
    let state_inner = state.inner().clone();
    let options = ZaloDeleteSafetyCopyOptions {
        confirm_copy_may_be_newer: confirm_copy_may_be_newer.unwrap_or(false),
        force_delete_confirmation_text,
    };
    tokio::task::spawn_blocking(move || {
        let reparse = NativeReparsePointApi;
        let proc_probe = NativeProcessServiceProbe;
        let lifecycle = NativeAppLifecycleApi;

        let (_, running_exe_dirs) = detect_zalo_running(&lifecycle, &proc_probe);
        let (known_relocated_sources, _) = known_zalo_relocations(&state_inner);
        let discovered_data_dir = discover_zalo_installation(&ZaloDiscoveryOptions {
            running_exe_dirs,
            known_relocated_sources,
            ..Default::default()
        })
        .and_then(|d| d.data_dir);

        ZaloRelocator::delete_safety_copy_by_id_with_probes(
            &copy_id,
            discovered_data_dir.as_deref(),
            &options,
            &state_inner.db,
            &reparse,
            &proc_probe,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("Lỗi tác vụ xoá bản sao Zalo: {e}"))?
}
