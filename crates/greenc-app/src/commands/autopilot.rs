//! One-click "Make Green Now" end-to-end autopilot orchestrator (`make_green_now`).
//!
//! Pipeline:
//! 1. `analyze`  — measure C: before, auto-select backup drive (or respect single-drive mode).
//! 2. `clean`    — per-app Vault backup (or direct regenerable cache delete on single-drive PC) + Zalo safe cache clean.
//! 3. `compact`  — WOF XPRESS8K in-place compression on eligible cold C: folders not queued for Phase 4.
//! 4. `relocate` — curated safe auto-move (`!is_advanced` + non-OneDrive Known Folders) to backup drive, skipping locked apps.
//! 5. `done`     — reopen user-confirmed closed apps, measure C: after, persist savings & snapshot, return report.

use crate::state::{AppState, TaskProgressEvent};
use chrono::Utc;
use greenc_core::config::{default_vault_dir, MIN_VAULT_RESERVE_BYTES};
use greenc_core::models::{DailySnapshot, RegenerableCacheMode};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{AppLifecycleApi, AppShutdownSession, NativeAppLifecycleApi};
use greenc_platform::traits::{
    NativeCompactionApi, NativeFileLockProbe, NativeProcessServiceProbe, NativeReparsePointApi,
    NativeShellFolderApi, NativeWin32VolumeApi, ProcessServiceProbe, Win32VolumeApi, WofAlgorithm,
};
use greenc_platform::volume::{list_drives_native, recommend_drive};
use greenc_relocate::autopilot::{
    filter_non_overlapping_compaction_dirs, safe_profiles_for_mode,
    select_safe_relocation_candidates, LockedAppSummary,
};
use greenc_relocate::candidate::CandidateScanner;
use greenc_relocate::compress::InPlaceCompactor;
use greenc_relocate::known_folders::KnownFolderService;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::zalo_move::{is_zalo_auto_move_supported, ZaloMoveOptions, ZaloRelocator};
use greenc_scanner::zalo::{
    discover_zalo_installation, ZaloDiscoveryOptions, ZALO_PROCESS_NAMES,
};
use greenc_storage::SavingsEventInput;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::profiles::{group_candidates_by_app, PresetProfiles};
use greenc_vault::zalo_cleanup::{execute_zalo_cleanup, ZaloCleanupOptions};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tauri::State;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MakeGreenOptionsDto {
    pub target_drive: Option<String>,
    /// `"direct_cache"` or `"local_vault"` when no secondary backup drive exists.
    pub single_drive_mode: Option<String>,
    /// Optional app IDs from a previous report's `skipped_locked` explicitly confirmed for close & finish.
    #[serde(default)]
    pub close_app_ids: Vec<String>,
    /// Must be `true` to allow closing running/tray-resident apps in `close_app_ids`.
    pub confirm_close_running_apps: Option<bool>,
    /// Whether to include non-OneDrive Known Folders in auto-move (default `true`).
    pub include_known_folders: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeGreenActionItemDto {
    pub name: String,
    pub action_kind: String, // "clean" | "compact" | "relocate"
    pub bytes_freed: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeGreenReportDto {
    pub task_id: String,
    pub before_used_bytes: u64,
    pub after_used_bytes: u64,
    pub before_free_bytes: u64,
    pub after_free_bytes: u64,
    pub total_bytes: u64,
    pub cleaned_bytes: u64,
    pub compacted_bytes: u64,
    pub relocated_bytes: u64,
    pub total_freed_bytes: u64,
    pub backup_drive: Option<String>,
    pub single_drive_mode: Option<String>,
    pub actions: Vec<MakeGreenActionItemDto>,
    pub skipped_locked: Vec<LockedAppSummary>,
    pub reopened_apps: Vec<String>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

fn known_process_names_for_app_id(app_id: &str) -> Vec<String> {
    if app_id.eq_ignore_ascii_case("zalo") {
        return ZALO_PROCESS_NAMES.iter().map(|s| s.to_string()).collect();
    }
    for entry in greenc_relocate::catalog::get_app_catalog() {
        if entry.app_id.eq_ignore_ascii_case(app_id) {
            return entry.process_names;
        }
    }
    match app_id.to_lowercase().as_str() {
        "chrome" | "google chrome" => vec!["chrome.exe".to_string()],
        "edge" | "microsoft edge" => vec!["msedge.exe".to_string()],
        "brave" | "brave browser" => vec!["brave.exe".to_string()],
        "firefox" | "mozilla firefox" => vec!["firefox.exe".to_string()],
        "teams" | "microsoft teams" => vec!["ms-teams.exe".to_string(), "teams.exe".to_string()],
        "whatsapp" => vec!["whatsapp.exe".to_string()],
        _ => Vec::new(),
    }
}

/// Runs the full one-click "Make Green Now" sweep in a single background-capable task.
#[tauri::command]
pub async fn make_green_now(
    options: Option<MakeGreenOptionsDto>,
    state: State<'_, AppState>,
) -> std::result::Result<MakeGreenReportDto, String> {
    let opts = options.unwrap_or_default();
    let task_id = format!("make-green-{}", Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let vol_api = NativeWin32VolumeApi;
        let shell_api = NativeShellFolderApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;
        let reparse_api = NativeReparsePointApi;
        let compaction_api = NativeCompactionApi;
        let lifecycle = NativeAppLifecycleApi;

        let emit = |percent: u32,
                    phase: &str,
                    title: &str,
                    detail: String,
                    done_b: u64,
                    total_b: u64| {
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "make_green".to_string(),
                state: "running".to_string(),
                percent,
                phase: phase.to_string(),
                title: title.to_string(),
                detail,
                done_files: 0,
                total_files: 0,
                done_bytes: done_b,
                total_bytes: total_b,
                throughput_bps: 0,
                eta_seconds: 0,
            });
        };

        // ── Phase 1: Analyze ─────────────────────────────────────────────
        emit(
            3,
            "analyze",
            "Đang phân tích ổ đĩa và chọn ổ dự phòng...",
            "Kiểm tra dung lượng ổ C: và ổ đĩa phụ...".to_string(),
            0,
            0,
        );

        let drives = list_drives_native().unwrap_or_default();
        let sys_drive = drives
            .iter()
            .find(|d| d.is_system)
            .or_else(|| drives.first())
            .cloned();
        let sys_letter = sys_drive.as_ref().map(|d| d.drive_letter).unwrap_or('C');
        let (total_bytes, before_free_bytes) = vol_api
            .get_disk_free_space(&PathBuf::from(format!("{sys_letter}:\\")))
            .unwrap_or_else(|_| {
                sys_drive
                    .as_ref()
                    .map(|d| (d.total_bytes, d.free_bytes))
                    .unwrap_or((0, 0))
            });
        let before_used_bytes = total_bytes.saturating_sub(before_free_bytes);

        // Resolve backup drive (explicit > configured data_root > recommended fixed NTFS)
        let configured_root = state_inner.settings().get_data_root().ok().flatten();
        let configured_drive = configured_root
            .as_ref()
            .and_then(|p| p.to_string_lossy().chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| {
                drives
                    .iter()
                    .any(|d| d.drive_letter == *c && d.is_eligible && !d.is_system)
            });

        let explicit_drive = opts
            .target_drive
            .as_deref()
            .and_then(|s| s.chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| {
                drives
                    .iter()
                    .any(|d| d.drive_letter == *c && d.is_eligible && !d.is_system)
            });

        let backup_drive: Option<char> = explicit_drive
            .or(configured_drive)
            .or_else(|| recommend_drive(&drives).map(|d| d.drive_letter));

        let mut warnings: Vec<String> = Vec::new();
        let mut actions: Vec<MakeGreenActionItemDto> = Vec::new();
        let mut skipped_locked: Vec<LockedAppSummary> = Vec::new();
        let mut closed_sessions: Vec<(String, AppShutdownSession)> = Vec::new();

        // Ensure backup data_root directories exist if we have a backup drive
        let vault_dir = if let Some(bdrv) = backup_drive {
            let root = configured_root
                .filter(|p| {
                    p.to_string_lossy()
                        .chars()
                        .next()
                        .is_some_and(|c| c.to_ascii_uppercase() == bdrv)
                })
                .unwrap_or_else(|| {
                    if cfg!(windows) {
                        PathBuf::from(format!("{bdrv}:\\GreenC"))
                    } else {
                        drives
                            .iter()
                            .find(|d| d.drive_letter == bdrv)
                            .map(|d| d.root.join(greenc_core::config::DATA_FOLDER_NAME))
                            .unwrap_or_else(greenc_core::config::app_home)
                    }
                });
            let _ = std::fs::create_dir_all(root.join("Vault"));
            let _ = std::fs::create_dir_all(root.join("Apps"));
            let _ = std::fs::create_dir_all(root.join("Moving"));
            let _ = state_inner.settings().set_data_root(&root);
            root.join("Vault")
        } else {
            state_inner
                .settings()
                .vault_dir(&default_vault_dir())
                .unwrap_or_else(|_| default_vault_dir())
        };

        // If user explicitly confirmed closing open apps from a previous report, close them now.
        let confirmed_close = opts.confirm_close_running_apps.unwrap_or(false);
        let close_set: HashSet<String> = opts
            .close_app_ids
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        if confirmed_close && !close_set.is_empty() {
            for app_id in &opts.close_app_ids {
                if app_id.eq_ignore_ascii_case("zalo")
                    && lifecycle.has_visible_window(&["ZaloCall.exe"])
                {
                    warnings.push(
                        "Zalo đang có cuộc gọi nên GreenC không đóng để tránh gián đoạn."
                            .to_string(),
                    );
                    continue;
                }
                let procs = known_process_names_for_app_id(app_id);
                if procs.is_empty() {
                    continue;
                }
                let refs: Vec<&str> = procs.iter().map(String::as_str).collect();
                emit(
                    8,
                    "analyze",
                    "Đang đóng ứng dụng đã xác nhận...",
                    app_id.clone(),
                    0,
                    0,
                );
                match lifecycle.shutdown_confirmed_apps(&refs, Duration::from_secs(10)) {
                    Ok(session) if !session.captured_exes.is_empty() => {
                        closed_sessions.push((app_id.clone(), session));
                    }
                    Ok(_) => {}
                    Err(e) => warnings.push(format!("Không thể tự đóng {app_id}: {e}")),
                }
            }
        }

        // Pre-plan Phase 4 relocation targets so Phase 3 compaction never overlaps with Phase 4.
        let include_kf = opts.include_known_folders.unwrap_or(true);
        let (planned_kf, planned_reloc, mut reloc_locked) = if let Some(bdrv) = backup_drive {
            let kf_list = if include_kf && !cfg!(feature = "store") {
                KnownFolderService::list_known_folders(bdrv, &shell_api)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|kf| !kf.is_onedrive && !kf.is_redirected && kf.size_bytes > 0)
                    .collect()
            } else {
                Vec::new()
            };
            let discovered = CandidateScanner::discover_all(
                bdrv,
                &vol_api,
                &shell_api,
                &lock_probe,
                &proc_probe,
            );
            let (ready, locked) = select_safe_relocation_candidates(discovered);
            (kf_list, ready, locked)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        skipped_locked.append(&mut reloc_locked);

        let planned_reloc_paths: Vec<PathBuf> = planned_kf
            .iter()
            .map(|k| k.current_path.clone())
            .chain(planned_reloc.iter().map(|c| c.source_path.clone()))
            .collect();

        // ── Phase 2: Clean & Archive ─────────────────────────────────────
        let mut cleaned_bytes = 0u64;
        let use_direct_cache =
            backup_drive.is_none() && opts.single_drive_mode.as_deref() == Some("direct_cache");
        let profiles =
            safe_profiles_for_mode(backup_drive.is_some(), opts.single_drive_mode.as_deref());

        if !task_ctrl.is_cancelled() {
            emit(
                12,
                "clean",
                "Đang quét rác hệ thống, trình duyệt & ứng dụng chat...",
                "Phân tích bộ nhớ đệm an toàn...".to_string(),
                0,
                0,
            );

            let mut all_clean_cands = Vec::new();
            for pid in &profiles {
                if task_ctrl.is_cancelled() {
                    break;
                }
                let found = PresetProfiles::scan_profile_candidates(pid, &task_ctrl, &lock_probe);
                all_clean_cands.extend(found);
            }

            // Separate unlocked files from locked files
            let (unlocked_cands, locked_cands): (Vec<_>, Vec<_>) =
                all_clean_cands.into_iter().partition(|c| !c.is_locked);

            if !locked_cands.is_empty() {
                for (app_name, _cat, items) in group_candidates_by_app(&locked_cands) {
                    let bytes: u64 = items.iter().map(|c| c.size_bytes).sum();
                    if bytes > 1024 * 1024 {
                        let procs = known_process_names_for_app_id(&app_name);
                        if !procs.is_empty()
                            && !skipped_locked
                                .iter()
                                .any(|s| s.app_name.eq_ignore_ascii_case(&app_name))
                        {
                            skipped_locked.push(LockedAppSummary {
                                app_id: app_name.to_lowercase().replace(' ', "_"),
                                app_name: app_name.clone(),
                                size_bytes: bytes,
                                process_names: procs,
                                reason: format!("{app_name} đang mở nên một số tệp đệm chưa dọn được"),
                            });
                        }
                    }
                }
            }

            if !unlocked_cands.is_empty() && !task_ctrl.is_cancelled() {
                let grouped = group_candidates_by_app(&unlocked_cands);
                let total_clean_bytes: u64 = unlocked_cands.iter().map(|c| c.size_bytes).sum();
                let mut done_clean_bytes = 0u64;

                let mut snapshots = HashMap::with_capacity(unlocked_cands.len());
                for c in &unlocked_cands {
                    let snap = match std::fs::metadata(&c.path) {
                        Ok(m) => (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                        Err(_) => (c.size_bytes, c.mtime),
                    };
                    snapshots.insert(c.path.clone(), snap);
                    for sib in &c.hardlink_siblings {
                        snapshots.entry(sib.clone()).or_insert(snap);
                    }
                }

                let cleaner = SafeCleaner::new(
                    &lock_probe,
                    &vol_api,
                    None,
                    CleanupOptions {
                        deletion_method: if use_direct_cache {
                            DeletionMethod::Permanent
                        } else {
                            DeletionMethod::RecycleBin
                        },
                        min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                        prune_empty_directories: true,
                    },
                );

                let pipeline = ArchiverPipeline::new(ArchiveOptions {
                    vault_dir: vault_dir.clone(),
                    min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                    ..Default::default()
                });

                for (app_name, category, group_items) in grouped {
                    if task_ctrl.is_cancelled() {
                        break;
                    }
                    let mut archive_ref = task_id_worker.clone();
                    let mut source_kind = "cleanup_direct";

                    if !use_direct_cache {
                        let st_prog = state_inner.clone();
                        let tid_clone = task_id_worker.clone();
                        let base_b = done_clean_bytes;
                        let app_label = app_name.clone();
                        match pipeline.archive_candidates(
                            0,
                            &app_name,
                            &group_items,
                            &vol_api,
                            &task_ctrl,
                            move |p| {
                                let cur = base_b.saturating_add(p.done_bytes);
                                let pct = 15
                                    + (((cur as f64 / total_clean_bytes.max(1) as f64) * 20.0)
                                        as u32);
                                st_prog.emit_progress(&TaskProgressEvent {
                                    task_id: tid_clone.clone(),
                                    kind: "make_green".to_string(),
                                    state: "running".to_string(),
                                    percent: pct.min(35),
                                    phase: "clean".to_string(),
                                    title: format!("Đang dọn & sao lưu {}", app_label),
                                    detail: p.current_file,
                                    done_files: p.done_files,
                                    total_files: p.total_files,
                                    done_bytes: cur,
                                    total_bytes: total_clean_bytes,
                                    throughput_bps: p.throughput_bps,
                                    eta_seconds: p.eta_seconds,
                                });
                            },
                        ) {
                            Ok(manifest) => {
                                let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
                                let man_p =
                                    vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
                                let _ = manifest.persist_to_db(&state_inner.db, &zip_p, &man_p);
                                archive_ref = manifest.archive_id;
                                source_kind = "cleanup_vault";
                            }
                            Err(e) => {
                                warnings.push(format!("Bỏ qua sao lưu {app_name}: {e}"));
                                continue;
                            }
                        }
                    }

                    let manifest_items: Vec<greenc_core::models::FileManifestItem> = group_items
                        .iter()
                        .enumerate()
                        .map(|(i, c)| greenc_core::models::FileManifestItem {
                            id: None,
                            archive_id: None,
                            original_path: c.path.clone(),
                            rel_arcname: format!("item_{:05}", i + 1),
                            size_bytes: c.size_bytes,
                            sha256: String::new(),
                            mtime: 0.0,
                            atime: 0.0,
                            hardlink_paths: c.hardlink_siblings.clone(),
                            restored_individually: false,
                        })
                        .collect();

                    if let Ok(clean_res) = cleaner.unlink_archived_items(&manifest_items, &snapshots)
                    {
                        if clean_res.bytes_freed > 0 {
                            cleaned_bytes = cleaned_bytes.saturating_add(clean_res.bytes_freed);
                            let now_utc = Utc::now();
                            let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                                timestamp: now_utc.to_rfc3339(),
                                event_date: now_utc.format("%Y-%m-%d").to_string(),
                                source_kind: source_kind.to_string(),
                                source_ref: archive_ref,
                                app_name: app_name.clone(),
                                category,
                                bytes_freed: clean_res.bytes_freed,
                                drive_letter: sys_letter.to_string(),
                            });
                            actions.push(MakeGreenActionItemDto {
                                name: app_name,
                                action_kind: "clean".to_string(),
                                bytes_freed: clean_res.bytes_freed,
                                detail: Some(format!("{} tệp", clean_res.files_deleted)),
                            });
                        }
                    }

                    done_clean_bytes = done_clean_bytes
                        .saturating_add(group_items.iter().map(|c| c.size_bytes).sum::<u64>());
                }
            }

            // Zalo safe cache & old-versions cleanup (only if installed)
            if !task_ctrl.is_cancelled() {
                let running_exe_dirs = lifecycle.find_running_exe_dirs(ZALO_PROCESS_NAMES);
                let zalo_running = !running_exe_dirs.is_empty()
                    || ZALO_PROCESS_NAMES
                        .iter()
                        .any(|p| proc_probe.is_process_running(p));
                let known_relocated_sources: HashSet<PathBuf> = state_inner
                    .db
                    .list_relocations(Some("done"))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.app_id == "zalo")
                    .map(|r| r.source_path)
                    .collect();

                if let Some(disc) = discover_zalo_installation(&ZaloDiscoveryOptions {
                    running_exe_dirs,
                    known_relocated_sources,
                    ..Default::default()
                }) {
                    let retention_days = state_inner.settings().get_retention_days().unwrap_or(14);
                    let zmode = if use_direct_cache {
                        RegenerableCacheMode::Direct
                    } else {
                        RegenerableCacheMode::Vault
                    };
                    if let Ok(zrep) = execute_zalo_cleanup(
                        &disc,
                        &vault_dir,
                        &state_inner.db,
                        &vol_api,
                        &lock_probe,
                        zalo_running,
                        &ZaloCleanupOptions {
                            dry_run: false,
                            mode: zmode,
                            retention_days,
                            ..Default::default()
                        },
                        &task_ctrl,
                    ) {
                        if zrep.logical_freed_bytes > 0 {
                            cleaned_bytes =
                                cleaned_bytes.saturating_add(zrep.logical_freed_bytes);
                            let now_utc = Utc::now();
                            let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                                timestamp: now_utc.to_rfc3339(),
                                event_date: now_utc.format("%Y-%m-%d").to_string(),
                                source_kind: if zrep.vault_archive_id.is_some() {
                                    "cleanup_vault".to_string()
                                } else {
                                    "cleanup_direct".to_string()
                                },
                                source_ref: zrep
                                    .vault_archive_id
                                    .unwrap_or_else(|| task_id_worker.clone()),
                                app_name: "Zalo PC".to_string(),
                                category: "chat".to_string(),
                                bytes_freed: zrep.logical_freed_bytes,
                                drive_letter: sys_letter.to_string(),
                            });
                            actions.push(MakeGreenActionItemDto {
                                name: "Zalo PC".to_string(),
                                action_kind: "clean".to_string(),
                                bytes_freed: zrep.logical_freed_bytes,
                                detail: Some("Bộ đệm & phiên bản cũ".to_string()),
                            });
                        }
                    }
                }
            }
        }

        // ── Phase 3: In-place WOF Compaction (non-overlapping with Phase 4) ──
        let mut compacted_bytes = 0u64;
        if !task_ctrl.is_cancelled() {
            emit(
                42,
                "compact",
                "Đang nén gọn thư mục hệ thống ít dùng...",
                "Rà soát các thư mục có thể nén không mất dữ liệu...".to_string(),
                0,
                0,
            );
            let dirs = filter_non_overlapping_compaction_dirs(
                InPlaceCompactor::discover_directories_for_compaction(),
                &planned_reloc_paths,
            );
            let total_dirs = dirs.len().max(1);
            for (idx, d) in dirs.into_iter().enumerate() {
                if task_ctrl.is_cancelled() {
                    break;
                }
                let dir_name = d
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Folder")
                    .to_string();
                let pct = 44 + (((idx * 18) / total_dirs) as u32);
                emit(
                    pct.min(62),
                    "compact",
                    "Đang nén gọn thư mục...",
                    dir_name.clone(),
                    0,
                    d.logical_bytes,
                );
                if let Ok(rec) = InPlaceCompactor::compact_directory(
                    &d.path,
                    WofAlgorithm::Xpress8k,
                    &state_inner.db,
                    &compaction_api,
                    &lock_probe,
                    &task_ctrl,
                    |_, _| {},
                ) {
                    let saved = rec
                        .physical_before_bytes
                        .saturating_sub(rec.physical_after_bytes);
                    if saved > 0 {
                        compacted_bytes = compacted_bytes.saturating_add(saved);
                        let now_utc = Utc::now();
                        let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                            timestamp: now_utc.to_rfc3339(),
                            event_date: now_utc.format("%Y-%m-%d").to_string(),
                            source_kind: "compact".to_string(),
                            source_ref: task_id_worker.clone(),
                            app_name: dir_name.clone(),
                            category: "compact".to_string(),
                            bytes_freed: saved,
                            drive_letter: sys_letter.to_string(),
                        });
                        actions.push(MakeGreenActionItemDto {
                            name: dir_name,
                            action_kind: "compact".to_string(),
                            bytes_freed: saved,
                            detail: Some("Nén gọn tại chỗ".to_string()),
                        });
                    }
                }
            }
        }

        // ── Phase 4: Safe Curated Auto-Move to Backup Drive ──────────────
        let mut relocated_bytes = 0u64;
        if let Some(bdrv) = backup_drive {
            if !task_ctrl.is_cancelled() {
                let total_move_bytes: u64 = planned_kf
                    .iter()
                    .map(|k| k.size_bytes)
                    .chain(planned_reloc.iter().map(|c| c.size_bytes))
                    .fold(0u64, u64::saturating_add);
                let mut moved_so_far = 0u64;

                // 4a. Non-OneDrive Known Folders (non-Store builds)
                #[cfg(not(feature = "store"))]
                for kf in planned_kf {
                    if task_ctrl.is_cancelled() {
                        break;
                    }
                    let st_prog = state_inner.clone();
                    let tid = task_id_worker.clone();
                    let kf_name = kf.name.clone();
                    let base_b = moved_so_far;
                    let mut last_emit = Instant::now()
                        .checked_sub(Duration::from_millis(200))
                        .unwrap_or_else(Instant::now);

                    match KnownFolderService::relocate_known_folder_with_file(
                        &kf.folder_id,
                        bdrv,
                        None,
                        &state_inner.db,
                        &shell_api,
                        &vol_api,
                        &task_ctrl,
                        move |done_b, done_f, rel_file| {
                            if last_emit.elapsed() < Duration::from_millis(100) {
                                return;
                            }
                            last_emit = Instant::now();
                            let cur = base_b.saturating_add(done_b);
                            let pct = 65
                                + (((cur as f64 / total_move_bytes.max(1) as f64) * 28.0) as u32);
                            st_prog.emit_progress(&TaskProgressEvent {
                                task_id: tid.clone(),
                                kind: "make_green".to_string(),
                                state: "running".to_string(),
                                percent: pct.min(93),
                                phase: "relocate".to_string(),
                                title: format!("Đang chuyển {} sang {}:\\", kf_name, bdrv),
                                detail: rel_file.to_string(),
                                done_files: done_f,
                                total_files: kf.file_count,
                                done_bytes: cur,
                                total_bytes: total_move_bytes,
                                throughput_bps: 0,
                                eta_seconds: 0,
                            });
                        },
                    ) {
                        Ok(rec) => {
                            relocated_bytes = relocated_bytes.saturating_add(rec.size_bytes);
                            if rec.size_bytes > 0 {
                                let now_utc = Utc::now();
                                let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                                    timestamp: now_utc.to_rfc3339(),
                                    event_date: now_utc.format("%Y-%m-%d").to_string(),
                                    source_kind: "relocate".to_string(),
                                    source_ref: rec.id.map(|i| i.to_string()).unwrap_or_default(),
                                    app_name: rec.app_name.clone(),
                                    category: "relocate".to_string(),
                                    bytes_freed: rec.size_bytes,
                                    drive_letter: sys_letter.to_string(),
                                });
                                actions.push(MakeGreenActionItemDto {
                                    name: kf.name,
                                    action_kind: "relocate".to_string(),
                                    bytes_freed: rec.size_bytes,
                                    detail: Some(format!("Chuyển sang {}:\\", bdrv)),
                                });
                            }
                        }
                        Err(e) => warnings.push(format!("Bỏ qua chuyển {}: {e}", kf.name)),
                    }
                    moved_so_far = moved_so_far.saturating_add(kf.size_bytes);
                }
                #[cfg(feature = "store")]
                {
                    let _ = planned_kf;
                    warnings.push(
                        "Bản Microsoft Store giới hạn di chuyển thư mục hệ thống & AppData do hộp cát MSIX."
                            .to_string(),
                    );
                }

                // 4b. Curated unlocked app & dev tool folders
                for cand in planned_reloc {
                    if task_ctrl.is_cancelled() {
                        break;
                    }
                    let st_prog = state_inner.clone();
                    let tid = task_id_worker.clone();
                    let cand_name = cand.name.clone();
                    let base_b = moved_so_far;
                    let mut last_emit = Instant::now()
                        .checked_sub(Duration::from_millis(200))
                        .unwrap_or_else(Instant::now);

                    match TransactionalMover::relocate_junction_with_file(
                        &cand,
                        MoveOptions {
                            target_drive: bdrv,
                            custom_target_path: None,
                        },
                        &state_inner.db,
                        &vol_api,
                        &reparse_api,
                        &proc_probe,
                        &shell_api,
                        &task_ctrl,
                        move |done_b, done_f, rel_file| {
                            if last_emit.elapsed() < Duration::from_millis(100) {
                                return;
                            }
                            last_emit = Instant::now();
                            let cur = base_b.saturating_add(done_b);
                            let pct = 65
                                + (((cur as f64 / total_move_bytes.max(1) as f64) * 28.0) as u32);
                            st_prog.emit_progress(&TaskProgressEvent {
                                task_id: tid.clone(),
                                kind: "make_green".to_string(),
                                state: "running".to_string(),
                                percent: pct.min(93),
                                phase: "relocate".to_string(),
                                title: format!("Đang chuyển {} sang {}:\\", cand_name, bdrv),
                                detail: rel_file.to_string(),
                                done_files: done_f,
                                total_files: cand.file_count,
                                done_bytes: cur,
                                total_bytes: total_move_bytes,
                                throughput_bps: 0,
                                eta_seconds: 0,
                            });
                        },
                    ) {
                        Ok(rec) => {
                            relocated_bytes = relocated_bytes.saturating_add(rec.size_bytes);
                            if rec.size_bytes > 0 {
                                let now_utc = Utc::now();
                                let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                                    timestamp: now_utc.to_rfc3339(),
                                    event_date: now_utc.format("%Y-%m-%d").to_string(),
                                    source_kind: "relocate".to_string(),
                                    source_ref: rec.id.map(|i| i.to_string()).unwrap_or_default(),
                                    app_name: rec.app_name.clone(),
                                    category: "relocate".to_string(),
                                    bytes_freed: rec.size_bytes,
                                    drive_letter: sys_letter.to_string(),
                                });
                                actions.push(MakeGreenActionItemDto {
                                    name: cand.name,
                                    action_kind: "relocate".to_string(),
                                    bytes_freed: rec.size_bytes,
                                    detail: Some(format!("Chuyển sang {}:\\", bdrv)),
                                });
                            }
                        }
                        Err(e) => warnings.push(format!("Bỏ qua chuyển {}: {e}", cand.name)),
                    }
                    moved_so_far = moved_so_far.saturating_add(cand.size_bytes);
                }

                // 4c. Zalo PC chat data (ONLY if installed, on C:, and either NOT running or confirmed in close_app_ids)
                if !task_ctrl.is_cancelled() && is_zalo_auto_move_supported() {
                    let running_exe_dirs = lifecycle.find_running_exe_dirs(ZALO_PROCESS_NAMES);
                    let zalo_running = !running_exe_dirs.is_empty()
                        || ZALO_PROCESS_NAMES
                            .iter()
                            .any(|p| proc_probe.is_process_running(p));
                    let known_relocated_sources: HashSet<PathBuf> = state_inner
                        .db
                        .list_relocations(Some("done"))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|r| r.app_id == "zalo")
                        .map(|r| r.source_path)
                        .collect();

                    if let Some(disc) = discover_zalo_installation(&ZaloDiscoveryOptions {
                        running_exe_dirs,
                        known_relocated_sources,
                        ..Default::default()
                    }) {
                        if disc.data_dir.is_some()
                            && disc.data_on_system_drive
                            && !disc.data_is_known_relocation
                            && disc.chat_data_bytes > 0
                        {
                            let allow_zalo_move = !zalo_running
                                || (confirmed_close && close_set.contains("zalo"));
                            if allow_zalo_move {
                                let st_prog = state_inner.clone();
                                let tid = task_id_worker.clone();
                                match ZaloRelocator::move_zalo_data(
                                    &disc,
                                    &ZaloMoveOptions {
                                        target_drive: bdrv,
                                        reopen_zalo: confirmed_close && close_set.contains("zalo"),
                                        ..Default::default()
                                    },
                                    &state_inner.db,
                                    &vol_api,
                                    &reparse_api,
                                    &shell_api,
                                    &lock_probe,
                                    &proc_probe,
                                    &lifecycle,
                                    &task_ctrl,
                                    move |p| {
                                        st_prog.emit_progress(&TaskProgressEvent {
                                            task_id: tid.clone(),
                                            kind: "make_green".to_string(),
                                            state: "running".to_string(),
                                            percent: 90 + (p.percent / 20).min(5),
                                            phase: "relocate".to_string(),
                                            title: format!("Đang chuyển Zalo PC sang {}:\\", bdrv),
                                            detail: p.detail,
                                            done_files: p.done_files,
                                            total_files: p.total_files,
                                            done_bytes: p.done_bytes,
                                            total_bytes: p.total_bytes,
                                            throughput_bps: 0,
                                            eta_seconds: 0,
                                        });
                                    },
                                ) {
                                    Ok(rep) if !rep.already_on_other_drive && rep.moved_bytes > 0 => {
                                        relocated_bytes =
                                            relocated_bytes.saturating_add(rep.moved_bytes);
                                        let now_utc = Utc::now();
                                        let _ = state_inner.db.record_savings_event(
                                            &SavingsEventInput {
                                                timestamp: now_utc.to_rfc3339(),
                                                event_date: now_utc.format("%Y-%m-%d").to_string(),
                                                source_kind: "relocate".to_string(),
                                                source_ref: rep
                                                    .record_id
                                                    .map(|i| i.to_string())
                                                    .unwrap_or_default(),
                                                app_name: "Zalo PC".to_string(),
                                                category: "chat".to_string(),
                                                bytes_freed: rep.moved_bytes,
                                                drive_letter: sys_letter.to_string(),
                                            },
                                        );
                                        actions.push(MakeGreenActionItemDto {
                                            name: "Zalo PC".to_string(),
                                            action_kind: "relocate".to_string(),
                                            bytes_freed: rep.moved_bytes,
                                            detail: Some(format!("Chuyển sang {}:\\", bdrv)),
                                        });
                                    }
                                    Ok(_) => {}
                                    Err(e) => warnings.push(format!("Bỏ qua chuyển Zalo PC: {e}")),
                                }
                            } else {
                                skipped_locked.push(LockedAppSummary {
                                    app_id: "zalo".to_string(),
                                    app_name: "Zalo PC".to_string(),
                                    size_bytes: disc.chat_data_bytes,
                                    process_names: ZALO_PROCESS_NAMES
                                        .iter()
                                        .map(|s| s.to_string())
                                        .collect(),
                                    reason: "Zalo PC đang mở nên tạm bỏ qua".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // ── Phase 5: Reopen confirmed closed apps & Finalize ─────────────
        let mut reopened_apps = Vec::new();
        for (app_id, mut session) in closed_sessions {
            if lifecycle.restart_app(&mut session).unwrap_or(false) {
                reopened_apps.push(app_id);
            }
        }

        let (_, after_free_bytes) = vol_api
            .get_disk_free_space(&PathBuf::from(format!("{sys_letter}:\\")))
            .unwrap_or((total_bytes, before_free_bytes));
        let after_used_bytes = total_bytes.saturating_sub(after_free_bytes);
        let computed_total = cleaned_bytes
            .saturating_add(compacted_bytes)
            .saturating_add(relocated_bytes);
        let os_delta = after_free_bytes.saturating_sub(before_free_bytes);
        let total_freed_bytes = computed_total.max(os_delta);

        // Update daily snapshot so Overview & History reflect the new C: state immediately.
        let now_utc = Utc::now();
        let _ = state_inner.db.upsert_daily_snapshot(&DailySnapshot {
            id: None,
            drive_letter: sys_letter.to_string(),
            snapshot_date: now_utc.format("%Y-%m-%d").to_string(),
            total_bytes,
            used_bytes: after_used_bytes,
            free_bytes: after_free_bytes,
            net_change_bytes: -(total_freed_bytes as i64),
            reclaimed_bytes: total_freed_bytes,
            updated_at: now_utc,
        });

        let cancelled = task_ctrl.is_cancelled();
        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "make_green".to_string(),
            state: if cancelled { "cancelled" } else { "completed" }.to_string(),
            percent: 100,
            phase: "done".to_string(),
            title: "Đã hoàn tất làm xanh ổ đĩa!".to_string(),
            detail: format!("Hoàn tất {} tác vụ", actions.len()),
            done_files: actions.len() as u64,
            total_files: actions.len() as u64,
            done_bytes: total_freed_bytes,
            total_bytes: total_freed_bytes,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        Ok(MakeGreenReportDto {
            task_id: task_id_worker,
            before_used_bytes,
            after_used_bytes,
            before_free_bytes,
            after_free_bytes,
            total_bytes,
            cleaned_bytes,
            compacted_bytes,
            relocated_bytes,
            total_freed_bytes,
            backup_drive: backup_drive.map(|c| c.to_string()),
            single_drive_mode: opts.single_drive_mode,
            actions,
            skipped_locked,
            reopened_apps,
            warnings,
            cancelled,
        })
    })
    .await
    .map_err(|e| format!("Make green now join error: {e}"))?;

    state.unregister_task(&task_id);
    result
}
