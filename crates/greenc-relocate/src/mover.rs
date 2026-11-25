//! Transactional Mover implementing the 4-phase safe relocation state machine and 1-click Undo (Tasks T039, T046).

use crate::candidate::{CandidateQualificationEngine, RelocationCandidate};
use crate::recovery::path_has_content_or_uncertain;
use crate::verified_copy::{remove_dir_all_safe, verified_copy_tree_manifest, CopyTreeOptions};
use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    ProcessServiceProbe, ReparsePointApi, ShellFolderApi, Win32VolumeApi,
};
use greenc_platform::RelocationLock;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};

pub struct MoveOptions {
    pub target_drive: char,
    pub custom_target_path: Option<PathBuf>,
}

/// `true` when Zalo chat data lives in or below `path`. Plain "Zalo Received Files" does not count.
pub(crate) fn contains_zalo_chat_data(path: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    let root = norm(path);
    let root = root.trim_end_matches('\\');
    if greenc_scanner::zalo::discover_zalo_installation(&Default::default())
        .and_then(|d| d.data_dir)
        .is_some_and(|d| {
            let d = norm(&d);
            d == root || d.starts_with(&format!("{root}\\"))
        })
    {
        return true;
    }
    let mut stack = vec![(path.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            if entry.file_name().eq_ignore_ascii_case("zalo data") {
                return true;
            }
            if depth < 3 && entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((p, depth + 1));
            }
        }
    }
    false
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
        let _lock = RelocationLock::acquire_for_db(db.path())?;
        if reparse_api.is_reparse_point(&source_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason: format!("{} đã là một liên kết", source_path.display()),
            });
        }

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

        let (copy_stats, manifest) = verified_copy_tree_manifest(
            &source_path,
            &target_path,
            // A copy without the links inside would look complete but break the app.
            &CopyTreeOptions {
                fail_on_reparse: true,
                ..CopyTreeOptions::default()
            },
            ctrl,
            on_progress,
        )?;

        let uuid_suffix = uuid::Uuid::new_v4().to_string();
        let source_name = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("folder");
        let source_parent = source_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        let moving_dir_name = format!("{source_name}.greenc-moving.{uuid_suffix}");
        let moving_path = source_parent.join(&moving_dir_name);

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
            // Crash recovery only ever touches the folder named here.
            detail: Some(format!("Đang chuyển; bản gốc tạm: {moving_dir_name}")),
        };

        let rec_id = db.save_relocation(&record)?;
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

        // Anything written after it was copied exists only in the source: put it back and stop.
        if !manifest.matches_tree(&moving_path) {
            let restored = fs::rename(&moving_path, &source_path);
            let reason = format!(
                "Dữ liệu trong {} thay đổi trong lúc chuyển; đã giữ nguyên bản gốc",
                source_path.display()
            );
            match restored {
                Ok(()) => {
                    remove_dir_all_safe(&target_path);
                    record.status = RelocationStatus::Failed;
                    record.detail = Some(reason.clone());
                }
                // Stays `Moving`: crash recovery renames the original back.
                Err(e) => {
                    record.detail = Some(format!("{reason} tại {} ({e})", moving_path.display()))
                }
            }
            let _ = db.save_relocation(&record);
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate".to_string(),
                reason,
            });
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
            if reparse_api.is_reparse_point(&source_path) {
                let _ = reparse_api.delete_junction(&source_path);
            } else {
                // Creating a junction starts with an empty folder; non-empty ones are kept.
                let _ = fs::remove_dir(&source_path);
            }
            let reason = format!("Lỗi tạo liên kết NTFS Junction: {j_err}");
            match fs::rename(&moving_path, &source_path) {
                Ok(()) => {
                    remove_dir_all_safe(&target_path);
                    record.status = RelocationStatus::Failed;
                    record.detail = Some(reason);
                }
                // Stays `Moving` with both copies: crash recovery puts the original back.
                Err(e) => {
                    record.detail = Some(format!(
                        "{reason}; bản gốc đang ở {} ({e})",
                        moving_path.display()
                    ));
                }
            }
            let _ = db.save_relocation(&record);
            return Err(j_err);
        }

        // 5. Phase 4: Purge temporary .greenc-moving.<uuid> folder & mark Done
        // Only files that were copied and did not change since are removed.
        manifest.remove_copied(&moving_path, &target_path);
        let mut detail = None;
        if fs::symlink_metadata(&moving_path).is_ok() {
            tracing::warn!("Some files were kept in {}", moving_path.display());
            detail = Some(format!("Còn tệp chưa xóa trong {moving_dir_name}"));
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
        let _lock = RelocationLock::acquire_for_db(db.path())?;
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
        // Records only list what was running when the folder was found, so ask the catalog too.
        let mut processes = record.process_names.clone();
        processes.extend(
            crate::catalog::get_app_catalog()
                .into_iter()
                .filter(|p| p.app_id == record.app_id)
                .flat_map(|p| p.process_names),
        );
        CandidateQualificationEngine::check_process_running(&processes, proc_probe)?;

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

                let is_link = reparse_api.is_reparse_point(&source_path);
                let blocked = if is_link {
                    !reparse_api
                        .get_junction_target(&source_path)
                        .is_ok_and(|t| greenc_core::models::same_windows_path(&t, &target_path))
                } else {
                    path_has_content_or_uncertain(&source_path)
                };
                if blocked {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "undo_relocation".to_string(),
                        reason: format!(
                            "{} không còn là liên kết do GreenC tạo; chưa thay đổi gì.",
                            source_path.display()
                        ),
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

                // Records made before Zalo folders were excluded have no process names.
                if holds_zalo_data(&source_path) || holds_zalo_data(&target_path) {
                    let names: Vec<String> = greenc_scanner::zalo::ZALO_PROCESS_NAMES
                        .iter()
                        .map(|n| n.to_string())
                        .collect();
                    CandidateQualificationEngine::check_process_running(&names, proc_probe)?;
                }

                let (_, manifest) = verified_copy_tree_manifest(
                    &target_path,
                    &restore_tmp,
                    &CopyTreeOptions::default(),
                    ctrl,
                    |_, _| {},
                )?;
                if !manifest.matches_tree(&target_path) {
                    remove_dir_all_safe(&restore_tmp);
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "undo_relocation".to_string(),
                        reason: format!(
                            "Dữ liệu tại {} thay đổi trong lúc hoàn tác; chưa thay đổi gì. Hãy đóng ứng dụng rồi thử lại.",
                            target_path.display()
                        ),
                    });
                }

                // Step 2: Move the link aside; it is only deleted once the folder is in place
                let old_link = source_parent.join(format!(".greenc-oldlink.{uuid_suffix}"));
                if is_link {
                    if let Err(e) = reparse_api.rename_link(&source_path, &old_link) {
                        remove_dir_all_safe(&restore_tmp);
                        return Err(e);
                    }
                } else {
                    let _ = fs::remove_dir(&source_path);
                }

                // Step 3: Rename restore_tmp to source_path
                if let Err(e) = fs::rename(&restore_tmp, &source_path) {
                    // The target still holds everything: go back to the linked state.
                    remove_dir_all_safe(&restore_tmp);
                    if is_link {
                        if let Err(link_err) = reparse_api.rename_link(&old_link, &source_path) {
                            tracing::error!("Could not restore the link after undo: {link_err}");
                        }
                    }
                    return Err(GreenCError::Io(e));
                }
                if is_link {
                    let _ = reparse_api.delete_junction(&old_link);
                }

                // Step 4: Remove from the secondary disk only what was copied back
                manifest.remove_copied(&target_path, &source_path);

                // Step 5: Mark record Undone
                record.status = RelocationStatus::Undone;
                record.detail = Some(if target_path.exists() {
                    format!(
                        "Hoàn tác di chuyển thành công; phần còn lại được giữ tại {}",
                        target_path.display()
                    )
                } else {
                    "Hoàn tác di chuyển thành công về vị trí ban đầu".to_string()
                });
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

                if !target_path.exists() {
                    return Err(GreenCError::NotFound {
                        entity: "RelocatedTarget".to_string(),
                        id: target_path.display().to_string(),
                    });
                }

                // Move files back
                let (_, manifest) = verified_copy_tree_manifest(
                    &target_path,
                    &source_path,
                    &CopyTreeOptions::default(),
                    ctrl,
                    |_, _| {},
                )?;

                // Windows must point at the restored folder before anything is removed.
                shell_api.set_known_folder(guid, &source_path)?;
                manifest.remove_copied(&target_path, &source_path);

                record.status = RelocationStatus::Undone;
                record.detail = Some(if target_path.exists() {
                    format!(
                        "Hoàn tác thư mục Known Folder về vị trí ban đầu; phần còn lại được giữ tại {}",
                        target_path.display()
                    )
                } else {
                    "Hoàn tác thư mục Known Folder về vị trí ban đầu".to_string()
                });
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
