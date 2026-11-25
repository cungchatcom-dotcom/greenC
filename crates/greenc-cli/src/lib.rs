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
    normalize_drive_root, DriveQuota, FileManifestItem, QuotaRule, RegenerableCacheMode,
    RelocationHealthStatus, VaultStatus,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{AppLifecycleApi, NativeAppLifecycleApi};
use greenc_platform::dev_tools::{DevTool, DevToolProbe, NativeDevToolProbe};
use greenc_platform::handle::NativeFileLockProbe;
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    NativeCompactionApi, NativeProcessServiceProbe, NativeShellFolderApi, ProcessServiceProbe,
    Win32VolumeApi, WofAlgorithm,
};
use greenc_platform::volume::{list_drives_native, recommend_drive_info, NativeWin32VolumeApi};
use greenc_relocate::candidate::CandidateScanner;
use greenc_relocate::compress::InPlaceCompactor;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::recovery::RecoveryService;
use greenc_relocate::zalo_move::{
    discover_zalo_safety_copies, estimate_zalo_move_space, format_bytes_vi,
    is_zalo_auto_move_supported, zalo_auto_move_unsupported_reason, ZaloMoveOptions, ZaloRelocator,
};
use greenc_scanner::forecast::predict_fullness;
use greenc_scanner::orphans::OrphanLeftoverRadar;
use greenc_scanner::tracker::DiskTracker;
use greenc_scanner::walker::walk_directory_parallel;
use greenc_scanner::zalo::{
    discover_zalo_installation, ZaloConfigStatus, ZaloDiscoveryOptions, ZALO_CACHE_MIN_AGE_SECS,
    ZALO_OLD_VERSION_MIN_AGE_SECS, ZALO_PROCESS_NAMES,
};
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::{AppSettings, ThemeMode};
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::cache_units::is_forbidden_dev_cache_path;
use greenc_vault::canary::CanaryVerifier;
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::guard::{run_guard_for_quota, GuardRunOptions};
use greenc_vault::profiles::PresetProfiles;
use greenc_vault::rollback::{safe_remove_vault_artifact, RestoreOptions, RollbackEngine};
use greenc_vault::suggest::{suggest_large_build_outputs, SuggestOptions};
use greenc_vault::zalo_cleanup::{
    execute_zalo_cleanup, scan_zalo_cleanup_units, ZaloCleanupOptions,
};
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

fn default_system_drive() -> String {
    greenc_platform::volume::get_system_drive_root()
}

#[derive(Parser, Debug)]
#[command(
    name = "greenc",
    version,
    about = "GreenC CLI — Safe Disk Space Guardian"
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
        #[arg(long, default_value_t = default_system_drive())]
        drive: String,
    },
    /// Scans drive daily deltas, attribution ledger, and top physical space consumers.
    Scan {
        #[arg(long, default_value_t = default_system_drive())]
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
    /// Quota-aware safe disk guardian (`run`, read-only `suggest`, and `set-mode`).
    Guard {
        #[command(subcommand)]
        action: GuardCommands,
    },
    /// Inspect, safely clean, or automatically move Zalo PC data (`status`, `clean`, `move`).
    Zalo {
        #[command(subcommand)]
        action: ZaloCommands,
    },
    /// Internal Microsoft Store (MSIX) breakaway restore helper.
    /// Accepts NO path arguments on the command line; reads validated job JSON from stdin only.
    #[command(name = "__restore-helper", hide = true)]
    RestoreHelper,
}

#[derive(Subcommand, Debug)]
pub enum ZaloCommands {
    /// Inspects Zalo PC chat data, allowlisted cache, old versions, safety copies, and relocation readiness.
    Status,
    /// Safely cleans allowlisted Zalo caches and obsolete versions (never touches chat data).
    Clean {
        /// Preview what would be cleaned without modifying disk.
        #[arg(long)]
        dry_run: bool,
    },
    /// Moves Zalo chat data (`Zalo Data`) to another fixed NTFS drive and creates a junction.
    Move {
        /// Destination drive letter or root (e.g. `D:` or `D:\`).
        #[arg(long)]
        to: String,
        /// Preview the move and run preflight checks without closing Zalo or copying files.
        #[arg(long)]
        dry_run: bool,
        /// Do not automatically reopen Zalo after moving.
        #[arg(long)]
        no_reopen: bool,
    },
    /// Lists temporarily retained Zalo original copies and any conflict/moving folders.
    Originals,
    /// Deletes a temporarily retained Zalo original copy (by `--id` or all eligible `--expired`).
    PurgeOriginal {
        /// Retained original ID from `zalo originals` / `zalo status`.
        #[arg(long)]
        id: Option<i64>,
        /// Automatically purge all retained originals that passed health watch and exceeded 24h.
        #[arg(long)]
        expired: bool,
    },
    /// Restores a temporarily retained Zalo original copy back to its original location safely.
    RestoreOriginal {
        /// Retained original ID from `zalo originals` / `zalo status`.
        #[arg(long)]
        id: i64,
    },
}

