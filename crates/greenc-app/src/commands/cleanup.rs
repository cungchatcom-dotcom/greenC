//! Safe cleanup commands and candidate evaluation (Tasks T030, T036).

use crate::commands::diagnostics::{record_action_result, record_scan_result, CommandFailure};
use crate::state::{AppState, TaskProgressEvent};
use greenc_core::config::{default_vault_dir, sandbox_demo_dir, MIN_VAULT_RESERVE_BYTES};
use greenc_core::task::TaskControl;
use greenc_diagnostics::Outcome;
use greenc_platform::handle::NativeFileLockProbe;
use greenc_platform::volume::NativeWin32VolumeApi;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::canary::CanaryVerifier;
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::profiles::PresetProfiles;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tauri::State;

/// Discovered cleanup candidate proposal presented to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupCandidateDto {
    pub id: String,
    pub profile_id: String,
    pub app_name: String,
    pub category: String,
    pub path: String,
    pub target_paths: Vec<String>,
    pub total_physical_bytes: u64,
    pub reclaimable_bytes: u64,
    pub file_count: u64,
    pub is_locked: bool,
    pub locked_file_count: u64,
    pub canary_probe_passed: bool,
    pub canary_probe_detail: String,
    pub is_orphan: bool,
    pub selected: bool,
}

/// Diagnostic item skipped during safe deletion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedItemDto {
    pub path: String,
    pub reason: String,
}

/// Final summary report returned upon completion of cleanup execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupReportDto {
    pub task_id: String,
    pub success: bool,
    pub total_candidates: u32,
    pub processed_candidates: u32,
    pub total_reclaimed_bytes: u64,
    pub total_files_deleted: u64,
    pub vault_archive_id: Option<String>,
    pub vault_archive_path: Option<String>,
    pub compressed_vault_bytes: u64,
    pub skipped_locked_files_count: u64,
    pub skipped_items: Vec<SkippedItemDto>,
    pub error_message: Option<String>,
}

