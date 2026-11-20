//! Transactional rollback engine and safety watchdog auto-rollback (Tasks T034, T035).
//!
//! Features:
//! - Full archive restoration and single-file selective restore
//! - Non-destructive conflict handling (`.restored_<timestamp>`)
//! - Hardware SHA-256 integrity verification during extraction
//! - HardLink sibling reconstruction and timestamp restoration
//! - Automatic watchdog missing-file log scanning and service crash auto-rollback
//! - Automatic quarantine expiration purging

use crate::manifest::VaultManifest;
use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{VaultFileItem, VaultStatus};
use greenc_storage::db::GreenCDb;
use greenc_win32::traits::ProcessServiceProbe;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, FileTimes};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zip::ZipArchive;

/// Strategy to resolve collisions when the restore target file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreConflictStrategy {
    /// Non-destructive: writes to `<original_name>.restored_<timestamp>`.
    RenameWithTimestamp,
    /// Overwrites only if the existing file is 0 bytes; otherwise renames.
    #[default]
    OverwriteIfZeroLength,
    /// Overwrites existing file unconditionally.
    Overwrite,
}

/// Options configuring archive restoration.
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Conflict strategy for existing files on disk.
    pub conflict_strategy: RestoreConflictStrategy,
    /// Whether to verify SHA-256 integrity during extraction.
    pub verify_sha256: bool,
    /// Whether to recreate NTFS hardlink siblings.
    pub restore_hardlinks: bool,
    /// Whether to restore original access and modification timestamps.
    pub restore_timestamps: bool,
    /// Whether to add restored paths to permanent protected files list.
    pub add_to_protected_list: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            conflict_strategy: RestoreConflictStrategy::OverwriteIfZeroLength,
            verify_sha256: true,
            restore_hardlinks: true,
            restore_timestamps: true,
            add_to_protected_list: true,
        }
    }
}

/// Result summary of an archive or selective rollback operation.
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// ID of the affected archive.
    pub archive_id: i64,
    /// Number of files restored.
    pub restored_count: usize,
    /// Total bytes restored.
    pub restored_bytes: u64,
    /// Restored file paths on disk.
    pub restored_paths: Vec<PathBuf>,
    /// Whether this rollback was triggered automatically by the watchdog.
    pub auto_triggered: bool,
    /// Reason string for the rollback.
    pub reason: String,
}

/// Transactional Rollback Engine for restoring Canary Vault archives.
pub struct RollbackEngine<'a> {
    db: &'a GreenCDb,
    options: RestoreOptions,
}

impl<'a> RollbackEngine<'a> {
    /// Creates a new rollback engine bound to a SQLite storage instance.
    pub fn new(db: &'a GreenCDb, options: RestoreOptions) -> Self {
        Self { db, options }
    }

    /// Converts float Unix seconds to SystemTime.
    fn sec_to_system_time(sec: f64) -> SystemTime {
        if sec <= 0.0 {
            UNIX_EPOCH
        } else {
            UNIX_EPOCH + Duration::from_secs_f64(sec)
        }
    }

    /// Full archive rollback (manual or automatic).
    pub fn rollback_archive(
        &self,
        archive_id: i64,
        reason: &str,
        auto_triggered: bool,
    ) -> Result<RestoreResult> {
        let archive =
            self.db
                .get_vault_archive(archive_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "VaultArchive".to_string(),
                    id: archive_id.to_string(),
                })?;

        if !archive.zip_path.exists() {
            return Err(GreenCError::Vault(format!(
                "Archive ZIP file not found at: {}",
                archive.zip_path.display()
            )));
        }

        let files = self.db.get_vault_files(archive_id)?;
        let mut restored_paths = Vec::new();
        let mut restored_bytes = 0u64;

        let zip_file = File::open(&archive.zip_path)?;
        let mut zip_reader = ZipArchive::new(zip_file)
            .map_err(|e| GreenCError::Vault(format!("Failed to open ZIP archive: {}", e)))?;

        let mut restored_paths_set = HashSet::new();

