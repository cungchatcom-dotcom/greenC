use crate::db::GreenCDb;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::VaultArchive;
use greenc_core::task::TaskControl;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

pub fn resolve_default_db_path() -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    PathBuf::from(local_app_data)
        .join("GreenC")
        .join("greenc.db")
}

/// Set to `1` to disable importing legacy databases. Test harnesses must set it: the candidate
/// list contains absolute machine-wide paths that `LOCALAPPDATA` redirection cannot sandbox, and an
/// imported legacy DB carries real Vault archives whose restore/purge would touch real files.
pub const SKIP_LEGACY_MIGRATION_ENV: &str = "GREENC_SKIP_LEGACY_MIGRATION";

pub fn get_legacy_db_candidates() -> Vec<PathBuf> {
    if std::env::var(SKIP_LEGACY_MIGRATION_ENV).is_ok_and(|v| v == "1") {
        return Vec::new();
    }
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    vec![
        PathBuf::from(&local_app_data)
            .join("Green C")
            .join("greenc.db"),
        PathBuf::from(r"C:\DiskGuard\data\diskguard.db"),
        PathBuf::from(r"C:\GreenC\data\diskguard.db"),
    ]
}

pub fn auto_migrate_legacy_db(target_db_path: &Path) -> Result<bool> {
    let candidates = get_legacy_db_candidates();
    auto_migrate_legacy_db_with_candidates(target_db_path, &candidates)
}

