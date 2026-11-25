//! Safe Zalo PC cleanup engine: allowlisted regenerable caches (root + `Partitions\*`) and
//! obsolete `Zalo-x.y.z` versions (>= 3 days old, keeping running + newest or 2 newest).
//!
//! Safety invariants:
//! - NEVER touches `message`, `media`, `Local Storage`, `Session Storage`, `IndexedDB`,
//!   `WebStorage`, `Network`, `databases`, `shared_proto_db`, `Service Worker`, `blob_storage`,
//!   `tf-models`, `config.json`, or any `*.db*` / `*.rescache` files.
//! - Skips locked files gracefully and reports `"Còn X MB sẽ dọn được khi đóng Zalo"`.
//! - Respects `RegenerableCacheMode`: archives into Canary Vault in `vault` mode (default),
//!   or deletes directly in `direct` mode.
//! - Computes and reports real net freed bytes (accounting for same-drive Vault archive size).

use crate::archiver::{ArchiveOptions, ArchiverPipeline};
use crate::cache_units::{CleanUnit, CleanUnitKind};
use crate::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_core::config::MIN_VAULT_RESERVE_BYTES;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{is_same_volume, CleanCandidate, RegenerableCacheMode};
use greenc_core::task::TaskControl;
use greenc_platform::handle::query_file_identity;
use greenc_platform::traits::{FileLockProbe, Win32VolumeApi};
use greenc_scanner::zalo::{
    is_allowed_zalo_partition_cache_name, is_allowed_zalo_root_cache_name, is_protected_zalo_entry,
    is_symlink_or_reparse, ZaloDiscovery, ZALO_CACHE_MIN_AGE_SECS, ZALO_OLD_VERSION_MIN_AGE_SECS,
};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Summary of scanning a discovered Zalo installation for cleanable cache & old-version units.
#[derive(Debug, Clone)]
pub struct ZaloCleanupScan {
    pub units: Vec<CleanUnit>,
    pub cache_reclaimable_bytes: u64,
    pub cache_reclaimable_files: u64,
    pub cache_locked_bytes: u64,
    pub cache_locked_files: u64,
    pub old_versions_reclaimable_bytes: u64,
    pub old_versions_reclaimable_files: u64,
    pub old_versions_locked_bytes: u64,
    pub old_versions_locked_files: u64,
    pub total_reclaimable_bytes: u64,
    pub total_locked_bytes: u64,
    pub zalo_running: bool,
    pub locked_notice: Option<String>,
}

/// Options controlling `execute_zalo_cleanup`.
#[derive(Debug, Clone)]
pub struct ZaloCleanupOptions {
    pub dry_run: bool,
    pub mode: RegenerableCacheMode,
    pub retention_days: u32,
    pub cache_min_age: Duration,
    pub old_version_min_age: Duration,
    pub now: SystemTime,
}

impl Default for ZaloCleanupOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            mode: RegenerableCacheMode::Vault,
            retention_days: 14,
            cache_min_age: Duration::from_secs(ZALO_CACHE_MIN_AGE_SECS),
            old_version_min_age: Duration::from_secs(ZALO_OLD_VERSION_MIN_AGE_SECS),
            now: SystemTime::now(),
        }
    }
}

/// Execution report returned by `execute_zalo_cleanup`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloCleanupReport {
    pub dry_run: bool,
    pub mode: RegenerableCacheMode,
    pub cleaned_units: usize,
    pub cleaned_files: u64,
    pub logical_freed_bytes: u64,
    pub vault_bytes: u64,
    pub net_freed_bytes: i64,
    pub vault_same_drive: bool,
    pub vault_archive_id: Option<String>,
    pub locked_skipped_files: u64,
    pub locked_skipped_bytes: u64,
    pub zalo_running: bool,
    pub locked_notice: Option<String>,
}

/// Formats the plain-Vietnamese notice when Zalo is running and holds locks on cache files.
pub fn format_zalo_locked_notice(locked_bytes: u64, zalo_running: bool) -> Option<String> {
    if locked_bytes == 0 || !zalo_running {
        return None;
    }
    let mb = (locked_bytes as f64) / (1024.0 * 1024.0);
    let size_label = if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0).replace('.', ",")
    } else if mb >= 1.0 {
        format!("{:.0} MB", mb.round())
    } else {
        let kb = ((locked_bytes as f64) / 1024.0).max(1.0);
        format!("{:.0} KB", kb.round())
    };
    Some(format!("Còn {size_label} sẽ dọn được khi đóng Zalo"))
}

