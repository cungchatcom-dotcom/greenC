//! Crash recovery state machine and live junction health check (Tasks T039, T046, GC-ZALO2).

use crate::candidate::CandidateScanner;
use crate::verified_copy::{is_meta_reparse_or_symlink, to_extended_io_path};
use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
    ZaloRetainedOriginal, ZaloRetainedStatus,
};
use greenc_platform::traits::ReparsePointApi;
use greenc_platform::RelocationLock;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct RecoveryService;

/// Fail-closed check: returns `true` if `path` exists and is a non-empty directory, a file,
/// a symlink, or cannot be read (permission/I/O error). Returns `false` only when `path` is
/// verified absent or a readable empty directory.
pub(crate) fn path_has_content_or_uncertain(path: &Path) -> bool {
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

/// Deletes from `redundant` the files that `keep` also holds in an equal or newer version, then
/// the folders left empty. Returns `true` when nothing remains. Links are never followed.
fn remove_files_held_elsewhere(redundant: &Path, keep: &Path) -> bool {
    // Some file systems round timestamps (exFAT: 2 s).
    const ROUNDING: Duration = Duration::from_secs(2);
    let io_path = to_extended_io_path(redundant);
    let Ok(entries) = fs::read_dir(&io_path) else {
        return fs::symlink_metadata(&io_path).is_err();
    };
    for entry in entries.flatten() {
        let (path, kept) = (
            redundant.join(entry.file_name()),
            keep.join(entry.file_name()),
        );
        let Ok(meta) = entry.metadata() else { continue };
        if is_meta_reparse_or_symlink(&meta) {
            continue;
        }
        if meta.is_dir() {
            remove_files_held_elsewhere(&path, &kept);
            continue;
        }
        let held = fs::symlink_metadata(to_extended_io_path(&kept)).is_ok_and(|k| {
            let (Ok(old), Ok(new)) = (meta.modified(), k.modified()) else {
                return false;
            };
            let same_version = k.len() == meta.len() && old <= new + ROUNDING;
            k.is_file() && (same_version || new > old + ROUNDING)
        });
        if held {
            let _ = fs::remove_file(to_extended_io_path(&path));
        }
    }
    fs::remove_dir(&io_path).is_ok()
}

/// `true` when one path equals or lies inside the other.
fn paths_overlap(a: &Path, b: &Path) -> bool {
    let resolve = |p: &Path| {
        // The link itself must not be followed, only the folders above it.
        let full = match (
            p.parent().and_then(|d| fs::canonicalize(d).ok()),
            p.file_name(),
        ) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => p.to_path_buf(),
        };
        full.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .split(['\\', '/'])
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let (a, b) = (resolve(a), resolve(b));
    a.starts_with(&b) || b.starts_with(&a)
}

impl RecoveryService {
    /// Inspects the database for relocations interrupted in the `Moving` state.
    /// - Case A (valid junction pointing to existing target):
    ///   - For Zalo: preserves any `.greenc-moving.*` as `.greenc-original.<uuid>` in DB
    ///     (`health_verified = false`, never auto-deleted) and marks `Done`.
    ///   - For other apps: removes from its `.greenc-moving.*` what the target also holds and
    ///     marks `Done`.
    /// - Link to a target that is offline: nothing is touched, retried on the next run.
    /// - Link to any other place: not GreenC's link, marks `NeedsReview`.
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
        // A `Moving` record may belong to a move still running in the app or the command line.
        let Some(_lock) = RelocationLock::try_acquire_for_db(db.path())? else {
            return Ok(Vec::new());
        };
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
            let is_link = reparse.is_reparse_point(&source);
            let has_valid_junction = is_link
                && reparse
                    .get_junction_target(&source)
                    .is_ok_and(|jt| greenc_core::models::same_windows_path(&jt, &target));

            if is_link && !(has_valid_junction && target.exists()) {
                // The original may already be partly deleted, so it must not replace the link.
                if has_valid_junction {
                    record.health_status = RelocationHealthStatus::TargetOffline;
                    record.detail = Some(format!(
                        "Chưa truy cập được {}; GreenC sẽ kiểm tra lại khi ổ đĩa sẵn sàng",
                        target.display()
                    ));
                } else {
                    record.status = RelocationStatus::NeedsReview;
                    record.health_status = RelocationHealthStatus::LinkBroken;
                    record.detail = Some(format!(
                        "Bị gián đoạn, {} đang trỏ tới nơi khác — cần người dùng xử lý. GreenC đã giữ nguyên tất cả",
                        source.display()
                    ));
                }
                db.save_relocation(&record)?;
                if record.status == RelocationStatus::NeedsReview {
                    recovered.push(record);
                }
                continue;
            }

            let moving_dir =
                Self::find_moving_dir(&source_parent, source_name, record.id.unwrap_or(0));

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
                    let kept = moving_dir.filter(|dir| !remove_files_held_elsewhere(dir, &target));

                    record.status = RelocationStatus::Done;
                    record.health_status = RelocationHealthStatus::Healthy;
                    record.detail = Some(match kept {
                        None => "Khôi phục sau gián đoạn: Liên kết junction hợp lệ, đã dọn tệp tạm"
                            .to_string(),
                        Some(dir) => format!(
                            "Khôi phục sau gián đoạn: Liên kết junction hợp lệ; tệp chưa có ở đích được giữ tại {}",
                            dir.display()
                        ),
                    });
                    db.save_relocation(&record)?;
                    recovered.push(record);
                }
            } else {
                // Case B: Junction creation did not happen or link is broken.
                // NEVER delete any directory that contains data or cannot be inspected.
                if fs::symlink_metadata(&source).is_ok() && !path_has_content_or_uncertain(&source)
                {
                    // Source is a verified empty regular directory shell left from an interrupted
                    // junction creation before setting the reparse tag; removing an empty dir
                    // cannot delete any user file.
                    let _ = fs::remove_dir(&source);
                }

                let source_occupied = fs::symlink_metadata(&source).is_ok();
                let target_has_content = path_has_content_or_uncertain(&target);

                if !source_occupied {
                    if let Some(mdir) = moving_dir {
                        match fs::rename(&mdir, &source) {
                            Ok(()) => {
                                // Verified clean rollback: original `moving_dir` is now back at `source`.
                                record.health_status = RelocationHealthStatus::LinkBroken;
                                if remove_files_held_elsewhere(&target, &source) {
                                    record.status = RelocationStatus::Failed;
                                    record.detail =
                                        Some("Bị gián đoạn, đã khôi phục về vị trí cũ".to_string());
                                } else {
                                    record.status = RelocationStatus::NeedsReview;
                                    record.detail = Some(format!(
                                        "Bị gián đoạn, đã khôi phục về vị trí cũ; tệp chỉ có ở đích được giữ tại {} — cần người dùng xử lý",
                                        target.display()
                                    ));
                                }
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
        let _lock = RelocationLock::acquire_for_db(db.path())?;
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

        let refuse = |reason: String| {
            Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason,
            })
        };
        let source = record.source_path.clone();
        if record.strategy != RelocationStrategyKind::Junction
            || !matches!(
                record.status,
                RelocationStatus::Done | RelocationStatus::NeedsReview
            )
        {
            return refuse(format!(
                "Bản ghi ở trạng thái '{}' không có liên kết để sửa",
                record.status.as_str()
            ));
        }
        if paths_overlap(&source, new_target_path) {
            return refuse(format!(
                "{} và {} nằm trong nhau nên không thể liên kết",
                source.display(),
                new_target_path.display()
            ));
        }
        let old_target = if reparse.is_reparse_point(&source) {
            Some(reparse.get_junction_target(&source)?)
        } else if path_has_content_or_uncertain(&source) {
            return refuse(format!(
                "{} đang là thư mục có dữ liệu; chưa thay đổi gì",
                source.display()
            ));
        } else {
            None
        };

        // Prove that a link to the new target works here before giving up the current one: an old
        // link whose drive is offline could not be created again.
        let probe = source.with_file_name(format!(".greenc-relink.{}", uuid::Uuid::new_v4()));
        Self::create_verified_junction(&probe, new_target_path, reparse)?;
        reparse.delete_junction(&probe)?;

        if old_target.is_some() {
            reparse.delete_junction(&source)?;
        }
        if let Err(e) = Self::create_verified_junction(&source, new_target_path, reparse) {
            if let Some(old) = old_target {
                if let Err(link_err) = reparse.create_junction(&source, &old) {
                    tracing::error!("Could not restore the previous link: {link_err}");
                }
            }
            return Err(e);
        }

        record.target_path = new_target_path.to_path_buf();
        record.health_status = RelocationHealthStatus::Healthy;
        record.detail = Some(format!(
            "Đã cập nhật liên kết sang {}",
            new_target_path.display()
        ));
        db.save_relocation(&record)?;

        Ok(record)
    }

    /// Creates a junction and checks where it points; leaves nothing behind on failure.
    fn create_verified_junction(
        link: &Path,
        target: &Path,
        reparse: &dyn ReparsePointApi,
    ) -> Result<()> {
        let res = reparse.create_junction(link, target).and_then(|()| {
            let got = reparse.get_junction_target(link)?;
            if greenc_core::models::same_windows_path(&got, target) {
                Ok(())
            } else {
                Err(GreenCError::Relocation(format!(
                    "Junction target mismatch: expected {}, got {}",
                    target.display(),
                    got.display()
                )))
            }
        });
        if res.is_err() {
            if reparse.is_reparse_point(link) {
                let _ = reparse.delete_junction(link);
            } else {
                let _ = fs::remove_dir(link); // only ever removes an empty folder
            }
        }
        res
    }

    /// The temporary folder of record `record_id`: the one labelled with it, or, for moves started
    /// by an older version, the only unlabelled one.
    fn find_moving_dir(parent: &Path, source_name: &str, record_id: i64) -> Option<PathBuf> {
        let prefix = format!("{source_name}.greenc-moving.");
        let own = format!("r{record_id}-");
        let mut unlabelled = Vec::new();
        for path in Self::find_all_moving_dirs(parent, source_name) {
            let name = path.file_name()?.to_string_lossy().to_string();
            let suffix = name.strip_prefix(&prefix).unwrap_or_default();
            if suffix.starts_with(&own) {
                return Some(path);
            }
            // UUIDs of older versions are hexadecimal, so they never start with `r`.
            if !suffix.starts_with('r') {
                unlabelled.push(path);
            }
        }
        match unlabelled.len() {
            1 => unlabelled.pop(),
            _ => None,
        }
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
}