pub fn auto_migrate_legacy_db_with_candidates(
    target_db_path: &Path,
    candidates: &[PathBuf],
) -> Result<bool> {
    if target_db_path.exists() {
        // Self-heal: If an empty (0 bytes) orphan file exists from a previous aborted migration,
        // remove it so auto-migration can proceed instead of being permanently blocked.
        if let Ok(meta) = fs::metadata(target_db_path) {
            if meta.len() == 0 {
                info!(
                    "Removing 0-byte orphan database file at {:?} left by aborted previous migration",
                    target_db_path
                );
                let _ = fs::remove_file(target_db_path);
            } else {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }

    for candidate in candidates {
        if candidate.exists() && candidate.is_file() {
            info!(
                "Found legacy database at {:?}. Starting migration to {:?}",
                candidate, target_db_path
            );
            migrate_legacy_db(candidate, target_db_path)?;
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn migrate_legacy_db(legacy_db: &Path, new_db: &Path) -> Result<()> {
    if !legacy_db.exists() {
        return Err(GreenCError::Database(format!(
            "Legacy database not found: {:?}",
            legacy_db
        )));
    }
    if new_db.exists() {
        if let Ok(meta) = fs::metadata(new_db) {
            if meta.len() == 0 {
                let _ = fs::remove_file(new_db);
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }

    if let Some(parent) = new_db.parent() {
        fs::create_dir_all(parent)?;
    }

    // Co-locate temporary file in the same directory as new_db to ensure atomic std::fs::rename
    // across the same filesystem volume boundary.
    let tmp_db = {
        let mut os_str = new_db.as_os_str().to_os_string();
        os_str.push(".tmp");
        PathBuf::from(os_str)
    };

    // Remove any stale leftover temporary file
    if tmp_db.exists() {
        let _ = fs::remove_file(&tmp_db);
    }

    // Run the backup to the temporary file
    let backup_result = (|| -> Result<()> {
        // Open source connection in read-only mode
        let src =
            Connection::open_with_flags(legacy_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

        // Open destination connection to temporary file
        let mut dst = Connection::open(&tmp_db).map_err(|e| GreenCError::Sqlite(e.to_string()))?;

        // Perform online backup
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| GreenCError::Sqlite(format!("SQLite backup init failed: {}", e)))?;

        backup
            .run_to_completion(100, std::time::Duration::from_millis(50), None)
            .map_err(|e| GreenCError::Sqlite(format!("SQLite backup execution failed: {}", e)))?;

        // Explicitly drop backup and SQLite connections before renaming.
        // On Windows, holding open handles to tmp_db prevents atomic fs::rename (ERROR_SHARING_VIOLATION).
        drop(backup);
        drop(dst);
        drop(src);

        Ok(())
    })();

    match backup_result {
        Ok(()) => {
            // Atomically rename temporary file to destination database
            if let Err(e) = fs::rename(&tmp_db, new_db) {
                let _ = fs::remove_file(&tmp_db);
                return Err(GreenCError::Io(e));
            }
            info!(
                "Successfully copied legacy database {:?} to {:?}",
                legacy_db, new_db
            );
            Ok(())
        }
        Err(e) => {
            // Clean up temporary file on failure so no orphan file remains
            let _ = fs::remove_file(&tmp_db);
            Err(e)
        }
    }
}

#[derive(Debug, Clone)]
pub struct MoveVaultSummary {
    pub moved_count: usize,
    pub errors: Vec<String>,
}

pub fn move_vault(
    db: &GreenCDb,
    new_vault_dir: &Path,
    target_drive_free_bytes: u64,
    task_ctrl: Option<&TaskControl>,
) -> Result<MoveVaultSummary> {
    const MIN_RESERVE_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB strict reserve

    fs::create_dir_all(new_vault_dir)?;
    let archives = db.list_vault_archives(Some("QUARANTINED"))?;

    // Identify archives that actually require moving
    let to_move: Vec<_> = archives
        .into_iter()
        .filter(|arch| {
            let zip_src = PathBuf::from(&arch.zip_path);
            zip_src.parent() != Some(new_vault_dir)
        })
        .collect();

    // Helper closure to determine archive payload size (ZIP + manifest)
    let get_archive_payload_bytes = |arch: &VaultArchive| -> u64 {
        let zip_src = PathBuf::from(&arch.zip_path);
        let man_src = PathBuf::from(&arch.manifest_path);
        let zip_size = fs::metadata(&zip_src)
            .map(|m| m.len())
            .unwrap_or(arch.compressed_bytes);
        let man_size = fs::metadata(&man_src).map(|m| m.len()).unwrap_or(0);
        zip_size.saturating_add(man_size)
    };

    // Calculate total payload size across all archives to be moved
    let total_payload_bytes: u64 = to_move.iter().map(get_archive_payload_bytes).sum();
    let required_bytes = total_payload_bytes.saturating_add(MIN_RESERVE_BYTES);

    // Validate that target drive has sufficient free space for entire payload plus strict 512 MiB reserve
    if target_drive_free_bytes < required_bytes {
        return Err(GreenCError::InsufficientVaultSpace(format!(
            "Target drive free space ({} bytes) is insufficient: requires {} bytes ({} bytes payload + 512 MiB reserve)",
            target_drive_free_bytes, required_bytes, total_payload_bytes
        )));
    }

    let mut current_free_bytes = target_drive_free_bytes;
    let mut moved_count = 0;
    let mut errors = Vec::new();
    let total = to_move.len().max(1);

    for (idx, arch) in to_move.into_iter().enumerate() {
        if let Some(ctrl) = task_ctrl {
            if ctrl.is_cancelled() {
                break;
            }
            ctrl.report_progress(
                (idx * 100 / total) as u32,
                &format!("Moving archive {}/{}", idx + 1, total),
            );
        }

        let zip_src = PathBuf::from(&arch.zip_path);
        let man_src = PathBuf::from(&arch.manifest_path);

        let zip_dest = new_vault_dir.join(zip_src.file_name().unwrap_or_default());
        let man_dest = new_vault_dir.join(man_src.file_name().unwrap_or_default());

        // In-loop per-archive reserve check to guard against concurrent disk space exhaustion
        let archive_bytes = get_archive_payload_bytes(&arch);
        if current_free_bytes.saturating_sub(archive_bytes) < MIN_RESERVE_BYTES {
            let err_msg = format!(
                "Cannot move archive {} ({} bytes): remaining free space on target drive ({} bytes) would drop below 512 MiB reserve",
                arch.id.unwrap_or(0),
                archive_bytes,
                current_free_bytes
            );
            errors.push(err_msg);
            break; // Stop immediately to preserve the strict 512 MiB reserve invariant
        }

        // Copy phase with size verification
        let mut copied: Vec<PathBuf> = Vec::new();
        let mut copy_err = None;
        let mut total_copied_for_arch: u64 = 0;

        for (src, dst) in [(&zip_src, &zip_dest), (&man_src, &man_dest)] {
            if !src.exists() {
                continue;
            }
            match fs::copy(src, dst) {
                Ok(bytes_copied) => {
                    let src_size = fs::metadata(src).map(|m| m.len()).unwrap_or(0);
                    let dst_size = fs::metadata(dst).map(|m| m.len()).unwrap_or(1);
                    if src_size != dst_size {
                        copy_err = Some(format!(
                            "Size mismatch after copy: {:?} ({} vs {})",
                            src, src_size, dst_size
                        ));
                        break;
                    }
                    total_copied_for_arch = total_copied_for_arch.saturating_add(bytes_copied);
                    copied.push(dst.clone());
                }
                Err(e) => {
                    copy_err = Some(format!("Failed to copy {:?}: {}", src, e));
                    break;
                }
            }
        }

        if let Some(err) = copy_err {
            // Clean up partial target files
            for dst in copied {
                let _ = fs::remove_file(dst);
            }
            errors.push(err);
            continue;
        }

        // Update database record
        if let Err(e) = db.update_vault_paths(arch.id.unwrap_or(0), &zip_dest, &man_dest) {
            // Roll back targets if DB update failed
            let _ = fs::remove_file(&zip_dest);
            let _ = fs::remove_file(&man_dest);
            errors.push(format!(
                "Database update failed for archive {}: {}",
                arch.id.unwrap_or(0),
                e
            ));
            continue;
        }

        // Unlink source files
        let _ = fs::remove_file(&zip_src);
        let _ = fs::remove_file(&man_src);

        // Deduct actual transferred bytes from target drive's tracked free space
        current_free_bytes = current_free_bytes.saturating_sub(total_copied_for_arch);
        moved_count += 1;
    }

    Ok(MoveVaultSummary {
        moved_count,
        errors,
    })
}
