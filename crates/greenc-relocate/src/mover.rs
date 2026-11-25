//! Transactional Mover implementing the 4-phase safe relocation state machine and 1-click Undo (Tasks T039, T046).

use crate::candidate::{CandidateQualificationEngine, RelocationCandidate};
use crate::verified_copy::{
    remove_dir_all_safe, to_extended_io_path, verified_copy_tree, verified_copy_tree_strict,
};
use crate::zalo_move::ZaloRelocator;
use chrono::Utc;
use greenc_core::config::is_windows_appdata_app_dir;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::AppLifecycleApi;
use greenc_platform::breakaway::{
    current_package_full_name, format_breakaway_unavailable_guidance, BreakawayHelperClient,
    NativeBreakawayHelperClient, PrivateBreakawayJobDir, RestoreHelperRequest,
    RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE,
};
use greenc_platform::handle::ensure_not_protected_system_path;
use greenc_platform::traits::{
    FileLockProbe, NativeFileLockProbe, ProcessServiceProbe, ReparsePointApi, ShellFolderApi,
    Win32VolumeApi,
};
use greenc_platform::RelocationLock;
use greenc_storage::db::GreenCDb;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub struct MoveOptions {
    pub target_drive: char,
    pub custom_target_path: Option<PathBuf>,
}

/// `true` when `path` is, contains or lies inside a Zalo folder, whatever catalog entry offered it.
fn holds_zalo_data(path: &Path) -> bool {
    let is_zalo_name = |name: &std::ffi::OsStr| {
        name.to_string_lossy()
            .to_lowercase()
            .trim_start()
            .starts_with("zalo")
    };
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::Normal(n) if is_zalo_name(n)))
    {
        return true;
    }
    if fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| is_zalo_name(&e.file_name()) && e.path().is_dir())
    {
        return true;
    }
    let data_dir = greenc_scanner::zalo::discover_zalo_installation(&Default::default())
        .and_then(|d| d.data_dir);
    data_dir.is_some_and(|d| {
        let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        let (d, p) = (norm(&d), norm(path));
        let within = |child: &str, parent: &str| {
            child == parent || child.starts_with(&format!("{}\\", parent.trim_end_matches('\\')))
        };
        within(&d, &p) || within(&p, &d)
    })
}

pub struct TransactionalMover;