/// Validates that `cache_dir` is strictly inside `roaming_dir` and matches the Zalo cache allowlist.
fn is_verified_zalo_cache_dir(cache_dir: &Path, roaming_dir: &Path) -> bool {
    if is_symlink_or_reparse(cache_dir) || is_symlink_or_reparse(roaming_dir) {
        return false;
    }
    let Ok(rel) = cache_dir.strip_prefix(roaming_dir) else {
        return false;
    };
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    match comps.as_slice() {
        [single] => is_allowed_zalo_root_cache_name(single) && !is_protected_zalo_entry(single),
        [partitions, part_name, sub]
            if partitions.eq_ignore_ascii_case("Partitions")
                && !part_name.is_empty()
                && part_name != "."
                && part_name != ".." =>
        {
            is_allowed_zalo_partition_cache_name(sub) && !is_protected_zalo_entry(sub)
        }
        _ => false,
    }
}

/// Scans a [`ZaloDiscovery`] into safe, directory-oriented [`CleanUnit`] entries for:
/// 1. Allowlisted regenerable cache directories (root + `Partitions\*`), skipping locked files
///    and respecting `cache_min_age`.
/// 2. Eligible obsolete `Zalo-x.y.z` directories (already filtered for running/newest/2-newest
///    and >= 3 days age), skipped atomically if any file inside is locked.
pub fn scan_zalo_cleanup_units(
    discovery: &ZaloDiscovery,
    now: SystemTime,
    cache_min_age: Duration,
    old_version_min_age: Duration,
    lock_probe: &dyn FileLockProbe,
    zalo_running: bool,
    ctrl: &TaskControl,
) -> ZaloCleanupScan {
    let mut units = Vec::new();
    let mut cache_reclaimable_bytes = 0u64;
    let mut cache_reclaimable_files = 0u64;
    let mut cache_locked_bytes = 0u64;
    let mut cache_locked_files = 0u64;
    let mut old_versions_reclaimable_bytes = 0u64;
    let mut old_versions_reclaimable_files = 0u64;
    let mut old_versions_locked_bytes = 0u64;
    let mut old_versions_locked_files = 0u64;

    // 1. Allowlisted cache directories (organized by directory unit).
    for cinfo in &discovery.cache_dirs {
        if ctrl.is_cancelled() {
            break;
        }
        if !is_verified_zalo_cache_dir(&cinfo.path, &discovery.roaming_dir) {
            continue;
        }
        let mut dir_files = Vec::new();
        let mut dir_bytes = 0u64;
        let mut newest_used = SystemTime::UNIX_EPOCH;
        let mut stack = vec![cinfo.path.clone()];

        while let Some(curr) = stack.pop() {
            if ctrl.is_cancelled() {
                break;
            }
            let Ok(rd) = fs::read_dir(&curr) else {
                continue;
            };
            for entry in rd.flatten() {
                if ctrl.is_cancelled() {
                    break;
                }
                let p = entry.path();
                if is_symlink_or_reparse(&p) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if is_protected_zalo_entry(&name)
                    || name.to_ascii_lowercase().ends_with(".lock")
                    || name.to_ascii_lowercase().ends_with(".lck")
                {
                    continue;
                }
                let Ok(meta) = fs::symlink_metadata(&p) else {
                    continue;
                };
                if meta.is_dir() {
                    stack.push(p);
                } else if meta.is_file() {
                    let size = meta.len();
                    if lock_probe.is_file_locked(&p) {
                        cache_locked_bytes = cache_locked_bytes.saturating_add(size);
                        cache_locked_files = cache_locked_files.saturating_add(1);
                        continue;
                    }
                    let mtime = meta.modified().unwrap_or(now);
                    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
                    if age < cache_min_age {
                        continue;
                    }
                    if mtime > newest_used {
                        newest_used = mtime;
                    }
                    dir_bytes = dir_bytes.saturating_add(size);
                    dir_files.push(CleanCandidate {
                        path: p.clone(),
                        size_bytes: size,
                        mtime: meta.modified().unwrap_or(now),
                        is_locked: false,
                        profile_id: "chat_media".to_string(),
                        is_canary: false,
                        is_orphan: false,
                        hardlink_siblings: vec![p],
                    });
                }
            }
        }

        if !dir_files.is_empty() {
            cache_reclaimable_bytes = cache_reclaimable_bytes.saturating_add(dir_bytes);
            cache_reclaimable_files =
                cache_reclaimable_files.saturating_add(dir_files.len() as u64);
            let root_identity = query_file_identity(&cinfo.path);
            units.push(CleanUnit {
                root_path: cinfo.path.clone(),
                allowed_parent: discovery.roaming_dir.clone(),
                files: dir_files,
                total_bytes: dir_bytes,
                last_used: if newest_used == SystemTime::UNIX_EPOCH {
                    now
                } else {
                    newest_used
                },
                profile_id: "chat_media".to_string(),
                kind: CleanUnitKind::RegenerableAppCache,
                dev_tool: None,
                unit_label: format!("zalo_cache_dir:{}", cinfo.label),
                root_identity,
                min_age: cache_min_age,
            });
        }
    }

    // 2. Obsolete `Zalo-x.y.z` installation directories (atomic per version dir).
    for ver in &discovery.versions {
        if ctrl.is_cancelled() {
            break;
        }
        if !ver.eligible_for_clean || ver.is_kept || ver.is_running || ver.is_newest {
            continue;
        }
        if is_symlink_or_reparse(&ver.path) || !ver.path.starts_with(&discovery.programs_dir) {
            continue;
        }

        let mut ver_files = Vec::new();
        let mut ver_bytes = 0u64;
        let mut ver_locked = false;
        let mut newest_used = SystemTime::UNIX_EPOCH;
        let mut stack = vec![ver.path.clone()];

        while let Some(curr) = stack.pop() {
            if ctrl.is_cancelled() {
                break;
            }
            let Ok(rd) = fs::read_dir(&curr) else {
                ver_locked = true;
                break;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if is_symlink_or_reparse(&p) {
                    continue;
                }
                let Ok(meta) = fs::symlink_metadata(&p) else {
                    continue;
                };
                if meta.is_dir() {
                    stack.push(p);
                } else if meta.is_file() {
                    let size = meta.len();
                    if lock_probe.is_file_locked(&p) {
                        ver_locked = true;
                    }
                    let mtime = meta.modified().unwrap_or(now);
                    if mtime > newest_used {
                        newest_used = mtime;
                    }
                    ver_bytes = ver_bytes.saturating_add(size);
                    ver_files.push(CleanCandidate {
                        path: p.clone(),
                        size_bytes: size,
                        mtime: meta.modified().unwrap_or(now),
                        is_locked: false,
                        profile_id: "chat_media".to_string(),
                        is_canary: false,
                        is_orphan: false,
                        hardlink_siblings: vec![p],
                    });
                }
            }
        }

        if ver_locked {
            old_versions_locked_bytes = old_versions_locked_bytes.saturating_add(ver_bytes);
            old_versions_locked_files =
                old_versions_locked_files.saturating_add(ver_files.len() as u64);
            continue;
        }

        if newest_used != SystemTime::UNIX_EPOCH {
            let age = now.duration_since(newest_used).unwrap_or(Duration::ZERO);
            if age < old_version_min_age {
                continue;
            }
        }

        if !ver_files.is_empty() {
            old_versions_reclaimable_bytes =
                old_versions_reclaimable_bytes.saturating_add(ver_bytes);
            old_versions_reclaimable_files =
                old_versions_reclaimable_files.saturating_add(ver_files.len() as u64);
            let root_identity = query_file_identity(&ver.path);
            units.push(CleanUnit {
                root_path: ver.path.clone(),
                allowed_parent: discovery.programs_dir.clone(),
                files: ver_files,
                total_bytes: ver_bytes,
                last_used: if newest_used == SystemTime::UNIX_EPOCH {
                    now
                } else {
                    newest_used
                },
                profile_id: "chat_media".to_string(),
                kind: CleanUnitKind::RegenerableAppCache,
                dev_tool: None,
                unit_label: "zalo_old_version".to_string(),
                root_identity,
                min_age: old_version_min_age,
            });
        }
    }

    let total_reclaimable_bytes =
        cache_reclaimable_bytes.saturating_add(old_versions_reclaimable_bytes);
    let total_locked_bytes = cache_locked_bytes.saturating_add(old_versions_locked_bytes);
    let locked_notice = format_zalo_locked_notice(total_locked_bytes, zalo_running);

    ZaloCleanupScan {
        units,
        cache_reclaimable_bytes,
        cache_reclaimable_files,
        cache_locked_bytes,
        cache_locked_files,
        old_versions_reclaimable_bytes,
        old_versions_reclaimable_files,
        old_versions_locked_bytes,
        old_versions_locked_files,
        total_reclaimable_bytes,
        total_locked_bytes,
        zalo_running,
        locked_notice,
    }
}

