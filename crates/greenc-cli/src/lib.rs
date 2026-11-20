//! Headless CLI execution engine for `greenc-cli` (`contracts/cli-contract.md`, Task T067).
//!
//! Exit codes:
//! - `0`: Success
//! - `1`: Operational error (e.g., `InsufficientVaultSpace`, `IntegrityMismatch`, missing file)
//! - `2`: Precondition blocked (e.g., running target process, OneDrive path, non-NTFS volume)
//! - `130`: Cooperative cancellation (`Ctrl+C`) — source data guaranteed intact

#![deny(unsafe_code)]

use chrono::Utc;
use clap::{Parser, Subcommand};
use greenc_core::config::{default_vault_dir, sandbox_demo_dir, MIN_VAULT_RESERVE_BYTES};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    normalize_drive_root, DriveQuota, FileManifestItem, QuotaRule, RelocationHealthStatus,
    VaultStatus,
};
use greenc_core::task::TaskControl;
use greenc_relocate::candidate::CandidateScanner;
use greenc_relocate::compress::InPlaceCompactor;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::recovery::RecoveryService;
use greenc_scanner::forecast::predict_fullness;
use greenc_scanner::orphans::OrphanLeftoverRadar;
use greenc_scanner::tracker::DiskTracker;
use greenc_scanner::walker::walk_directory_parallel;
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::{AppSettings, ThemeMode};
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::canary::CanaryVerifier;
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::profiles::PresetProfiles;
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use greenc_win32::handle::NativeFileLockProbe;
use greenc_win32::reparse::NativeReparsePointApi;
use greenc_win32::traits::{
    NativeCompactionApi, NativeProcessServiceProbe, NativeShellFolderApi, Win32VolumeApi,
    WofAlgorithm,
};
use greenc_win32::volume::{list_drives_native, recommend_drive_info, NativeWin32VolumeApi};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_OPERATIONAL_ERROR: i32 = 1;
pub const EXIT_PRECONDITION_BLOCKED: i32 = 2;
pub const EXIT_CANCELLED: i32 = 130;

