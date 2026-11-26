//! Tauri v2 IPC commands for WOF CompactOS and VHDX Compaction (Tasks T051, T052).

use crate::commands::diagnostics::{record_action_result, record_scan_result, CommandFailure};
use crate::state::{AppState, TaskProgressEvent};
use greenc_core::task::TaskControl;
use greenc_diagnostics::Outcome;
use greenc_platform::traits::{
    NativeCompactionApi, NativeFileLockProbe, NativeProcessServiceProbe, WofAlgorithm,
};
use greenc_relocate::compress::InPlaceCompactor;
use greenc_relocate::vhdx::VhdxCompactor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCandidateDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: String, // "wof_directory" | "vhdx_trim"
    pub logical_bytes: u64,
    pub physical_bytes: u64,
    pub estimated_reclaimable_bytes: u64,
    pub file_count: u64,
    pub recommended_algorithm: String,
    pub is_locked: bool,
    pub locked_processes: Vec<String>,
    pub is_compacted: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResultDto {
    pub task_id: String,
    pub success: bool,
    pub kind: String,
    pub target_path: String,
    pub algorithm: String,
    pub files_processed: u64,
    pub files_compacted: u64,
    pub files_skipped: u64,
    pub logical_bytes: u64,
    pub physical_before_bytes: u64,
    pub physical_after_bytes: u64,
    pub reclaimed_bytes: u64,
    pub compression_ratio: f64,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionHistoryDto {
    pub id: Option<i64>,
    pub target_path: String,
    pub kind: String,
    pub algorithm: String,
    pub logical_bytes: u64,
    pub physical_before_bytes: u64,
    pub physical_after_bytes: u64,
    pub file_count: u64,
    pub status: String,
    pub updated_at: String,
}

/// Scans for in-place compaction candidates (cold toolchain dirs and VHDX images).
#[tauri::command]
pub async fn scan_compaction_candidates(
    target_path: Option<String>,
    state: State<'_, AppState>,
) -> std::result::Result<Vec<CompactionCandidateDto>, String> {
    let started = Instant::now();
    let task_id = format!("compact-scan-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let join_res = tokio::task::spawn_blocking(
        move || -> std::result::Result<Vec<CompactionCandidateDto>, CommandFailure> {
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "compact_scan".to_string(),
                state: "running".to_string(),
                percent: 10,
                phase: "scan".to_string(),
                title: "Đang quét dữ liệu có thể thu gọn".to_string(),
                detail: "Rà soát thư mục SDK và đĩa ảo VHDX...".to_string(),
                done_files: 0,
                total_files: 0,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let proc_probe = NativeProcessServiceProbe;
            let mut dtos = Vec::new();

            if let Some(custom) = target_path {
                let custom_p = PathBuf::from(&custom);
                if custom_p.exists() {
                    let candidates = InPlaceCompactor::scan_candidate_files(&custom_p, 0);
                    let mut total_b = 0u64;
                    for f in &candidates {
                        if let Ok(m) = std::fs::metadata(f) {
                            total_b += m.len();
                        }
                    }
                    let est = (total_b as f64 * 0.40) as u64;
                    let name = custom_p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Directory")
                        .to_string();

                    dtos.push(CompactionCandidateDto {
                        id: format!("custom_{}", name.to_lowercase()),
                        name,
                        path: custom,
                        kind: "wof_directory".to_string(),
                        logical_bytes: total_b,
                        physical_bytes: total_b,
                        estimated_reclaimable_bytes: est,
                        file_count: candidates.len() as u64,
                        recommended_algorithm: "XPRESS8K".to_string(),
                        is_locked: false,
                        locked_processes: Vec::new(),
                        is_compacted: false,
                        selected: true,
                    });
                }
            } else {
                // Discovers standard developer SDK directories
                let dirs = InPlaceCompactor::discover_directories_for_compaction();
                for d in dirs {
                    let name = d
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Folder")
                        .to_string();
                    let path_str = d.path.to_string_lossy().to_string();

                    dtos.push(CompactionCandidateDto {
                        id: format!("dir_{}", name.to_lowercase()),
                        name: format!("SDK {}", name),
                        path: path_str,
                        kind: "wof_directory".to_string(),
                        logical_bytes: d.logical_bytes,
                        physical_bytes: d.logical_bytes,
                        estimated_reclaimable_bytes: d.estimated_reclaim_bytes,
                        file_count: d.file_count,
                        recommended_algorithm: "XPRESS8K".to_string(),
                        is_locked: false,
                        locked_processes: Vec::new(),
                        is_compacted: false,
                        selected: true,
                    });
                }

                // Discovers WSL2 and Docker VHDX disks
                let vhdxs = VhdxCompactor::discover_vhdx_images(&proc_probe);
                for v in vhdxs {
                    let est = (v.size_bytes as f64 * 0.30) as u64; // Est. 30% slack
                    dtos.push(CompactionCandidateDto {
                        id: format!("vhdx_{}", v.name.to_lowercase().replace(' ', "_")),
                        name: v.name,
                        path: v.path.to_string_lossy().to_string(),
                        kind: "vhdx_trim".to_string(),
                        logical_bytes: v.size_bytes,
                        physical_bytes: v.size_bytes,
                        estimated_reclaimable_bytes: est,
                        file_count: 1,
                        recommended_algorithm: "VIRTDISK".to_string(),
                        is_locked: v.is_locked,
                        locked_processes: v.running_processes,
                        is_compacted: false,
                        selected: !v.is_locked,
                    });
                }
            }

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker,
                kind: "compact_scan".to_string(),
                state: "completed".to_string(),
                percent: 100,
                phase: "complete".to_string(),
                title: "Hoàn tất tìm kiếm dữ liệu thu gọn".to_string(),
                detail: format!("Tìm thấy {} mục", dtos.len()),
                done_files: dtos.len() as u64,
                total_files: dtos.len() as u64,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            Ok(dtos)
        },
    )
    .await;

    state.unregister_task(&task_id);
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("Task execution join error: {e}"),
            "unknown",
        )),
    };
    record_scan_result(&state.diagnostics, "compact", started, diag_res, |_| {
        Outcome::Ok
    })
}