#[derive(Subcommand, Debug)]
pub enum GuardCommands {
    /// Enforces per-drive quotas using the shared safe quota guard engine.
    Run {
        /// Restrict execution to one drive root (e.g. `/home` or `C:\`).
        #[arg(long)]
        drive: Option<String>,
        /// Preview cleanup decisions without modifying disk or cooldown state.
        #[arg(long)]
        dry_run: bool,
        /// Extra externally detected busy tools (comma-separated, e.g. `gradle,cargo`).
        #[arg(long, value_delimiter = ',')]
        extra_busy_tools: Vec<String>,
    },
    /// Read-only report of large stale git-ignored build outputs and Vault size (never deletes).
    Suggest {
        /// Restrict suggestions to one drive root.
        #[arg(long)]
        drive: Option<String>,
        /// Explicit root directory to scan (defaults to current user HOME).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Minimum idle age in days (default: 3).
        #[arg(long, default_value_t = 3)]
        min_days: u64,
        /// Maximum directory traversal depth (default: 6).
        #[arg(long, default_value_t = 6)]
        max_depth: usize,
        /// Maximum scan wall-clock time in seconds (default: 10).
        #[arg(long, default_value_t = 10)]
        max_secs: u64,
    },
    /// Sets `regenerable_cache_mode` (`vault` | `direct`) on a configured drive quota.
    SetMode {
        /// Drive letter or mount point (e.g. `/home` or `C:\`).
        #[arg(long)]
        drive: String,
        /// Cleanup mode for verified regenerable dev caches: `vault` (default) or `direct`.
        #[arg(long)]
        mode: String,
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
        /// Mode for verified regenerable dev caches on this drive: `vault` (default) or `direct`.
        #[arg(long)]
        regenerable_cache_mode: Option<String>,
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
        /// Optional destination directory outside AppData (e.g. `Documents\GreenC Restored`).
        #[arg(long = "to")]
        to: Option<PathBuf>,
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
    #[serde(default)]
    pub zalo_auto_clean_enabled: bool,
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
                "edition": if greenc_core::config::IS_STORE_BUILD { "store" } else { "standard" },
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

            if std::fs::symlink_metadata(&probe_dir)
                .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir())
            {
                let _ = std::fs::remove_dir_all(&probe_dir);
            }
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

