//! Tauri v2 IPC commands for Multi-Strategy Relocation and Recovery (Tasks T047, T048).

use crate::state::{AppState, TaskProgressEvent};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    NativeFileLockProbe, NativeProcessServiceProbe, NativeReparsePointApi, NativeShellFolderApi,
    NativeWin32VolumeApi,
};
use greenc_relocate::candidate::CandidateScanner;
use greenc_relocate::known_folders::KnownFolderService;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::recovery::RecoveryService;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelocationCandidateDto {
    pub id: String,
    pub name: String,
    pub strategy: String,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub is_locked: bool,
    pub locked_processes: Vec<String>,
    pub how_description: String,
    pub is_advanced: bool,
    pub is_eligible: bool,
    pub ineligible_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownFolderDto {
    pub folder_id: String,
    pub guid: String,
    pub name: String,
    pub current_path: String,
    pub target_path: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub is_redirected: bool,
    pub is_onedrive: bool,
    pub target_drive: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelocationResultDto {
    pub id: Option<i64>,
    pub candidate_id: String,
    pub app_name: String,
    pub strategy: String,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: u64,
    pub status: String,
    pub health_status: String,
    pub detail: Option<String>,
    pub created_at: String,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Scans the system for movable data candidates matching all 4 strategies.
#[tauri::command]
pub async fn scan_relocation_candidates(
    target_drive: Option<String>,
    state: State<'_, AppState>,
) -> std::result::Result<Vec<RelocationCandidateDto>, String> {
    let task_id = format!("relocate-scan-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "relocate_scan".to_string(),
            state: "running".to_string(),
            percent: 10,
            phase: "scan".to_string(),
            title: "Đang tìm dữ liệu có thể chuyển".to_string(),
            detail: "Đang rà soát thư mục hồ sơ người dùng...".to_string(),
            done_files: 0,
            total_files: 0,
            done_bytes: 0,
            total_bytes: 0,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        let target_letter = target_drive
            .as_deref()
            .and_then(|d| d.chars().next())
            .unwrap_or('D');

        let vol_api = NativeWin32VolumeApi;
        let shell_api = NativeShellFolderApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;

        let candidates = CandidateScanner::discover_all(
            target_letter,
            &vol_api,
            &shell_api,
            &lock_probe,
            &proc_probe,
        );

        let dtos: Vec<RelocationCandidateDto> = candidates
            .into_iter()
            .map(|c| RelocationCandidateDto {
                id: c.id,
                name: c.name,
                strategy: c.strategy.as_str().to_string(),
                source_path: c.source_path.to_string_lossy().to_string(),
                target_path: c.default_target_path.to_string_lossy().to_string(),
                size_bytes: c.size_bytes,
                file_count: c.file_count,
                is_locked: c.is_locked,
                locked_processes: c.locked_processes,
                how_description: c.how_description,
                is_advanced: c.is_advanced,
                is_eligible: c.is_eligible,
                ineligible_reason: c.ineligible_reason,
            })
            .collect();

        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker,
            kind: "relocate_scan".to_string(),
            state: "completed".to_string(),
            percent: 100,
            phase: "complete".to_string(),
            title: "Đã phân tích xong dữ liệu có thể chuyển".to_string(),
            detail: format!("Tìm thấy {} mục", dtos.len()),
            done_files: dtos.len() as u64,
            total_files: dtos.len() as u64,
            done_bytes: 0,
            total_bytes: 0,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        Ok(dtos)
    })
    .await
    .map_err(|e| format!("Task execution join error: {e}"))?;

    state.unregister_task(&task_id);
    result
}

/// Relocates a candidate directory to target drive using 4-phase transaction.
#[tauri::command]
pub async fn relocate_directory(
    candidate_id: String,
    target_drive: String,
    state: State<'_, AppState>,
) -> std::result::Result<RelocationResultDto, String> {
    let task_id = format!("relocate-exec-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let drive_char = target_drive
            .chars()
            .next()
            .ok_or_else(|| "Chưa chọn ổ đĩa hợp lệ".to_string())?;

        let vol_api = NativeWin32VolumeApi;
        let shell_api = NativeShellFolderApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;
        let reparse_api = NativeReparsePointApi;

        if candidate_id.eq_ignore_ascii_case("zalo") {
            let lifecycle = greenc_platform::app_lifecycle::NativeAppLifecycleApi;
            let running_exe_dirs =
                greenc_platform::app_lifecycle::AppLifecycleApi::find_running_exe_dirs(
                    &lifecycle,
                    greenc_scanner::zalo::ZALO_PROCESS_NAMES,
                );
            let known_relocated_sources: std::collections::HashSet<PathBuf> = state_inner
                .db
                .list_relocations(Some("done"))
                .unwrap_or_default()
                .into_iter()
                .filter(|r| r.app_id == "zalo")
                .map(|r| r.source_path)
                .collect();
            let disc = greenc_scanner::zalo::discover_zalo_installation(
                &greenc_scanner::zalo::ZaloDiscoveryOptions {
                    running_exe_dirs,
                    known_relocated_sources,
                    ..Default::default()
                },
            )
            .ok_or_else(|| "Không tìm thấy cài đặt Zalo PC".to_string())?;

            let opts = greenc_relocate::zalo_move::ZaloMoveOptions {
                target_drive: drive_char.to_ascii_uppercase(),
                reopen_zalo: true,
                ..Default::default()
            };
            let state_progress = state_inner.clone();
            let task_id_clone = task_id_worker.clone();
            let report = greenc_relocate::zalo_move::ZaloRelocator::move_zalo_data(
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
                    state_progress.emit_progress(&TaskProgressEvent {
                        task_id: task_id_clone.clone(),
                        kind: "relocate".to_string(),
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
            .map_err(|err| format!("Lỗi khi di chuyển: {err}"))?;

            return Ok(RelocationResultDto {
                id: report.record_id,
                candidate_id: "zalo".to_string(),
                app_name: "Zalo PC".to_string(),
                strategy: "junction".to_string(),
                source_path: report.source_path.to_string_lossy().to_string(),
                target_path: report.target_path.to_string_lossy().to_string(),
                size_bytes: report.moved_bytes,
                status: report.status,
                health_status: if report.health_verified {
                    "healthy"
                } else {
                    "link_broken"
                }
                .to_string(),
                detail: Some(report.message),
                created_at: chrono::Utc::now().to_rfc3339(),
                success: true,
                error_message: None,
            });
        }

        let candidates = CandidateScanner::discover_all(
            drive_char,
            &vol_api,
            &shell_api,
            &lock_probe,
            &proc_probe,
        );

        let candidate = candidates
            .into_iter()
            .find(|c| c.id == candidate_id)
            .ok_or_else(|| format!("Không tìm thấy mục di chuyển: {}", candidate_id))?;

        let state_progress = state_inner.clone();
        let task_id_clone = task_id_worker.clone();
        let cand_name = candidate.name.clone();
        let total_bytes = candidate.size_bytes;

        let options = MoveOptions {
            target_drive: drive_char,
            custom_target_path: None,
        };

        let move_res = TransactionalMover::relocate_junction(
            &candidate,
            options,
            &state_inner.db,
            &vol_api,
            &reparse_api,
            &proc_probe,
            &shell_api,
            &task_ctrl,
            move |done_b, done_f| {
                let percent = if total_bytes > 0 {
                    ((done_b as f64 / total_bytes as f64) * 100.0) as u32
                } else {
                    0
                };
                state_progress.emit_progress(&TaskProgressEvent {
                    task_id: task_id_clone.clone(),
                    kind: "relocate".to_string(),
                    state: "running".to_string(),
                    percent,
                    phase: "moving".to_string(),
                    title: format!("Đang chuyển {}", cand_name),
                    detail: format!("Đã chuyển {} tệp", done_f),
                    done_files: done_f,
                    total_files: 0,
                    done_bytes: done_b,
                    total_bytes,
                    throughput_bps: 0,
                    eta_seconds: 0,
                });
            },
        );

        match move_res {
            Ok(rec) => {
                state_inner.emit_progress(&TaskProgressEvent {
                    task_id: task_id_worker,
                    kind: "relocate".to_string(),
                    state: "completed".to_string(),
                    percent: 100,
                    phase: "done".to_string(),
                    title: format!("Đã chuyển {}", rec.app_name),
                    detail: format!("Dữ liệu mới ở {}", rec.target_path.display()),
                    done_files: 1,
                    total_files: 1,
                    done_bytes: rec.size_bytes,
                    total_bytes: rec.size_bytes,
                    throughput_bps: 0,
                    eta_seconds: 0,
                });

                Ok(RelocationResultDto {
                    id: rec.id,
                    candidate_id: rec.app_id,
                    app_name: rec.app_name,
                    strategy: rec.strategy.as_str().to_string(),
                    source_path: rec.source_path.to_string_lossy().to_string(),
                    target_path: rec.target_path.to_string_lossy().to_string(),
                    size_bytes: rec.size_bytes,
                    status: rec.status.as_str().to_string(),
                    health_status: rec.health_status.as_str().to_string(),
                    detail: rec.detail,
                    created_at: rec.created_at.to_rfc3339(),
                    success: true,
                    error_message: None,
                })
            }
            Err(err) => Err(format!("Lỗi khi di chuyển: {err}")),
        }
    })
    .await
    .map_err(|e| format!("Task execution join error: {e}"))?;

    state.unregister_task(&task_id);
    result
}

/// Enumerates Windows Known Folders and reports current paths and redirection status.
#[tauri::command]
pub async fn get_known_folders(
    target_drive: Option<String>,
    _state: State<'_, AppState>,
) -> std::result::Result<Vec<KnownFolderDto>, String> {
    tokio::task::spawn_blocking(move || {
        let shell_api = NativeShellFolderApi;
        let drive_char = target_drive
            .as_deref()
            .and_then(|d| d.chars().next())
            .unwrap_or('D');

        let folders = KnownFolderService::list_known_folders(drive_char, &shell_api)
            .map_err(|e| format!("Lỗi truy vấn thư mục hệ thống: {e}"))?;

        let dtos: Vec<KnownFolderDto> = folders
            .into_iter()
            .map(|f| KnownFolderDto {
                folder_id: f.folder_id,
                guid: f.guid,
                name: f.name,
                current_path: f.current_path.to_string_lossy().to_string(),
                target_path: f.default_target_path.to_string_lossy().to_string(),
                size_bytes: f.size_bytes,
                file_count: f.file_count,
                is_redirected: f.is_redirected,
                is_onedrive: f.is_onedrive,
                target_drive: drive_char.to_string(),
            })
            .collect();

        Ok(dtos)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Relocates and updates shell registration for a Windows Known Folder.
#[tauri::command]
pub async fn relocate_known_folder(
    folder_id: String,
    target_drive: String,
    state: State<'_, AppState>,
) -> std::result::Result<RelocationResultDto, String> {
    #[cfg(feature = "store")]
    {
        let _ = (folder_id, target_drive, state);
        Err(
            "Bản Microsoft Store tạm tắt chuyển thư mục hệ thống (Known Folders) do cơ chế ảo hoá HKCU của MSIX. Hãy đổi trong File Explorer → Properties → Location hoặc dùng bản cài EXE."
                .to_string(),
        )
    }

    #[cfg(not(feature = "store"))]
    {
        let task_id = format!("relocate-kf-{}", chrono::Utc::now().timestamp_millis());
        let task_ctrl = Arc::new(TaskControl::new());
        state.register_task(task_id.clone(), task_ctrl.clone());

        let state_inner = state.inner().clone();
        let result = tokio::task::spawn_blocking(move || {
            let drive_char = target_drive
                .chars()
                .next()
                .ok_or_else(|| "Chưa chọn ổ đĩa hợp lệ".to_string())?;

            let shell_api = NativeShellFolderApi;
            let vol_api = NativeWin32VolumeApi;

            let rec = KnownFolderService::relocate_known_folder(
                &folder_id,
                drive_char,
                None,
                &state_inner.db,
                &shell_api,
                &vol_api,
                &task_ctrl,
                |_, _| {},
            )
            .map_err(|e| format!("Lỗi chuyển thư mục hệ thống: {e}"))?;

            Ok(RelocationResultDto {
                id: rec.id,
                candidate_id: rec.app_id,
                app_name: rec.app_name,
                strategy: rec.strategy.as_str().to_string(),
                source_path: rec.source_path.to_string_lossy().to_string(),
                target_path: rec.target_path.to_string_lossy().to_string(),
                size_bytes: rec.size_bytes,
                status: rec.status.as_str().to_string(),
                health_status: rec.health_status.as_str().to_string(),
                detail: rec.detail,
                created_at: rec.created_at.to_rfc3339(),
                success: true,
                error_message: None,
            })
        })
        .await
        .map_err(|e| format!("Task execution join error: {e}"))?;

        state.unregister_task(&task_id);
        result
    }
}

/// Checks and recovers any interrupted moves from previous crash or power loss.
#[tauri::command]
pub async fn recover_interrupted_relocations(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<RelocationResultDto>, String> {
    let state_inner = state.inner().clone();
    // A `moving` record may belong to a move that is still running (the window can be closed and
    // reopened meanwhile); recovering it would delete the copy being written.
    if state_inner.has_foreground_tasks() {
        return Ok(Vec::new());
    }
    tokio::task::spawn_blocking(move || {
        let reparse = NativeReparsePointApi;
        let recovered = RecoveryService::recover_interrupted(&state_inner.db, &reparse)
            .map_err(|e| format!("Lỗi khôi phục sau sự cố: {e}"))?;

        let dtos: Vec<RelocationResultDto> = recovered
            .into_iter()
            .map(|r| RelocationResultDto {
                id: r.id,
                candidate_id: r.app_id,
                app_name: r.app_name,
                strategy: r.strategy.as_str().to_string(),
                source_path: r.source_path.to_string_lossy().to_string(),
                target_path: r.target_path.to_string_lossy().to_string(),
                size_bytes: r.size_bytes,
                status: r.status.as_str().to_string(),
                health_status: r.health_status.as_str().to_string(),
                detail: r.detail,
                created_at: r.created_at.to_rfc3339(),
                success: true,
                error_message: None,
            })
            .collect();

        Ok(dtos)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Lists all completed and active relocation records with live health checks.
#[tauri::command]
pub async fn list_relocation_history(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<RelocationResultDto>, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let records = state_inner
            .db
            .list_relocations(None)
            .map_err(|e| format!("Lỗi truy vấn lịch sử di chuyển: {e}"))?;

        let reparse = NativeReparsePointApi;
        let healths = RecoveryService::check_junction_health(&records, &reparse);

        let dtos: Vec<RelocationResultDto> = records
            .into_iter()
            .zip(healths)
            .map(|(r, h)| {
                if let Some(id) = r.id {
                    let _ = state_inner.db.update_relocation_health(id, h.as_str());
                }

                RelocationResultDto {
                    id: r.id,
                    candidate_id: r.app_id,
                    app_name: r.app_name,
                    strategy: r.strategy.as_str().to_string(),
                    source_path: r.source_path.to_string_lossy().to_string(),
                    target_path: r.target_path.to_string_lossy().to_string(),
                    size_bytes: r.size_bytes,
                    status: r.status.as_str().to_string(),
                    health_status: h.as_str().to_string(),
                    detail: r.detail,
                    created_at: r.created_at.to_rfc3339(),
                    success: true,
                    error_message: None,
                }
            })
            .collect();

        Ok(dtos)
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Reverses a previous relocation, restoring files and removing junction.
#[tauri::command]
pub async fn undo_relocation(
    record_id: i64,
    state: State<'_, AppState>,
) -> std::result::Result<RelocationResultDto, String> {
    let task_id = format!("relocate-undo-{}", chrono::Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), task_ctrl.clone());

    let state_inner = state.inner().clone();
    let result = tokio::task::spawn_blocking(move || {
        let vol_api = NativeWin32VolumeApi;
        let reparse = NativeReparsePointApi;
        let shell_api = NativeShellFolderApi;
        let proc_probe = NativeProcessServiceProbe;
        let lifecycle = greenc_platform::app_lifecycle::NativeAppLifecycleApi;

        let existing_zalo = state_inner.db.list_relocations(None).ok().and_then(|list| {
            list.into_iter()
                .find(|r| r.id == Some(record_id) && r.app_id == "zalo")
        });

        let mut shutdown_session = None;
        if let Some(ref zrec) = existing_zalo {
            if greenc_platform::app_lifecycle::AppLifecycleApi::has_visible_window(
                &lifecycle,
                &["ZaloCall.exe"],
            ) {
                return Err(format!(
                    "Không thể hoàn tác: {}",
                    greenc_platform::app_lifecycle::ZALO_ACTIVE_CALL_MESSAGE
                ));
            }
            let session = greenc_platform::app_lifecycle::AppLifecycleApi::graceful_shutdown(
                &lifecycle,
                &[zrec.source_path.clone(), zrec.target_path.clone()],
                greenc_scanner::zalo::ZALO_PROCESS_NAMES,
                greenc_platform::app_lifecycle::DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
            )
            .map_err(|e| format!("Không thể hoàn tác: {e}"))?;
            shutdown_session = Some(session);
        }

        let undo_res = TransactionalMover::undo_relocation(
            record_id,
            &state_inner.db,
            &vol_api,
            &reparse,
            &shell_api,
            &proc_probe,
            &task_ctrl,
        );

        if let Some(mut session) = shutdown_session {
            if !session.captured_exes.is_empty() {
                let _ = greenc_platform::app_lifecycle::AppLifecycleApi::restart_app(
                    &lifecycle,
                    &mut session,
                );
            }
        }

        let undone_rec = undo_res.map_err(|e| format!("Không thể hoàn tác: {e}"))?;
        let _ = state_inner
            .db
            .revert_savings_by_ref("relocate", &record_id.to_string());

        Ok(RelocationResultDto {
            id: undone_rec.id,
            candidate_id: undone_rec.app_id,
            app_name: undone_rec.app_name,
            strategy: undone_rec.strategy.as_str().to_string(),
            source_path: undone_rec.source_path.to_string_lossy().to_string(),
            target_path: undone_rec.target_path.to_string_lossy().to_string(),
            size_bytes: undone_rec.size_bytes,
            status: undone_rec.status.as_str().to_string(),
            health_status: undone_rec.health_status.as_str().to_string(),
            detail: undone_rec.detail,
            created_at: undone_rec.created_at.to_rfc3339(),
            success: true,
            error_message: None,
        })
    })
    .await
    .map_err(|e| format!("Task execution join error: {e}"))?;

    state.unregister_task(&task_id);
    result
}

/// Repairs a broken junction whose target drive or folder was modified.
#[tauri::command]
pub async fn repair_junction_target(
    record_id: i64,
    new_target_path: String,
    state: State<'_, AppState>,
) -> std::result::Result<RelocationResultDto, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let reparse = NativeReparsePointApi;
        let target = PathBuf::from(new_target_path);
        let rec =
            RecoveryService::repair_junction_target(record_id, &target, &state_inner.db, &reparse)
                .map_err(|e| format!("Lỗi sửa liên kết: {e}"))?;

        Ok(RelocationResultDto {
            id: rec.id,
            candidate_id: rec.app_id,
            app_name: rec.app_name,
            strategy: rec.strategy.as_str().to_string(),
            source_path: rec.source_path.to_string_lossy().to_string(),
            target_path: rec.target_path.to_string_lossy().to_string(),
            size_bytes: rec.size_bytes,
            status: rec.status.as_str().to_string(),
            health_status: rec.health_status.as_str().to_string(),
            detail: rec.detail,
            created_at: rec.created_at.to_rfc3339(),
            success: true,
            error_message: None,
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}