/// Compacts specified directories in-place using WOF CompactOS.
#[tauri::command]
pub async fn compact_files(
    target_paths: Vec<String>,
    algorithm: Option<String>,
    state: State<'_, AppState>,
) -> std::result::Result<CompactionResultDto, String> {
    let started = Instant::now();
    let task_id = format!("compact-exec-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let task_ctrl_worker = Arc::clone(&task_ctrl);
    let join_res = tokio::task::spawn_blocking(
        move || -> std::result::Result<CompactionResultDto, CommandFailure> {
            let algo = match algorithm.as_deref() {
                Some("XPRESS4K") => WofAlgorithm::Xpress4k,
                Some("XPRESS16K") => WofAlgorithm::Xpress16k,
                Some("LZX") => WofAlgorithm::Lzx,
                Some("LZNT1") => WofAlgorithm::Lznt1,
                _ => WofAlgorithm::Xpress8k,
            };

            let compaction_api = NativeCompactionApi;
            let lock_probe = NativeFileLockProbe;
            let state_progress = state_inner.clone();
            let task_id_clone = task_id_worker.clone();

            let total_files_processed = 0u64;
            let mut total_files_compacted = 0u64;
            let total_files_skipped = 0u64;
            let mut total_logical = 0u64;
            let mut total_before = 0u64;
            let mut total_after = 0u64;

            for path_str in &target_paths {
                if task_ctrl_worker.is_cancelled() {
                    break;
                }

                let path = PathBuf::from(path_str);
                let state_prog = state_progress.clone();
                let task_id_inner = task_id_clone.clone();

                let rec = InPlaceCompactor::compact_directory(
                    &path,
                    algo,
                    &state_inner.db,
                    &compaction_api,
                    &lock_probe,
                    &task_ctrl_worker,
                    move |cur_b, cur_f| {
                        state_prog.emit_progress(&TaskProgressEvent {
                            task_id: task_id_inner.clone(),
                            kind: "compact".to_string(),
                            state: "running".to_string(),
                            percent: 50,
                            phase: "compacting".to_string(),
                            title: "Đang thu gọn tệp".to_string(),
                            detail: format!("Đã xử lý {} tệp", cur_f),
                            done_files: cur_f,
                            total_files: 0,
                            done_bytes: cur_b,
                            total_bytes: 0,
                            throughput_bps: 0,
                            eta_seconds: 0,
                        });
                    },
                )
                .map_err(|e| {
                    CommandFailure::from_err(&e, format!("Lỗi thu gọn {}: {}", path_str, e))
                })?;

                total_files_compacted += rec.file_count;
                total_logical += rec.logical_bytes;
                total_before += rec.physical_before_bytes;
                total_after += rec.physical_after_bytes;
            }

            let reclaimed = total_before.saturating_sub(total_after);
            let ratio = if total_before > 0 {
                total_after as f64 / total_before as f64
            } else {
                1.0
            };

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "compact".to_string(),
                state: "completed".to_string(),
                percent: 100,
                phase: "done".to_string(),
                title: "Đã thu gọn xong tệp tin".to_string(),
                detail: format!("Tiết kiệm được {} MB", reclaimed / (1024 * 1024)),
                done_files: total_files_compacted,
                total_files: total_files_compacted,
                done_bytes: total_after,
                total_bytes: total_before,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            Ok(CompactionResultDto {
                task_id: task_id_worker,
                success: true,
                kind: "wof_directory".to_string(),
                target_path: target_paths.join("; "),
                algorithm: algo.as_str().to_string(),
                files_processed: total_files_processed,
                files_compacted: total_files_compacted,
                files_skipped: total_files_skipped,
                logical_bytes: total_logical,
                physical_before_bytes: total_before,
                physical_after_bytes: total_after,
                reclaimed_bytes: reclaimed,
                compression_ratio: ratio,
                error_message: None,
            })
        },
    )
    .await;

    state.unregister_task(&task_id);
    let cancelled = task_ctrl.is_cancelled();
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("Task execution join error: {e}"),
            "unknown",
        )),
    };
    record_action_result(&state.diagnostics, "compact", started, diag_res, |r| {
        if cancelled {
            Outcome::Cancelled
        } else if !r.success {
            Outcome::Failed
        } else if r.files_skipped > 0 {
            Outcome::Partial
        } else {
            Outcome::Ok
        }
    })
}