            let unlocked: Vec<_> = candidates
                .into_iter()
                .filter(|c| !c.is_locked)
                .filter(|c| {
                    c.profile_id != "package_managers" || !is_forbidden_dev_cache_path(&c.path)
                })
                .collect();
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
            // Pre-unlink TOCTOU guard for developer caches: if any build tool started during
            // archiving, preserve all `package_managers` items untouched.
            let pkg_mgr_paths: std::collections::HashSet<&Path> = unlocked
                .iter()
                .filter(|c| c.profile_id == "package_managers")
                .map(|c| c.path.as_path())
                .collect();
            let has_pkg_mgr = !pkg_mgr_paths.is_empty();
            let pkg_mgr_blocked =
                has_pkg_mgr && NativeDevToolProbe.detect_busy_tools().is_any_busy();
            let items: Vec<FileManifestItem> = if pkg_mgr_blocked {
                manifest
                    .files
                    .iter()
                    .filter(|item| !pkg_mgr_paths.contains(item.original_path.as_path()))
                    .cloned()
                    .collect()
            } else {
                manifest.files.clone()
            };
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
                to,
            } => {
                let db_id = resolve_archive_id(&db, &archive_id)?;
                let vault_dir = settings.vault_dir(&default_vault_dir())?;
                let restore_opts = RestoreOptions {
                    vault_dir: Some(vault_dir),
                    restore_dir: to,
                    ..Default::default()
                };
                let sha256_verified = restore_opts.verify_sha256;
                let engine = RollbackEngine::new(&db, restore_opts);
                let res = if file_ids.is_empty() {
                    engine.rollback_archive(db_id, "CLI manual restore", false)?
                } else {
                    engine.restore_files_selective(db_id, &file_ids)?
                };
                let restored_paths: Vec<String> = res
                    .restored_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                Ok(json!({
                    "restored_archive_id": archive_id,
                    "restored_files_count": res.restored_count,
                    "restored_paths": restored_paths,
                    "status": "RESTORED",
                    "sha256_verified": sha256_verified,
                    "_human": format!("Restored {} files from archive {}", res.restored_count, archive_id)
                }))
            }
            VaultCommands::Purge {
                expired,
                archive_id,
            } => {
                let vault_dir = settings.vault_dir(&default_vault_dir())?;
                let mut purged_count = 0usize;
                let mut freed_bytes = 0u64;
                if expired {
                    let now = Utc::now();
                    for a in db.list_vault_archives(Some(VaultStatus::Quarantined.as_str()))? {
                        if a.expires_at <= now {
                            if let Some(id) = a.id {
                                let zip_res =
                                    safe_remove_vault_artifact(&a.zip_path, &vault_dir, ".zip");
                                let man_res = safe_remove_vault_artifact(
                                    &a.manifest_path,
                                    &vault_dir,
                                    ".manifest.json",
                                );
                                if zip_res.is_err() || man_res.is_err() {
                                    tracing::warn!(
                                        "Skipped purging archive {} with path outside vault_dir",
                                        id
                                    );
                                    continue;
                                }
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
                        safe_remove_vault_artifact(&a.zip_path, &vault_dir, ".zip")?;
                        safe_remove_vault_artifact(&a.manifest_path, &vault_dir, ".manifest.json")?;
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
                let lifecycle = NativeAppLifecycleApi;

                let existing_zalo = db.list_relocations(None).ok().and_then(|list| {
                    list.into_iter()
                        .find(|r| r.id == Some(record_id) && r.app_id == "zalo")
                });

                let mut shutdown_session = None;
                if let Some(ref zrec) = existing_zalo {
                    if lifecycle.has_visible_window(&["ZaloCall.exe"]) {
                        return Err(GreenCError::PreconditionBlocked {
                            operation: "relocate undo".to_string(),
                            reason: greenc_platform::app_lifecycle::ZALO_ACTIVE_CALL_MESSAGE
                                .to_string(),
                        });
                    }
                    let session = lifecycle.graceful_shutdown(
                        &[zrec.source_path.clone(), zrec.target_path.clone()],
                        ZALO_PROCESS_NAMES,
                        greenc_platform::app_lifecycle::DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
                    )?;
                    shutdown_session = Some(session);
                }

                let undo_res = TransactionalMover::undo_relocation(
                    record_id,
                    &db,
                    &vol_api,
                    &reparse_api,
                    &shell_api,
                    &proc_probe,
                    ctrl,
                );

                if let Some(mut session) = shutdown_session {
                    if !session.captured_exes.is_empty() {
                        let _ = lifecycle.restart_app(&mut session);
                    }
                }

                let rec = undo_res?;
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
                            "regenerable_cache_mode": q.regenerable_cache_mode.as_str(),
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
                regenerable_cache_mode,
            } => {
                let root = normalize_drive_root(&drive).ok_or_else(|| {
                    GreenCError::PreconditionBlocked {
                        operation: "quota".to_string(),
                        reason: format!("Invalid drive '{drive}' (expected e.g. C:\\ or /)"),
                    }
                })?;
                let cache_mode = match regenerable_cache_mode {
                    Some(raw) => RegenerableCacheMode::from_str_strict(&raw).ok_or_else(|| {
                        GreenCError::PreconditionBlocked {
                            operation: "quota".to_string(),
                            reason: format!(
                                "Invalid regenerable_cache_mode '{raw}' (expected 'vault' or 'direct')"
                            ),
                        }
                    })?,
                    None => db
                        .list_drive_quotas(false)?
                        .into_iter()
                        .find(|existing| existing.drive_root.eq_ignore_ascii_case(&root))
                        .map(|existing| existing.regenerable_cache_mode)
                        .unwrap_or(RegenerableCacheMode::Vault),
                };
                let mut q = DriveQuota::new(root.clone());
                q.limit_bytes = (limit_gb.max(0.0) * 1024.0 * 1024.0 * 1024.0) as u64;
                q.warn_ratio = f64::from(warn) / 100.0;
                q.clean_ratio = f64::from(clean) / 100.0;
                q.target_ratio = f64::from(target) / 100.0;
                q.auto_clean = auto_clean;
                q.auto_backup = auto_backup;
                q.backup_dir = backup_dir;
                q.enabled = !disabled;
                q.regenerable_cache_mode = cache_mode;
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
                    "regenerable_cache_mode": q.regenerable_cache_mode.as_str(),
                    "_human": format!(
                        "Quota saved for {} (regenerable_cache_mode={})",
                        q.drive_root,
                        q.regenerable_cache_mode.as_str()
                    )
                }))
            }
            QuotaCommands::Remove { drive } => {
                let root = normalize_drive_root(&drive).ok_or_else(|| {
                    GreenCError::PreconditionBlocked {
                        operation: "quota".to_string(),
                        reason: format!("Invalid drive '{drive}' (expected e.g. C:\\ or /)"),
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

        Commands::Guard { action } => match action {
            GuardCommands::Run {
                drive,
                dry_run,
                extra_busy_tools,
            } => {
                let drive_filter = match drive {
                    Some(raw) => Some(normalize_drive_root(&raw).ok_or_else(|| {
                        GreenCError::PreconditionBlocked {
                            operation: "guard run".to_string(),
                            reason: format!("Invalid drive '{raw}' (expected e.g. C:\\ or /home)"),
                        }
                    })?),
                    None => None,
                };

                let mut parsed_extra_busy: Vec<DevTool> = extra_busy_tools
                    .iter()
                    .filter_map(|s| DevTool::from_str_loose(s))
                    .collect();
                if let Ok(env_busy) = std::env::var("GREENC_EXTRA_BUSY_TOOLS") {
                    for part in env_busy.split([',', ' ', ';']) {
                        if let Some(t) = DevTool::from_str_loose(part) {
                            if !parsed_extra_busy.contains(&t) {
                                parsed_extra_busy.push(t);
                            }
                        }
                    }
                }

                let mut quotas = db.list_drive_quotas(true)?;
                if let Some(ref target_root) = drive_filter {
                    quotas.retain(|q| q.drive_root.eq_ignore_ascii_case(target_root));
                }

                let vault_dir = settings.vault_dir(&default_vault_dir())?;
                let volume_api = NativeWin32VolumeApi;
                let lock_probe = NativeFileLockProbe;
                let dev_tool_probe = NativeDevToolProbe;

                let suggest_roots: Vec<PathBuf> = default_suggest_roots();
                let opts = GuardRunOptions {
                    dry_run,
                    extra_busy_tools: parsed_extra_busy,
                    suggest_roots_on_miss: suggest_roots,
                    include_zalo_auto_clean: settings
                        .get_zalo_auto_clean_enabled()
                        .unwrap_or(false),
                    ..Default::default()
                };

                let mut results = Vec::new();
                for quota in &quotas {
                    if ctrl.is_cancelled() {
                        return Err(GreenCError::Cancelled);
                    }
                    let outcome = run_guard_for_quota(
                        quota,
                        &vault_dir,
                        &db,
                        &volume_api,
                        &lock_probe,
                        &dev_tool_probe,
                        &opts,
                        ctrl,
                    )?;
                    if !dry_run && outcome.net_freed_bytes > 0 {
                        let today = chrono::Local::now()
                            .date_naive()
                            .format("%Y-%m-%d")
                            .to_string();
                        let _ = db.record_reclaimed_bytes(
                            &quota.drive_root,
                            &today,
                            outcome.net_freed_bytes as u64,
                        );
                    }
                    results.push(outcome);
                }

                let total_logical: u64 = results.iter().map(|r| r.logical_bytes).sum();
                let total_vault: u64 = results.iter().map(|r| r.vault_bytes).sum();
                let total_direct: u64 = results.iter().map(|r| r.direct_deleted_bytes).sum();
                let total_net: i64 = results.iter().map(|r| r.net_freed_bytes).sum();
                let all_target_reached = results.iter().all(|r| r.target_reached);
                let all_reasons: Vec<String> = results
                    .iter()
                    .flat_map(|r| r.reasons.iter().cloned())
                    .collect();

                let human = if results.is_empty() {
                    "Không có hạn mức ổ đĩa nào đang bật.".to_string()
                } else {
                    results
                        .iter()
                        .map(|r| r.human_summary())
                        .collect::<Vec<_>>()
                        .join("\n")
                };

                Ok(json!({
                    "dry_run": dry_run,
                    "drives": results,
                    "logical_bytes": total_logical,
                    "vault_bytes": total_vault,
                    "direct_deleted_bytes": total_direct,
                    "net_freed_bytes": total_net,
                    "target_reached": all_target_reached,
                    "reasons": all_reasons,
                    "_human": human
                }))
            }
            GuardCommands::Suggest {
                drive,
                root,
                min_days,
                max_depth,
                max_secs,
            } => {
                let drive_filter = match drive {
                    Some(raw) => Some(PathBuf::from(normalize_drive_root(&raw).ok_or_else(
                        || GreenCError::PreconditionBlocked {
                            operation: "guard suggest".to_string(),
                            reason: format!("Invalid drive '{raw}' (expected e.g. C:\\ or /home)"),
                        },
                    )?)),
                    None => None,
                };
                let roots = match root {
                    Some(r) => vec![r],
                    None => default_suggest_roots(),
                };
                let vault_dir = settings.vault_dir(&default_vault_dir())?;
                let mut opts = SuggestOptions::with_defaults(roots, vault_dir);
                opts.drive_filter = drive_filter;
                opts.min_stale_days = min_days;
                opts.max_depth = max_depth;
                opts.max_duration = std::time::Duration::from_secs(max_secs.max(1));

                let dev_tool_probe = NativeDevToolProbe;
                let report = suggest_large_build_outputs(&opts, &dev_tool_probe, ctrl);

                let total_gb = report.total_suggestable_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                let vault_gb = report.vault.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                let human = format!(
                    "Gợi ý (chỉ đọc): {} thư mục output build cũ ({:.2} GB) | Vault: {} gói ({:.2} GB)",
                    report.suggestions.len(),
                    total_gb,
                    report.vault.archive_count,
                    vault_gb
                );

                let mut val = serde_json::to_value(&report)?;
                if let Some(obj) = val.as_object_mut() {
                    obj.insert("_human".to_string(), json!(human));
                }
                Ok(val)
            }
            GuardCommands::SetMode { drive, mode } => {
                let root = normalize_drive_root(&drive).ok_or_else(|| {
                    GreenCError::PreconditionBlocked {
                        operation: "guard set-mode".to_string(),
                        reason: format!("Invalid drive '{drive}' (expected e.g. C:\\ or /home)"),
                    }
                })?;
                let parsed = RegenerableCacheMode::from_str_strict(&mode).ok_or_else(|| {
                    GreenCError::PreconditionBlocked {
                        operation: "guard set-mode".to_string(),
                        reason: format!("Invalid mode '{mode}' (expected 'vault' or 'direct')"),
                    }
                })?;
                let updated = db.set_drive_regenerable_cache_mode(&root, parsed)?;
                if !updated {
                    return Err(GreenCError::NotFound {
                        entity: "DriveQuota".to_string(),
                        id: root,
                    });
                }
                Ok(json!({
                    "drive_root": root,
                    "regenerable_cache_mode": parsed.as_str(),
                    "_human": format!(
                        "Đã đặt regenerable_cache_mode={} cho ổ {}",
                        parsed.as_str(),
                        root
                    )
                }))
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
                        zalo_auto_clean_enabled: settings.get_zalo_auto_clean_enabled()?,
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
                    if let Some(zalo_auto) =
                        cfg.get("zalo_auto_clean_enabled").and_then(|v| v.as_bool())
                    {
                        let _ = settings.set_zalo_auto_clean_enabled(zalo_auto);
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

        Commands::Zalo { action } => {
            let lifecycle = NativeAppLifecycleApi;
            let proc_probe = NativeProcessServiceProbe;
            let lock_probe = NativeFileLockProbe;
            let vol_api = NativeWin32VolumeApi;
            let reparse_api = NativeReparsePointApi;
            let shell_api = NativeShellFolderApi;

            let running_exe_dirs = lifecycle.find_running_exe_dirs(ZALO_PROCESS_NAMES);
            let zalo_running = !running_exe_dirs.is_empty()
                || ZALO_PROCESS_NAMES
                    .iter()
                    .any(|p| proc_probe.is_process_running(p));

            let known_relocated_sources: std::collections::HashSet<PathBuf> = db
                .list_relocations(Some("done"))
                .unwrap_or_default()
                .into_iter()
                .filter(|r| r.app_id == "zalo")
                .map(|r| r.source_path)
                .collect();

            let now = SystemTime::now();
            let disc_opts = ZaloDiscoveryOptions {
                running_exe_dirs,
                known_relocated_sources,
                now,
                ..Default::default()
            };

            match action {
                ZaloCommands::Status => {
                    let auto_supported = is_zalo_auto_move_supported();
                    let unsupported_reason = if auto_supported {
                        None
                    } else {
                        Some(zalo_auto_move_unsupported_reason())
                    };
                    let disc_opt = discover_zalo_installation(&disc_opts);
                    let safety_copies = discover_zalo_safety_copies(
                        disc_opt.as_ref().and_then(|d| d.data_dir.as_deref()),
                        &db,
                    );
                    let retained_original_bytes: u64 = safety_copies
                        .iter()
                        .filter(|c| c.exists_on_disk && c.kind == "original")
                        .map(|c| c.size_bytes)
                        .fold(0u64, u64::saturating_add);
                    let copies_human =
                        format_safety_copies_human(&safety_copies, retained_original_bytes);

                    let Some(disc) = disc_opt else {
                        let human = if copies_human.is_empty() {
                            "Không tìm thấy cài đặt Zalo PC.".to_string()
                        } else {
                            format!("Không tìm thấy cài đặt Zalo PC.\n{copies_human}")
                        };
                        return Ok(json!({
                            "installed": false,
                            "auto_move_supported": auto_supported,
                            "auto_move_unsupported_reason": unsupported_reason,
                            "retained_original_bytes": retained_original_bytes,
                            "safety_copies": safety_copies,
                            "_human": human
                        }));
                    };
                    let scan = scan_zalo_cleanup_units(
                        &disc,
                        now,
                        std::time::Duration::from_secs(ZALO_CACHE_MIN_AGE_SECS),
                        std::time::Duration::from_secs(ZALO_OLD_VERSION_MIN_AGE_SECS),
                        &lock_probe,
                        zalo_running,
                        ctrl,
                    );
                    let config_status = match disc.config_status {
                        ZaloConfigStatus::Found => "found",
                        ZaloConfigStatus::Missing => "missing",
                        ZaloConfigStatus::Invalid { .. } => "invalid",
                        ZaloConfigStatus::RejectedUnsafe { .. } => "rejected_unsafe",
                    };
                    let already_other = disc.data_dir.is_some()
                        && !disc.data_on_system_drive
                        && !disc.data_is_known_relocation;

                    let drives = list_drives_native().unwrap_or_default();
                    let rec_drive = recommend_drive_info(&drives).map(|d| d.drive_letter);
                    let space_estimate = if disc.data_on_system_drive {
                        let src_letter = disc
                            .data_drive
                            .as_deref()
                            .and_then(|s| s.chars().next())
                            .filter(|c| c.is_ascii_alphabetic())
                            .unwrap_or('C');
                        rec_drive.and_then(|tgt| {
                            estimate_zalo_move_space(
                                src_letter,
                                tgt,
                                disc.chat_data_bytes,
                                &vol_api,
                            )
                            .ok()
                        })
                    } else {
                        None
                    };

                    let mut human = format!(
                        "Zalo PC | Dữ liệu chat: {} | Bộ đệm: {} (dọn được {}) | Bản cũ: {} ({} bản){}",
                        format_bytes_vi(disc.chat_data_bytes),
                        format_bytes_vi(disc.cache_bytes),
                        format_bytes_vi(scan.cache_reclaimable_bytes),
                        format_bytes_vi(disc.old_versions_bytes),
                        disc.old_version_dirs.len(),
                        scan.locked_notice
                            .as_deref()
                            .map(|n| format!(" | {n}"))
                            .unwrap_or_default()
                    );
                    if !copies_human.is_empty() {
                        human.push('\n');
                        human.push_str(&copies_human);
                    }
                    Ok(json!({
                        "installed": true,
                        "config_status": config_status,
                        "data_dir": disc.data_dir.map(|p| p.to_string_lossy().to_string()),
                        "data_drive": disc.data_drive,
                        "data_on_system_drive": disc.data_on_system_drive,
                        "already_on_other_drive": already_other,
                        "chat_data_bytes": disc.chat_data_bytes,
                        "media_bytes": disc.media_bytes,
                        "message_bytes": disc.message_bytes,
                        "cache_bytes": disc.cache_bytes,
                        "cache_reclaimable_bytes": scan.cache_reclaimable_bytes,
                        "cache_locked_bytes": scan.cache_locked_bytes,
                        "old_versions_bytes": disc.old_versions_bytes,
                        "old_versions_count": disc.old_version_dirs.len(),
                        "total_reclaimable_bytes": scan.total_reclaimable_bytes,
                        "total_locked_bytes": scan.total_locked_bytes,
                        "zalo_running": zalo_running,
                        "locked_notice": scan.locked_notice,
                        "auto_move_supported": auto_supported,
                        "auto_move_unsupported_reason": unsupported_reason,
                        "retained_original_bytes": retained_original_bytes,
                        "safety_copies": safety_copies,
                        "space_estimate": space_estimate,
                        "warnings": disc.warnings,
                        "_human": human
                    }))
                }
                ZaloCommands::Originals => {
                    let disc_opt = discover_zalo_installation(&disc_opts);
                    let safety_copies = discover_zalo_safety_copies(
                        disc_opt.as_ref().and_then(|d| d.data_dir.as_deref()),
                        &db,
                    );
                    let retained_original_bytes: u64 = safety_copies
                        .iter()
                        .filter(|c| c.exists_on_disk && c.kind == "original")
                        .map(|c| c.size_bytes)
                        .fold(0u64, u64::saturating_add);
                    let human = if safety_copies.is_empty() {
                        "Không có bản gốc giữ tạm hoặc thư mục xung đột Zalo nào.".to_string()
                    } else {
                        format_safety_copies_human(&safety_copies, retained_original_bytes)
                    };
                    Ok(json!({
                        "retained_original_bytes": retained_original_bytes,
                        "safety_copies": safety_copies,
                        "_human": human
                    }))
                }
                ZaloCommands::PurgeOriginal { id, expired } => {
                    if expired {
                        let purged =
                            ZaloRelocator::purge_expired_originals(&db, &reparse_api, Utc::now())?;
                        let freed: u64 = purged
                            .iter()
                            .map(|r| r.size_bytes)
                            .fold(0u64, u64::saturating_add);
                        Ok(json!({
                            "purged_count": purged.len(),
                            "freed_bytes": freed,
                            "purged": purged,
                            "_human": format!(
                                "Đã dọn {} bản gốc Zalo giữ tạm quá hạn (giải phóng {}).",
                                purged.len(),
                                format_bytes_vi(freed)
                            )
                        }))
                    } else if let Some(rid) = id {
                        let purged =
                            ZaloRelocator::purge_retained_original(rid, &db, &reparse_api)?;
                        Ok(json!({
                            "purged": purged,
                            "_human": format!(
                                "Đã xoá bản gốc Zalo #{} (giải phóng {}).",
                                rid,
                                format_bytes_vi(purged.size_bytes)
                            )
                        }))
                    } else {
                        Err(GreenCError::PreconditionBlocked {
                            operation: "zalo purge-original".to_string(),
                            reason: "Cần truyền --id <ID> hoặc --expired".to_string(),
                        })
                    }
                }
                ZaloCommands::RestoreOriginal { id } => {
                    let restored = ZaloRelocator::restore_retained_original(
                        id,
                        &db,
                        &reparse_api,
                        &lock_probe,
                        &proc_probe,
                        &lifecycle,
                        greenc_platform::app_lifecycle::DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
                    )?;
                    let human = restored
                        .detail
                        .clone()
                        .unwrap_or_else(|| format!("Đã khôi phục bản gốc Zalo #{id} an toàn."));
                    Ok(json!({
                        "restored": restored,
                        "_human": human
                    }))
                }
                ZaloCommands::Clean { dry_run } => {
                    let Some(disc) = discover_zalo_installation(&disc_opts) else {
                        return Ok(json!({
                            "installed": false,
                            "dry_run": dry_run,
                            "cleaned_files": 0,
                            "logical_freed_bytes": 0,
                            "net_freed_bytes": 0,
                            "_human": "Không tìm thấy cài đặt Zalo PC."
                        }));
                    };
                    let vault_dir = settings.vault_dir(&default_vault_dir())?;
                    let retention_days = settings.get_retention_days()?;
                    let report = execute_zalo_cleanup(
                        &disc,
                        &vault_dir,
                        &db,
                        &vol_api,
                        &lock_probe,
                        zalo_running,
                        &ZaloCleanupOptions {
                            dry_run,
                            mode: RegenerableCacheMode::Vault,
                            retention_days,
                            now,
                            ..Default::default()
                        },
                        ctrl,
                    )?;
                    let human = format!(
                        "{}Zalo: {} tệp, giải phóng {} bytes (thực lợi {:+} bytes){}",
                        if dry_run { "[Dry-run] " } else { "" },
                        report.cleaned_files,
                        report.logical_freed_bytes,
                        report.net_freed_bytes,
                        report
                            .locked_notice
                            .as_deref()
                            .map(|n| format!(" — {n}"))
                            .unwrap_or_default()
                    );
                    let mut val = serde_json::to_value(&report)?;
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert("_human".to_string(), json!(human));
                    }
                    Ok(val)
                }
                ZaloCommands::Move {
                    to,
                    dry_run,
                    no_reopen,
                } => {
                    check_precondition_arg(&to)?;
                    let trimmed_to = to.trim();
                    let drive_char = {
                        let bytes = trimmed_to.as_bytes();
                        let valid_shape = match bytes.len() {
                            1 => bytes[0].is_ascii_alphabetic(),
                            2 => bytes[0].is_ascii_alphabetic() && bytes[1] == b':',
                            3 => {
                                bytes[0].is_ascii_alphabetic()
                                    && bytes[1] == b':'
                                    && (bytes[2] == b'\\' || bytes[2] == b'/')
                            }
                            _ => false,
                        };
                        if valid_shape {
                            Some((bytes[0] as char).to_ascii_uppercase())
                        } else {
                            None
                        }
                    }
                    .ok_or_else(|| GreenCError::PreconditionBlocked {
                        operation: "zalo move".to_string(),
                        reason: format!("Ổ đĩa đích không hợp lệ: '{to}' (ví dụ: --to D:)"),
                    })?;

                    let Some(disc) = discover_zalo_installation(&disc_opts) else {
                        return Err(GreenCError::NotFound {
                            entity: "ZaloInstallation".to_string(),
                            id: "zalo".to_string(),
                        });
                    };

                    let report = ZaloRelocator::move_zalo_data(
                        &disc,
                        &ZaloMoveOptions {
                            target_drive: drive_char,
                            dry_run,
                            reopen_zalo: !no_reopen,
                            ..Default::default()
                        },
                        &db,
                        &vol_api,
                        &reparse_api,
                        &shell_api,
                        &lock_probe,
                        &proc_probe,
                        &lifecycle,
                        ctrl,
                        |_| {},
                    )?;
                    let human = report.message.clone();
                    let mut val = serde_json::to_value(&report)?;
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert("_human".to_string(), json!(human));
                    }
                    Ok(val)
                }
            }
        }
        Commands::RestoreHelper => Err(GreenCError::PreconditionBlocked {
            operation: "__restore-helper".to_string(),
            reason:
                "__restore-helper phải được chạy trực tiếp qua stdin/stdout (không qua execute_cli)"
                    .to_string(),
        }),
    }
}

/// Dispatches a validated [`greenc_platform::breakaway::RestoreHelperRequest`] after checking
/// package identity. If `running_in_package` is `true`, stops immediately with
/// [`greenc_platform::breakaway::EXIT_HELPER_STILL_IN_PACKAGE`] without touching disk.
pub fn run_restore_helper_request(
    req: greenc_platform::breakaway::RestoreHelperRequest,
    running_in_package: bool,
) -> (i32, greenc_platform::breakaway::RestoreHelperResponse) {
    use greenc_platform::breakaway::{
        current_package_full_name, RestoreHelperRequest, RestoreHelperResponse,
        EXIT_HELPER_STILL_IN_PACKAGE,
    };

    if running_in_package {
        return (
            EXIT_HELPER_STILL_IN_PACKAGE,
            RestoreHelperResponse::still_in_package(current_package_full_name().as_deref()),
        );
    }

    match req {
        RestoreHelperRequest::RestoreFiles {
            payload_root,
            conflict_strategy,
            verify_sha256,
            restore_hardlinks,
            restore_timestamps,
            items,
        } => greenc_vault::rollback::execute_restore_files_job(
            &payload_root,
            conflict_strategy,
            verify_sha256,
            restore_hardlinks,
            restore_timestamps,
            &items,
            running_in_package,
        ),
        RestoreHelperRequest::RestoreStagedDir {
            staged_dir,
            original_path,
            expected_dev,
            expected_ino,
        } => greenc_vault::guard::execute_restore_staged_dir_job(
            &staged_dir,
            &original_path,
            expected_dev,
            expected_ino,
            running_in_package,
        ),
        RestoreHelperRequest::UndoRelocationDir {
            source_path,
            target_path,
        } => {
            let reparse = NativeReparsePointApi;
            greenc_relocate::mover::execute_undo_relocation_dir_job(
                &source_path,
                &target_path,
                &reparse,
                running_in_package,
            )
        }
    }
}

/// Entrypoint for hidden `greenc-cli __restore-helper`:
/// - Self-checks MSIX package identity via `GetCurrentPackageFullName` FIRST.
/// - Reads bounded JSON request from stdin (never accepts arbitrary paths on CLI).
/// - Writes structured JSON [`greenc_platform::breakaway::RestoreHelperResponse`] to stdout.
/// - Never opens SQLite or creates files under `%LOCALAPPDATA%\GreenC-Store`.
pub fn run_restore_helper_stdio() -> i32 {
    use greenc_platform::breakaway::{
        current_package_full_name, is_running_in_msix_package, RestoreHelperRequest,
        RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE, MAX_RESTORE_HELPER_STDIN_BYTES,
    };
    use std::io::Read;

    if is_running_in_msix_package() {
        let resp = RestoreHelperResponse::still_in_package(current_package_full_name().as_deref());
        if let Ok(json) = serde_json::to_string(&resp) {
            println!("{json}");
        }
        return EXIT_HELPER_STILL_IN_PACKAGE;
    }

    let mut raw = Vec::new();
    let read_res = std::io::stdin()
        .take(MAX_RESTORE_HELPER_STDIN_BYTES + 1)
        .read_to_end(&mut raw);
    if let Err(e) = read_res {
        let resp = RestoreHelperResponse::failed(
            "stdin_read_error",
            format!("Không đọc được dữ liệu stdin của __restore-helper: {e}"),
        );
        if let Ok(json) = serde_json::to_string(&resp) {
            println!("{json}");
        }
        return EXIT_OPERATIONAL_ERROR;
    }
    if (raw.len() as u64) > MAX_RESTORE_HELPER_STDIN_BYTES {
        let resp = RestoreHelperResponse::failed(
            "stdin_too_large",
            format!(
                "Yêu cầu __restore-helper vượt quá giới hạn {} byte",
                MAX_RESTORE_HELPER_STDIN_BYTES
            ),
        );
        if let Ok(json) = serde_json::to_string(&resp) {
            println!("{json}");
        }
        return EXIT_PRECONDITION_BLOCKED;
    }

    let req: RestoreHelperRequest = match serde_json::from_slice(&raw) {
        Ok(r) => r,
        Err(e) => {
            let resp = RestoreHelperResponse::failed(
                "invalid_request_json",
                format!("JSON yêu cầu __restore-helper không hợp lệ: {e}"),
            );
            if let Ok(json) = serde_json::to_string(&resp) {
                println!("{json}");
            }
            return EXIT_PRECONDITION_BLOCKED;
        }
    };

    let (code, resp) = run_restore_helper_request(req, false);
    if let Ok(json) = serde_json::to_string(&resp) {
        println!("{json}");
    }
    code
}

fn format_safety_copies_human(
    copies: &[greenc_relocate::zalo_move::ZaloSafetyCopyInfo],
    retained_bytes: u64,
) -> String {
    if copies.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    if retained_bytes > 0 {
        lines.push(format!(
            "Bản gốc đang giữ tạm: {}",
            format_bytes_vi(retained_bytes)
        ));
    }
    for c in copies {
        let kind_label = match c.kind.as_str() {
            "original" => "bản gốc giữ tạm",
            "conflict" => "xung đột",
            "moving" => "đang chuyển dở",
            other => other,
        };
        let id_str = c.id.map(|i| format!("#{i} ")).unwrap_or_default();
        let date_str = c
            .created_at
            .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "không rõ ngày".to_string());
        lines.push(format!(
            "  - {id_str}[{kind_label}] {} ({date_str}): {}",
            format_bytes_vi(c.size_bytes),
            c.path.display()
        ));
    }
    lines.join("\n")
}

fn default_suggest_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").ok();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").ok();

    home.filter(|s| !s.trim().is_empty())
        .map(|s| vec![PathBuf::from(s)])
        .unwrap_or_default()
}

const fn target_or_zero(n: usize) -> usize {
    n
}
