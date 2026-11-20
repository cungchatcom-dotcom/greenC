//! Crash recovery state machine and live junction health check (Tasks T039, T046).

use crate::verified_copy::remove_dir_all_safe;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_platform::traits::ReparsePointApi;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};

pub struct RecoveryService;

impl RecoveryService {
    /// Inspects the database for relocations interrupted in the `Moving` state.
    /// - If valid junction exists and points to target: cleans leftover `.greenc-moving.*` and marks `Done`.
    /// - If junction is missing or broken: rolls back `.greenc-moving.*` to source, deletes partial target, marks `Failed`.
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
                    let clean_jt = jt.to_string_lossy().to_lowercase();
                    let clean_tgt = target.to_string_lossy().to_lowercase();
                    clean_jt == clean_tgt
                        || clean_jt.ends_with(&clean_tgt)
                        || clean_tgt.ends_with(&clean_jt)
                } else {
                    false
                }
            } else {
                false
            };

            if has_valid_junction && target.exists() {
                // Case A: Junction was successfully created before the crash.
                // Find and delete any leftover .greenc-moving.* directories.
                Self::clean_leftover_moving_dirs(&source_parent, source_name);

                record.status = RelocationStatus::Done;
                record.health_status = RelocationHealthStatus::Healthy;
                record.detail = Some(
                    "Khôi phục sau gián đoạn: Liên kết junction hợp lệ, đã dọn tệp tạm".to_string(),
                );
                db.save_relocation(&record)?;
                recovered.push(record);
            } else {
                // Case B: Junction creation did not happen or link is broken.
                // Roll back to source from .greenc-moving.*
                if reparse.is_reparse_point(&source) {
                    let _ = reparse.delete_junction(&source);
                } else if source.exists() && source.is_dir() {
                    // If an interruption occurred after directory creation but before setting the reparse tag,
                    // source exists as an empty regular directory. Remove it so the atomic rename can succeed.
                    let is_empty = fs::read_dir(&source)
                        .map(|mut r| r.next().is_none())
                        .unwrap_or(false);
                    if is_empty {
                        let _ = fs::remove_dir(&source);
                    }
                }

                if let Some(moving_dir) = Self::find_moving_dir(&source_parent, source_name) {
                    if !source.exists() {
                        let _ = fs::rename(&moving_dir, &source);
                    }
                }

                // Clean partial target
                remove_dir_all_safe(&target);

                record.status = RelocationStatus::Failed;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.detail = Some("Bị gián đoạn, đã khôi phục về vị trí cũ".to_string());
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

                // 1. Check if target directory exists and is reachable
                if !r.target_path.exists() {
                    return RelocationHealthStatus::TargetOffline;
                }

                // 2. Check if source path is still a reparse point
                if !reparse.is_reparse_point(&r.source_path) {
                    return RelocationHealthStatus::LinkBroken;
                }

                // 3. Verify junction target points to the registered target path
                if let Ok(link_target) = reparse.get_junction_target(&r.source_path) {
                    let clean_lt = link_target.to_string_lossy().to_lowercase();
                    let clean_tp = r.target_path.to_string_lossy().to_lowercase();
                    if clean_lt == clean_tp
                        || clean_lt.ends_with(&clean_tp)
                        || clean_tp.ends_with(&clean_lt)
                    {
                        RelocationHealthStatus::Healthy
                    } else {
                        RelocationHealthStatus::LinkBroken
                    }
                } else {
                    RelocationHealthStatus::LinkBroken
                }
            })
            .collect()
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
        let prefix = format!("{}.greenc-moving.", source_name);
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) && entry.path().is_dir() {
                    return Some(entry.path());
                }
            }
        }
        None
    }

    fn clean_leftover_moving_dirs(parent: &Path, source_name: &str) {
        let prefix = format!("{}.greenc-moving.", source_name);
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&prefix) && entry.path().is_dir() {
                    remove_dir_all_safe(&entry.path());
                }
            }
        }
    }
}