/// Decompresses files back to standard uncompressed NTFS format.
#[tauri::command]
pub async fn uncompact_files(
    target_paths: Vec<String>,
    state: State<'_, AppState>,
) -> std::result::Result<CompactionResultDto, String> {
    let started = Instant::now();
    let task_id = format!("uncompact-exec-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let task_ctrl_worker = Arc::clone(&task_ctrl);
    let join_res = tokio::task::spawn_blocking(
        move || -> std::result::Result<CompactionResultDto, CommandFailure> {
        let compaction_api = NativeCompactionApi;
        let mut total_uncompacted = 0u64;
        let mut failed_files = 0usize;

        for path_str in &target_paths {
            if task_ctrl_worker.is_cancelled() {
                break;
            }
            let path = PathBuf::from(path_str);
            let report = InPlaceCompactor::decompress_directory(
                &path,
                &state_inner.db,
                &compaction_api,
                &task_ctrl_worker,
            )
            .map_err(|e| CommandFailure::from_err(&e, format!("Lỗi giải nén {}: {}", path_str, e)))?;
            failed_files += report.failed.len();
            total_uncompacted += 1;
        }

        Ok(CompactionResultDto {
            task_id: task_id_worker,
            success: failed_files == 0,
            kind: "wof_directory".to_string(),
            target_path: target_paths.join("; "),
            algorithm: "NONE".to_string(),
            files_processed: total_uncompacted,
            files_compacted: 0,
            files_skipped: 0,
            logical_bytes: 0,
            physical_before_bytes: 0,
            physical_after_bytes: 0,
            reclaimed_bytes: 0,
            compression_ratio: 1.0,
            error_message: (failed_files > 0).then(|| {
                format!("{failed_files} tệp chưa giải nén được (đang được ứng dụng khác dùng?). Hãy thử lại.")
            }),
        })
    })
    .await;

    state.unregister_task(&task_id);
    let cancelled = task_ctrl.is_cancelled();
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("Task execution join error: {e}"),
            "unknown",
        )),
    };
    record_action_result(&state.diagnostics, "uncompact", started, diag_res, |r| {
        if cancelled {
            Outcome::Cancelled
        } else if !r.success {
            Outcome::Failed
        } else if r.files_skipped > 0 {
            Outcome::Partial
        } else {
            Outcome::Ok
        }
    })
}

