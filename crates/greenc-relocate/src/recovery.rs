//! Crash recovery state machine and live junction health check (Tasks T039, T046, GC-ZALO2).

use crate::candidate::CandidateScanner;
use crate::verified_copy::{remove_dir_all_safe, to_extended_io_path};
use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
    ZaloRetainedOriginal, ZaloRetainedStatus,
};
use greenc_platform::traits::ReparsePointApi;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};

pub struct RecoveryService;

/// Fail-closed check: returns `true` if `path` exists and is a non-empty directory, a file,
/// a symlink, or cannot be read (permission/I/O error). Returns `false` only when `path` is
/// verified absent or a readable empty directory.
fn path_has_content_or_uncertain(path: &Path) -> bool {
    let io_path = to_extended_io_path(path);
    let meta = match fs::symlink_metadata(&io_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true, // fail-closed: cannot inspect => assume non-empty
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return true;
    }
    match fs::read_dir(&io_path) {
        Ok(mut rd) => rd.next().is_some(),
        Err(_) => true, // fail-closed: unreadable dir => assume non-empty
    }
}

impl RecoveryService {
    /// Inspects the database for relocations interrupted in the `Moving` state.
    /// - Case A (valid junction pointing to existing target):
    ///   - For Zalo: preserves any `.greenc-moving.*` as `.greenc-original.<uuid>` in DB
    ///     (`health_verified = false`, never auto-deleted) and marks `Done`.
    ///   - For other apps: cleans leftover `.greenc-moving.*` and marks `Done`.
    /// - Case B (junction missing or broken):
    ///   - ONLY restores `.greenc-moving.*` -> `source` and removes partial `target` when
    ///     `source` is absent/empty, `moving_dir` exists, and `rename(moving_dir, source)` succeeds.
    ///   - In ANY ambiguous state (non-empty `source`, missing `moving_dir` with non-empty
    ///     `target`, or failed rename): keeps ALL copies with content untouched and marks
    ///     `RelocationStatus::NeedsReview` so the user can review safely.
    pub fn recover_interrupted(
        db: &GreenCDb,
        reparse: &dyn ReparsePointApi,
    ) -> Result<Vec<RelocationRecord>> {
        let interrupted = db.list_relocations(Some("moving"))?;
        let mut recovered = Vec::new();

        for mut record in interrupted {
            tracing::warn!(
                "Found interrupted relocation record #{} for '{}'",
                record.id.unwrap_or(0),
                record.app_name
            );

            let source = record.source_path.clone();
            let target = record.target_path.clone();

            let source_name = source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("folder");
            let source_parent = source
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("C:\\"));

            // Check if junction exists at source and points to target
            let has_valid_junction = if reparse.is_reparse_point(&source) {
                if let Ok(jt) = reparse.get_junction_target(&source) {
                    greenc_core::models::same_windows_path(&jt, &target)
                } else {
                    false
                }
            } else {
                false
            };

            if has_valid_junction && target.exists() {
                // Case A: Junction was successfully created before the crash.
                if record.app_id.eq_ignore_ascii_case("zalo") {
                    let moving_dirs = Self::find_all_moving_dirs(&source_parent, source_name);
                    let mut retained_notes = Vec::new();
                    let now = Utc::now();
                    for moving_dir in moving_dirs {
                        let orig_name =
                            format!("{}.greenc-original.{}", source_name, uuid::Uuid::new_v4());
                        let orig_path = source_parent.join(&orig_name);
                        let (final_path, status, note) = match fs::rename(&moving_dir, &orig_path) {
                            Ok(()) => (
                                orig_path,
                                ZaloRetainedStatus::Retained,
                                "Giữ tạm sau khôi phục sự cố (chưa qua theo dõi tự động)"
                                    .to_string(),
                            ),
                            Err(err) => (
                                moving_dir,
                                ZaloRetainedStatus::NeedsReview,
                                format!("Không đổi tên được thư mục tạm sau sự cố: {err}"),
                            ),
                        };
                        let (size_bytes, file_count) =
                            CandidateScanner::compute_dir_size(&final_path);
                        let _ = db.save_zalo_retained_original(&ZaloRetainedOriginal {
                            id: None,
                            relocation_id: record.id.unwrap_or(0),
                            source_path: source.clone(),
                            target_path: target.clone(),
                            retained_path: final_path.clone(),
                            size_bytes,
                            file_count,
                            health_verified: false, // Never auto-purge after crash without user check
                            created_at: now,
                            purge_after: now + ChronoDuration::hours(24),
                            status,
                            resolved_at: None,
                            detail: Some(note),
                        });
                        retained_notes.push(final_path.display().to_string());
                    }

                    record.status = RelocationStatus::Done;
                    record.health_status = RelocationHealthStatus::Healthy;
                    record.detail = Some(if retained_notes.is_empty() {
                        "Khôi phục sau gián đoạn: Liên kết Zalo hợp lệ".to_string()
                    } else {
                        format!(
                            "Khôi phục sau gián đoạn: Liên kết Zalo hợp lệ; bản gốc được giữ tạm an toàn tại {}",
                            retained_notes.join(", ")
                        )
                    });
                    db.save_relocation(&record)?;
                    recovered.push(record);
                } else {
                    Self::clean_leftover_moving_dirs(&source_parent, source_name);

                    record.status = RelocationStatus::Done;
                    record.health_status = RelocationHealthStatus::Healthy;
                    record.detail = Some(
                        "Khôi phục sau gián đoạn: Liên kết junction hợp lệ, đã dọn tệp tạm"
                            .to_string(),
                    );
                    db.save_relocation(&record)?;
                    recovered.push(record);
                }
            } else {
                // Case B: Junction creation did not happen or link is broken.
                // NEVER delete any directory that contains data or cannot be inspected.
                if reparse.is_reparse_point(&source) {
                    let _ = reparse.delete_junction(&source);
                } else if fs::symlink_metadata(&source).is_ok()
                    && !path_has_content_or_uncertain(&source)
                {
                    // Source is a verified empty regular directory shell left from an interrupted
                    // junction creation before setting the reparse tag; removing an empty dir
                    // cannot delete any user file.
                    let _ = fs::remove_dir(&source);
                }

                let source_occupied = fs::symlink_metadata(&source).is_ok();
                let moving_dir = Self::find_moving_dir(&source_parent, source_name);
                let target_has_content = path_has_content_or_uncertain(&target);

                if !source_occupied {
                    if let Some(mdir) = moving_dir {
                        match fs::rename(&mdir, &source) {
                            Ok(()) => {
                                // Verified clean rollback: original `moving_dir` is now back at `source`.
                                remove_dir_all_safe(&target);
                                record.status = RelocationStatus::Failed;
                                record.health_status = RelocationHealthStatus::LinkBroken;
                                record.detail =
                                    Some("Bị gián đoạn, đã khôi phục về vị trí cũ".to_string());
                                db.save_relocation(&record)?;
                                recovered.push(record);
                                continue;
                            }
                            Err(rename_err) => {
                                // Rename failed: keep BOTH `moving_dir` and `target`!
                                let mut kept = vec![mdir.display().to_string()];
                                if target_has_content {
                                    kept.push(target.display().to_string());
                                }
                                record.status = RelocationStatus::NeedsReview;
                                record.health_status = RelocationHealthStatus::LinkBroken;
                                record.detail = Some(format!(
                                    "Bị gián đoạn, không thể trả bản gốc về chỗ cũ ({rename_err}) — cần người dùng xử lý. Đã giữ nguyên: {}",
                                    kept.join(", ")
                                ));
                                db.save_relocation(&record)?;
                                recovered.push(record);
                                continue;
                            }
                        }
                    }
                }

                // Ambiguous or multi-copy state (e.g., `source` is non-empty AND `moving_dir`
                // exists, OR `moving_dir` is missing while `target` has content):
                // DO NOT delete anything! Keep every copy and mark `NeedsReview`.
                let mut preserved = Vec::new();
                if path_has_content_or_uncertain(&source) {
                    preserved.push(format!("nguồn ({})", source.display()));
                }
                if let Some(ref mdir) = moving_dir {
                    preserved.push(format!("bản gốc tạm ({})", mdir.display()));
                }
                if target_has_content {
                    preserved.push(format!("đích ({})", target.display()));
                }

                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.detail = Some(if preserved.is_empty() {
                    "Bị gián đoạn, không tìm thấy dữ liệu để khôi phục tự động — cần người dùng xử lý"
                        .to_string()
                } else {
                    format!(
                        "Bị gián đoạn, phát hiện dữ liệu ở nhiều nơi — cần người dùng xử lý. GreenC đã giữ nguyên tất cả bản: {}",
                        preserved.join("; ")
                    )
                });
                db.save_relocation(&record)?;
                recovered.push(record);
            }
        }