        for item in &files {
            // Guard against legacy archives containing duplicate hardlink entries
            if restored_paths_set.contains(&item.original_path) {
                continue;
            }
            // Already brought back by a selective restore: restoring again would only duplicate it.
            if item.restored_individually {
                continue;
            }

            let primary_dest = self.resolve_restore_target(&item.original_path)?;
            if let Some(parent) = primary_dest.parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Extract entry from zip
            let mut entry = zip_reader.by_name(&item.rel_arcname).map_err(|e| {
                GreenCError::Vault(format!("ZIP entry {} not found: {}", item.rel_arcname, e))
            })?;

            let mut out = File::create(&primary_dest)?;
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 64 * 1024];

            loop {
                let n = entry.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
            }
            out.flush()?;

            // Verify SHA-256
            let extracted_sha256 = format!("{:02x}", hasher.finalize());
            if self.options.verify_sha256 && extracted_sha256 != item.sha256 {
                let _ = std::fs::remove_file(&primary_dest);
                return Err(GreenCError::IntegrityMismatch {
                    path: primary_dest,
                    expected: item.sha256.clone(),
                    actual: extracted_sha256,
                });
            }

            // Restore original timestamps
            if self.options.restore_timestamps {
                let times = FileTimes::new()
                    .set_accessed(Self::sec_to_system_time(item.atime))
                    .set_modified(Self::sec_to_system_time(item.mtime));
                let _ = out.set_times(times);
            }
            drop(out);

            restored_paths_set.insert(primary_dest.clone());
            restored_paths.push(primary_dest.clone());