/// Compacts a virtual disk (.vhdx) after verifying host process safety.
#[tauri::command]
pub async fn compact_vhdx(
    vhdx_path: String,
    state: State<'_, AppState>,
) -> std::result::Result<CompactionResultDto, String> {
    let started = Instant::now();
    let task_id = format!("compact-vhdx-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let join_res = tokio::task::spawn_blocking(
        move || -> std::result::Result<CompactionResultDto, CommandFailure> {
            let compaction_api = NativeCompactionApi;
            let proc_probe = NativeProcessServiceProbe;
            let path = PathBuf::from(&vhdx_path);

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "compact_vhdx".to_string(),
                state: "running".to_string(),
                percent: 20,
                phase: "trimming".to_string(),
                title: "Đang thu gọn đĩa ảo VHDX".to_string(),
                detail: vhdx_path.clone(),
                done_files: 0,
                total_files: 1,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let rec = VhdxCompactor::compact_vhdx_image(
                &path,
                &state_inner.db,
                &compaction_api,
                &proc_probe,
            )
            .map_err(|e| CommandFailure::from_err(&e, format!("Lỗi thu gọn VHDX: {e}")))?;

            let reclaimed = rec
                .physical_before_bytes
                .saturating_sub(rec.physical_after_bytes);
            let ratio = if rec.physical_before_bytes > 0 {
                rec.physical_after_bytes as f64 / rec.physical_before_bytes as f64
            } else {
                1.0
            };

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "compact_vhdx".to_string(),
                state: "completed".to_string(),
                percent: 100,
                phase: "done".to_string(),
                title: "Đã thu gọn đĩa ảo VHDX".to_string(),
                detail: format!("Tiết kiệm {} MB", reclaimed / (1024 * 1024)),
                done_files: 1,
                total_files: 1,
                done_bytes: rec.physical_after_bytes,
                total_bytes: rec.physical_before_bytes,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            Ok(CompactionResultDto {
                task_id: task_id_worker,
                success: true,
                kind: "vhdx_trim".to_string(),
                target_path: vhdx_path,
                algorithm: "VIRTDISK".to_string(),
                files_processed: 1,
                files_compacted: 1,
                files_skipped: 0,
                logical_bytes: rec.logical_bytes,
                physical_before_bytes: rec.physical_before_bytes,
                physical_after_bytes: rec.physical_after_bytes,
                reclaimed_bytes: reclaimed,
                compression_ratio: ratio,
                error_message: None,
            })
        },
    )
    .await;

    state.unregister_task(&task_id);
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("Task execution join error: {e}"),
            "unknown",
        )),
    };
    record_action_result(&state.diagnostics, "compact_vhdx", started, diag_res, |r| {
        if r.success {
            Outcome::Ok
        } else {
            Outcome::Failed
        }
    })
}

/// Lists all historical compaction records from the database.
#[tauri::command]
pub async fn list_compaction_history(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<CompactionHistoryDto>, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let records = state_inner
            .db
            .list_compaction_records(None)
            .map_err(|e| format!("Lỗi truy vấn lịch sử thu gọn: {e}"))?;

        let dtos = records
            .into_iter()
            .map(|r| CompactionHistoryDto {
                id: r.id,
                target_path: r.target_path.to_string_lossy().to_string(),
                kind: r.kind.as_str().to_string(),
                algorithm: r.algorithm,
                logical_bytes: r.logical_bytes,
                physical_before_bytes: r.physical_before_bytes,
                physical_after_bytes: r.physical_after_bytes,
                file_count: r.file_count,
                status: r.status,
                updated_at: r.updated_at.to_rfc3339(),
            })
            .collect();

        Ok(dtos)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}