        Ok(recovered)
    }

    /// Evaluates live junction health status for a slice of relocation records.
    pub fn check_junction_health(
        records: &[RelocationRecord],
        reparse: &dyn ReparsePointApi,
    ) -> Vec<RelocationHealthStatus> {
        records
            .iter()
            .map(|r| {
                if r.status != RelocationStatus::Done {
                    return r.health_status;
                }

                if r.strategy != RelocationStrategyKind::Junction {
                    return RelocationHealthStatus::Healthy;
                }

                Self::check_single_junction_live(&r.source_path, &r.target_path, reparse)
            })
            .collect()
    }

    /// Evaluates live junction health for a `(source_path, target_path)` pair regardless of DB status.
    pub fn check_single_junction_live(
        source_path: &Path,
        target_path: &Path,
        reparse: &dyn ReparsePointApi,
    ) -> RelocationHealthStatus {
        // 1. Check if target directory exists and is reachable
        if !target_path.exists() {
            return RelocationHealthStatus::TargetOffline;
        }

        // 2. Check if source path is still a reparse point
        if !reparse.is_reparse_point(source_path) {
            return RelocationHealthStatus::LinkBroken;
        }

        // 3. Verify junction target points to the registered target path
        if let Ok(link_target) = reparse.get_junction_target(source_path) {
            if greenc_core::models::same_windows_path(&link_target, target_path) {
                RelocationHealthStatus::Healthy
            } else {
                RelocationHealthStatus::LinkBroken
            }
        } else {
            RelocationHealthStatus::LinkBroken
        }
    }

    /// Repairs a broken junction by re-creating the link to a new or reconnected target path.
    pub fn repair_junction_target(
        record_id: i64,
        new_target_path: &Path,
        db: &GreenCDb,
        reparse: &dyn ReparsePointApi,
    ) -> Result<RelocationRecord> {
        let records = db.list_relocations(None)?;
        let mut record = records
            .into_iter()
            .find(|r| r.id == Some(record_id))
            .ok_or_else(|| GreenCError::NotFound {
                entity: "RelocationRecord".to_string(),
                id: record_id.to_string(),
            })?;

        if !new_target_path.exists() || !new_target_path.is_dir() {
            return Err(GreenCError::NotFound {
                entity: "NewTargetPath".to_string(),
                id: new_target_path.display().to_string(),
            });
        }

        // Delete old junction link if present
        if reparse.is_reparse_point(&record.source_path) {
            let _ = reparse.delete_junction(&record.source_path);
        }

        // Re-create junction pointing to new target
        reparse.create_junction(&record.source_path, new_target_path)?;

        record.target_path = new_target_path.to_path_buf();
        record.health_status = RelocationHealthStatus::Healthy;
        record.detail = Some(format!(
            "Đã cập nhật liên kết sang {}",
            new_target_path.display()
        ));
        db.save_relocation(&record)?;

        Ok(record)
    }

    fn find_moving_dir(parent: &Path, source_name: &str) -> Option<PathBuf> {
        Self::find_all_moving_dirs(parent, source_name)
            .into_iter()
            .next()
    }

    fn find_all_moving_dirs(parent: &Path, source_name: &str) -> Vec<PathBuf> {
        let prefix = format!("{}.greenc-moving.", source_name);
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) && entry.path().is_dir() {
                    out.push(entry.path());
                }
            }
        }
        out
    }

    fn clean_leftover_moving_dirs(parent: &Path, source_name: &str) {
        for path in Self::find_all_moving_dirs(parent, source_name) {
            remove_dir_all_safe(&path);
        }
    }
}