impl TransactionalMover {
    /// Executes the 4-phase atomic relocation transaction:
    /// Phase 1: Pre-qualification & verified streaming copy
    /// Phase 2: SQLite state set to `Moving` & atomic rename of source to `.greenc-moving.<uuid>`
    /// Phase 3: Create NTFS junction at source pointing to target & verify link target
    /// Phase 4: Purge temporary `.greenc-moving.<uuid>` directory & mark `Done`
    /// Checks that `candidate` may be moved with a copy-verify-junction transaction at all.
    ///
    /// Allowed: app data folders (`Junction`) and tool caches/homes (`ToolConfig`; changing the
    /// tool's own config is not implemented, a junction is the reversible equivalent).
    /// Refused: Known Folders (they must go through the shell API, a junction would break the
    /// user's Desktop/Documents registration) and guided apps such as WSL/Docker (moved only with
    /// the vendor's own tooling), plus any candidate that failed qualification.
    pub fn preflight(candidate: &RelocationCandidate) -> Result<()> {
        // Chat data needs `ZaloRelocator` (strict copy, verification, retained original).
        if candidate.id.eq_ignore_ascii_case("zalo") || holds_zalo_data(&candidate.source_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason: "Dữ liệu Zalo không được chuyển bằng cơ chế chung".to_string(),
            });
        }
        if !matches!(
            candidate.strategy,
            RelocationStrategyKind::Junction | RelocationStrategyKind::ToolConfig
        ) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason: format!(
                    "'{}' uses the '{}' strategy and cannot be moved with a junction",
                    candidate.id,
                    candidate.strategy.as_str()
                ),
            });
        }
        if !candidate.is_eligible {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason: candidate.ineligible_reason.clone().unwrap_or_else(|| {
                    format!("'{}' is not eligible for relocation", candidate.id)
                }),
            });
        }
        if greenc_core::config::is_windows_apps_or_packages_path(&candidate.source_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason: format!(
                    "'{}' nằm trong vùng bảo vệ WindowsApps/Packages và không được di chuyển",
                    candidate.source_path.display()
                ),
            });
        }
        if cfg!(feature = "store")
            && greenc_core::config::is_windows_appdata_app_dir(&candidate.source_path)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason: format!(
                    "Bản Microsoft Store không tạo NTFS Junction trong AppData ('{}') do cơ chế ảo hoá AppData của MSIX.",
                    candidate.source_path.display()
                ),
            });
        }
        Ok(())
    }

    pub fn relocate_junction<F>(
        candidate: &RelocationCandidate,
        options: MoveOptions,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        proc_probe: &dyn ProcessServiceProbe,
        shell_api: &dyn ShellFolderApi,
        ctrl: &TaskControl,
        on_progress: F,
    ) -> Result<RelocationRecord>
    where
        F: FnMut(u64, u64),
    {
        let target_drive = options.target_drive;
        let source_path = candidate.source_path.clone();
        let target_path = options
            .custom_target_path
            .unwrap_or_else(|| candidate.default_target_path.clone());

        Self::preflight(candidate)?;
        let _reloc_lock = RelocationLock::acquire_for_db(db.path())?;

        // 1. Pre-qualification guards
        CandidateQualificationEngine::check_target_volume_eligible(target_drive, volume_api)?;
        CandidateQualificationEngine::check_free_space_reserve(
            target_drive,
            candidate.size_bytes,
            volume_api,
        )?;
        CandidateQualificationEngine::check_process_running(
            &candidate.locked_processes,
            proc_probe,
        )?;
        if candidate.is_locked && candidate.locked_processes.is_empty() {
            return Err(GreenCError::FileLocked {
                path: source_path.clone(),
                reason: format!(
                    "Tệp tin trong {} đang bị khóa bởi ứng dụng khác",
                    source_path.display()
                ),
            });
        }
        CandidateQualificationEngine::check_target_directory_empty_or_absent(&target_path)?;
        CandidateQualificationEngine::check_onedrive(&source_path, shell_api)?;

        // Ensure target parent directory exists
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // 2. Phase 1: Verified streaming copy to target
        tracing::info!(
            "Starting verified copy from {} to {}",
            source_path.display(),
            target_path.display()
        );

        let copy_stats = verified_copy_tree(&source_path, &target_path, ctrl, on_progress)?;

        let uuid_suffix = uuid::Uuid::new_v4().to_string();
        let source_name = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("folder");
        let source_parent = source_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        let moving_dir_name = format!("{}.greenc-moving.{}", source_name, uuid_suffix);
        let moving_path = source_parent.join(&moving_dir_name);

        // 3. Phase 2: Persist state as `Moving` (including exact `moving_path`) before touching source filesystem
        let mut record = RelocationRecord {
            id: None,
            app_id: candidate.id.clone(),
            app_name: candidate.name.clone(),
            strategy: RelocationStrategyKind::Junction,
            source_path: source_path.clone(),
            target_path: target_path.clone(),
            size_bytes: copy_stats.bytes_copied,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: candidate.locked_processes.clone(),
            created_at: Utc::now(),
            detail: None,
            moving_path: Some(moving_path.clone()),
            target_user_repaired: false,
        };

        let rec_id = db.save_relocation_durable(&record)?;
        record.id = Some(rec_id);

        // Atomic rename of source to .greenc-moving.<uuid>
        if let Err(rename_err) = fs::rename(&source_path, &moving_path) {
            tracing::error!(
                "Failed to rename source {} to {}: {}. Rolling back target.",
                source_path.display(),
                moving_path.display(),
                rename_err
            );
            remove_dir_all_safe(&target_path);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!("Lỗi đổi tên thư mục nguồn: {}", rename_err));
            let _ = db.save_relocation(&record);
            return Err(GreenCError::Io(rename_err));
        }

        // 4. Phase 3: Create and verify NTFS junction at source pointing to target
        let junction_res = (|| -> Result<()> {
            reparse_api.create_junction(&source_path, &target_path)?;
            let verified_target = reparse_api.get_junction_target(&source_path)?;

            // Canonical check
            if !greenc_core::models::same_windows_path(&verified_target, &target_path) {
                return Err(GreenCError::Relocation(format!(
                    "Junction target mismatch: expected {}, got {}",
                    target_path.display(),
                    verified_target.display()
                )));
            }

            Ok(())
        })();

        if let Err(j_err) = junction_res {
            tracing::error!(
                "Junction creation failed: {}. Executing atomic rollback to source.",
                j_err
            );
            let _ = reparse_api.delete_junction(&source_path);
            let _ = fs::rename(&moving_path, &source_path);
            remove_dir_all_safe(&target_path);

            record.status = RelocationStatus::Failed;
            record.detail = Some(format!("Lỗi tạo liên kết NTFS Junction: {}", j_err));
            let _ = db.save_relocation(&record);
            return Err(j_err);
        }

        // 5. Phase 4: Purge temporary .greenc-moving.<uuid> folder & mark Done
        let mut detail = None;
        if let Err(del_err) = fs::remove_dir_all(&moving_path) {
            tracing::warn!(
                "Could not delete all files in moving directory {}: {}. Flagging in detail.",
                moving_path.display(),
                del_err
            );
            detail = Some(format!(
                "Còn tệp chưa xóa trong {}: {}",
                moving_dir_name, del_err
            ));
        }

        record.status = RelocationStatus::Done;
        record.health_status = RelocationHealthStatus::Healthy;
        record.detail = detail;
        let _ = db.save_relocation(&record);

        tracing::info!("Relocation completed successfully for record #{}", rec_id);

        Ok(record)
    }

    /// 1-Click Undo: Copies data back to source volume, unlinks junction, restores directory,
    /// and removes data from secondary disk (or routes Zalo through its dedicated safe undo path).
    pub fn undo_relocation(
        record_id: i64,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        proc_probe: &dyn ProcessServiceProbe,
        ctrl: &TaskControl,
    ) -> Result<RelocationRecord> {
        Self::undo_relocation_impl(
            record_id,
            db,
            volume_api,
            reparse_api,
            shell_api,
            proc_probe,
            &NativeFileLockProbe,
            None,
            ctrl,
            None,
            &NativeBreakawayHelperClient,
        )
    }

    /// 1-Click Undo with an injectable [`BreakawayHelperClient`] for deterministic Store MSIX tests.
    #[allow(clippy::too_many_arguments)]
    pub fn undo_relocation_with_breakaway(
        record_id: i64,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        proc_probe: &dyn ProcessServiceProbe,
        ctrl: &TaskControl,
        breakaway_client: &dyn BreakawayHelperClient,
    ) -> Result<RelocationRecord> {
        Self::undo_relocation_impl(
            record_id,
            db,
            volume_api,
            reparse_api,
            shell_api,
            proc_probe,
            &NativeFileLockProbe,
            None,
            ctrl,
            None,
            breakaway_client,
        )
    }

    /// 1-Click Undo with explicit lock probe, optional lifecycle API (for graceful Zalo close/reopen),
    /// and optional fault-injection hook invoked immediately before swapping the junction.
    #[allow(clippy::too_many_arguments)]
    pub fn undo_relocation_with_deps(
        record_id: i64,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        proc_probe: &dyn ProcessServiceProbe,
        lock_probe: &dyn FileLockProbe,
        lifecycle: Option<&dyn AppLifecycleApi>,
        ctrl: &TaskControl,
        before_swap_hook: Option<&dyn Fn()>,
    ) -> Result<RelocationRecord> {
        Self::undo_relocation_impl(
            record_id,
            db,
            volume_api,
            reparse_api,
            shell_api,
            proc_probe,
            lock_probe,
            lifecycle,
            ctrl,
            before_swap_hook,
            &NativeBreakawayHelperClient,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn undo_relocation_impl(
        record_id: i64,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        proc_probe: &dyn ProcessServiceProbe,
        lock_probe: &dyn FileLockProbe,
        lifecycle: Option<&dyn AppLifecycleApi>,
        ctrl: &TaskControl,
        before_swap_hook: Option<&dyn Fn()>,
        breakaway_client: &dyn BreakawayHelperClient,
    ) -> Result<RelocationRecord> {
        let _reloc_lock = RelocationLock::acquire_for_db(db.path())?;

        let records = db.list_relocations(None)?;
        let mut record = records
            .into_iter()
            .find(|r| r.id == Some(record_id))
            .ok_or_else(|| GreenCError::NotFound {
                entity: "RelocationRecord".to_string(),
                id: record_id.to_string(),
            })?;

        if record.status != RelocationStatus::Done {
            return Err(GreenCError::PreconditionBlocked {
                operation: "undo_relocation".to_string(),
                reason: format!(
                    "Bản ghi relocation ở trạng thái '{}' không thể hoàn tác",
                    record.status.as_str()
                ),
            });
        }

        let source_drive = extract_drive_letter(&record.source_path).unwrap_or('C');

        // Dedicated safe Zalo Undo path: strict copy, full verification, pre-swap re-check of
        // Zalo closed + target size/mtime unchanged, and 24h target retention instead of delete.
        if record.app_id.eq_ignore_ascii_case("zalo")
            && record.strategy == RelocationStrategyKind::Junction
        {
            return ZaloRelocator::undo_zalo_relocation_locked(
                &mut record,
                source_drive,
                db,
                volume_api,
                reparse_api,
                proc_probe,
                lock_probe,
                lifecycle,
                ctrl,
                before_swap_hook,
            );
        }

        // Verify processes are not running
        CandidateQualificationEngine::check_process_running(&record.process_names, proc_probe)?;

        // Validate free space on the destination of the undo (the source volume)
        // Enforces `free_bytes >= record.size_bytes + 512 MiB`
        CandidateQualificationEngine::check_free_space_reserve(
            source_drive,
            record.size_bytes,
            volume_api,
        )?;

        match record.strategy {
            RelocationStrategyKind::Junction => {
                let source_path = record.source_path.clone();
                let target_path = record.target_path.clone();

                if !target_path.exists() {
                    return Err(GreenCError::NotFound {
                        entity: "RelocatedTarget".to_string(),
                        id: target_path.display().to_string(),
                    });
                }

                if cfg!(feature = "store")
                    && breakaway_client.is_packaged()
                    && is_windows_appdata_app_dir(&source_path)
                {
                    let job_dir = PrivateBreakawayJobDir::create().map_err(|e| {
                        GreenCError::PreconditionBlocked {
                            operation: "undo_relocation_store_breakaway".to_string(),
                            reason: format_breakaway_unavailable_guidance(
                                "hoàn tác chuyển thư mục",
                                &e.to_string(),
                            ),
                        }
                    })?;
                    let req = RestoreHelperRequest::UndoRelocationDir {
                        source_path: source_path.clone(),
                        target_path: target_path.clone(),
                    };
                    let req_bytes = serde_json::to_vec(&req)?;
                    let exec_out = match breakaway_client.execute(&job_dir, &req_bytes) {
                        Ok(out) => out,
                        Err(err) => {
                            return Err(GreenCError::PreconditionBlocked {
                                operation: "undo_relocation_store_breakaway".to_string(),
                                reason: format_breakaway_unavailable_guidance(
                                    "hoàn tác chuyển thư mục",
                                    &err.to_string(),
                                ),
                            });
                        }
                    };
                    if exec_out.exit_code == EXIT_HELPER_STILL_IN_PACKAGE {
                        let detail =
                            serde_json::from_str::<RestoreHelperResponse>(&exec_out.stdout)
                                .ok()
                                .and_then(|r| r.error)
                                .unwrap_or_else(|| {
                                    "Tiến trình trợ giúp vẫn còn trong gói MSIX".to_string()
                                });
                        return Err(GreenCError::PreconditionBlocked {
                            operation: "undo_relocation_store_breakaway".to_string(),
                            reason: format_breakaway_unavailable_guidance(
                                "hoàn tác chuyển thư mục",
                                &detail,
                            ),
                        });
                    }
                    let resp: RestoreHelperResponse = serde_json::from_str(&exec_out.stdout)
                        .map_err(|e| {
                            GreenCError::Relocation(format!(
                                "Phản hồi hoàn tác ngoài gói không hợp lệ (exit={}): {e}; dữ liệu tại '{}' được giữ nguyên",
                                exec_out.exit_code,
                                target_path.display()
                            ))
                        })?;
                    if !resp.running_outside_package
                        || resp.error_code.as_deref() == Some("still_in_package")
                    {
                        let detail = resp.error.unwrap_or_else(|| {
                            "Không xác nhận được trạng thái ngoài gói MSIX".to_string()
                        });
                        return Err(GreenCError::PreconditionBlocked {
                            operation: "undo_relocation_store_breakaway".to_string(),
                            reason: format_breakaway_unavailable_guidance(
                                "hoàn tác chuyển thư mục",
                                &detail,
                            ),
                        });
                    }
                    if exec_out.exit_code != 0
                        || !resp.ok
                        || resp.verified_dir.as_deref() != Some(source_path.as_path())
                    {
                        let detail = resp
                            .error
                            .unwrap_or_else(|| format!("exit code {}", exec_out.exit_code));
                        return Err(GreenCError::Relocation(format!(
                            "Hoàn tác chuyển thư mục ngoài gói MSIX thất bại (dữ liệu tại '{}' được giữ nguyên): {detail}",
                            target_path.display()
                        )));
                    }

                    // Only delete secondary-disk copy AFTER outside-package verification passed!
                    if !record.target_user_repaired {
                        remove_dir_all_safe(&target_path);
                    }
                    record.status = RelocationStatus::Undone;
                    record.detail = Some(
                        "Hoàn tác di chuyển về AppData thật thành công (đã xác minh ngoài gói MSIX)"
                            .to_string(),
                    );
                    db.save_relocation(&record)?;
                    return Ok(record);
                }

                let source_parent = source_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("C:\\"));
                let source_name = source_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("folder");
                let uuid_suffix = uuid::Uuid::new_v4().to_string();
                let restore_tmp =
                    source_parent.join(format!("{source_name}.greenc-restoring.{uuid_suffix}"));

                // Step 1: Copy from target back to temporary restoring folder on source volume
                tracing::info!(
                    "Undoing relocation: copying back from {} to {}",
                    target_path.display(),
                    restore_tmp.display()
                );

                if let Err(copy_err) =
                    verified_copy_tree(&target_path, &restore_tmp, ctrl, |_, _| {})
                {
                    remove_dir_all_safe(&restore_tmp);
                    return Err(copy_err);
                }

                // Persist durable intermediate state BEFORE touching junction / renaming!
                let prev_status = record.status;
                let prev_health = record.health_status;
                let prev_detail = record.detail.clone();
                record.status = RelocationStatus::Moving;
                record.moving_path = Some(restore_tmp.clone());
                record.detail = Some("Đang hoàn tác dữ liệu về vị trí ban đầu...".to_string());
                db.save_relocation_durable(&record)?;

                // Step 2: Delete junction at source; on failure clean temp and revert to Done.
                let was_junction = reparse_api.is_reparse_point(&source_path);
                let vacate_res = if was_junction {
                    reparse_api.delete_junction(&source_path)
                } else if source_path.exists() {
                    fs::remove_dir(&source_path).map_err(GreenCError::Io)
                } else {
                    Ok(())
                };
                if let Err(vacate_err) = vacate_res {
                    remove_dir_all_safe(&restore_tmp);
                    record.status = prev_status;
                    record.health_status = prev_health;
                    record.moving_path = None;
                    record.detail = prev_detail;
                    let _ = db.save_relocation_durable(&record);
                    return Err(vacate_err);
                }

                // Step 3: Rename restore_tmp to source_path; on failure rebuild junction and
                // revert to Done (or NeedsReview + LinkBroken if junction rebuild fails).
                if let Err(rename_err) = fs::rename(&restore_tmp, &source_path) {
                    let rebuilt = if was_junction {
                        reparse_api
                            .create_junction(&source_path, &target_path)
                            .is_ok()
                    } else {
                        false
                    };
                    if rebuilt {
                        remove_dir_all_safe(&restore_tmp);
                        record.status = prev_status;
                        record.health_status = prev_health;
                        record.moving_path = None;
                        record.detail = prev_detail;
                    } else {
                        record.status = RelocationStatus::NeedsReview;
                        record.health_status = RelocationHealthStatus::LinkBroken;
                        record.moving_path = Some(restore_tmp);
                        record.detail = Some(format!(
                            "Hoàn tác thất bại ({rename_err}) và không dựng lại được liên kết tới {} — cần kiểm tra.",
                            target_path.display()
                        ));
                    }
                    let _ = db.save_relocation_durable(&record);
                    return Err(GreenCError::Io(rename_err));
                }

                // Step 4: Remove files from secondary disk — NEVER delete a user-repaired target,
                // and NEVER swallow deletion errors!
                let undo_detail = if record.target_user_repaired {
                    format!(
                        "Hoàn tác di chuyển thành công về vị trí ban đầu. GreenC giữ nguyên thư mục đích {} do người dùng chỉ định (không tự xoá).",
                        target_path.display()
                    )
                } else {
                    if target_path.exists() {
                        if let Err(del_err) = fs::remove_dir_all(to_extended_io_path(&target_path))
                        {
                            record.status = RelocationStatus::NeedsReview;
                            record.detail = Some(format!(
                                "Đã hoàn trả dữ liệu về {} nhưng không xoá được thư mục đích {}: {del_err}",
                                source_path.display(),
                                target_path.display()
                            ));
                            let _ = db.save_relocation_durable(&record);
                            return Err(GreenCError::Io(del_err));
                        }
                    }
                    "Hoàn tác di chuyển thành công về vị trí ban đầu".to_string()
                };

                // Step 5: Mark record Undone
                record.status = RelocationStatus::Undone;
                record.moving_path = None;
                record.detail = Some(undo_detail);
                db.save_relocation_durable(&record)?;

                Ok(record)
            }
            RelocationStrategyKind::KnownFolder => {
                if cfg!(feature = "store") {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "undo_relocation".to_string(),
                        reason: "Bản Microsoft Store tạm tắt hoàn tác chuyển thư mục hệ thống (Known Folders) do cơ chế ảo hoá HKCU của MSIX; dữ liệu hiện tại được giữ nguyên. Hãy đổi vị trí trong File Explorer → Properties → Location hoặc dùng bản cài EXE.".to_string(),
                    });
                }

                let source_path = record.source_path.clone();
                let target_path = record.target_path.clone();

                // Find matching GUID
                let guid = match record
                    .app_id
                    .strip_prefix("known_folder_")
                    .unwrap_or(&record.app_id)
                {
                    "downloads" => greenc_platform::FOLDERID_DOWNLOADS,
                    "documents" => greenc_platform::FOLDERID_DOCUMENTS,
                    "pictures" => greenc_platform::FOLDERID_PICTURES,
                    "videos" => greenc_platform::FOLDERID_VIDEOS,
                    "music" => greenc_platform::FOLDERID_MUSIC,
                    "desktop" => greenc_platform::FOLDERID_DESKTOP,
                    g => g,
                };

                // Move files back (never delete a user-repaired target)
                if target_path.exists() {
                    verified_copy_tree(&target_path, &source_path, ctrl, |_, _| {})?;
                    if !record.target_user_repaired {
                        if let Err(del_err) = fs::remove_dir_all(to_extended_io_path(&target_path))
                        {
                            record.status = RelocationStatus::NeedsReview;
                            record.detail = Some(format!(
                                "Đã chép lại Known Folder về {} nhưng không xoá được thư mục đích {}: {del_err}",
                                source_path.display(),
                                target_path.display()
                            ));
                            let _ = db.save_relocation(&record);
                            return Err(GreenCError::Io(del_err));
                        }
                    }
                }

                shell_api.set_known_folder(guid, &source_path)?;

                record.status = RelocationStatus::Undone;
                record.detail = Some("Hoàn tác thư mục Known Folder về vị trí ban đầu".to_string());
                db.save_relocation(&record)?;

                Ok(record)
            }
            _ => {
                record.status = RelocationStatus::Undone;
                record.detail = Some("Đã đánh dấu hoàn tác".to_string());
                db.save_relocation(&record)?;
                Ok(record)
            }
        }
    }
}