/// Scans the system for cleanup candidates matching the given profile IDs.
///
/// Semantics:
/// - `None`: Parameter omitted -> defaults to the 7 safe preset profiles (excluding opt-in `orphan_radar`).
/// - `Some(vec)` if `vec.is_empty()`: Explicitly empty filter -> returns empty candidates immediately without scanning.
/// - `Some(vec)`: Scans exclusively the specified profile IDs.
#[tauri::command]
pub async fn scan_cleanup_candidates(
    profile_ids: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> std::result::Result<Vec<CleanupCandidateDto>, String> {
    let started = Instant::now();
    // Fast-path: Explicit empty profile selection returns empty result immediately
    let active_profiles = match profile_ids {
        None => vec![
            "windows_temp".to_string(),
            "browser_cache".to_string(),
            "chat_media".to_string(),
            "crash_dumps".to_string(),
            "package_managers".to_string(),
            "app_logs".to_string(),
            "old_versions".to_string(),
        ],
        Some(ids) if ids.is_empty() => {
            return record_scan_result(&state.diagnostics, "cleanup", started, Ok(Vec::new()), |_| {
                Outcome::Ok
            });
        }
        Some(ids) => ids,
    };

    let task_id = format!("cleanup-scan-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let task_ctrl_worker = Arc::clone(&task_ctrl);
    let join_res = tokio::task::spawn_blocking(move || {
        let mut candidate_dtos = Vec::new();
        let total_profiles = active_profiles.len();
        let lock_probe = NativeFileLockProbe;

        for (idx, pid) in active_profiles.iter().enumerate() {
            if task_ctrl_worker.is_cancelled() {
                break;
            }

            let percent = ((idx as f32 / total_profiles as f32) * 90.0) as u32;
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "cleanup_scan".to_string(),
                state: "running".to_string(),
                percent,
                phase: "scan".to_string(),
                title: format!(
                    "Đang quét nhóm dữ liệu {} ({}/{})",
                    pid,
                    idx + 1,
                    total_profiles
                ),
                detail: format!("Phân tích hồ sơ {}...", pid),
                done_files: idx as u64,
                total_files: total_profiles as u64,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let candidates =
                PresetProfiles::scan_profile_candidates(pid, &task_ctrl_worker, &lock_probe);
            let profile_meta = PresetProfiles::get_by_id(pid);

            if !candidates.is_empty() {
                let total_bytes: u64 = candidates.iter().map(|c| c.size_bytes).sum();
                let file_count = candidates.len() as u64;
                let locked_count = candidates.iter().filter(|c| c.is_locked).count() as u64;
                let reclaimable_bytes: u64 = candidates
                    .iter()
                    .filter(|c| !c.is_locked)
                    .map(|c| c.size_bytes)
                    .sum();

                // Run 1-cycle canary probe on first unlocked candidate <= 15 MiB
                let sandbox_dir = sandbox_demo_dir();
                let canary_candidate = candidates
                    .iter()
                    .find(|c| !c.is_locked && c.size_bytes <= 15 * 1024 * 1024);
                let (canary_passed, canary_detail) = if let Some(cand) = canary_candidate {
                    match CanaryVerifier::run_sandbox_probe(cand, &sandbox_dir) {
                        Ok(res) => (
                            res.passed,
                            if res.passed {
                                "Thử nghiệm Canary thành công, dữ liệu an toàn".to_string()
                            } else {
                                res.error_detail
                                    .unwrap_or_else(|| "Thử nghiệm Canary không đạt".to_string())
                            },
                        ),
                        Err(e) => (false, format!("Canary probe error: {}", e)),
                    }
                } else {
                    (
                        true,
                        "Không cần chạy Canary (tập tin nhỏ hoặc rỗng)".to_string(),
                    )
                };

                let primary_path = candidates[0]
                    .path
                    .parent()
                    .unwrap_or(&candidates[0].path)
                    .to_string_lossy()
                    .to_string();

                let target_paths = candidates
                    .iter()
                    .take(10)
                    .map(|c| c.path.to_string_lossy().to_string())
                    .collect();

                let app_name = profile_meta
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| pid.clone());

                let category = format!(
                    "{:?}",
                    profile_meta
                        .map(|p| p.category)
                        .unwrap_or(greenc_vault::profiles::ProfileCategory::System)
                );

                candidate_dtos.push(CleanupCandidateDto {
                    id: format!("cand_{}", pid),
                    profile_id: pid.clone(),
                    app_name,
                    category,
                    path: primary_path,
                    target_paths,
                    total_physical_bytes: total_bytes,
                    reclaimable_bytes,
                    file_count,
                    is_locked: locked_count > 0,
                    locked_file_count: locked_count,
                    canary_probe_passed: canary_passed,
                    canary_probe_detail: canary_detail,
                    is_orphan: pid == "orphan_radar",
                    selected: true,
                });
            }
        }

        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker,
            kind: "cleanup_scan".to_string(),
            state: "completed".to_string(),
            percent: 100,
            phase: "completed".to_string(),
            title: "Hoàn tất quét dọn dẹp".to_string(),
            detail: format!(
                "Tìm thấy {} nhóm ứng dụng có thể giải phóng",
                candidate_dtos.len()
            ),
            done_files: candidate_dtos.len() as u64,
            total_files: candidate_dtos.len() as u64,
            done_bytes: 0,
            total_bytes: 0,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        candidate_dtos
    })
    .await;

    state.unregister_task(&task_id);
    let cancelled = task_ctrl.is_cancelled();
    let diag_res = join_res
        .map_err(|e| CommandFailure::from_msg(format!("Task failed: {}", e), "unknown"));
    record_scan_result(&state.diagnostics, "cleanup", started, diag_res, |_| {
        if cancelled {
            Outcome::Cancelled
        } else {
            Outcome::Ok
        }
    })
}

/// Executes safe cleanup for the selected candidate groups.
#[tauri::command]
pub async fn execute_cleanup(
    candidate_ids: Vec<String>,
    enable_vault: bool,
    permanent: bool,
    state: State<'_, AppState>,
) -> std::result::Result<CleanupReportDto, String> {
    let started = Instant::now();
    if candidate_ids.is_empty() {
        return record_action_result(
            &state.diagnostics,
            "cleanup",
            started,
            Err(CommandFailure::from_msg(
                "Không có mục nào được chọn để dọn dẹp",
                "PreconditionBlocked",
            )),
            |_| Outcome::Ok,
        );
    }

    let task_id = format!("cleanup-exec-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let join_res = tokio::task::spawn_blocking(
        move || -> std::result::Result<CleanupReportDto, CommandFailure> {
        let lock_probe = NativeFileLockProbe;
        let volume_api = NativeWin32VolumeApi;

        // Resolve vault dir
        let data_root = state_inner.settings().get_data_root().ok().flatten();
        let vault_dir = data_root
            .map(|d| d.join("Vault"))
            .unwrap_or_else(default_vault_dir);

        // Gather all clean candidates for selected profile IDs
        // Re-checking each group can take a while (it walks the folders again and probes locks).
        // Report it as a "prepare" phase so the UI shows 0/N instead of a stale finished state.
        let total_groups = candidate_ids.len() as u64;
        let emit_prepare = |done: u64, detail: String| {
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "cleanup_execute".to_string(),
                state: "running".to_string(),
                percent: (done * 10 / total_groups.max(1)) as u32,
                phase: "prepare".to_string(),
                title: "Đang chuẩn bị dọn dẹp...".to_string(),
                detail,
                done_files: done,
                total_files: total_groups,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });
        };

        let mut candidates_to_clean = Vec::new();
        for (i, cid) in candidate_ids.iter().enumerate() {
            let pid = cid.trim_start_matches("cand_");
            emit_prepare(
                i as u64,
                format!("Kiểm tra lại nhóm {} ({}/{})", pid, i + 1, total_groups),
            );
            let cands = PresetProfiles::scan_profile_candidates(pid, &task_ctrl, &lock_probe);
            candidates_to_clean.extend(cands);
        }
        emit_prepare(
            total_groups,
            format!(
                "Ghi nhận trạng thái {} tệp trước khi dọn...",
                candidates_to_clean.len()
            ),
        );

        if candidates_to_clean.is_empty() {
            return Ok(CleanupReportDto {
                task_id: task_id_worker.clone(),
                success: true,
                total_candidates: candidate_ids.len() as u32,
                processed_candidates: 0,
                total_reclaimed_bytes: 0,
                total_files_deleted: 0,
                vault_archive_id: None,
                vault_archive_path: None,
                compressed_vault_bytes: 0,
                skipped_locked_files_count: 0,
                skipped_items: vec![],
                error_message: None,
            });
        }

        let total_files = candidates_to_clean.len() as u64;
        let total_bytes: u64 = candidates_to_clean.iter().map(|c| c.size_bytes).sum();

        // Step 0: Capture pre-archive metadata snapshot BEFORE archiving commences.
        // This baseline snapshot captures (len, mtime) at T0.
        // If any file is modified, written to, or touched while ArchiverPipeline is streaming
        // or during the deletion prep window, its metadata at unlinking time will mismatch this T0 snapshot,
        // and SafeCleaner will skip unlinking the file, strictly protecting against mid-flight data loss.
        let mut snapshots = HashMap::with_capacity(candidates_to_clean.len());
        for c in &candidates_to_clean {
            let snap = match std::fs::metadata(&c.path) {
                Ok(m) => (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                Err(_) => (c.size_bytes, c.mtime),
            };
            snapshots.insert(c.path.clone(), snap);
            // Defensively cover hardlink siblings under the same snapshot
            for sib in &c.hardlink_siblings {
                snapshots.entry(sib.clone()).or_insert(snap);
            }
        }

        // Step 1: Canary Vault Archiving (if enabled)
        let mut vault_archive_id = None;
        let mut vault_archive_path = None;
        let mut compressed_vault_bytes = 0u64;

        if enable_vault {
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "cleanup_execute".to_string(),
                state: "running".to_string(),
                percent: 10,
                phase: "archive".to_string(),
                title: "Đang lưu trữ vào Kho an toàn (Canary Vault)...".to_string(),
                detail: format!("Nén {} tệp tin...", total_files),
                done_files: 0,
                total_files,
                done_bytes: 0,
                total_bytes,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let arch_options = ArchiveOptions {
                vault_dir: vault_dir.clone(),
                min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                ..Default::default()
            };

            let pipeline = ArchiverPipeline::new(arch_options);
            let state_clone_for_progress = state_inner.clone();
            let task_id_clone = task_id_worker.clone();

            let manifest = pipeline
                .archive_candidates(
                    0, // Ad-hoc cleanup
                    "QuickCleanup",
                    &candidates_to_clean,
                    &volume_api,
                    &task_ctrl,
                    move |p| {
                        state_clone_for_progress.emit_progress(&TaskProgressEvent {
                            task_id: task_id_clone.clone(),
                            kind: "cleanup_execute".to_string(),
                            state: "running".to_string(),
                            percent: 10
                                + ((p.done_bytes as f64 / p.total_bytes.max(1) as f64) * 50.0)
                                    as u32,
                            phase: "archive".to_string(),
                            title: "Đang nén dữ liệu vào Kho an toàn...".to_string(),
                            detail: p.current_file,
                            done_files: p.done_files,
                            total_files: p.total_files,
                            done_bytes: p.done_bytes,
                            total_bytes: p.total_bytes,
                            throughput_bps: p.throughput_bps,
                            eta_seconds: p.eta_seconds,
                        });
                    },
                )
                .map_err(|e| CommandFailure::from_err(&e, format!("Lỗi tạo kho an toàn: {}", e)))?;

            // Persist manifest to SQLite
            let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
            let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
            let _ = manifest.persist_to_db(&state_inner.db, &zip_p, &man_p);

            vault_archive_id = Some(manifest.archive_id.clone());
            vault_archive_path = Some(zip_p.to_string_lossy().to_string());
            compressed_vault_bytes = manifest.compressed_size;
        }

        // Step 2: Two-Phase Safe Deletion
        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "cleanup_execute".to_string(),
            state: "running".to_string(),
            percent: 70,
            phase: "delete".to_string(),
            title: "Đang xóa an toàn các tệp đã lưu trữ...".to_string(),
            detail: "Kiểm tra khóa và phiên bản tệp tin trước khi giải phóng...".to_string(),
            done_files: 0,
            total_files,
            done_bytes: 0,
            total_bytes,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        // (Snapshot was already captured in Step 0 prior to archiving)

        let deletion_method = if permanent {
            DeletionMethod::Permanent
        } else {
            DeletionMethod::RecycleBin
        };

        let cleaner = SafeCleaner::new(
            &lock_probe,
            &volume_api,
            None,
            CleanupOptions {
                deletion_method,
                min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                prune_empty_directories: true,
            },
        );

        let manifest_items: Vec<greenc_core::models::FileManifestItem> = candidates_to_clean
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

        let clean_res = cleaner
            .unlink_archived_items(&manifest_items, &snapshots)
            .map_err(|e| CommandFailure::from_err(&e, format!("Lỗi xóa tệp tin: {}", e)))?;

        let skipped_dtos = clean_res
            .skipped_items
            .into_iter()
            .map(|(p, r)| SkippedItemDto {
                path: p.to_string_lossy().to_string(),
                reason: r,
            })
            .collect();

        // Final completion event
        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "cleanup_execute".to_string(),
            state: "completed".to_string(),
            percent: 100,
            phase: "completed".to_string(),
            title: "Hoàn tất dọn dẹp an toàn".to_string(),
            detail: format!(
                "Đã giải phóng {} tệp tin ({})",
                clean_res.files_deleted, clean_res.bytes_freed
            ),
            done_files: clean_res.files_deleted,
            total_files,
            done_bytes: clean_res.bytes_freed,
            total_bytes,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        Ok(CleanupReportDto {
            task_id: task_id_worker,
            success: true,
            total_candidates: candidate_ids.len() as u32,
            processed_candidates: candidate_ids.len() as u32,
            total_reclaimed_bytes: clean_res.bytes_freed,
            total_files_deleted: clean_res.files_deleted,
            vault_archive_id,
            vault_archive_path,
            compressed_vault_bytes,
            skipped_locked_files_count: clean_res.skipped_files_count,
            skipped_items: skipped_dtos,
            error_message: None,
        })
    })
    .await;

    state.unregister_task(&task_id);
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("Cleanup task error: {}", e),
            "unknown",
        )),
    };
    record_action_result(&state.diagnostics, "cleanup", started, diag_res, |r| {
        if !r.success {
            Outcome::Failed
        } else if r.skipped_locked_files_count > 0 || !r.skipped_items.is_empty() {
            Outcome::Partial
        } else {
            Outcome::Ok
        }
    })
}

/// Controls an active background task (pause, resume, cancel).
#[tauri::command]
pub async fn control_task(
    task_id: String,
    action: String,
    state: State<'_, AppState>,
) -> std::result::Result<bool, String> {
    Ok(state.control_task(&task_id, &action))
}