            // Reconstruct HardLink siblings
            if self.options.restore_hardlinks && item.hardlink_paths.len() > 1 {
                for sibling in &item.hardlink_paths {
                    if sibling != &item.original_path && sibling != &primary_dest {
                        let target_sibling = if sibling.exists() {
                            let meta = std::fs::metadata(sibling).ok();
                            let is_empty = meta.as_ref().map(|m| m.len() == 0).unwrap_or(false);

                            if is_empty
                                || self.options.conflict_strategy
                                    == RestoreConflictStrategy::Overwrite
                            {
                                let _ = std::fs::remove_file(sibling);
                                sibling.clone()
                            } else if self.options.conflict_strategy
                                == RestoreConflictStrategy::RenameWithTimestamp
                                || self.options.conflict_strategy
                                    == RestoreConflictStrategy::OverwriteIfZeroLength
                            {
                                self.resolve_restore_target(sibling)?
                            } else {
                                sibling.clone()
                            }
                        } else {
                            sibling.clone()
                        };

                        if let Some(parent) = target_sibling.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }

                        let link_ok = std::fs::hard_link(&primary_dest, &target_sibling).is_ok();
                        if !link_ok && !target_sibling.exists() {
                            // Cross-volume fallback
                            let _ = std::fs::copy(&primary_dest, &target_sibling);
                        }

                        restored_paths_set.insert(target_sibling.clone());
                        restored_paths_set.insert(sibling.clone());
                        restored_paths.push(target_sibling);
                    }
                }
            }

            restored_bytes += item.size_bytes;
        }

        // Add permanently to protected files list
        if self.options.add_to_protected_list && archive.rule_id > 0 {
            let str_paths: Vec<String> = restored_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let _ = self.db.add_protected_files(archive.rule_id, &str_paths);
        }

        // Clean up ZIP and sidecar manifest from vault storage
        let _ = std::fs::remove_file(&archive.zip_path);
        let _ = std::fs::remove_file(&archive.manifest_path);

        // Update database state
        let status_reason = if auto_triggered {
            format!("[AUTO-ROLLBACK] {}", reason)
        } else {
            format!("[MANUAL ROLLBACK] {}", reason)
        };
        self.db.update_vault_status(
            archive_id,
            VaultStatus::RolledBack.as_str(),
            &status_reason,
        )?;

        Ok(RestoreResult {
            archive_id,
            restored_count: restored_paths.len(),
            restored_bytes,
            restored_paths,
            auto_triggered,
            reason: reason.to_string(),
        })
    }

    /// Granular single-file selective restore.
    pub fn restore_files_selective(
        &self,
        archive_id: i64,
        file_ids: &[i64],
    ) -> Result<RestoreResult> {
        let archive =
            self.db
                .get_vault_archive(archive_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "VaultArchive".to_string(),
                    id: archive_id.to_string(),
                })?;

        if !archive.zip_path.exists() {
            return Err(GreenCError::Vault(format!(
                "Archive ZIP file not found at: {}",
                archive.zip_path.display()
            )));
        }

        let all_files = self.db.get_vault_files(archive_id)?;
        let target_files: Vec<&VaultFileItem> = all_files
            .iter()
            .filter(|f| f.id.map(|id| file_ids.contains(&id)).unwrap_or(false))
            .collect();

        if target_files.is_empty() {
            return Err(GreenCError::Vault(
                "No matching file IDs found in archive".to_string(),
            ));
        }

        let zip_file = File::open(&archive.zip_path)?;
        let mut zip_reader = ZipArchive::new(zip_file)
            .map_err(|e| GreenCError::Vault(format!("Failed to open ZIP archive: {}", e)))?;

        let mut restored_paths = Vec::new();
        let mut restored_bytes = 0u64;

        for item in target_files {
            let primary_dest = self.resolve_restore_target(&item.original_path)?;
            if let Some(parent) = primary_dest.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let mut entry = zip_reader.by_name(&item.rel_arcname).map_err(|e| {
                GreenCError::Vault(format!("ZIP entry {} not found: {}", item.rel_arcname, e))
            })?;

            let mut out = File::create(&primary_dest)?;
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 64 * 1024];

            loop {
                let n = entry.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
            }
            out.flush()?;

            let extracted_sha256 = format!("{:02x}", hasher.finalize());
            if self.options.verify_sha256 && extracted_sha256 != item.sha256 {
                let _ = std::fs::remove_file(&primary_dest);
                return Err(GreenCError::IntegrityMismatch {
                    path: primary_dest,
                    expected: item.sha256.clone(),
                    actual: extracted_sha256,
                });
            }

            if self.options.restore_timestamps {
                let times = FileTimes::new()
                    .set_accessed(Self::sec_to_system_time(item.atime))
                    .set_modified(Self::sec_to_system_time(item.mtime));
                let _ = out.set_times(times);
            }
            drop(out);

            if let Some(fid) = item.id {
                let _ = self.db.mark_file_restored(fid);
            }

            restored_paths.push(primary_dest);
            restored_bytes += item.size_bytes;
        }

        Ok(RestoreResult {
            archive_id,
            restored_count: restored_paths.len(),
            restored_bytes,
            restored_paths,
            auto_triggered: false,
            reason: "Selective manual file restore".to_string(),
        })
    }

    /// Resolves destination path according to conflict strategy.
    fn resolve_restore_target(&self, original_path: &Path) -> Result<PathBuf> {
        if !original_path.exists() {
            return Ok(original_path.to_path_buf());
        }

        let meta = std::fs::metadata(original_path)?;
        if meta.len() == 0
            && self.options.conflict_strategy == RestoreConflictStrategy::OverwriteIfZeroLength
        {
            return Ok(original_path.to_path_buf());
        }

        if self.options.conflict_strategy == RestoreConflictStrategy::Overwrite {
            return Ok(original_path.to_path_buf());
        }

        // Non-destructive rename: file.dat.restored_YYYYMMDD_HHMMSS
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = original_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        // Seconds resolution collides when restores run back to back: never reuse an existing name,
        // otherwise File::create would truncate an earlier restored copy.
        let mut candidate = original_path.with_file_name(format!("{file_name}.restored_{timestamp}"));
        let mut n = 2u32;
        while candidate.exists() {
            candidate = original_path.with_file_name(format!("{file_name}.restored_{timestamp}_{n}"));
            n += 1;
        }
        Ok(candidate)
    }

    /// Periodic Safety Watchdog evaluation:
    /// 1. Checks service status of quarantined archives.
    /// 2. Scans application logs for missing-file error keywords.
    /// 3. Purges expired archives past their retention period.
    pub fn run_watchdog_cycle(
        &self,
        service_probe: &dyn ProcessServiceProbe,
    ) -> Result<Vec<RestoreResult>> {
        let active_archives = self
            .db
            .list_vault_archives(Some(VaultStatus::Quarantined.as_str()))?;
        let now = Utc::now();
        let mut results = Vec::new();

        for arch in active_archives {
            let arch_id = match arch.id {
                Some(id) => id,
                None => continue,
            };

            // 1. Check expiration for auto-purge
            if arch.expires_at <= now {
                info!(
                    "Quarantine retention period expired for archive ID {}. Purging.",
                    arch_id
                );
                let _ = std::fs::remove_file(&arch.zip_path);
                let _ = std::fs::remove_file(&arch.manifest_path);
                let _ = self.db.update_vault_status(
                    arch_id,
                    VaultStatus::Purged.as_str(),
                    "Auto-purged after quarantine retention window expired",
                );
                continue;
            }

            // 2. Service crash check
            // If the archive's quota rule was associated with a Windows service and service stopped
            // We can inspect manifest or storage
            let manifest = VaultManifest::read_from_file(&arch.manifest_path).ok();
            if let Some(ref m) = manifest {
                if !m.service_name.is_empty() {
                    if let Ok(running) = service_probe.is_service_running(&m.service_name) {
                        if !running {
                            warn!("Service {} linked to archive {} stopped. Triggering auto-rollback.", m.service_name, arch_id);
                            if let Ok(res) = self.rollback_archive(
                                arch_id,
                                &format!("Windows service {} stopped unexpectedly", m.service_name),
                                true,
                            ) {
                                results.push(res);
                                continue;
                            }
                        }
                    }
                }
            }

            // 3. Application error log check
            if let Some(ref m) = manifest {
                if let Some(log_error) = self.scan_target_logs_for_errors(m) {
                    warn!("Missing file error detected in application log: {}. Triggering auto-rollback.", log_error);
                    if let Ok(res) = self.rollback_archive(
                        arch_id,
                        &format!("Missing file error in log: {}", log_error),
                        true,
                    ) {
                        results.push(res);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Scans recent log files in the target root for missing file error keywords.
    fn scan_target_logs_for_errors(&self, manifest: &VaultManifest) -> Option<String> {
        let root = &manifest.original_root;
        if !root.exists() || !root.is_dir() {
            return None;
        }

        let error_keywords = [
            "filenotfounderror",
            "enoent",
            "cannot find the file",
            "cannot open",
            "missing file",
            "failed to load",
            "no such file",
        ];

        let read_dir = std::fs::read_dir(root).ok()?;
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            let is_log = ext == "log" || ext == "txt" || ext == "err" || ext == "calf";
            if !is_log {
                continue;
            }

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Size cap <= 10 MiB to prevent memory exhaustion
            if meta.len() > 10 * 1024 * 1024 {
                continue;
            }

            // Filter recent logs modified after archive creation
            let mtime_sec = match meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            {
                Some(d) => d.as_secs_f64(),
                None => continue,
            };

            let arch_sec = manifest.created_at.timestamp() as f64 - 5.0;
            if mtime_sec < arch_sec {
                continue;
            }

            // Read log file content
            if let Ok(mut f) = File::open(&path) {
                let mut content = String::new();
                if f.read_to_string(&mut content).is_ok() {
                    let lower = content.to_lowercase();
                    for &kw in &error_keywords {
                        if lower.contains(kw) {
                            // Verify if any archived file name is referenced in the log
                            for archived_file in &manifest.files {
                                if let Some(fname) = archived_file
                                    .original_path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                {
                                    if lower.contains(&fname.to_lowercase()) {
                                        return Some(format!(
                                            "Found '{}' referencing '{}' in {}",
                                            kw,
                                            fname,
                                            path.display()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }
}
