//! Transactional Mover implementing the 4-phase safe relocation state machine and 1-click Undo (Tasks T039, T046).

use crate::candidate::{CandidateQualificationEngine, RelocationCandidate};
use crate::verified_copy::{remove_dir_all_safe, verified_copy_tree};
use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    ProcessServiceProbe, ReparsePointApi, ShellFolderApi, Win32VolumeApi,
};
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};

pub struct MoveOptions {
    pub target_drive: char,
    pub custom_target_path: Option<PathBuf>,
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

        // 3. Phase 2: Persist state as `Moving` before touching source filesystem
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
        };

        let rec_id = db.save_relocation(&record)?;
        record.id = Some(rec_id);

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
            let clean_v = verified_target.to_string_lossy().to_lowercase();
            let clean_t = target_path.to_string_lossy().to_lowercase();
            if clean_v != clean_t && !clean_v.ends_with(&clean_t) && !clean_t.ends_with(&clean_v) {
                return Err(GreenCError::Relocation(format!(
                    "Junction target mismatch: expected {}, got {}",
                    clean_t, clean_v
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
    /// and removes data from secondary disk.
    pub fn undo_relocation(
        record_id: i64,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        proc_probe: &dyn ProcessServiceProbe,
        ctrl: &TaskControl,
    ) -> Result<RelocationRecord> {
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

        // Verify processes are not running
        CandidateQualificationEngine::check_process_running(&record.process_names, proc_probe)?;

        // Validate free space on the destination of the undo (the source volume)
        // Enforces `free_bytes >= record.size_bytes + 512 MiB`
        let source_drive = extract_drive_letter(&record.source_path).unwrap_or('C');
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

                let source_parent = source_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("C:\\"));
                let uuid_suffix = uuid::Uuid::new_v4().to_string();
                let restore_tmp = source_parent.join(format!("restoring.{}", uuid_suffix));

                // Step 1: Copy from target back to temporary restoring folder on source volume
                tracing::info!(
                    "Undoing relocation: copying back from {} to {}",
                    target_path.display(),
                    restore_tmp.display()
                );

                verified_copy_tree(&target_path, &restore_tmp, ctrl, |_, _| {})?;

                // Step 2: Delete junction at source
                if reparse_api.is_reparse_point(&source_path) {
                    reparse_api.delete_junction(&source_path)?;
                } else if source_path.exists() {
                    let _ = fs::remove_dir(&source_path);
                }

                // Step 3: Rename restore_tmp to source_path
                fs::rename(&restore_tmp, &source_path)?;

                // Step 4: Remove files from secondary disk
                remove_dir_all_safe(&target_path);

                // Step 5: Mark record Undone
                record.status = RelocationStatus::Undone;
                record.detail = Some("Hoàn tác di chuyển thành công về vị trí ban đầu".to_string());
                db.save_relocation(&record)?;

                Ok(record)
            }
            RelocationStrategyKind::KnownFolder => {
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

                // Move files back
                if target_path.exists() {
                    verified_copy_tree(&target_path, &source_path, ctrl, |_, _| {})?;
                    remove_dir_all_safe(&target_path);
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
