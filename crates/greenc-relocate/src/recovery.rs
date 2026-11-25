//! Crash recovery state machine and live junction health check (Tasks T039, T046, ).

use crate::candidate::CandidateScanner;
use crate::verified_copy::{remove_dir_all_safe, to_extended_io_path};
use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
    ZaloRetainedOriginal, ZaloRetainedStatus,
};
use greenc_platform::traits::ReparsePointApi;
use greenc_platform::RelocationLock;
use greenc_storage::db::GreenCDb;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn is_restoring_dir_name(name: &str) -> bool {
    name.starts_with("zalo-restoring.")
        || name.starts_with("restoring.")
        || name.contains(".greenc-restoring.")
}

fn is_restoring_dir_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_restoring_dir_name)
}

fn norm_cmp_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

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

enum MovingDirSelection {
    Exact(PathBuf),
    None,
    Ambiguous(Vec<PathBuf>),
}

fn make_conflict_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("folder");
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(r"C:\"));
    parent.join(format!("{name}.greenc-conflict.{}", uuid::Uuid::new_v4()))
}

fn has_parent_dir_component(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn normalized_segments(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy();
    let stripped = raw
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| raw.strip_prefix(r"\\?\"))
        .unwrap_or(&raw);
    stripped
        .split(['\\', '/'])
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn is_drive_or_fs_root(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    let is_unc = trimmed.starts_with(r"\\?\UNC\")
        || (trimmed.starts_with(r"\\") && !trimmed.starts_with(r"\\?\"))
        || trimmed.starts_with("//");
    let segs = normalized_segments(path);
    if segs.is_empty() {
        return true;
    }
    if segs.len() == 1 && segs[0].len() == 2 && segs[0].ends_with(':') {
        return true;
    }
    if is_unc && segs.len() <= 2 {
        return true;
    }
    false
}

/// Returns `true` only when `path` is a strict descendant inside a GreenC-managed relocation root
/// (`...\GreenC\Relocated\<subdir>`).
fn is_under_greenc_managed_dir(path: &Path) -> bool {
    let segs = normalized_segments(path);
    for i in 0..segs.len() {
        if segs[i] == "greenc"
            && segs.get(i + 1).map(String::as_str) == Some("relocated")
            && i + 2 < segs.len()
        {
            return true;
        }
    }
    false
}

fn resolve_for_overlap(path: &Path) -> Vec<String> {
    if let Ok(canon) = fs::canonicalize(path) {
        normalized_segments(&canon)
    } else {
        normalized_segments(path)
    }
}

/// Returns `true` if `a` and `b` are the same path, or either is a parent/ancestor of the other.
fn paths_overlap_or_equal(a: &Path, b: &Path) -> bool {
    let sa = resolve_for_overlap(a);
    let sb = resolve_for_overlap(b);
    if sa.is_empty() || sb.is_empty() {
        return false;
    }
    sa == sb || sa.starts_with(&sb) || sb.starts_with(&sa)
}

impl RecoveryService {
    /// Inspects the database for relocations interrupted in the `Moving` state.
    /// - Offline target (`has_valid_junction && !target.exists()`):
    ///   - NEVER deletes the junction or silently restores an old copy (would risk losing newer
    ///     messages written on the offline drive); sets `NeedsReview` + `TargetOffline` clearly.
    /// - Case A (`has_valid_junction && target.exists()`):
    ///   - For Zalo: preserves matching `.greenc-moving.*` as `.greenc-original.<uuid>` in DB
    ///     (`health_verified = false`, never auto-deleted). If leftover `zalo-restoring.*` or
    ///     `restoring.<uuid>` dirs exist, keeps them and marks `NeedsReview`; else marks `Done`.
    ///   - For other apps: cleans matching `.greenc-moving.*` and marks `Done` (or `NeedsReview`
    ///     if non-empty `restoring.*` dirs exist).
    /// - Case B (junction missing or broken):
    ///   - Selects the EXACT `.greenc-moving.*` matching `record.moving_path` (or the sole
    ///     matching dir when legacy `moving_path` is unset; NEVER blindly picks the first when
    ///     multiple exist!).
    ///   - Detects both `zalo-restoring.*` and `restoring.<uuid>` directories.
    ///   - For Zalo: NEVER deletes `target`; renames `target` to `.greenc-conflict.<uuid>`.
    ///   - In ANY ambiguous state (non-empty `source`, multiple `.greenc-moving.*`, leftover
    ///     `zalo-restoring.*`/`restoring.<uuid>`, missing `moving_dir` with non-empty `target`,
    ///     or failed rename): keeps ALL copies untouched and marks `NeedsReview`.
    pub fn recover_interrupted(
        db: &GreenCDb,
        reparse: &dyn ReparsePointApi,
    ) -> Result<Vec<RelocationRecord>> {
        // Machine-wide relocation lock: if ANY live relocation (GUI or CLI, same or other
        // process) holds the lock, NEVER touch its in-flight `Moving` record or temp dirs.
        let Some(_reloc_lock) = RelocationLock::try_acquire_for_db(db.path())? else {
            return Ok(Vec::new());
        };

        let interrupted = db.list_relocations(Some("moving"))?;
        let all_relocs = db.list_relocations(None).unwrap_or_default();
        let claimed_moving_paths: HashSet<String> = all_relocs
            .iter()
            .filter_map(|r| r.moving_path.as_ref().map(|p| norm_cmp_key(p)))
            .collect();
        let restoring_originals = db
            .list_zalo_retained_originals(Some(ZaloRetainedStatus::Restoring.as_str()))
            .unwrap_or_default();
        let mut handled_restoring_ids: HashSet<i64> = HashSet::new();
        let mut recovered = Vec::new();

        for mut record in interrupted {
            tracing::warn!(
                "Found interrupted relocation record #{} for '{}'",
                record.id.unwrap_or(0),
                record.app_name
            );

            let is_zalo = record.app_id.eq_ignore_ascii_case("zalo");
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

            let restoring_dirs = Self::find_restoring_dirs_for_record(
                &record,
                &source_parent,
                source_name,
                is_zalo,
                &claimed_moving_paths,
            );
            let matching_restoring_orig = if is_zalo {
                restoring_originals.iter().find(|ro| {
                    record.id == Some(ro.relocation_id)
                        || greenc_core::models::same_windows_path(&ro.source_path, &source)
                })
            } else {
                None
            };

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

            if has_valid_junction && !target.exists() {
                // Target drive/folder is OFFLINE while the junction itself is valid!
                // NEVER delete the junction or silently restore an older copy over `source`.
                let mut kept_notes = Vec::new();
                if is_zalo {
                    let now = Utc::now();
                    for mdir in Self::matching_moving_dirs(&record, &source_parent, source_name) {
                        let (size_bytes, file_count) = CandidateScanner::compute_dir_size(&mdir);
                        let _ = db.save_zalo_retained_original(&ZaloRetainedOriginal {
                            id: None,
                            relocation_id: record.id.unwrap_or(0),
                            source_path: source.clone(),
                            target_path: target.clone(),
                            retained_path: mdir.clone(),
                            size_bytes,
                            file_count,
                            message_file_count: 0,
                            health_verified: false,
                            created_at: now,
                            purge_after: now + ChronoDuration::hours(24),
                            purge_after_valid: true,
                            observed_healthy_secs: 0,
                            last_observed_at: Some(now),
                            status: ZaloRetainedStatus::NeedsReview,
                            resolved_at: None,
                            detail: Some(
                                "Ổ đĩa đích đang ngoại tuyến khi khôi phục — giữ nguyên bản cũ, không ghi đè tự động"
                                    .to_string(),
                            ),
                        });
                        kept_notes.push(mdir.display().to_string());
                    }
                }
                for rdir in &restoring_dirs {
                    kept_notes.push(rdir.display().to_string());
                }

                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::TargetOffline;
                record.detail = Some(if kept_notes.is_empty() {
                    format!(
                        "Ổ đĩa đích ({}) đang ngoại tuyến; GreenC KHÔNG tự khôi phục bản cũ để tránh mất dữ liệu mới trên ổ đích. Hãy kết nối lại ổ đích rồi kiểm tra.",
                        target.display()
                    )
                } else {
                    format!(
                        "Ổ đĩa đích ({}) đang ngoại tuyến; GreenC KHÔNG tự khôi phục bản cũ để tránh mất dữ liệu mới trên ổ đích. Các bản tạm vẫn được giữ nguyên: {}. Hãy kết nối lại ổ đích rồi kiểm tra.",
                        target.display(),
                        kept_notes.join(", ")
                    )
                });
                db.save_relocation(&record)?;
                recovered.push(record);
                continue;
            }

            if has_valid_junction && target.exists() {
                // Case A: Junction was successfully created before the crash and target is online.
                if is_zalo {
                    let moving_dirs =
                        Self::matching_moving_dirs(&record, &source_parent, source_name);
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
                        let _ = db.save_zalo_retained_original_durable(&ZaloRetainedOriginal {
                            id: None,
                            relocation_id: record.id.unwrap_or(0),
                            source_path: source.clone(),
                            target_path: target.clone(),
                            retained_path: final_path.clone(),
                            size_bytes,
                            file_count,
                            message_file_count: 0,
                            health_verified: false, // Never auto-purge after crash without user check
                            created_at: now,
                            purge_after: now + ChronoDuration::hours(24),
                            purge_after_valid: true,
                            observed_healthy_secs: 0,
                            last_observed_at: Some(now),
                            status,
                            resolved_at: None,
                            detail: Some(note),
                        });
                        retained_notes.push(final_path.display().to_string());
                    }

                    // Item 11: Handle crash between `fs::rename(moving_path, original_path)`
                    // and persisting `zalo_retained_originals` / `Done`! Adopt any untracked
                    // `{source_name}.greenc-original.*` directory in `source_parent` so it stays
                    // durably tracked in `zalo_retained_originals`.
                    let existing_retained =
                        db.list_zalo_retained_originals(None).unwrap_or_default();
                    let tracked_retained_keys: HashSet<String> = existing_retained
                        .iter()
                        .map(|r| norm_cmp_key(&r.retained_path))
                        .collect();
                    let orig_prefix = format!("{source_name}.greenc-original.");
                    if let Ok(entries) = fs::read_dir(&source_parent) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let path = entry.path();
                            if !name.starts_with(&orig_prefix)
                                || !fs::symlink_metadata(&path)
                                    .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                            {
                                continue;
                            }
                            if tracked_retained_keys.contains(&norm_cmp_key(&path)) {
                                continue;
                            }
                            let (size_bytes, file_count) =
                                CandidateScanner::compute_dir_size(&path);
                            // If a pre-rename retained row for this relocation still points to a
                            // missing `.greenc-moving.*` path, update it in place; else insert new.
                            if let Some(mut stale_row) = existing_retained
                                .iter()
                                .find(|r| {
                                    r.relocation_id == record.id.unwrap_or(0)
                                        && fs::symlink_metadata(to_extended_io_path(
                                            &r.retained_path,
                                        ))
                                        .is_err()
                                })
                                .cloned()
                            {
                                stale_row.retained_path = path.clone();
                                stale_row.size_bytes = size_bytes;
                                stale_row.file_count = file_count;
                                stale_row.health_verified = false;
                                stale_row.status = ZaloRetainedStatus::Retained;
                                stale_row.detail = Some(
                                    "Khôi phục sau gián đoạn đổi tên bản gốc — tiếp tục theo dõi an toàn."
                                        .to_string(),
                                );
                                let _ = db.save_zalo_retained_original_durable(&stale_row);
                            } else {
                                let _ = db.save_zalo_retained_original_durable(
                                    &ZaloRetainedOriginal {
                                        id: None,
                                        relocation_id: record.id.unwrap_or(0),
                                        source_path: source.clone(),
                                        target_path: target.clone(),
                                        retained_path: path.clone(),
                                        size_bytes,
                                        file_count,
                                        message_file_count: 0,
                                        health_verified: false,
                                        created_at: now,
                                        purge_after: now + ChronoDuration::hours(24),
                                        purge_after_valid: true,
                                        observed_healthy_secs: 0,
                                        last_observed_at: Some(now),
                                        status: ZaloRetainedStatus::Retained,
                                        resolved_at: None,
                                        detail: Some(
                                            "Phát hiện bản gốc .greenc-original sau gián đoạn — đã đưa vào theo dõi an toàn."
                                                .to_string(),
                                        ),
                                    },
                                );
                            }
                            retained_notes.push(path.display().to_string());
                        }
                    }

                    if !restoring_dirs.is_empty() {
                        let r_list: Vec<String> = restoring_dirs
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect();
                        record.status = RelocationStatus::NeedsReview;
                        record.health_status = RelocationHealthStatus::Healthy;
                        record.detail = Some(format!(
                            "Khôi phục sau gián đoạn: Liên kết Zalo hợp lệ nhưng phát hiện thư mục khôi phục dở ({}) — cần người dùng kiểm tra.",
                            r_list.join(", ")
                        ));
                    } else {
                        record.status = RelocationStatus::Done;
                        record.health_status = RelocationHealthStatus::Healthy;
                        record.moving_path = None;
                        record.detail = Some(if retained_notes.is_empty() {
                            "Khôi phục sau gián đoạn: Liên kết Zalo hợp lệ".to_string()
                        } else {
                            format!(
                                "Khôi phục sau gián đoạn: Liên kết Zalo hợp lệ; bản gốc được giữ tạm an toàn tại {}",
                                retained_notes.join(", ")
                            )
                        });
                    }
                    db.save_relocation_durable(&record)?;
                    recovered.push(record);
                } else {
                    match Self::select_moving_dir_for_record(&record, &source_parent, source_name) {
                        MovingDirSelection::Exact(mdir) => remove_dir_all_safe(&mdir),
                        MovingDirSelection::None => {}
                        MovingDirSelection::Ambiguous(_) => {
                            // Do not blindly delete unrelated `.greenc-moving.*` directories
                        }
                    }

                    if !restoring_dirs.is_empty() {
                        let r_list: Vec<String> = restoring_dirs
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect();
                        record.status = RelocationStatus::NeedsReview;
                        record.health_status = RelocationHealthStatus::Healthy;
                        record.detail = Some(format!(
                            "Khôi phục sau gián đoạn: Liên kết junction hợp lệ nhưng phát hiện thư mục khôi phục dở ({}) — cần kiểm tra.",
                            r_list.join(", ")
                        ));
                    } else {
                        record.status = RelocationStatus::Done;
                        record.health_status = RelocationHealthStatus::Healthy;
                        record.detail = Some(
                            "Khôi phục sau gián đoạn: Liên kết junction hợp lệ, đã dọn tệp tạm"
                                .to_string(),
                        );
                    }
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
                    let _ = fs::remove_dir(&source);
                }

                let source_occupied = fs::symlink_metadata(&source).is_ok();
                let moving_sel =
                    Self::select_moving_dir_for_record(&record, &source_parent, source_name);
                let target_exists = fs::symlink_metadata(to_extended_io_path(&target)).is_ok();
                let target_has_content = path_has_content_or_uncertain(&target);
                let copying_dirs = Self::find_copying_dirs(&target);

                // Item 4: Crash during Undo (`record.moving_path` is an undo temp
                // `zalo-restoring.*`/`restoring.*`/`*.greenc-restoring.*`) OR during
                // `restore_retained_original` (a `zalo_retained_originals` row in `Restoring`)
                // right after `delete_junction`, leaving `source` absent while `target` is online:
                // rebuild the junction `source -> target` cleanly instead of stranding the app!
                let moving_is_undo_temp = record
                    .moving_path
                    .as_deref()
                    .is_some_and(is_restoring_dir_path);
                if !source_occupied && (moving_is_undo_temp || matching_restoring_orig.is_some()) {
                    if target_exists && target.is_dir() {
                        let rebuild_ok = reparse.create_junction(&source, &target).is_ok()
                            && reparse.get_junction_target(&source).is_ok_and(|jt| {
                                greenc_core::models::same_windows_path(&jt, &target)
                            });
                        if rebuild_ok {
                            if moving_is_undo_temp {
                                if let Some(ref mp) = record.moving_path {
                                    remove_dir_all_safe(mp);
                                }
                            }
                            if let Some(ro) = matching_restoring_orig {
                                if let Some(ro_id) = ro.id {
                                    let _ = db.update_zalo_retained_original_status_durable(
                                        ro_id,
                                        ZaloRetainedStatus::Retained.as_str(),
                                        "Khôi phục sau gián đoạn: đã dựng lại liên kết tới ổ đích, giữ nguyên bản gốc tạm.",
                                    );
                                    handled_restoring_ids.insert(ro_id);
                                }
                            }
                            record.status = RelocationStatus::Done;
                            record.health_status = RelocationHealthStatus::Healthy;
                            record.moving_path = None;
                            record.detail = Some(
                                "Khôi phục sau gián đoạn hoàn tác/khôi phục: đã dựng lại liên kết tới ổ đích an toàn."
                                    .to_string(),
                            );
                            db.save_relocation_durable(&record)?;
                            recovered.push(record);
                            continue;
                        }
                    } else if let Some(ro) = matching_restoring_orig {
                        if let Some(ro_id) = ro.id {
                            let _ = db.update_zalo_retained_original_status_durable(
                                ro_id,
                                ZaloRetainedStatus::NeedsReview.as_str(),
                                "Bị gián đoạn khi khôi phục và ổ đích đang ngoại tuyến — giữ nguyên mọi bản dữ liệu.",
                            );
                            handled_restoring_ids.insert(ro_id);
                        }
                        record.status = RelocationStatus::NeedsReview;
                        record.health_status = RelocationHealthStatus::TargetOffline;
                        record.detail = Some(format!(
                            "Bị gián đoạn khi khôi phục bản gốc nhưng ổ đích ({}) đang ngoại tuyến — cần người dùng kiểm tra.",
                            target.display()
                        ));
                        db.save_relocation_durable(&record)?;
                        recovered.push(record);
                        continue;
                    }
                }

                // Item 6 + Item 9: Crash BEFORE `source` was ever renamed:
                // `source` is still a real intact directory on disk, NO `.greenc-moving.*` exists
                // (including when a recorded `moving_path` is absent on disk), and no restoring
                // dir for this record exists. Clean any `.greenc-copying.*` staging dirs, rename
                // any copied `target` to `.greenc-conflict.<uuid>` (never delete!), and mark
                // `Failed` + `Healthy` instead of trapping the user in a `NeedsReview` dead-end!
                if !reparse.is_reparse_point(&source)
                    && path_has_content_or_uncertain(&source)
                    && matches!(moving_sel, MovingDirSelection::None)
                    && restoring_dirs.is_empty()
                {
                    for cdir in &copying_dirs {
                        remove_dir_all_safe(cdir);
                    }
                    if target_exists {
                        let conflict_target = make_conflict_sibling(&target);
                        match fs::rename(&target, &conflict_target) {
                            Ok(()) => {
                                record.status = RelocationStatus::Failed;
                                record.health_status = RelocationHealthStatus::Healthy;
                                record.moving_path = None;
                                record.detail = Some(format!(
                                    "Quá trình chuyển bị gián đoạn trước khi đổi tên thư mục nguồn; dữ liệu gốc vẫn nguyên vẹn và bản sao tại đích đã được đổi sang bản xung đột {}.",
                                    conflict_target.display()
                                ));
                            }
                            Err(rename_tgt_err) => {
                                record.status = RelocationStatus::NeedsReview;
                                record.health_status = RelocationHealthStatus::LinkBroken;
                                record.detail = Some(format!(
                                    "Dữ liệu gốc vẫn nguyên vẹn nhưng không đổi tên được thư mục đích {} sang bản xung đột ({rename_tgt_err}); GreenC giữ nguyên thư mục đích để tránh mất dữ liệu.",
                                    target.display()
                                ));
                            }
                        }
                    } else {
                        record.status = RelocationStatus::Failed;
                        record.health_status = RelocationHealthStatus::Healthy;
                        record.moving_path = None;
                        record.detail = Some(
                            "Quá trình sao chép bị gián đoạn trước khi chuyển đổi; đã dọn thư mục đích sao chép dở, dữ liệu gốc vẫn nguyên vẹn."
                                .to_string(),
                        );
                    }
                    db.save_relocation_durable(&record)?;
                    recovered.push(record);
                    continue;
                }

                if !source_occupied && restoring_dirs.is_empty() {
                    if let MovingDirSelection::Exact(mdir) = moving_sel {
                        // In Store (MSIX) packaged mode, never rename into virtualized AppData directly:
                        // mark `NeedsReview` preserving BOTH `mdir` and `target` so nothing is hidden
                        // in LocalCache or deleted prematurely.
                        if cfg!(feature = "store")
                            && greenc_platform::breakaway::is_running_in_msix_package()
                            && greenc_core::config::is_windows_appdata_app_dir(&source)
                        {
                            let mut kept = vec![mdir.display().to_string()];
                            if target_has_content {
                                kept.push(target.display().to_string());
                            }
                            record.status = RelocationStatus::NeedsReview;
                            record.health_status = RelocationHealthStatus::LinkBroken;
                            record.detail = Some(format!(
                                "Bị gián đoạn trong AppData ở bản Microsoft Store — GreenC không đổi tên trực tiếp vào AppData ảo hoá và đã giữ nguyên tất cả bản để xử lý an toàn: {}",
                                kept.join(", ")
                            ));
                            db.save_relocation_durable(&record)?;
                            recovered.push(record);
                            continue;
                        }
                        match fs::rename(&mdir, &source) {
                            Ok(()) => {
                                if is_zalo {
                                    // Item 3: Zalo recovery NEVER deletes `target` — rename to `.greenc-conflict.<uuid>`.
                                    if target_exists {
                                        let conflict_target = make_conflict_sibling(&target);
                                        match fs::rename(&target, &conflict_target) {
                                            Ok(()) => {
                                                record.status = RelocationStatus::Failed;
                                                record.health_status =
                                                    RelocationHealthStatus::LinkBroken;
                                                record.detail = Some(format!(
                                                    "Bị gián đoạn, đã khôi phục bản gốc Zalo về vị trí cũ; thư mục đích được đổi tên thành bản xung đột tại {} (không xoá).",
                                                    conflict_target.display()
                                                ));
                                            }
                                            Err(rename_tgt_err) => {
                                                record.status = RelocationStatus::NeedsReview;
                                                record.health_status =
                                                    RelocationHealthStatus::LinkBroken;
                                                record.detail = Some(format!(
                                                    "Đã khôi phục bản gốc Zalo về vị trí cũ nhưng không đổi tên được thư mục đích {} sang bản xung đột ({rename_tgt_err}); GreenC giữ nguyên thư mục đích để tránh mất dữ liệu.",
                                                    target.display()
                                                ));
                                            }
                                        }
                                    } else {
                                        record.status = RelocationStatus::Failed;
                                        record.health_status = RelocationHealthStatus::LinkBroken;
                                        record.detail = Some(
                                            "Bị gián đoạn, đã khôi phục về vị trí cũ".to_string(),
                                        );
                                    }
                                } else {
                                    remove_dir_all_safe(&target);
                                    record.status = RelocationStatus::Failed;
                                    record.health_status = RelocationHealthStatus::LinkBroken;
                                    record.detail =
                                        Some("Bị gián đoạn, đã khôi phục về vị trí cũ".to_string());
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

                // Ambiguous or multi-copy state (e.g., `source` is non-empty, multiple `.greenc-moving.*`,
                // leftover `zalo-restoring.*`/`restoring.<uuid>`, or `moving_dir` missing while `target`
                // has content): DO NOT delete anything! Keep every copy and mark `NeedsReview`.
                let mut preserved = Vec::new();
                if path_has_content_or_uncertain(&source) {
                    preserved.push(format!("nguồn ({})", source.display()));
                }
                match moving_sel {
                    MovingDirSelection::Exact(ref mdir) => {
                        preserved.push(format!("bản gốc tạm ({})", mdir.display()));
                    }
                    MovingDirSelection::Ambiguous(ref mdirs) => {
                        for mdir in mdirs {
                            preserved.push(format!("bản gốc tạm ({})", mdir.display()));
                        }
                    }
                    MovingDirSelection::None => {}
                }
                for rdir in &restoring_dirs {
                    preserved.push(format!("bản khôi phục dở ({})", rdir.display()));
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
                        "Bị gián đoạn, phát hiện dữ liệu ở nhiều nơi hoặc chưa xác định rõ bản gốc — cần người dùng xử lý. GreenC đã giữ nguyên tất cả bản: {}",
                        preserved.join("; ")
                    )
                });
                db.save_relocation(&record)?;
                recovered.push(record);
            }
        }

        // Item 4: Also recover any `zalo_retained_originals` rows left in `Restoring`
        // whose corresponding `relocations` row was NOT in `Moving` (e.g., crash between DB writes
        // or pre-upgrade DB). If `source_path` is missing and `target_path` is online, rebuild the
        // junction to `target_path` and revert the retained row to `Retained`!
        for ro in restoring_originals {
            let Some(ro_id) = ro.id else {
                continue;
            };
            if handled_restoring_ids.contains(&ro_id) {
                continue;
            }
            let src_exists = fs::symlink_metadata(to_extended_io_path(&ro.source_path)).is_ok();
            let has_valid_j = reparse.is_reparse_point(&ro.source_path)
                && reparse
                    .get_junction_target(&ro.source_path)
                    .is_ok_and(|jt| greenc_core::models::same_windows_path(&jt, &ro.target_path));

            if !src_exists
                && !reparse.is_reparse_point(&ro.source_path)
                && ro.target_path.exists()
                && ro.target_path.is_dir()
            {
                let rebuild_ok = reparse
                    .create_junction(&ro.source_path, &ro.target_path)
                    .is_ok()
                    && reparse
                        .get_junction_target(&ro.source_path)
                        .is_ok_and(|jt| {
                            greenc_core::models::same_windows_path(&jt, &ro.target_path)
                        });
                if rebuild_ok {
                    let _ = db.update_zalo_retained_original_status_durable(
                        ro_id,
                        ZaloRetainedStatus::Retained.as_str(),
                        "Khôi phục sau gián đoạn: đã dựng lại liên kết tới ổ đích, giữ nguyên bản gốc tạm.",
                    );
                    if let Some(mut reloc) = all_relocs
                        .iter()
                        .find(|r| r.id == Some(ro.relocation_id))
                        .cloned()
                    {
                        reloc.status = RelocationStatus::Done;
                        reloc.health_status = RelocationHealthStatus::Healthy;
                        reloc.moving_path = None;
                        reloc.detail = Some(
                            "Khôi phục sau gián đoạn khôi phục bản gốc: đã dựng lại liên kết tới ổ đích."
                                .to_string(),
                        );
                        let _ = db.save_relocation_durable(&reloc);
                        recovered.push(reloc);
                    }
                    continue;
                }
            } else if has_valid_j && ro.target_path.exists() {
                let _ = db.update_zalo_retained_original_status_durable(
                    ro_id,
                    ZaloRetainedStatus::Retained.as_str(),
                    "Khôi phục sau gián đoạn: liên kết Zalo vẫn hợp lệ, trả bản gốc về trạng thái giữ tạm.",
                );
                continue;
            }

            let _ = db.update_zalo_retained_original_status_durable(
                ro_id,
                ZaloRetainedStatus::NeedsReview.as_str(),
                "Quá trình khôi phục bản gốc bị gián đoạn — cần người dùng kiểm tra.",
            );
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
    ///
    /// Hardened for ALL apps (Item 8):
    /// - Rejects wrong record status (only `Done` or `NeedsReview` with `Junction`/`ToolConfig`)
    /// - Rejects drive/filesystem roots, `..` traversal, symlinks/reparse points as target
    /// - Rejects any target outside a GreenC-managed directory (`...\GreenC\Relocated\<app>`)
    /// - Rejects targets equal to, parent of, or child of another active record's target/source
    ///   (or this record's source)
    /// - Creates and verifies the NEW junction BEFORE removing the old junction
    /// - Marks `record.target_user_repaired = true` so Undo will NEVER delete a user-typed target
    pub fn repair_junction_target(
        record_id: i64,
        new_target_path: &Path,
        db: &GreenCDb,
        reparse: &dyn ReparsePointApi,
    ) -> Result<RelocationRecord> {
        let _reloc_lock = RelocationLock::acquire_for_db(db.path())?;

        let records = db.list_relocations(None)?;
        let mut record = records
            .iter()
            .find(|r| r.id == Some(record_id))
            .cloned()
            .ok_or_else(|| GreenCError::NotFound {
                entity: "RelocationRecord".to_string(),
                id: record_id.to_string(),
            })?;

        // 1. Validate record status and strategy
        if !matches!(
            record.status,
            RelocationStatus::Done | RelocationStatus::NeedsReview
        ) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Bản ghi relocation đang ở trạng thái '{}' nên không thể sửa liên kết",
                    record.status.as_str()
                ),
            });
        }
        if !matches!(
            record.strategy,
            RelocationStrategyKind::Junction | RelocationStrategyKind::ToolConfig
        ) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Bản ghi chiến lược '{}' không sử dụng liên kết junction",
                    record.strategy.as_str()
                ),
            });
        }

        // Guard BEFORE deleting the existing junction so Store builds never destroy a link they
        // cannot recreate due to MSIX AppData virtualization.
        if cfg!(feature = "store")
            && greenc_core::config::is_windows_appdata_app_dir(&record.source_path)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Bản Microsoft Store không tạo/sửa NTFS Junction trong AppData ('{}') do cơ chế ảo hoá AppData của MSIX; liên kết hiện tại được giữ nguyên.",
                    record.source_path.display()
                ),
            });
        }

        // 2. Reject drive/filesystem roots and parent-dir traversal BEFORE any FS mutation
        if has_parent_dir_component(new_target_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Đường dẫn đích chứa '..' không hợp lệ: {}",
                    new_target_path.display()
                ),
            });
        }
        if is_drive_or_fs_root(new_target_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Không được trỏ liên kết vào thư mục gốc ổ đĩa: {}",
                    new_target_path.display()
                ),
            });
        }

        // 3. Target must exist as a real (non-reparse/non-symlink) directory
        let target_io = to_extended_io_path(new_target_path);
        let target_meta = fs::symlink_metadata(&target_io).map_err(|_| GreenCError::NotFound {
            entity: "NewTargetPath".to_string(),
            id: new_target_path.display().to_string(),
        })?;
        if !target_meta.is_dir()
            || target_meta.file_type().is_symlink()
            || reparse.is_reparse_point(new_target_path)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Đường dẫn đích phải là thư mục thật (không phải tệp hay liên kết): {}",
                    new_target_path.display()
                ),
            });
        }

        // 4. Target (and its canonicalized path) must lie strictly under GreenC-managed dir
        let canon_target =
            fs::canonicalize(new_target_path).unwrap_or_else(|_| new_target_path.to_path_buf());
        if is_drive_or_fs_root(&canon_target)
            || !is_under_greenc_managed_dir(new_target_path)
            || !is_under_greenc_managed_dir(&canon_target)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Thư mục đích phải nằm dưới thư mục GreenC quản lý (GreenC\\Relocated\\<ứng dụng>): {}",
                    new_target_path.display()
                ),
            });
        }

        // 5. Reject overlap with own source_path or any other active record's target/source
        if paths_overlap_or_equal(&canon_target, &record.source_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Thư mục đích ({}) trùng hoặc lồng với thư mục nguồn ({})",
                    new_target_path.display(),
                    record.source_path.display()
                ),
            });
        }
        for other in &records {
            if other.id == record.id
                || matches!(
                    other.status,
                    RelocationStatus::Undone | RelocationStatus::Failed
                )
            {
                continue;
            }
            if paths_overlap_or_equal(&canon_target, &other.target_path)
                || paths_overlap_or_equal(&canon_target, &other.source_path)
            {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "repair_junction_target".to_string(),
                    reason: format!(
                        "Thư mục đích ({}) trùng hoặc là thư mục cha/con của bản ghi khác ('{}': {})",
                        new_target_path.display(),
                        other.app_name,
                        other.target_path.display()
                    ),
                });
            }
        }

        // 6. Never overwrite a real (non-reparse) directory or file at source_path
        let had_old_junction = reparse.is_reparse_point(&record.source_path);
        if !had_old_junction
            && fs::symlink_metadata(to_extended_io_path(&record.source_path)).is_ok()
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "repair_junction_target".to_string(),
                reason: format!(
                    "Vị trí nguồn ({}) đang là thư mục thật, không phải liên kết — từ chối ghi đè",
                    record.source_path.display()
                ),
            });
        }

        // 7. Create and verify the NEW junction FIRST before deleting/replacing the old one!
        let source_parent = record
            .source_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(r"C:\"));
        let source_name = record
            .source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("link");
        let staging_link = source_parent.join(format!(
            "{source_name}.greenc-repair.{}",
            uuid::Uuid::new_v4()
        ));

        reparse.create_junction(&staging_link, new_target_path)?;
        let verified_staging = reparse
            .get_junction_target(&staging_link)
            .inspect_err(|_| {
                let _ = reparse.delete_junction(&staging_link);
            })?;
        if !greenc_core::models::same_windows_path(&verified_staging, new_target_path) {
            let _ = reparse.delete_junction(&staging_link);
            return Err(GreenCError::Relocation(format!(
                "Liên kết tạm trỏ sai đích: kỳ vọng {}, thực tế {}",
                new_target_path.display(),
                verified_staging.display()
            )));
        }

        // Swap the verified new junction into place; only delete the old junction AFTER success.
        let staging_exists_on_disk =
            fs::symlink_metadata(to_extended_io_path(&staging_link)).is_ok();
        if staging_exists_on_disk {
            if had_old_junction {
                let old_backup = source_parent.join(format!(
                    "{source_name}.greenc-oldlink.{}",
                    uuid::Uuid::new_v4()
                ));
                if let Err(err) = fs::rename(&record.source_path, &old_backup) {
                    let _ = reparse.delete_junction(&staging_link);
                    return Err(GreenCError::Io(err));
                }
                if let Err(err) = fs::rename(&staging_link, &record.source_path) {
                    // Rollback: restore the old junction immediately
                    let _ = fs::rename(&old_backup, &record.source_path);
                    let _ = reparse.delete_junction(&staging_link);
                    return Err(GreenCError::Io(err));
                }
                // Verify final junction before deleting the old backup junction
                let final_target = reparse.get_junction_target(&record.source_path);
                if !final_target
                    .as_ref()
                    .is_ok_and(|t| greenc_core::models::same_windows_path(t, new_target_path))
                {
                    let _ = reparse.delete_junction(&record.source_path);
                    let _ = fs::rename(&old_backup, &record.source_path);
                    return Err(GreenCError::Relocation(
                        "Xác minh liên kết mới sau khi đổi chỗ thất bại; đã khôi phục liên kết cũ."
                            .to_string(),
                    ));
                }
                let _ = reparse.delete_junction(&old_backup);
            } else if let Err(err) = fs::rename(&staging_link, &record.source_path) {
                let _ = reparse.delete_junction(&staging_link);
                return Err(GreenCError::Io(err));
            }
        } else {
            // In-memory mock fallback (no OS directory entry created for staging_link)
            let old_target = if had_old_junction {
                reparse.get_junction_target(&record.source_path).ok()
            } else {
                None
            };
            if had_old_junction {
                if let Err(err) = reparse.delete_junction(&record.source_path) {
                    let _ = reparse.delete_junction(&staging_link);
                    return Err(err);
                }
            }
            if let Err(err) = reparse.create_junction(&record.source_path, new_target_path) {
                if let Some(ref ot) = old_target {
                    let _ = reparse.create_junction(&record.source_path, ot);
                }
                let _ = reparse.delete_junction(&staging_link);
                return Err(err);
            }
            let _ = reparse.delete_junction(&staging_link);
        }

        record.target_path = new_target_path.to_path_buf();
        record.status = RelocationStatus::Done;
        record.health_status = RelocationHealthStatus::Healthy;
        record.target_user_repaired = true;
        record.detail = Some(format!(
            "Đã cập nhật liên kết sang {} (đích do người dùng chỉ định — hoàn tác sẽ không xoá đích này)",
            new_target_path.display()
        ));
        db.save_relocation_durable(&record)?;

        Ok(record)
    }

    fn select_moving_dir_for_record(
        record: &RelocationRecord,
        parent: &Path,
        source_name: &str,
    ) -> MovingDirSelection {
        if let Some(ref recorded) = record.moving_path {
            if fs::symlink_metadata(to_extended_io_path(recorded)).is_ok() {
                return MovingDirSelection::Exact(recorded.clone());
            }
            return MovingDirSelection::None;
        }
        let all = Self::find_all_moving_dirs(parent, source_name);
        match all.len() {
            0 => MovingDirSelection::None,
            1 => MovingDirSelection::Exact(all.into_iter().next().unwrap()),
            _ => MovingDirSelection::Ambiguous(all),
        }
    }

    fn matching_moving_dirs(
        record: &RelocationRecord,
        parent: &Path,
        source_name: &str,
    ) -> Vec<PathBuf> {
        if let Some(ref recorded) = record.moving_path {
            if fs::symlink_metadata(to_extended_io_path(recorded)).is_ok() {
                return vec![recorded.clone()];
            }
            return Vec::new();
        }
        Self::find_all_moving_dirs(parent, source_name)
    }

    /// Matches restoring staging directories belonging ONLY to `record` so an unrelated app's
    /// leftover `restoring.*` directory in the same parent folder never blocks this record.
    fn find_restoring_dirs_for_record(
        record: &RelocationRecord,
        parent: &Path,
        source_name: &str,
        is_zalo: bool,
        claimed_moving_paths: &HashSet<String>,
    ) -> Vec<PathBuf> {
        if let Some(ref mp) = record.moving_path {
            if is_restoring_dir_path(mp) {
                return if fs::symlink_metadata(to_extended_io_path(mp))
                    .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                {
                    vec![mp.clone()]
                } else {
                    Vec::new()
                };
            }
            // `record.moving_path` is a `.greenc-moving.*` (forward move, NOT an undo/restore).
            // Only a restoring dir explicitly named for this source could relate to it; never
            // grab another app's `restoring.*` directory.
            let own_prefix = format!("{source_name}.greenc-restoring.");
            let mut out = Vec::new();
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let matches_own = name.starts_with(&own_prefix)
                        || (is_zalo && name.starts_with("zalo-restoring."));
                    if matches_own
                        && fs::symlink_metadata(entry.path())
                            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                    {
                        let key = norm_cmp_key(&entry.path());
                        if !claimed_moving_paths.contains(&key) {
                            out.push(entry.path());
                        }
                    }
                }
            }
            out.sort();
            return out;
        }

        // Legacy record (`moving_path == None`): match only restoring dirs that belong to this
        // record (own prefix, or `zalo-restoring.*` for Zalo, or unclaimed legacy `restoring.*`).
        let own_prefix = format!("{source_name}.greenc-restoring.");
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let matches_record = name.starts_with(&own_prefix)
                    || (is_zalo && name.starts_with("zalo-restoring."))
                    || name.starts_with("restoring.");
                if matches_record
                    && fs::symlink_metadata(entry.path())
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                {
                    let key = norm_cmp_key(&entry.path());
                    if !claimed_moving_paths.contains(&key) {
                        out.push(entry.path());
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Finds any leftover `zalo-restoring.*`, `<name>.greenc-restoring.*`, or `restoring.<uuid>`
    /// directories in `parent`.
    pub fn find_restoring_dirs(parent: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if is_restoring_dir_name(&name)
                    && fs::symlink_metadata(entry.path())
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                {
                    out.push(entry.path());
                }
            }
        }
        out.sort();
        out
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
        out.sort();
        out
    }

    fn find_copying_dirs(target_path: &Path) -> Vec<PathBuf> {
        let Some(parent) = target_path.parent() else {
            return Vec::new();
        };
        let Some(target_name) = target_path.file_name().and_then(|n| n.to_str()) else {
            return Vec::new();
        };
        let prefix = format!("{target_name}.greenc-copying.");
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix)
                    && fs::symlink_metadata(entry.path())
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                {
                    out.push(entry.path());
                }
            }
        }
        out.sort();
        out
    }
}