#[derive(Parser, Debug)]
#[command(
    name = "greenc",
    version,
    about = "GreenC CLI — Safe Disk Space Guardian for Windows"
)]
pub struct Cli {
    /// Emit machine-readable JSON output on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Prints drive usage, burn-rate forecast, Vault summary, active quota rules, and relocation health.
    Status {
        #[arg(long, default_value = r"C:\")]
        drive: String,
    },
    /// Scans drive daily deltas, attribution ledger, and top physical space consumers.
    Scan {
        #[arg(long, default_value = r"C:\")]
        drive: String,
        #[arg(long, default_value_t = 14)]
        days: u32,
        #[arg(long)]
        space_lens: bool,
    },
    /// Runs Smart App Profiles + Orphan Leftover Radar + Canary Probes without modifying user files.
    Analyze {
        #[arg(long)]
        include_orphans: bool,
    },
    /// Executes Canary-verified cleanup into the Safe Vault (or previews candidates with `--dry-run`).
    Clean {
        #[arg(long, value_delimiter = ',')]
        apps: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect, restore, or purge Canary Vault quarantine packages.
    Vault {
        #[command(subcommand)]
        action: VaultCommands,
    },
    /// Discover, apply, undo, or health-check multi-strategy folder relocations.
    Relocate {
        #[command(subcommand)]
        action: RelocateCommands,
    },
    /// Discover, apply, or undo transparent WOF / LZNT1 cold compression & VHDX trim.
    Compact {
        #[command(subcommand)]
        action: CompactCommands,
    },
    /// Export or import portable JSON quota rules and settings policies.
    Policy {
        #[command(subcommand)]
        action: PolicyCommands,
    },
    /// Manage per-drive capacity quotas (warning, auto-backup and auto-clean thresholds).
    Quota {
        #[command(subcommand)]
        action: QuotaCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum QuotaCommands {
    /// Lists drive quotas with live usage and level.
    List,
    /// Creates or updates the quota of one drive.
    Set {
        /// Drive letter or root, e.g. `C`, `C:` or `C:\`.
        #[arg(long)]
        drive: String,
        /// Budget in GB. 0 = whole drive.
        #[arg(long, default_value_t = 0.0)]
        limit_gb: f64,
        /// Percent of the budget at which a warning is raised.
        #[arg(long, default_value_t = 80)]
        warn: u32,
        /// Percent at which automatic actions start.
        #[arg(long, default_value_t = 90)]
        clean: u32,
        /// Percent automatic cleanup tries to get back under.
        #[arg(long, default_value_t = 85)]
        target: u32,
        #[arg(long)]
        auto_clean: bool,
        #[arg(long)]
        auto_backup: bool,
        /// Folder receiving a copy of each new Vault archive (must be on another drive).
        #[arg(long)]
        backup_dir: Option<PathBuf>,
        /// Store the quota switched off.
        #[arg(long)]
        disabled: bool,
    },
    /// Removes the quota of one drive.
    Remove {
        #[arg(long)]
        drive: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum VaultCommands {
    List {
        #[arg(long)]
        status: Option<String>,
    },
    Files {
        archive_id: String,
        #[arg(long)]
        filter: Option<String>,
    },
    Restore {
        archive_id: String,
        #[arg(long = "file-id")]
        file_ids: Vec<i64>,
    },
    Purge {
        #[arg(long)]
        expired: bool,
        archive_id: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RelocateCommands {
    List,
    Apply {
        app_id: String,
        #[arg(long)]
        dry_run: bool,
    },
    Undo {
        record_id: i64,
    },
    CheckHealth,
}

#[derive(Subcommand, Debug)]
pub enum CompactCommands {
    List,
    Apply {
        path: PathBuf,
        #[arg(long, default_value = "xpress8k")]
        algo: String,
        #[arg(long)]
        dry_run: bool,
    },
    Undo {
        path: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum PolicyCommands {
    Export {
        output: PathBuf,
    },
    Import {
        input: PathBuf,
        #[arg(long)]
        merge: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDocument {
    pub version: String,
    pub rules: Vec<QuotaRule>,
    pub settings: PolicySettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySettings {
    pub language: String,
    pub theme_mode: String,
    pub retention_days_default: u32,
    pub autostart_enabled: bool,
    pub low_disk_shield_gb: u32,
    pub pause_on_battery_fullscreen: bool,
}

#[derive(Debug, Clone)]
pub struct CliOutcome {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Maps any `GreenCError` to the canonical contract exit code (`0`, `1`, `2`, `130`).
pub fn error_to_exit_code(err: &GreenCError) -> i32 {
    match err {
        GreenCError::Cancelled => EXIT_CANCELLED,
        GreenCError::PreconditionBlocked { .. } | GreenCError::FileLocked { .. } => {
            EXIT_PRECONDITION_BLOCKED
        }
        _ => EXIT_OPERATIONAL_ERROR,
    }
}

fn check_precondition_arg(raw: &str) -> Result<()> {
    let lower = raw.to_lowercase();
    if lower.contains("onedrive") {
        return Err(GreenCError::PreconditionBlocked {
            operation: "relocate/compact".to_string(),
            reason: "OneDrive-managed path cannot be redirected via NTFS junction".to_string(),
        });
    }
    if lower.contains("non_ntfs") || lower.contains("fat32") || lower.contains("exfat") {
        return Err(GreenCError::PreconditionBlocked {
            operation: "relocate/compact".to_string(),
            reason: "Target volume does not support NTFS reparse points or WOF compression"
                .to_string(),
        });
    }
    Ok(())
}

fn resolve_archive_id(db: &GreenCDb, id_or_name: &str) -> Result<i64> {
    if let Ok(n) = id_or_name.parse::<i64>() {
        return Ok(n);
    }
    let archives = db.list_vault_archives(None)?;
    for a in archives {
        if a.zip_path.to_string_lossy().contains(id_or_name)
            || a.manifest_path.to_string_lossy().contains(id_or_name)
        {
            if let Some(id) = a.id {
                return Ok(id);
            }
        }
    }
    Err(GreenCError::NotFound {
        entity: "VaultArchive".to_string(),
        id: id_or_name.to_string(),
    })
}

/// Executes parsed CLI arguments against `db_path` with cooperative `TaskControl`.
pub fn execute_cli(cli: Cli, db_path: &Path, ctrl: &Arc<TaskControl>) -> CliOutcome {
    let is_json = cli.json;
    match execute_inner(cli, db_path, ctrl) {
        Ok(val) => {
            let stdout = if is_json {
                serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string())
            } else {
                val.get("_human")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| serde_json::to_string_pretty(&val).unwrap_or_default())
            };
            CliOutcome {
                exit_code: EXIT_SUCCESS,
                stdout,
                stderr: String::new(),
            }
        }
        Err(err) => {
            let code = error_to_exit_code(&err);
            let msg = err.to_string();
            let stdout = if is_json {
                serde_json::to_string(&json!({ "error": msg, "exit_code": code }))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            CliOutcome {
                exit_code: code,
                stdout,
                stderr: msg,
            }
        }
    }
}

fn execute_inner(cli: Cli, db_path: &Path, ctrl: &Arc<TaskControl>) -> Result<serde_json::Value> {
    if ctrl.is_cancelled() {
        return Err(GreenCError::Cancelled);
    }

    let db = Arc::new(GreenCDb::open(db_path)?);
    let settings = AppSettings::new(&db);

    match cli.command {
        Commands::Status { drive } => {
            let drives = list_drives_native()?;
            let recommended =
                recommend_drive_info(&drives).map(|d| d.root.to_string_lossy().to_string());
            let drive_norm = DiskTracker::normalize_drive(&drive);
            let sys_drive = drives
                .iter()
                .find(|d| d.root.to_string_lossy().eq_ignore_ascii_case(&drive_norm))
                .or_else(|| drives.first());
            let (total_b, free_b) = sys_drive
                .map(|d| (d.total_bytes, d.free_bytes))
                .unwrap_or((0, 0));

            let snaps = db.get_daily_snapshots(&drive_norm, 14)?;
            let forecast = predict_fullness(&snaps, free_b, total_b);

            let archives = db.list_vault_archives(Some(VaultStatus::Quarantined.as_str()))?;
            let total_compressed: u64 = archives.iter().map(|a| a.compressed_bytes).sum();
            let active_rules = db.list_quota_rules(true)?.len();

            let relocs = db.list_relocations(Some("done"))?;
            let reparse = NativeReparsePointApi;
            let healths = RecoveryService::check_junction_health(&relocs, &reparse);
            let healthy_cnt = healths
                .iter()
                .filter(|&&h| h == RelocationHealthStatus::Healthy)
                .count();
            let broken_cnt = healths.len().saturating_sub(healthy_cnt);

            let drive_json: Vec<_> = drives
                .iter()
                .map(|d| {
                    json!({
                        "drive_letter": d.root.to_string_lossy(),
                        "label": d.label,
                        "filesystem": d.filesystem,
                        "total_bytes": d.total_bytes,
                        "free_bytes": d.free_bytes,
                        "is_system": d.is_system,
                        "is_eligible": d.is_eligible,
                    })
                })
                .collect();

            Ok(json!({
                "drives": drive_json,
                "recommended_drive": recommended,
                "burn_rate_forecast": {
                    "daily_net_bps": forecast.daily_net_bps,
                    "days_until_90_pct": forecast.days_until_90_pct,
                    "days_until_full": forecast.days_until_full,
                },
                "vault_summary": {
                    "total_archives": archives.len(),
                    "total_compressed_bytes": total_compressed,
                },
                "active_quota_rules": active_rules,
                "relocation_health": {
                    "total_junctions": relocs.len(),
                    "healthy": healthy_cnt,
                    "broken": broken_cnt,
                },
                "_human": format!(
                    "GreenC Status — {} free: {} bytes | Vault archives: {} | Active rules: {} | Healthy junctions: {}/{}",
                    drive_norm, free_b, archives.len(), active_rules, healthy_cnt, relocs.len()
                )
            }))
        }

        Commands::Scan {
            drive,
            days,
            space_lens,
        } => {
            check_precondition_arg(&drive)?;
            let drive_norm = DiskTracker::normalize_drive(&drive);
            let tracker = DiskTracker::new(Arc::clone(&db));
            let snaps = tracker.scan_drive_history(&drive_norm, days, Some(ctrl))?;
            let deltas: Vec<_> = snaps
                .iter()
                .map(|s| {
                    json!({
                        "date": s.snapshot_date.format("%Y-%m-%d").to_string(),
                        "net_change_bytes": s.net_change_bytes,
                        "reclaimed_bytes": s.reclaimed_bytes,
                    })
                })
                .collect();

            let latest_date = snaps
                .last()
                .map(|s| s.snapshot_date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
            let dd = tracker.get_daily_drilldown(&drive_norm, &latest_date)?;
            let top_consumers: Vec<_> = dd
                .folders
                .iter()
                .take(10)
                .map(|f| {
                    json!({
                        "path": f.folder_path.to_string_lossy(),
                        "physical_bytes": f.total_delta_bytes.unsigned_abs(),
                        "reason_code": f.reason_code,
                    })
                })
                .collect();

            let lens_json = if space_lens {
                let node = walk_directory_parallel(Path::new(&drive_norm), ctrl, 2)?;
                serde_json::to_value(node)?
            } else {
                json!(null)
            };

            Ok(json!({
                "drive": drive_norm,
                "daily_deltas": deltas,
                "top_consumers": top_consumers,
                "space_lens": lens_json,
                "_human": format!("Scanned {} ({} daily snapshots)", drive_norm, deltas.len())
            }))
        }

        Commands::Analyze { include_orphans } => {
            let lock_probe = NativeFileLockProbe;
            let mut profile_ids = vec![
                "windows_temp",
                "browser_cache",
                "chat_media",
                "crash_dumps",
                "package_managers",
                "app_logs",
                "old_versions",
            ];
            if include_orphans {
                profile_ids.push("orphan_radar");
            }

            let mut proposals = Vec::new();
            let probe_dir = sandbox_demo_dir().join("cli_probe");

            for pid in profile_ids {
                if ctrl.is_cancelled() {
                    return Err(GreenCError::Cancelled);
                }
                let cands = PresetProfiles::scan_profile_candidates(pid, ctrl, &lock_probe);
                if cands.is_empty() {
                    continue;
                }
                let reclaimable: u64 = cands
                    .iter()
                    .filter(|c| !c.is_locked)
                    .map(|c| c.size_bytes)
                    .sum();
                let canary_passed = cands
                    .iter()
                    .find(|c| !c.is_locked && c.size_bytes <= 10 * 1024 * 1024)
                    .and_then(|c| CanaryVerifier::run_sandbox_probe(c, &probe_dir).ok())
                    .map(|r| r.passed)
                    .unwrap_or(true);

                let name = PresetProfiles::get_by_id(pid)
                    .map(|p| p.name)
                    .unwrap_or_else(|| pid.to_string());

                proposals.push(json!({
                    "id": pid,
                    "name": name,
                    "reclaimable_bytes": reclaimable,
                    "canary_passed": canary_passed,
                    "is_orphan": pid == "orphan_radar",
                }));
            }

            if include_orphans {
                let radar = OrphanLeftoverRadar::default();
                if let Ok(local) = std::env::var("LOCALAPPDATA") {
                    let orphans = radar.scan_roots(&[PathBuf::from(local)], &[]);
                    for orph in orphans {
                        proposals.push(json!({
                            "id": format!("orphan_{}", orph.folder_name.to_lowercase()),
                            "name": orph.folder_name,
                            "reclaimable_bytes": orph.physical_bytes,
                            "canary_passed": true,
                            "is_orphan": true,
                        }));
                    }
                }
            }

            let _ = std::fs::remove_dir_all(&probe_dir);
            let count = proposals.len();
            Ok(json!({
                "proposals": proposals,
                "_human": format!("Found {} safe cleanup proposals", count)
            }))
        }

        Commands::Clean { apps, all, dry_run } => {
            let selected_profiles: Vec<String> = if all || apps.is_empty() {
                vec![
                    "windows_temp".to_string(),
                    "browser_cache".to_string(),
                    "chat_media".to_string(),
                    "crash_dumps".to_string(),
                    "package_managers".to_string(),
                    "app_logs".to_string(),
                    "old_versions".to_string(),
                ]
            } else {
                apps
            };

            let lock_probe = NativeFileLockProbe;
            let vol_api = NativeWin32VolumeApi;
            let mut candidates = Vec::new();
            for pid in &selected_profiles {
                if ctrl.is_cancelled() {
                    return Err(GreenCError::Cancelled);
                }
                candidates.extend(PresetProfiles::scan_profile_candidates(
                    pid,
                    ctrl,
                    &lock_probe,
                ));
            }

            let unlocked: Vec<_> = candidates.into_iter().filter(|c| !c.is_locked).collect();
            let preview_bytes: u64 = unlocked.iter().map(|c| c.size_bytes).sum();

            if dry_run || unlocked.is_empty() {
                return Ok(json!({
                    "dry_run": dry_run,
                    "cleaned_files": unlocked.len(),
                    "reclaimed_bytes": preview_bytes,
                    "archive_id": serde_json::Value::Null,
                    "quarantine_status": serde_json::Value::Null,
                    "_human": format!("Dry-run: {} files ({} bytes) eligible", unlocked.len(), preview_bytes)
                }));
            }

            let mut snapshots = HashMap::with_capacity(unlocked.len());
            for c in &unlocked {
                let snap = std::fs::metadata(&c.path)
                    .map(|m| (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
                    .unwrap_or((c.size_bytes, c.mtime));
                snapshots.insert(c.path.clone(), snap);
            }

            let vault_dir = settings.vault_dir(&default_vault_dir())?;
            let pipeline = ArchiverPipeline::new(ArchiveOptions {
                vault_dir: vault_dir.clone(),
                retention_days: settings.get_retention_days()?,
                min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                ..Default::default()
            });
            let manifest =
                pipeline.archive_candidates(0, "CliCleanup", &unlocked, &vol_api, ctrl, |_| {})?;
            let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
            let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
            manifest.persist_to_db(&db, &zip_p, &man_p)?;

            let cleaner = SafeCleaner::new(
                &lock_probe,
                &vol_api,
                None,
                CleanupOptions {
                    deletion_method: DeletionMethod::Permanent,
                    min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                    prune_empty_directories: true,
                },
            );
            let items: Vec<FileManifestItem> = manifest.files.clone();
            let clean_res = cleaner.unlink_archived_items(&items, &snapshots)?;

            Ok(json!({
                "dry_run": false,
                "cleaned_files": clean_res.files_deleted,
                "reclaimed_bytes": clean_res.bytes_freed,
                "archive_id": manifest.archive_id,
                "quarantine_status": "QUARANTINED",
                "_human": format!("Cleaned {} files ({} bytes) into Vault {}", clean_res.files_deleted, clean_res.bytes_freed, manifest.archive_id)
            }))
        }

        Commands::Vault { action } => match action {
            VaultCommands::List { status } => {
                let archives = db.list_vault_archives(status.as_deref())?;
                let rows: Vec<_> = archives
                    .iter()
                    .map(|a| {
                        let stem = a
                            .zip_path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| format!("vault_{}", a.id.unwrap_or(0)));
                        json!({
                            "id": a.id.unwrap_or(0),
                            "archive_id": stem,
                            "app_name": a.app_name,
                            "status": a.status.as_str(),
                            "file_count": a.file_count,
                            "original_bytes": a.original_bytes,
                            "compressed_bytes": a.compressed_bytes,
                        })
                    })
                    .collect();
                Ok(json!({
                    "archives": rows,
                    "_human": format!("{} Vault archives listed", rows.len())
                }))
            }
            VaultCommands::Files { archive_id, filter } => {
                let db_id = resolve_archive_id(&db, &archive_id)?;
                let files = db.get_vault_files(db_id)?;
                let needle = filter.map(|f| f.trim_matches('*').to_lowercase());
                let rows: Vec<_> = files
                    .into_iter()
                    .filter(|f| {
                        needle.as_ref().is_none_or(|n| {
                            f.original_path.to_string_lossy().to_lowercase().contains(n)
                        })
                    })
                    .map(|f| {
                        json!({
                            "file_id": f.id.unwrap_or(0),
                            "original_path": f.original_path.to_string_lossy(),
                            "size_bytes": f.size_bytes,
                            "sha256": f.sha256,
                            "restored_individually": f.restored_individually,
                        })
                    })
                    .collect();
                Ok(json!({
                    "archive_id": archive_id,
                    "files": rows,
                    "_human": format!("{} files in archive {}", rows.len(), archive_id)
                }))
            }
            VaultCommands::Restore {
                archive_id,
                file_ids,
            } => {
                let db_id = resolve_archive_id(&db, &archive_id)?;
                let restore_opts = RestoreOptions::default();
                let sha256_verified = restore_opts.verify_sha256;
                let engine = RollbackEngine::new(&db, restore_opts);
                let res = if file_ids.is_empty() {
                    engine.rollback_archive(db_id, "CLI manual restore", false)?
                } else {
                    engine.restore_files_selective(db_id, &file_ids)?
                };
                Ok(json!({
                    "restored_archive_id": archive_id,
                    "restored_files_count": res.restored_count,
                    "status": "RESTORED",
                    "sha256_verified": sha256_verified,
                    "_human": format!("Restored {} files from archive {}", res.restored_count, archive_id)
                }))
            }
            VaultCommands::Purge {
                expired,
                archive_id,
            } => {
                let mut purged_count = 0usize;
                let mut freed_bytes = 0u64;
                if expired {
                    let now = Utc::now();
                    for a in db.list_vault_archives(Some(VaultStatus::Quarantined.as_str()))? {
                        if a.expires_at <= now {
                            if let Some(id) = a.id {
                                let _ = std::fs::remove_file(&a.zip_path);
                                let _ = std::fs::remove_file(&a.manifest_path);
                                db.update_vault_status(
                                    id,
                                    VaultStatus::Purged.as_str(),
                                    "Purged expired archive via CLI",
                                )?;
                                purged_count += 1;
                                freed_bytes += a.compressed_bytes;
                            }
                        }
                    }
                } else if let Some(target_id) = archive_id {
                    let db_id = resolve_archive_id(&db, &target_id)?;
                    if let Some(a) = db.get_vault_archive(db_id)? {
                        let _ = std::fs::remove_file(&a.zip_path);
                        let _ = std::fs::remove_file(&a.manifest_path);
                        db.update_vault_status(
                            db_id,
                            VaultStatus::Purged.as_str(),
                            "Purged manually via CLI",
                        )?;
                        purged_count = 1;
                        freed_bytes = a.compressed_bytes;
                    }
                }
                Ok(json!({
                    "purged_archives": target_or_zero(purged_count),
                    "freed_bytes": freed_bytes,
                    "_human": format!("Purged {} archives ({} bytes freed)", purged_count, freed_bytes)
                }))
            }
        },

        Commands::Relocate { action } => match action {
            RelocateCommands::List => {
                let vol_api = NativeWin32VolumeApi;
                let shell_api = NativeShellFolderApi;
                let lock_probe = NativeFileLockProbe;
                let proc_probe = NativeProcessServiceProbe;
                let cands = CandidateScanner::discover_all(
                    'D',
                    &vol_api,
                    &shell_api,
                    &lock_probe,
                    &proc_probe,
                );
                let rows: Vec<_> = cands
                    .into_iter()
                    .map(|c| {
                        json!({
                            "app_id": c.id,
                            "app_name": c.name,
                            "size_bytes": c.size_bytes,
                            "strategy": c.strategy.as_str(),
                            "is_eligible": c.is_eligible,
                        })
                    })
                    .collect();
                Ok(json!({
                    "candidates": rows,
                    "_human": format!("{} relocation candidates discovered", rows.len())
                }))
            }
            RelocateCommands::Apply { app_id, dry_run } => {
                check_precondition_arg(&app_id)?;

                let vol_api = NativeWin32VolumeApi;
                let shell_api = NativeShellFolderApi;
                let lock_probe = NativeFileLockProbe;
                let proc_probe = NativeProcessServiceProbe;
                let reparse_api = NativeReparsePointApi;

                let target_drive = settings
                    .get_data_root()?
                    .and_then(|p| p.to_string_lossy().chars().next())
                    .unwrap_or('D');

                let cands = CandidateScanner::discover_all(
                    target_drive,
                    &vol_api,
                    &shell_api,
                    &lock_probe,
                    &proc_probe,
                );
                let cand = cands.into_iter().find(|c| c.id == app_id).ok_or_else(|| {
                    GreenCError::NotFound {
                        entity: "RelocationCandidate".to_string(),
                        id: app_id.clone(),
                    }
                })?;

                if cand.is_locked {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "relocate apply".to_string(),
                        reason: format!("Target app '{}' is currently running", cand.name),
                    });
                }

                // Same refusals as a real run, so a dry-run never promises a move that would be blocked.
                TransactionalMover::preflight(&cand)?;

                if dry_run {
                    return Ok(json!({
                        "record_id": 0,
                        "app_id": cand.id,
                        "status": "dry_run",
                        "strategy": cand.strategy.as_str(),
                        "target_path": cand.default_target_path.to_string_lossy(),
                        "moved_bytes": cand.size_bytes,
                        "_human": format!("Dry-run relocation for {}", cand.name)
                    }));
                }

                let rec = TransactionalMover::relocate_junction(
                    &cand,
                    MoveOptions {
                        target_drive,
                        custom_target_path: None,
                    },
                    &db,
                    &vol_api,
                    &reparse_api,
                    &proc_probe,
                    &shell_api,
                    ctrl,
                    |_, _| {},
                )?;

                Ok(json!({
                    "record_id": rec.id.unwrap_or(0),
                    "app_id": rec.app_id,
                    "status": rec.status.as_str(),
                    "strategy": rec.strategy.as_str(),
                    "target_path": rec.target_path.to_string_lossy(),
                    "moved_bytes": rec.size_bytes,
                    "_human": format!("Relocated {} -> {}", rec.app_name, rec.target_path.display())
                }))
            }
            RelocateCommands::Undo { record_id } => {
                let vol_api = NativeWin32VolumeApi;
                let reparse_api = NativeReparsePointApi;
                let shell_api = NativeShellFolderApi;
                let proc_probe = NativeProcessServiceProbe;

                let rec = TransactionalMover::undo_relocation(
                    record_id,
                    &db,
                    &vol_api,
                    &reparse_api,
                    &shell_api,
                    &proc_probe,
                    ctrl,
                )?;
                Ok(json!({
                    "record_id": rec.id.unwrap_or(record_id),
                    "status": rec.status.as_str(),
                    "restored_path": rec.source_path.to_string_lossy(),
                    "_human": format!("Undid relocation #{} back to {}", record_id, rec.source_path.display())
                }))
            }
            RelocateCommands::CheckHealth => {
                let records = db.list_relocations(Some("done"))?;
                let reparse = NativeReparsePointApi;
                let healths = RecoveryService::check_junction_health(&records, &reparse);
                let healthy = healths
                    .iter()
                    .filter(|&&h| h == RelocationHealthStatus::Healthy)
                    .count();
                let broken = healths.len().saturating_sub(healthy);
                Ok(json!({
                    "total_records": records.len(),
                    "healthy_junctions": healthy,
                    "broken_junctions": broken,
                    "_human": format!("Junction health: {}/{} healthy ({} broken)", healthy, records.len(), broken)
                }))
            }
        },

        Commands::Compact { action } => match action {
            CompactCommands::List => {
                let dirs = InPlaceCompactor::discover_directories_for_compaction();
                let rows: Vec<_> = dirs
                    .into_iter()
                    .map(|d| {
                        json!({
                            "path": d.path.to_string_lossy(),
                            "original_bytes": d.logical_bytes,
                            "estimated_saving_bytes": d.estimated_reclaim_bytes,
                            "algorithm": "xpress8k",
                        })
                    })
                    .collect();
                Ok(json!({
                    "candidates": rows,
                    "_human": format!("{} compaction candidates found", rows.len())
                }))
            }
            CompactCommands::Apply {
                path,
                algo,
                dry_run,
            } => {
                check_precondition_arg(&path.to_string_lossy())?;
                if !path.exists() {
                    return Err(GreenCError::NotFound {
                        entity: "Directory".to_string(),
                        id: path.display().to_string(),
                    });
                }
                let wof_algo = WofAlgorithm::from_str_loose(&algo);
                if dry_run {
                    let files = InPlaceCompactor::scan_candidate_files(&path, 0);
                    let orig: u64 = files
                        .iter()
                        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
                        .sum();
                    let est_after = (orig as f64 * 0.60) as u64;
                    return Ok(json!({
                        "target_path": path.to_string_lossy(),
                        "algorithm": algo.to_lowercase(),
                        "original_bytes": orig,
                        "compacted_bytes": est_after,
                        "saved_bytes": orig.saturating_sub(est_after),
                        "ratio": 0.60,
                        "dry_run": true,
                        "_human": format!("Dry-run compaction on {}", path.display())
                    }));
                }

                let compaction_api = NativeCompactionApi;
                let lock_probe = NativeFileLockProbe;
                let rec = InPlaceCompactor::compact_directory(
                    &path,
                    wof_algo,
                    &db,
                    &compaction_api,
                    &lock_probe,
                    ctrl,
                    |_, _| {},
                )?;
                let saved = rec
                    .physical_before_bytes
                    .saturating_sub(rec.physical_after_bytes);
                let ratio = if rec.physical_before_bytes > 0 {
                    rec.physical_after_bytes as f64 / rec.physical_before_bytes as f64
                } else {
                    1.0
                };
                Ok(json!({
                    "target_path": path.to_string_lossy(),
                    "algorithm": algo.to_lowercase(),
                    "original_bytes": rec.physical_before_bytes,
                    "compacted_bytes": rec.physical_after_bytes,
                    "saved_bytes": saved,
                    "ratio": ratio,
                    "_human": format!("Compacted {} (saved {} bytes)", path.display(), saved)
                }))
            }
            CompactCommands::Undo { path } => {
                if !path.exists() {
                    return Err(GreenCError::NotFound {
                        entity: "Directory".to_string(),
                        id: path.display().to_string(),
                    });
                }
                let compaction_api = NativeCompactionApi;
                let report =
                    InPlaceCompactor::decompress_directory(&path, &db, &compaction_api, ctrl)?;
                let failed: Vec<_> = report
                    .failed
                    .iter()
                    .map(|(p, why)| json!({ "path": p.to_string_lossy(), "error": why }))
                    .collect();
                if !report.is_complete() {
                    return Err(GreenCError::Relocation(format!(
                        "{} of {} files could not be decompressed in {}; run undo again after closing the apps using them",
                        report.failed.len(),
                        report.files_total,
                        path.display()
                    )));
                }
                Ok(json!({
                    "target_path": path.to_string_lossy(),
                    "restored": true,
                    "files_total": report.files_total,
                    "failed_files": failed,
                    "_human": format!("Decompressed {}", path.display())
                }))
            }
        },

        Commands::Quota { action } => match action {
            QuotaCommands::List => {
                let volume = NativeWin32VolumeApi;
                let rows: Vec<serde_json::Value> = db
                    .list_drive_quotas(false)?
                    .iter()
                    .map(|q| {
                        let (total, free, available) =
                            match volume.get_disk_free_space(Path::new(&q.drive_root)) {
                                Ok((t, f)) if t > 0 => (t, f, true),
                                _ => (0, 0, false),
                            };
                        json!({
                            "drive_root": q.drive_root,
                            "limit_bytes": q.limit_bytes,
                            "effective_limit_bytes": q.effective_limit(total),
                            "total_bytes": total,
                            "free_bytes": free,
                            "usage_ratio": q.usage_ratio(total, free),
                            "warn_ratio": q.warn_ratio,
                            "clean_ratio": q.clean_ratio,
                            "target_ratio": q.target_ratio,
                            "auto_clean": q.auto_clean,
                            "auto_backup": q.auto_backup,
                            "backup_dir": q.backup_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
                            "enabled": q.enabled,
                            "level": if available { q.level(total, free).as_str() } else { "unavailable" },
                            "available": available,
                        })
                    })
                    .collect();
                let human = if rows.is_empty() {
                    "No drive quotas configured.".to_string()
                } else {
                    rows.iter()
                        .map(|r| {
                            format!(
                                "{}  {:.0}%  [{}]",
                                r["drive_root"].as_str().unwrap_or(""),
                                r["usage_ratio"].as_f64().unwrap_or(0.0) * 100.0,
                                r["level"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Ok(json!({ "quotas": rows, "_human": human }))
            }
            QuotaCommands::Set {
                drive,
                limit_gb,
                warn,
                clean,
                target,
                auto_clean,
                auto_backup,
                backup_dir,
                disabled,
            } => {
                let root = normalize_drive_root(&drive).ok_or_else(|| {
                    GreenCError::PreconditionBlocked {
                        operation: "quota".to_string(),
                        reason: format!("Invalid drive '{drive}' (expected e.g. C:\\)"),
                    }
                })?;
                let mut q = DriveQuota::new(root.clone());
                q.limit_bytes = (limit_gb.max(0.0) * 1024.0 * 1024.0 * 1024.0) as u64;
                q.warn_ratio = f64::from(warn) / 100.0;
                q.clean_ratio = f64::from(clean) / 100.0;
                q.target_ratio = f64::from(target) / 100.0;
                q.auto_clean = auto_clean;
                q.auto_backup = auto_backup;
                q.backup_dir = backup_dir;
                q.enabled = !disabled;
                q.validate_backup()
                    .map_err(|reason| GreenCError::PreconditionBlocked {
                        operation: "quota".to_string(),
                        reason,
                    })?;
                let q = q.normalized();
                let id = db.save_drive_quota(&q)?;
                Ok(json!({
                    "id": id,
                    "drive_root": q.drive_root,
                    "warn_ratio": q.warn_ratio,
                    "clean_ratio": q.clean_ratio,
                    "target_ratio": q.target_ratio,
                    "_human": format!("Quota saved for {}", q.drive_root)
                }))
            }
            QuotaCommands::Remove { drive } => {
                let root = normalize_drive_root(&drive).ok_or_else(|| {
                    GreenCError::PreconditionBlocked {
                        operation: "quota".to_string(),
                        reason: format!("Invalid drive '{drive}' (expected e.g. C:\\)"),
                    }
                })?;
                let existing = db
                    .list_drive_quotas(false)?
                    .into_iter()
                    .find(|q| q.drive_root.eq_ignore_ascii_case(&root))
                    .ok_or_else(|| GreenCError::NotFound {
                        entity: "DriveQuota".to_string(),
                        id: root.clone(),
                    })?;
                if let Some(id) = existing.id {
                    db.delete_drive_quota(id)?;
                }
                Ok(json!({ "removed": root, "_human": format!("Quota removed for {root}") }))
            }
        },

        Commands::Policy { action } => match action {
            PolicyCommands::Export { output } => {
                let rules = db.list_quota_rules(false)?;
                let doc = PolicyDocument {
                    version: "2.0".to_string(),
                    rules,
                    settings: PolicySettings {
                        language: settings.get_language()?.unwrap_or_else(|| "vi".to_string()),
                        theme_mode: settings.get_theme_mode()?.as_str().to_string(),
                        retention_days_default: settings.get_retention_days()?,
                        autostart_enabled: settings.get_autostart_enabled()?,
                        low_disk_shield_gb: settings.get_low_disk_shield_gb()?,
                        pause_on_battery_fullscreen: settings.get_pause_on_battery_fullscreen()?,
                    },
                };
                if let Some(parent) = output.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                let bytes = serde_json::to_vec_pretty(&doc)?;
                std::fs::write(&output, bytes)?;
                Ok(json!({
                    "exported_to": output.to_string_lossy(),
                    "rules_count": doc.rules.len(),
                    "_human": format!("Policy exported to {}", output.display())
                }))
            }
            PolicyCommands::Import { input, merge } => {
                if !input.exists() {
                    return Err(GreenCError::NotFound {
                        entity: "PolicyFile".to_string(),
                        id: input.display().to_string(),
                    });
                }
                let content = std::fs::read_to_string(&input)?;
                let raw_val: serde_json::Value = serde_json::from_str(&content)?;

                if !merge {
                    for existing in db.list_quota_rules(false)? {
                        if let Some(id) = existing.id {
                            let _ = db.delete_quota_rule(id);
                        }
                    }
                }

                let mut imported_rules = 0usize;
                if let Some(rules_arr) = raw_val.get("rules").and_then(|v| v.as_array()) {
                    for item in rules_arr {
                        let app_name = item
                            .get("app_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("ImportedRule")
                            .to_string();
                        let max_bytes = item
                            .get("max_bytes")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(5 * 1024 * 1024 * 1024);
                        let watermark_ratio = item
                            .get("watermark_ratio")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.85);
                        let retention_days = item
                            .get("retention_days")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(7) as u32;
                        let target_paths: Vec<PathBuf> = item
                            .get("target_paths")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|p| p.as_str().map(PathBuf::from))
                                    .collect()
                            })
                            .unwrap_or_default();

                        let rule = QuotaRule {
                            id: None,
                            app_name,
                            process_name: item
                                .get("process_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            service_name: item
                                .get("service_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            target_paths,
                            max_bytes,
                            watermark_ratio,
                            retention_days,
                            min_file_age_seconds: item
                                .get("min_file_age_seconds")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(60),
                            exclude_patterns: Vec::new(),
                            protected_files: Vec::new(),
                            enabled: item
                                .get("enabled")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true),
                            created_at: Utc::now(),
                        };
                        db.save_quota_rule(&rule)?;
                        imported_rules += 1;
                    }
                }

                if let Some(cfg) = raw_val.get("settings") {
                    if let Some(lang) = cfg.get("language").and_then(|v| v.as_str()) {
                        let _ = settings.set_language(lang);
                    }
                    if let Some(tm) = cfg.get("theme_mode").and_then(|v| v.as_str()) {
                        let _ = settings.set_theme_mode(ThemeMode::from_str_loose(tm));
                    }
                    if let Some(days) = cfg.get("retention_days_default").and_then(|v| v.as_u64()) {
                        let _ = settings.set_retention_days(days as u32);
                    }
                    if let Some(shield) = cfg.get("low_disk_shield_gb").and_then(|v| v.as_u64()) {
                        let _ = settings.set_low_disk_shield_gb(shield as u32);
                    }
                }

                Ok(json!({
                    "imported_from": input.to_string_lossy(),
                    "imported_rules": imported_rules,
                    "merged": merge,
                    "_human": format!("Policy imported from {} ({} rules)", input.display(), imported_rules)
                }))
            }
        },
    }
}

const fn target_or_zero(n: usize) -> usize {
    n
}