fn validate_relocation_helper_path(path: &Path, operation: &str) -> Result<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Đường dẫn '{}' không tuyệt đối hoặc chứa '..'",
                path.display()
            ),
        });
    }
    ensure_not_protected_system_path(path, operation)?;
    Ok(())
}

fn hash_file_sha256(path: &Path) -> Result<String> {
    let mut f = fs::File::open(to_extended_io_path(path))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:02x}", hasher.finalize()))
}

type FileHashMap = BTreeMap<PathBuf, (u64, String)>;

fn collect_tree_hashes(root: &Path) -> Result<(FileHashMap, u64)> {
    let mut map = BTreeMap::new();
    let mut total_bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry_res in fs::read_dir(to_extended_io_path(&dir))? {
            let entry = entry_res?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "undo_relocation_verify".to_string(),
                    reason: format!("Phát hiện symlink ngoài ý muốn tại '{}'", path.display()),
                });
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map(Path::to_path_buf)
                    .map_err(|e| GreenCError::Relocation(e.to_string()))?;
                let sha = hash_file_sha256(&path)?;
                let len = meta.len();
                total_bytes = total_bytes.saturating_add(len);
                map.insert(rel, (len, sha));
            }
        }
    }
    Ok((map, total_bytes))
}

/// Executes an `UndoRelocationDir` job inside the unpackaged `greenc-cli __restore-helper` child
/// process: strictly copies `target_path` to a temporary sibling of `source_path`, removes the
/// junction at `source_path`, renames the verified copy into `source_path`, and verifies every
/// file's existence and SHA-256 hash against `target_path` from outside the MSIX package.
/// Never deletes `target_path` on failure.
pub fn execute_undo_relocation_dir_job(
    source_path: &Path,
    target_path: &Path,
    reparse_api: &dyn ReparsePointApi,
    running_in_package: bool,
) -> (i32, RestoreHelperResponse) {
    if running_in_package {
        return (
            EXIT_HELPER_STILL_IN_PACKAGE,
            RestoreHelperResponse::still_in_package(current_package_full_name().as_deref()),
        );
    }

    if let Err(e) = validate_relocation_helper_path(source_path, "undo_relocation_source") {
        return (
            2,
            RestoreHelperResponse::failed("invalid_source_path", e.to_string()),
        );
    }
    if let Err(e) = validate_relocation_helper_path(target_path, "undo_relocation_target") {
        return (
            2,
            RestoreHelperResponse::failed("invalid_target_path", e.to_string()),
        );
    }

    let tmeta = match fs::symlink_metadata(target_path) {
        Ok(m) if !m.file_type().is_symlink() && m.is_dir() => m,
        Ok(_) => {
            return (
                2,
                RestoreHelperResponse::failed(
                    "invalid_target_dir",
                    format!(
                        "Thư mục đích '{}' là symlink hoặc không phải thư mục thật",
                        target_path.display()
                    ),
                ),
            );
        }
        Err(e) => {
            return (
                1,
                RestoreHelperResponse::failed(
                    "target_missing",
                    format!("Không thấy thư mục đích '{}': {e}", target_path.display()),
                ),
            );
        }
    };
    let _ = tmeta;

    let Some(source_parent) = source_path.parent() else {
        return (
            2,
            RestoreHelperResponse::failed(
                "invalid_source_parent",
                format!("Đường dẫn '{}' không có thư mục cha", source_path.display()),
            ),
        );
    };
    if let Err(e) = fs::create_dir_all(source_parent) {
        return (
            1,
            RestoreHelperResponse::failed(
                "create_parent_failed",
                format!(
                    "Không tạo được thư mục cha '{}': {e}",
                    source_parent.display()
                ),
            ),
        );
    }

    let restore_tmp = source_parent.join(format!("restoring.{}", uuid::Uuid::new_v4()));
    let ctrl = TaskControl::new();
    if let Err(e) = verified_copy_tree_strict(target_path, &restore_tmp, &ctrl, |_, _| {}) {
        remove_dir_all_safe(&restore_tmp);
        return (
            1,
            RestoreHelperResponse::failed(
                "copy_failed",
                format!(
                    "Sao chép hoàn tác từ '{}' thất bại: {e}",
                    target_path.display()
                ),
            ),
        );
    }

    // Vacate `source_path` safely: only remove a junction or an empty directory, NEVER delete user data.
    if reparse_api.is_reparse_point(source_path) {
        if let Err(e) = reparse_api.delete_junction(source_path) {
            remove_dir_all_safe(&restore_tmp);
            return (
                1,
                RestoreHelperResponse::failed(
                    "delete_junction_failed",
                    format!(
                        "Không xoá được liên kết tại '{}': {e}",
                        source_path.display()
                    ),
                ),
            );
        }
    } else if fs::symlink_metadata(source_path).is_ok() {
        if let Err(e) = fs::remove_dir(source_path) {
            remove_dir_all_safe(&restore_tmp);
            return (
                2,
                RestoreHelperResponse::failed(
                    "source_occupied",
                    format!(
                        "Thư mục '{}' đang có dữ liệu hoặc không thể dọn rỗng ({e}); giữ nguyên mọi bản",
                        source_path.display()
                    ),
                ),
            );
        }
    }

    if let Err(e) = fs::rename(&restore_tmp, source_path) {
        return (
            1,
            RestoreHelperResponse::failed(
                "rename_failed",
                format!(
                    "Không đổi tên được bản hoàn tác về '{}': {e}",
                    source_path.display()
                ),
            ),
        );
    }

    // Verify from outside the package that `source_path` is a real directory matching `target_path`
    // file-for-file with exact SHA-256 digests.
    if reparse_api.is_reparse_point(source_path)
        || !fs::symlink_metadata(source_path)
            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir())
    {
        return (
            1,
            RestoreHelperResponse::failed(
                "verify_dir_failed",
                format!(
                    "Xác minh ngoài gói thất bại: '{}' không phải thư mục thật sau hoàn tác",
                    source_path.display()
                ),
            ),
        );
    }

    let (expected_map, expected_bytes) = match collect_tree_hashes(target_path) {
        Ok(v) => v,
        Err(e) => {
            return (
                1,
                RestoreHelperResponse::failed(
                    "verify_target_read_failed",
                    format!("Không đọc được cây đích để đối chiếu: {e}"),
                ),
            );
        }
    };
    let (actual_map, _) = match collect_tree_hashes(source_path) {
        Ok(v) => v,
        Err(e) => {
            return (
                1,
                RestoreHelperResponse::failed(
                    "verify_source_read_failed",
                    format!("Không đọc được cây sau hoàn tác để đối chiếu: {e}"),
                ),
            );
        }
    };

    if expected_map != actual_map {
        return (
            1,
            RestoreHelperResponse::failed(
                "integrity_mismatch",
                format!(
                    "Xác minh ngoài gói thất bại: tệp/băm SHA-256 tại '{}' không khớp '{}'",
                    source_path.display(),
                    target_path.display()
                ),
            ),
        );
    }

    (
        0,
        RestoreHelperResponse {
            ok: true,
            running_outside_package: true,
            error_code: None,
            error: None,
            files: Vec::new(),
            verified_dir: Some(source_path.to_path_buf()),
            verified_file_count: expected_map.len() as u64,
            verified_bytes: expected_bytes,
        },
    )
}

/// Extracts the drive letter from a Windows PathBuf/Path (e.g., 'C' from "C:\\Users" or "\\\\?\\C:\\...").
fn extract_drive_letter(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let s_clean = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let mut chars = s_clean.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}