/// Removes empty subdirectories inside `root` bottom-up without following symlinks.
/// When `remove_root_if_empty` is true (e.g. obsolete `Zalo-x.y.z`), removes `root` too if empty.
pub fn prune_empty_dirs_no_follow(root: &Path, remove_root_if_empty: bool) {
    if is_symlink_or_reparse(root) || !root.is_dir() {
        return;
    }
    let mut dirs = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        dirs.push(d.clone());
        if let Ok(rd) = fs::read_dir(&d) {
            for entry in rd.flatten() {
                let p = entry.path();
                if !is_symlink_or_reparse(&p) && p.is_dir() {
                    stack.push(p);
                }
            }
        }
    }
    for d in dirs.into_iter().rev() {
        if d == root && !remove_root_if_empty {
            continue;
        }
        let _ = fs::remove_dir(&d);
    }
}

/// Executes safe Zalo cache and obsolete-version cleanup.
///
/// - In `RegenerableCacheMode::Vault` (default): archives all unlocked files into Canary Vault
///   first, persists the manifest to SQLite, then unlinks verified files and reports real net gain.
/// - In `RegenerableCacheMode::Direct`: deletes unlocked files directly without creating a Vault
///   archive (only for allowlisted regenerable caches & obsolete versions, never chat data).
#[allow(clippy::too_many_arguments)]
pub fn execute_zalo_cleanup(
    discovery: &ZaloDiscovery,
    vault_dir: &Path,
    db: &GreenCDb,
    volume_api: &dyn Win32VolumeApi,
    lock_probe: &dyn FileLockProbe,
    zalo_running: bool,
    opts: &ZaloCleanupOptions,
    ctrl: &TaskControl,
) -> Result<ZaloCleanupReport> {
    if ctrl.is_cancelled() {
        return Err(GreenCError::Cancelled);
    }

    let scan = scan_zalo_cleanup_units(
        discovery,
        opts.now,
        opts.cache_min_age,
        opts.old_version_min_age,
        lock_probe,
        zalo_running,
        ctrl,
    );

    let sample_root = scan
        .units
        .first()
        .map(|u| u.root_path.as_path())
        .unwrap_or(discovery.roaming_dir.as_path());
    let vault_same_drive = is_same_volume(vault_dir, sample_root);
    let locked_files_total = scan
        .cache_locked_files
        .saturating_add(scan.old_versions_locked_files);

    if opts.dry_run || scan.units.is_empty() {
        let total_files: u64 = scan.units.iter().map(|u| u.files.len() as u64).sum();
        let net_est = if opts.mode == RegenerableCacheMode::Direct || !vault_same_drive {
            scan.total_reclaimable_bytes as i64
        } else {
            // Conservative estimate for same-drive Vault compression (~50%)
            (scan.total_reclaimable_bytes / 2) as i64
        };
        return Ok(ZaloCleanupReport {
            dry_run: opts.dry_run,
            mode: opts.mode,
            cleaned_units: scan.units.len(),
            cleaned_files: total_files,
            logical_freed_bytes: scan.total_reclaimable_bytes,
            vault_bytes: 0,
            net_freed_bytes: if scan.units.is_empty() { 0 } else { net_est },
            vault_same_drive,
            vault_archive_id: None,
            locked_skipped_files: locked_files_total,
            locked_skipped_bytes: scan.total_locked_bytes,
            zalo_running,
            locked_notice: scan.locked_notice,
        });
    }

    let mut cleaned_units = 0usize;
    let mut cleaned_files = 0u64;
    let mut logical_freed_bytes = 0u64;
    let mut extra_locked_files = 0u64;
    let mut extra_locked_bytes = 0u64;
    let mut vault_bytes = 0u64;
    let mut vault_archive_id = None;

    match opts.mode {
        RegenerableCacheMode::Direct => {
            for unit in &scan.units {
                if ctrl.is_cancelled() {
                    return Err(GreenCError::Cancelled);
                }
                let mut unit_deleted = 0u64;
                for cand in &unit.files {
                    if is_symlink_or_reparse(&cand.path) {
                        continue;
                    }
                    if lock_probe.is_file_locked(&cand.path) {
                        extra_locked_files += 1;
                        extra_locked_bytes = extra_locked_bytes.saturating_add(cand.size_bytes);
                        continue;
                    }
                    match fs::remove_file(&cand.path) {
                        Ok(()) => {
                            unit_deleted += 1;
                            cleaned_files += 1;
                            logical_freed_bytes =
                                logical_freed_bytes.saturating_add(cand.size_bytes);
                        }
                        Err(_) => {
                            extra_locked_files += 1;
                            extra_locked_bytes = extra_locked_bytes.saturating_add(cand.size_bytes);
                        }
                    }
                }
                if unit_deleted > 0 {
                    cleaned_units += 1;
                    let remove_root = unit.unit_label == "zalo_old_version";
                    prune_empty_dirs_no_follow(&unit.root_path, remove_root);
                }
            }
        }
        RegenerableCacheMode::Vault => {
            let all_candidates: Vec<CleanCandidate> =
                scan.units.iter().flat_map(|u| u.files.clone()).collect();
            let mut snapshots = HashMap::with_capacity(all_candidates.len());
            for c in &all_candidates {
                let snap = fs::metadata(&c.path)
                    .map(|m| (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
                    .unwrap_or((c.size_bytes, c.mtime));
                snapshots.insert(c.path.clone(), snap);
            }

            let pipeline = ArchiverPipeline::new(ArchiveOptions {
                vault_dir: vault_dir.to_path_buf(),
                retention_days: opts.retention_days,
                min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                ..Default::default()
            });
            let manifest = pipeline.archive_candidates(
                0,
                "Zalo PC",
                &all_candidates,
                volume_api,
                ctrl,
                |_| {},
            )?;
            let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
            let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
            let _ = manifest.write_to_file(&man_p);
            manifest.persist_to_db(db, &zip_p, &man_p)?;
            vault_bytes = fs::metadata(&zip_p)
                .map(|m| m.len())
                .unwrap_or(manifest.compressed_size);
            vault_archive_id = Some(manifest.archive_id.clone());

            let cleaner = SafeCleaner::new(
                lock_probe,
                volume_api,
                None,
                CleanupOptions {
                    deletion_method: DeletionMethod::Permanent,
                    min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                    prune_empty_directories: false,
                },
            );
            let clean_res = cleaner.unlink_archived_items(&manifest.files, &snapshots)?;
            cleaned_files = clean_res.files_deleted;
            logical_freed_bytes = clean_res.bytes_freed;
            extra_locked_files = clean_res.skipped_files_count;
            if clean_res.skipped_files_count > 0 {
                extra_locked_bytes = manifest.total_size.saturating_sub(clean_res.bytes_freed);
            }

            for unit in &scan.units {
                let any_gone = unit.files.iter().any(|c| !c.path.exists());
                if any_gone {
                    cleaned_units += 1;
                    let remove_root = unit.unit_label == "zalo_old_version";
                    prune_empty_dirs_no_follow(&unit.root_path, remove_root);
                }
            }
        }
    }

    let net_freed_bytes = if vault_same_drive {
        (logical_freed_bytes as i64) - (vault_bytes as i64)
    } else {
        logical_freed_bytes as i64
    };
    let total_locked_files = locked_files_total.saturating_add(extra_locked_files);
    let total_locked_bytes = scan.total_locked_bytes.saturating_add(extra_locked_bytes);
    let locked_notice = format_zalo_locked_notice(total_locked_bytes, zalo_running);

    Ok(ZaloCleanupReport {
        dry_run: false,
        mode: opts.mode,
        cleaned_units,
        cleaned_files,
        logical_freed_bytes,
        vault_bytes,
        net_freed_bytes,
        vault_same_drive,
        vault_archive_id,
        locked_skipped_files: total_locked_files,
        locked_skipped_bytes: total_locked_bytes,
        zalo_running,
        locked_notice,
    })
}
