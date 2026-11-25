//! Shared per-drive quota guard core used by both `greenc-cli` (`guard run`) and
//! `greenc-app` (`BackgroundGuardian`) (`GC-GUARD` / `GC-GUARD2`).
//!
//! Responsibilities:
//! - Drive-isolated cleanup: only touches units on the protected drive, stops once `target_ratio`
//!   is reached, prioritizing regenerable developer caches and oldest-unused units first.
//! - Atomic directory-unit staging in BOTH `vault` and `direct` modes: TOCTOU verify -> fresh
//!   busy-tool check right before rename -> atomic same-drive `rename` into a private (`0700`)
//!   GreenC staging directory OUTSIDE the tool's cache tree -> archive/delete from staging ->
//!   crash-safe journal recovery on restart. Never leaves partial or empty transform directories
//!   inside a tool's cache tree.
//! - Ordered multi-file deletion (e.g. npm `index-v5` BEFORE `content-v2`): never deletes content
//!   blobs if index removal fails.
//! - Accurate Vault reconciliation: when 0 originals are removed (e.g. tool started or status
//!   became `Unknown` mid-archive), the orphan `.zip`, `.manifest.json`, and DB row are removed;
//!   when only a subset of originals are removed, the Vault archive & DB retain only those units.
//! - Cooldown policy:
//!   - Net freed >= 5% of `bytes_needed` (meaningful progress): fixed 30m cooldown, no escalation.
//!   - Deferred due to busy / unknown tools: fixed 30m cooldown, no escalation.
//!   - Ineffective (< 5% freed and tools idle): persistent DB backoff `30m -> 2h -> 12h`.
//! - Performance: O(1) `HashMap` manifest lookups (no O(n^2) scans) and <= 1s cached busy-tool
//!   probing for bulk units, paired with a fresh live probe immediately before atomic renames.

use crate::archiver::{ArchiveOptions, ArchiverPipeline};
use crate::cache_units::{
    compute_unit_latest_used, effective_last_used, is_forbidden_dev_cache_path, CleanUnit,
    CleanUnitKind,
};
use crate::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use crate::profiles::PresetProfiles;
use crate::rollback::{safe_remove_vault_artifact, validate_restore_file_path};
use crate::sniff::FormatSniffer;
use crate::suggest::{suggest_large_build_outputs_with_busy, SuggestOptions};
use chrono::{DateTime, Utc};
use greenc_core::config::{is_windows_appdata_app_dir, MIN_VAULT_RESERVE_BYTES};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    is_same_volume, CleanCandidate, DirectDeleteRecord, DriveQuota, FileManifestItem,
    GuardCooldownState, QuotaLevel, RegenerableCacheMode,
};
use greenc_core::task::TaskControl;
use greenc_platform::breakaway::{
    current_package_full_name, format_breakaway_unavailable_guidance, BreakawayHelperClient,
    NativeBreakawayHelperClient, PrivateBreakawayJobDir, RestoreHelperRequest,
    RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE,
};
use greenc_platform::detect_atime_reliability;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool, DevToolProbe};
#[cfg(not(unix))]
use greenc_platform::handle::is_owned_by_current_user;
#[cfg(unix)]
use greenc_platform::handle::{current_user_uid, is_owned_by_uid};
use greenc_platform::handle::{
    is_trusted_user_dir, query_file_identity, validate_private_staging_dir,
};
use greenc_platform::traits::{FileLockProbe, Win32VolumeApi};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Safe preset profiles scanned by automatic drive-quota cleanup (excludes opt-in `orphan_radar`).
pub const GUARD_AUTO_CLEAN_PROFILE_IDS: [&str; 7] = [
    "package_managers",
    "windows_temp",
    "browser_cache",
    "crash_dumps",
    "app_logs",
    "old_versions",
    "chat_media",
];

/// Default Vault archive retention for drive-quota cleanup (14 days).
pub const DEFAULT_GUARD_RETENTION_DAYS: u32 = 14;

/// Minimum net space gain ratio relative to `bytes_needed` below which a run is deemed ineffective.
pub const MIN_EFFECTIVE_NET_RATIO: f64 = 0.05;

/// Fixed retry cooldown (30 minutes) used for partial progress (>= 5%) and busy-tool deferrals.
pub const FIXED_RETRY_COOLDOWN_MINUTES: i64 = 30;

/// Maximum cache TTL for repeated busy-tool probes across bulk file units (<= 1 second).
pub const BUSY_TOOL_CACHE_TTL: Duration = Duration::from_secs(1);

/// Maximum allowed size of a staging journal JSON file (64 KiB).
pub const MAX_STAGING_JOURNAL_BYTES: u64 = 64 * 1024;

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// Validates that a staging unit identifier strictly matches `^unit_[0-9]+_[0-9]+_[0-9]+$`.
pub fn is_valid_staging_unit_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("unit_") else {
        return false;
    };
    let mut parts = rest.split('_');
    let (Some(a), Some(b), Some(c), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    !a.is_empty()
        && !b.is_empty()
        && !c.is_empty()
        && a.bytes().all(|ch| ch.is_ascii_digit())
        && b.bytes().all(|ch| ch.is_ascii_digit())
        && c.bytes().all(|ch| ch.is_ascii_digit())
}

/// Backoff ladder for genuinely ineffective runs (< 5% freed, tools idle): 30 min -> 2 hours -> 12 hours.
pub fn cooldown_duration_for_failures(consecutive_ineffective: u32) -> chrono::Duration {
    match consecutive_ineffective {
        0 => chrono::Duration::zero(),
        1 => chrono::Duration::minutes(30),
        2 => chrono::Duration::hours(2),
        _ => chrono::Duration::hours(12),
    }
}

/// Mode under which a directory unit was atomically moved into GreenC's staging area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingMode {
    Vault,
    Direct,
}

/// Lifecycle state recorded in a crash-recovery staging journal file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingState {
    /// Written immediately BEFORE atomic `rename(original_path -> staged_path)`.
    Staging,
    /// `rename` completed; waiting for Vault archive commit before deleting staged tree.
    Staged,
    /// Vault archive has been finalized and persisted; staged tree can be safely deleted.
    Archived,
    /// Deletion of the staged tree is committed / in progress.
    Deleting,
    /// Rollback could not rename back because `original_path` already exists; kept safely in staging.
    RestoreConflict,
}

/// Persistent JSON journal entry written in GreenC's private (`0700`) staging directory for each
/// atomically staged directory unit so power loss / crash recovery is deterministic and safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingJournalEntry {
    pub version: u32,
    pub id: String,
    pub original_path: PathBuf,
    pub staged_path: PathBuf,
    pub mode: StagingMode,
    pub state: StagingState,
    pub created_at: DateTime<Utc>,
    pub drive_root: String,
    pub unit_label: String,
    pub dev_tool: Option<DevTool>,
    #[serde(default)]
    pub note: Option<String>,
    /// Device ID (`st_dev` / `dwVolumeSerialNumber`) of the unit captured immediately before rename.
    #[serde(default)]
    pub staged_dev: Option<u64>,
    /// Inode / file index (`st_ino` / NTFS 64-bit file index) captured immediately before rename.
    #[serde(default)]
    pub staged_ino: Option<u64>,
    /// Classification of the staged unit (only `RegenerableDevCache` enters staging and may have
    /// redundant `RestoreConflict` copies auto-deleted when recreated at `original_path`).
    #[serde(default)]
    pub unit_kind: Option<CleanUnitKind>,
}

/// Summary of startup/pre-run crash recovery on a GreenC staging directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StagingRecoveryReport {
    /// Unfinished `Vault`-mode staged units restored cleanly back to their original paths.
    pub restored_to_original: usize,
    /// Committed `Direct`/`Archived`/`Deleting` staged units whose deletion was finished
    /// (also includes redundant regenerable `RestoreConflict` deletions).
    pub completed_deletions: usize,
    /// Stale journal files whose staged directory did not exist (rename never happened or already deleted).
    pub stale_journals_removed: usize,
    /// Staged units deferred because the owning dev tool is currently running.
    pub deferred_busy: usize,
    /// Redundant `RestoreConflict` staged copies of regenerable dev caches deleted because the
    /// tool already recreated `original_path`.
    #[serde(default)]
    pub redundant_conflicts_cleaned: usize,
    /// Informational records of cleaned redundant regenerable cache conflicts.
    #[serde(default)]
    pub cleaned_conflicts: Vec<String>,
    /// Warnings for staged units kept in staging or rejected due to security/integrity checks.
    pub conflicts: Vec<String>,
    /// Detailed security/integrity rejection reasons (forged journals, wrong owner/perms,
    /// dev/ino mismatch, invalid `original_path`, etc.) where nothing was deleted or renamed.
    #[serde(default)]
    pub rejected: Vec<String>,
}

/// Optional overrides for [`recover_staging_dir_with_options`] (defaults enforce live current user
/// UID and `PresetProfiles::managed_dev_cache_roots()`).
#[derive(Clone, Default)]
pub struct StagingRecoveryOptions {
    /// Explicit list of managed developer cache roots (`None` -> `PresetProfiles::managed_dev_cache_roots()`).
    pub allowed_cache_roots: Option<Vec<PathBuf>>,
    /// Expected owner UID on Unix (`None` -> `greenc_platform::current_user_uid()`).
    #[cfg(unix)]
    pub expected_uid: Option<u32>,
    /// Optional injectable MSIX breakaway helper client (defaults to [`NativeBreakawayHelperClient`]).
    pub breakaway_client: Option<std::sync::Arc<dyn BreakawayHelperClient>>,
}

impl std::fmt::Debug for StagingRecoveryOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("StagingRecoveryOptions");
        ds.field("allowed_cache_roots", &self.allowed_cache_roots);
        #[cfg(unix)]
        ds.field("expected_uid", &self.expected_uid);
        ds.field("has_breakaway_client", &self.breakaway_client.is_some());
        ds.finish()
    }
}

/// Runtime options for a drive-quota guard pass.
#[derive(Debug, Clone)]
pub struct GuardRunOptions {
    pub dry_run: bool,
    pub extra_busy_tools: Vec<DevTool>,
    pub profile_ids: Vec<String>,
    pub retention_days: u32,
    pub now_utc: DateTime<Utc>,
    pub now_sys: SystemTime,
    /// Optional search roots for a quick bounded build-output hint when target is not reached.
    pub suggest_roots_on_miss: Vec<PathBuf>,
    /// When `true`, bypasses DB cooldown (mainly for explicit tests or manual forced inspection).
    pub ignore_cooldown: bool,
    /// Optional explicit same-drive staging directory override (defaults to a private `0700`
    /// folder in user-owned Vault or `<HOME>/.local/share/greenc/staging` on the same drive).
    pub staging_dir: Option<PathBuf>,
    /// Optional override for the `<HOME>` directory used when resolving `<HOME>/.local/share/greenc/staging`
    /// (intended for deterministic tests without mutating process-global `HOME`).
    pub staging_home_override: Option<PathBuf>,
    /// Opt-in toggle for scheduled automatic Zalo cache & old-version cleanup during guard runs
    /// (OFF by default per safety policy; never moves Zalo chat data).
    pub include_zalo_auto_clean: bool,
}

impl Default for GuardRunOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            extra_busy_tools: Vec::new(),
            profile_ids: GUARD_AUTO_CLEAN_PROFILE_IDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            retention_days: DEFAULT_GUARD_RETENTION_DAYS,
            now_utc: Utc::now(),
            now_sys: SystemTime::now(),
            suggest_roots_on_miss: Vec::new(),
            ignore_cooldown: false,
            staging_dir: None,
            staging_home_override: None,
            include_zalo_auto_clean: false,
        }
    }
}

/// Structured result of enforcing a single drive's quota.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardDriveOutcome {
    pub drive_root: String,
    pub status: String,
    pub level: String,
    pub regenerable_cache_mode: RegenerableCacheMode,
    pub vault_same_drive: bool,
    pub total_bytes: u64,
    pub free_before_bytes: u64,
    pub free_after_bytes: u64,
    pub usage_before_ratio: f64,
    pub usage_after_ratio: f64,
    pub bytes_needed: u64,
    pub logical_bytes: u64,
    pub vault_bytes: u64,
    pub direct_deleted_bytes: u64,
    pub net_freed_bytes: i64,
    pub estimated_net_bytes: i64,
    pub target_reached: bool,
    pub cleaned_units: usize,
    pub cleaned_files: u64,
    pub vault_zip: Option<PathBuf>,
    pub backed_up_to: Option<PathBuf>,
    pub backup_problem: Option<String>,
    pub busy_tools: Vec<String>,
    pub busy_tools_unknown: bool,
    pub stopped_mid_run_tool: Option<String>,
    pub cooldown_active: bool,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub consecutive_ineffective: u32,
    pub warning: Option<String>,
    pub reasons: Vec<String>,
    pub suggest_hint_bytes: u64,
    #[serde(default)]
    pub atime_unreliable: bool,
    #[serde(default)]
    pub staging_warnings: Vec<String>,
}

impl GuardDriveOutcome {
    fn base(quota: &DriveQuota, vault_same_drive: bool) -> Self {
        Self {
            drive_root: quota.drive_root.clone(),
            status: "ok".to_string(),
            level: "ok".to_string(),
            regenerable_cache_mode: quota.regenerable_cache_mode,
            vault_same_drive,
            total_bytes: 0,
            free_before_bytes: 0,
            free_after_bytes: 0,
            usage_before_ratio: 0.0,
            usage_after_ratio: 0.0,
            bytes_needed: 0,
            logical_bytes: 0,
            vault_bytes: 0,
            direct_deleted_bytes: 0,
            net_freed_bytes: 0,
            estimated_net_bytes: 0,
            target_reached: true,
            cleaned_units: 0,
            cleaned_files: 0,
            vault_zip: None,
            backed_up_to: None,
            backup_problem: None,
            busy_tools: Vec::new(),
            busy_tools_unknown: false,
            stopped_mid_run_tool: None,
            cooldown_active: false,
            cooldown_until: None,
            consecutive_ineffective: 0,
            warning: None,
            reasons: Vec::new(),
            suggest_hint_bytes: 0,
            atime_unreliable: false,
            staging_warnings: Vec::new(),
        }
    }

    /// Formats a concise one-line human summary (Vietnamese) suitable for CLI stdout and guardian logs.
    pub fn human_summary(&self) -> String {
        let pct_after = self.usage_after_ratio * 100.0;
        let net_mb = (self.net_freed_bytes as f64) / (1024.0 * 1024.0);
        let logical_mb = (self.logical_bytes as f64) / (1024.0 * 1024.0);
        let vault_mb = (self.vault_bytes as f64) / (1024.0 * 1024.0);

        let base = format!(
            "{}: {:.1}% [{}] (status={}, logical={:.1} MiB, vault={:.1} MiB, net={:+.1} MiB, target_reached={})",
            self.drive_root,
            pct_after,
            self.level,
            self.status,
            logical_mb,
            vault_mb,
            net_mb,
            self.target_reached
        );
        if self.reasons.is_empty() {
            base
        } else {
            format!("{base} — {}", self.reasons.join("; "))
        }
    }
}

/// Copies a Vault archive (`.zip` + `.manifest.json`) into `backup_dir`, refusing any folder on
/// the same volume as `protected_root`.
pub fn backup_vault_archive(
    zip: &Path,
    protected_root: &Path,
    backup_dir: &Path,
) -> std::result::Result<PathBuf, String> {
    if is_same_volume(backup_dir, protected_root) {
        return Err(format!(
            "Thư mục sao lưu {} nằm trên chính ổ đang được bảo vệ",
            backup_dir.display()
        ));
    }
    std::fs::create_dir_all(backup_dir)
        .map_err(|e| format!("Không tạo được thư mục sao lưu: {e}"))?;
    let name = zip
        .file_name()
        .ok_or_else(|| "Đường dẫn gói Vault không hợp lệ".to_string())?;
    let dest = backup_dir.join(name);
    std::fs::copy(zip, &dest).map_err(|e| format!("Không sao chép được gói Vault: {e}"))?;

    if let (Some(dir), Some(stem)) = (zip.parent(), zip.file_stem()) {
        let mname = format!("{}.manifest.json", stem.to_string_lossy());
        let manifest = dir.join(&mname);
        if manifest.exists() {
            let _ = std::fs::copy(&manifest, backup_dir.join(&mname));
        }
    }
    Ok(dest)
}

/// Scans preset profiles into atomic [`CleanUnit`] items strictly located on `drive_root`.
pub fn scan_drive_units(
    profile_ids: &[String],
    drive_root: &Path,
    busy_status: &BusyToolsStatus,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Result<Vec<CleanUnit>> {
    let mut out = Vec::new();
    for pid in profile_ids {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        let units =
            PresetProfiles::scan_profile_units_with_busy(pid, ctrl, lock_probe, busy_status);
        for u in units {
            if is_same_volume(&u.root_path, drive_root) {
                out.push(u);
            }
        }
    }
    Ok(out)
}

/// Convenience wrapper: probes busy tools, scans profiles for `quota.drive_root`, and enforces the quota.
#[allow(clippy::too_many_arguments)]
pub fn run_guard_for_quota(
    quota: &DriveQuota,
    vault_dir: &Path,
    db: &GreenCDb,
    volume_api: &dyn Win32VolumeApi,
    lock_probe: &dyn FileLockProbe,
    dev_tool_probe: &dyn DevToolProbe,
    opts: &GuardRunOptions,
    ctrl: &TaskControl,
) -> Result<GuardDriveOutcome> {
    let mut busy = dev_tool_probe.detect_busy_tools();
    busy.merge_extra_tools(opts.extra_busy_tools.iter().copied());

    let drive_root = Path::new(&quota.drive_root);
    let units = if let Ok((total, free)) = volume_api.get_disk_free_space(drive_root) {
        let level = quota.level(total, free);
        let on_cooldown = !opts.dry_run
            && !opts.ignore_cooldown
            && db
                .get_guard_cooldown(&quota.drive_root)
                .ok()
                .flatten()
                .and_then(|c| c.cooldown_until)
                .is_some_and(|until| until > opts.now_utc);

        if level == QuotaLevel::Critical && quota.auto_clean && !on_cooldown {
            scan_drive_units(&opts.profile_ids, drive_root, &busy, lock_probe, ctrl)?
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    enforce_drive_quota_units(
        quota,
        units,
        vault_dir,
        db,
        volume_api,
        lock_probe,
        dev_tool_probe,
        opts,
        ctrl,
    )
}

fn unit_priority(unit: &CleanUnit) -> u8 {
    if matches!(
        unit.kind,
        CleanUnitKind::RegenerableDevCache | CleanUnitKind::RegenerableAppCache
    ) {
        return 0;
    }
    match unit.profile_id.as_str() {
        "windows_temp" | "browser_cache" | "app_logs" | "crash_dumps" | "old_versions" => 1,
        _ => 2,
    }
}

/// Estimates net bytes freed on the protected drive if `unit` is cleaned.
///
/// - Direct delete or cross-drive Vault -> 100% of `unit.total_bytes`.
/// - Same-drive Vault -> incompressible extensions yield `0`, compressible files estimate `50%`.
pub fn estimate_unit_net_bytes(
    unit: &CleanUnit,
    vault_same_drive: bool,
    will_direct_delete: bool,
) -> u64 {
    if will_direct_delete || !vault_same_drive {
        return unit.total_bytes;
    }
    let mut est = 0u64;
    for f in &unit.files {
        let incompressible = f
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| {
                FormatSniffer::is_incompressible_extension(ext)
                    || matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "crate" | "whl" | "aar" | "nupkg"
                    )
            });
        if !incompressible {
            est = est.saturating_add(f.size_bytes / 2);
        }
    }
    est
}

#[cfg(unix)]
fn device_id(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

#[cfg(not(unix))]
fn device_id(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

/// `true` only when both device ids are known and differ. Unix metadata always carries a device id,
/// so the check stays strict there; Windows metadata has none (volume serial and file index are
/// verified separately through `query_file_identity`), and must not be treated as a mismatch.
fn crosses_device(meta: &std::fs::Metadata, expected: Option<u64>) -> bool {
    matches!((device_id(meta), expected), (Some(actual), Some(want)) if actual != want)
}

#[cfg(unix)]
fn enforce_dir_mode_0700(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn enforce_dir_mode_0700(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Short-TTL (<= 1s) cache around [`DevToolProbe`] so bulk file units do not re-scan `/proc`
/// thousands of times, while critical boundaries (immediately before atomic directory rename,
/// or post-archive before unlinking) always call `get_fresh()`.
struct CachedBusyToolProbe<'a> {
    probe: &'a dyn DevToolProbe,
    extra_busy: &'a [DevTool],
    ttl: Duration,
    cached: Option<(Instant, BusyToolsStatus)>,
}

impl<'a> CachedBusyToolProbe<'a> {
    fn new(probe: &'a dyn DevToolProbe, extra_busy: &'a [DevTool], ttl: Duration) -> Self {
        Self {
            probe,
            extra_busy,
            ttl,
            cached: None,
        }
    }

    fn get_cached(&mut self) -> BusyToolsStatus {
        if let Some((at, ref status)) = self.cached {
            if at.elapsed() < self.ttl {
                return status.clone();
            }
        }
        self.get_fresh()
    }

    fn get_fresh(&mut self) -> BusyToolsStatus {
        let mut live = self.probe.detect_busy_tools();
        live.merge_extra_tools(self.extra_busy.iter().copied());
        self.cached = Some((Instant::now(), live.clone()));
        live
    }
}

enum ToctouVerdict {
    Ok,
    Skip(String),
    ToolStarted(DevTool),
    StatusUnknown(String),
}

/// Performs right-before-delete TOCTOU verification on an atomic [`CleanUnit`]:
/// 1. Checks cached busy tools (<= 1s TTL); directory units additionally perform a fresh probe
///    immediately before atomic rename.
/// 2. Rejects symlinks, cross-volume paths, and paths escaping `allowed_parent`.
/// 3. Re-verifies `(volume_serial/st_dev, file_index/st_ino)` against scan-time `root_identity`.
/// 4. Re-verifies unit age (`max(mtime, reliable_atime) >= min_age`).
/// 5. Re-verifies no constituent file is locked.
fn verify_unit_toctou(
    unit: &CleanUnit,
    drive_root: &Path,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    busy_cache: &mut CachedBusyToolProbe<'_>,
) -> ToctouVerdict {
    // 1. Check busy tools (cached <= 1s for bulk throughput; fresh check runs before atomic rename).
    if let Some(tool) = unit.dev_tool {
        let live_busy = busy_cache.get_cached();
        match &live_busy {
            BusyToolsStatus::Unknown { reason } => {
                return ToctouVerdict::StatusUnknown(reason.clone());
            }
            BusyToolsStatus::Known(_) => {
                if live_busy.is_tool_busy(tool) {
                    return ToctouVerdict::ToolStarted(tool);
                }
            }
        }
    }

    // 2. Reject symlinks, forbidden paths, cross-volume paths, and containment escapes.
    if unit.kind == CleanUnitKind::RegenerableDevCache
        && is_forbidden_dev_cache_path(&unit.root_path)
    {
        return ToctouVerdict::Skip("Đường dẫn thuộc vùng điều khiển bị cấm".to_string());
    }
    let Ok(meta) = std::fs::symlink_metadata(&unit.root_path) else {
        return ToctouVerdict::Skip("Đơn vị đã không còn tồn tại".to_string());
    };
    if meta.file_type().is_symlink() {
        return ToctouVerdict::Skip("Bỏ qua symlink".to_string());
    }
    if !is_same_volume(&unit.root_path, drive_root) {
        return ToctouVerdict::Skip("Đơn vị không còn nằm trên ổ được bảo vệ".to_string());
    }
    if let (Ok(canon_unit), Ok(canon_parent)) = (
        unit.root_path.canonicalize(),
        unit.allowed_parent.canonicalize(),
    ) {
        if !canon_unit.starts_with(&canon_parent) {
            return ToctouVerdict::Skip("Đường dẫn thoát ra ngoài thư mục cho phép".to_string());
        }
    }

    // 3. Verify physical device/inode identity matches scan snapshot.
    if let Some(expected_id) = unit.root_identity {
        match query_file_identity(&unit.root_path) {
            Some(cur_id)
                if cur_id.volume_serial == expected_id.volume_serial
                    && cur_id.file_index == expected_id.file_index => {}
            _ => {
                return ToctouVerdict::Skip(
                    "Định danh tệp/thư mục (dev/inode) đã thay đổi trước khi xoá".to_string(),
                );
            }
        }
    }

    // 4. Re-verify age right before delete.
    //    For partial-directory cache units (`zalo_cache_dir:*`), another sibling file in the same
    //    cache folder (e.g. a locked active file) may bump the directory's own mtime; step 5 below
    //    still re-verifies every constituent file in `unit.files` against `unit.min_age`.
    let is_partial_cache_dir = unit.kind == CleanUnitKind::RegenerableAppCache
        && unit.unit_label.starts_with("zalo_cache_dir");
    if unit.min_age > Duration::ZERO && !is_partial_cache_dir {
        if let Some(latest) = compute_unit_latest_used(&unit.root_path, now) {
            let age = now.duration_since(latest).unwrap_or(Duration::ZERO);
            if age < unit.min_age {
                return ToctouVerdict::Skip(
                    "Đơn vị vừa được truy cập/ghi mới trước khi xoá".to_string(),
                );
            }
        } else {
            return ToctouVerdict::Skip("Không thể kiểm tra lại dấu thời gian đơn vị".to_string());
        }
    }

    // 5. Re-verify every constituent file: no symlink, same drive, within allowed_parent,
    // still >= min_age (important for multi-file units like npm index-v5 + content-v2), and unlocked.
    let canon_parent = unit.allowed_parent.canonicalize().ok();
    for f in &unit.files {
        if unit.kind == CleanUnitKind::RegenerableDevCache && is_forbidden_dev_cache_path(&f.path) {
            return ToctouVerdict::Skip("Tệp thành phần thuộc vùng điều khiển bị cấm".to_string());
        }
        let Ok(fm) = std::fs::symlink_metadata(&f.path) else {
            continue;
        };
        if fm.file_type().is_symlink() {
            return ToctouVerdict::Skip("Phát hiện symlink bên trong đơn vị".to_string());
        }
        if !is_same_volume(&f.path, drive_root) {
            return ToctouVerdict::Skip("Tệp thành phần không nằm trên ổ được bảo vệ".to_string());
        }
        if let Some(ref cp) = canon_parent {
            if let Ok(cf) = f.path.canonicalize() {
                if !cf.starts_with(cp) {
                    return ToctouVerdict::Skip(
                        "Tệp thành phần thoát ra ngoài thư mục cho phép".to_string(),
                    );
                }
            }
        }
        if unit.min_age > Duration::ZERO && fm.is_file() {
            let used = effective_last_used(&fm, now);
            let age = now.duration_since(used).unwrap_or(Duration::ZERO);
            if age < unit.min_age {
                return ToctouVerdict::Skip(
                    "Tệp thành phần vừa được truy cập/ghi mới trước khi xoá".to_string(),
                );
            }
        }
        if lock_probe.is_file_locked(&f.path) {
            return ToctouVerdict::Skip("Tệp trong đơn vị đang bị khoá".to_string());
        }
    }

    ToctouVerdict::Ok
}

/// Safely removes a file or directory tree WITHOUT following symlinks, crossing devices,
/// or truncating at arbitrary depth limits (> 32 levels are removed completely).
fn safe_remove_path_no_follow(root: &Path, expected_dev: Option<u64>) -> Result<(u64, u64)> {
    let meta = std::fs::symlink_metadata(root)?;
    if meta.file_type().is_symlink() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "direct_delete".to_string(),
            reason: format!("Refused to remove symlink '{}'", root.display()),
        });
    }
    if crosses_device(&meta, expected_dev) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "direct_delete".to_string(),
            reason: format!("Refused cross-device deletion at '{}'", root.display()),
        });
    }

    if meta.is_file() {
        let len = meta.len();
        std::fs::remove_file(root)?;
        return Ok((1, len));
    }

    if !meta.is_dir() {
        return Ok((0, 0));
    }

    let mut files_removed = 0u64;
    let mut bytes_removed = 0u64;
    let mut dirs_to_remove: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];

    // Iterative heap traversal: never overflows call stack, never follows symlinks, never
    // truncates trees > 32 levels deep.
    while let Some(dir) = stack.pop() {
        dirs_to_remove.push(dir.clone());
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(em) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if em.file_type().is_symlink() {
                // Never follow symlinked directories or targets.
                continue;
            }
            if crosses_device(&em, expected_dev) {
                continue;
            }
            if em.is_dir() {
                stack.push(p);
            } else if em.is_file() {
                let len = em.len();
                if std::fs::remove_file(&p).is_ok() {
                    files_removed += 1;
                    bytes_removed = bytes_removed.saturating_add(len);
                }
            }
        }
    }

    // Remove directories strictly bottom-up (children always follow parents in `dirs_to_remove`).
    for dir in dirs_to_remove.into_iter().rev() {
        let _ = std::fs::remove_dir(&dir);
    }

    Ok((files_removed, bytes_removed))
}

fn path_has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

fn path_is_inside_dir(candidate: &Path, forbidden_parent: &Path) -> bool {
    if candidate.starts_with(forbidden_parent) {
        return true;
    }
    if let (Ok(c_cand), Ok(c_forb)) = (candidate.canonicalize(), forbidden_parent.canonicalize()) {
        return c_cand.starts_with(&c_forb);
    }
    false
}

fn check_owner_matches(path: &Path, expected_uid: Option<u32>) -> std::result::Result<(), String> {
    #[cfg(unix)]
    {
        let uid = expected_uid.unwrap_or_else(current_user_uid);
        match is_owned_by_uid(path, uid) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "Đường dẫn '{}' không thuộc uid hiện tại ({uid})",
                path.display()
            )),
            Err(e) => Err(format!(
                "Không xác minh được chủ sở hữu '{}': {e}",
                path.display()
            )),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = expected_uid;
        match is_owned_by_current_user(path) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "Đường dẫn '{}' không thuộc người dùng hiện tại",
                path.display()
            )),
            Err(e) => Err(format!(
                "Không xác minh được chủ sở hữu '{}': {e}",
                path.display()
            )),
        }
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if std::fs::symlink_metadata(p).is_ok() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

fn prepare_private_staging_dir(
    dir: &Path,
    expected_uid: Option<u32>,
) -> std::result::Result<PathBuf, String> {
    if path_has_parent_dir(dir) {
        return Err(format!(
            "Đường dẫn khu chờ '{}' không được chứa '..'",
            dir.display()
        ));
    }

    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(format!(
                "Khu chờ '{}' là symlink hoặc không phải thư mục thật",
                dir.display()
            ));
        }
        check_owner_matches(dir, expected_uid)?;
        enforce_dir_mode_0700(dir)
            .map_err(|e| format!("Không đặt được quyền 0700 cho '{}': {e}", dir.display()))?;
    } else {
        // Verify nearest existing ancestor is owned by current user and not world-writable.
        let anc = nearest_existing_ancestor(dir).ok_or_else(|| {
            format!(
                "Không tìm thấy thư mục cha hợp lệ cho khu chờ '{}'",
                dir.display()
            )
        })?;
        is_trusted_user_dir(&anc, expected_uid)?;

        if let Some(parent) = dir.parent() {
            if std::fs::symlink_metadata(parent).is_err() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!("Không tạo được thư mục cha '{}': {e}", parent.display())
                })?;
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut b = std::fs::DirBuilder::new();
            b.mode(0o700);
            if let Err(e) = b.create(dir) {
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(format!("Không tạo được khu chờ '{}': {e}", dir.display()));
                }
            }
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Không tạo được khu chờ '{}': {e}", dir.display()))?;
        }
        enforce_dir_mode_0700(dir)
            .map_err(|e| format!("Không đặt được quyền 0700 cho '{}': {e}", dir.display()))?;
    }

    validate_private_staging_dir(dir, expected_uid)?;
    Ok(dir.to_path_buf())
}

fn resolve_home_staging_path(home_override: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
    if let Some(h) = home_override {
        if h.as_os_str().is_empty() {
            return None;
        }
        return Some((h.to_path_buf(), h.join(".local/share/greenc/staging")));
    }

    #[cfg(unix)]
    {
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)?;
        Some((home.clone(), home.join(".local/share/greenc/staging")))
    }

    #[cfg(all(windows, feature = "store"))]
    {
        // On MSIX (Microsoft Store), directories created inside %LOCALAPPDATA% are virtualized into
        // the package's LocalCache, so atomic same-drive `rename` from a real cache directory into
        // AppData cannot be relied upon. Place staging in a user-owned folder OUTSIDE AppData.
        let user_profile = std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)?;
        Some((
            user_profile.clone(),
            user_profile.join(".greenc-store-staging"),
        ))
    }

    #[cfg(all(windows, not(feature = "store")))]
    {
        let app = greenc_core::config::app_home();
        let base = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)?;
        Some((base, app.join("staging")))
    }
}

fn validate_path_chain_trusted(
    base: &Path,
    target: &Path,
    expected_uid: Option<u32>,
) -> std::result::Result<(), String> {
    is_trusted_user_dir(base, expected_uid)?;
    if let Ok(rel) = target.strip_prefix(base) {
        let mut cur = base.to_path_buf();
        let comps: Vec<_> = rel.components().collect();
        // Check every existing intermediate directory between `base` and `target` (exclusive of leaf).
        for comp in comps.iter().take(comps.len().saturating_sub(1)) {
            cur.push(comp);
            if std::fs::symlink_metadata(&cur).is_ok() {
                is_trusted_user_dir(&cur, expected_uid)?;
            }
        }
    }
    Ok(())
}

/// Resolves a writable, private (`0700` on Unix, current-user owned on all OS) GreenC staging
/// directory on the SAME volume as `unit_root` and strictly OUTSIDE `allowed_parent`.
///
/// ONLY three locations are permitted (NEVER `drive_root/.greenc-staging` and NEVER shared/foreign
/// ancestor directories):
/// 1. User-specified `explicit_staging`.
/// 2. `vault_dir/.greenc-staging` when `vault_dir` is a trusted directory owned by the current user
///    (and not world-writable on Unix) on the same volume.
/// 3. `<HOME>/.local/share/greenc/staging` (or `%LOCALAPPDATA%\GreenC\staging` on Windows) when
///    owned by the current user (not world-writable) on the same volume.
pub fn resolve_staging_dir_checked(
    unit_root: &Path,
    allowed_parent: &Path,
    vault_dir: &Path,
    explicit_staging: Option<&Path>,
    home_override: Option<&Path>,
    expected_uid: Option<u32>,
) -> std::result::Result<PathBuf, String> {
    let mut reject_reasons: Vec<String> = Vec::new();

    // 1. User-specified explicit staging directory
    if let Some(exp) = explicit_staging {
        if cfg!(feature = "store") && is_windows_appdata_app_dir(exp) {
            reject_reasons.push(format!(
                "Bản Store không đặt khu chờ chỉ định '{}' trong AppData ảo hoá",
                exp.display()
            ));
        } else if path_is_inside_dir(exp, allowed_parent) {
            reject_reasons.push(format!(
                "khu chờ chỉ định '{}' nằm trong cây cache",
                exp.display()
            ));
        } else if !is_same_volume(exp, unit_root) {
            reject_reasons.push(format!(
                "khu chờ chỉ định '{}' khác ổ với đơn vị",
                exp.display()
            ));
        } else {
            match prepare_private_staging_dir(exp, expected_uid) {
                Ok(ready)
                    if is_same_volume(&ready, unit_root)
                        && !path_is_inside_dir(&ready, allowed_parent) =>
                {
                    return Ok(ready);
                }
                Ok(_) => reject_reasons.push(format!(
                    "khu chờ chỉ định '{}' không hợp lệ sau khi chuẩn bị",
                    exp.display()
                )),
                Err(e) => reject_reasons.push(e),
            }
        }
    }

    // 2. `vault_dir/.greenc-staging` (ONLY if Vault itself is trusted, user-owned, not world-writable,
    //    and in Store/MSIX builds not inside virtualized AppData)
    if cfg!(feature = "store") && greenc_core::config::is_windows_appdata_app_dir(vault_dir) {
        reject_reasons.push(
            "Bản Store không đặt khu chờ trong AppData ảo hoá; chuyển sang khu chờ ngoài AppData"
                .to_string(),
        );
    } else if is_same_volume(vault_dir, unit_root) {
        let vault_trust = if std::fs::symlink_metadata(vault_dir).is_ok() {
            is_trusted_user_dir(vault_dir, expected_uid)
        } else if let Some(anc) = nearest_existing_ancestor(vault_dir) {
            is_trusted_user_dir(&anc, expected_uid)
        } else {
            Err(format!(
                "Vault '{}' không có thư mục cha hợp lệ",
                vault_dir.display()
            ))
        };

        match vault_trust {
            Ok(()) => {
                let cand = vault_dir.join(".greenc-staging");
                if !path_is_inside_dir(&cand, allowed_parent) && is_same_volume(&cand, unit_root) {
                    match prepare_private_staging_dir(&cand, expected_uid) {
                        Ok(ready)
                            if is_same_volume(&ready, unit_root)
                                && !path_is_inside_dir(&ready, allowed_parent) =>
                        {
                            return Ok(ready);
                        }
                        Ok(_) => {}
                        Err(e) => reject_reasons.push(e),
                    }
                }
            }
            Err(e) => reject_reasons.push(format!("Vault không đủ tin cậy: {e}")),
        }
    } else {
        reject_reasons.push("Vault khác ổ với đơn vị".to_string());
    }

    // 3. `<HOME>/.local/share/greenc/staging` on the same volume
    if let Some((home_base, home_staging)) = resolve_home_staging_path(home_override) {
        if is_same_volume(&home_base, unit_root) {
            match validate_path_chain_trusted(&home_base, &home_staging, expected_uid) {
                Ok(()) => {
                    if !path_is_inside_dir(&home_staging, allowed_parent)
                        && is_same_volume(&home_staging, unit_root)
                    {
                        match prepare_private_staging_dir(&home_staging, expected_uid) {
                            Ok(ready)
                                if is_same_volume(&ready, unit_root)
                                    && !path_is_inside_dir(&ready, allowed_parent) =>
                            {
                                return Ok(ready);
                            }
                            Ok(_) => {}
                            Err(e) => reject_reasons.push(e),
                        }
                    }
                }
                Err(e) => reject_reasons.push(format!("Thư mục HOME không đủ tin cậy: {e}")),
            }
        } else {
            reject_reasons.push("<HOME>/.local/share/greenc/staging khác ổ với đơn vị".to_string());
        }
    } else {
        reject_reasons.push("Không xác định được thư mục HOME".to_string());
    }

    Err(format!(
        "Bỏ qua đơn vị {}: không có khu chờ an toàn cùng ổ ({})",
        unit_root.display(),
        reject_reasons.join("; ")
    ))
}

fn candidate_recovery_staging_dirs(
    drive_root: &Path,
    vault_dir: &Path,
    explicit_staging: Option<&Path>,
    home_override: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    };

    if let Some(exp) = explicit_staging {
        if is_same_volume(exp, drive_root) {
            push_unique(exp.to_path_buf());
        }
    }
    if is_same_volume(vault_dir, drive_root) {
        push_unique(vault_dir.join(".greenc-staging"));
    }
    if let Some((home_base, home_staging)) = resolve_home_staging_path(home_override) {
        if is_same_volume(&home_base, drive_root) {
            push_unique(home_staging);
        }
    }

    dirs.into_iter()
        .filter(|d| std::fs::symlink_metadata(d).is_ok())
        .collect()
}

fn write_staging_journal(journal_path: &Path, entry: &StagingJournalEntry) -> std::io::Result<()> {
    let tmp_path = journal_path.with_extension("json.tmp");
    if std::fs::symlink_metadata(&tmp_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refused to write staging journal temp file through symlink",
        ));
    }
    if std::fs::symlink_metadata(journal_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refused to replace symlink staging journal",
        ));
    }
    let data = serde_json::to_vec_pretty(entry).map_err(std::io::Error::other)?;
    {
        #[cfg(unix)]
        let mut f = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp_path)?
        };
        #[cfg(not(unix))]
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(&data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, journal_path)?;
    Ok(())
}

struct StagedDirUnit {
    unit: CleanUnit,
    journal_path: PathBuf,
    journal: StagingJournalEntry,
    expected_dev: Option<u64>,
    staged_candidates: Vec<CleanCandidate>,
    staged_to_original: HashMap<PathBuf, PathBuf>,
}

enum StagingRollbackOutcome {
    Restored,
    DiscardedRedundant(String),
    KeptInStaging(String),
}

/// Walks a staged directory tree (never following symlinks, unlimited depth) to enumerate files
/// and map each staged path back to its true `original_path` for Vault restoration.
fn collect_staged_tree_files(
    staged_root: &Path,
    original_root: &Path,
    profile_id: &str,
    expected_dev: Option<u64>,
    now: SystemTime,
) -> (Vec<CleanCandidate>, HashMap<PathBuf, PathBuf>) {
    let mut candidates = Vec::new();
    let mut map = HashMap::new();
    let mut stack = vec![staged_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(em) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if em.file_type().is_symlink() {
                continue;
            }
            if crosses_device(&em, expected_dev) {
                continue;
            }
            if em.is_dir() {
                stack.push(p);
            } else if em.is_file() {
                if let Ok(rel) = p.strip_prefix(staged_root) {
                    let orig = original_root.join(rel);
                    map.insert(p.clone(), orig);
                    candidates.push(CleanCandidate {
                        path: p.clone(),
                        size_bytes: em.len(),
                        mtime: em.modified().unwrap_or(now),
                        is_locked: false,
                        profile_id: profile_id.to_string(),
                        is_canary: false,
                        is_orphan: false,
                        hardlink_siblings: vec![p],
                    });
                }
            }
        }
    }

    (candidates, map)
}

/// Atomically moves a regenerable developer-cache directory unit out of its tool cache tree
/// into GreenC's private (`0700`) same-drive staging directory with a persistent crash-recovery
/// journal recording the unit's physical `(dev, ino)` identity.
#[allow(clippy::too_many_arguments)]
fn stage_directory_unit(
    unit: CleanUnit,
    mode: StagingMode,
    drive_root: &Path,
    vault_dir: &Path,
    explicit_staging: Option<&Path>,
    home_override: Option<&Path>,
    now_utc: DateTime<Utc>,
    now_sys: SystemTime,
) -> std::result::Result<StagedDirUnit, String> {
    if unit.kind != CleanUnitKind::RegenerableDevCache {
        return Err(format!(
            "Chỉ đơn vị cache tái tạo được mới được đưa vào khu chờ (từ chối {})",
            unit.root_path.display()
        ));
    }

    let staging_dir = resolve_staging_dir_checked(
        &unit.root_path,
        &unit.allowed_parent,
        vault_dir,
        explicit_staging,
        home_override,
        None,
    )?;

    let phys_id = query_file_identity(&unit.root_path).ok_or_else(|| {
        format!(
            "Không đọc được định danh vật lý (dev/inode) của {} trước khi chuyển vào khu chờ",
            unit.root_path.display()
        )
    })?;

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let id = format!("unit_{millis}_{pid}_{seq}");
    debug_assert!(is_valid_staging_unit_id(&id));
    let staged_path = staging_dir.join(&id);
    let journal_path = staging_dir.join(format!("{id}.journal.json"));

    let expected_dev = std::fs::symlink_metadata(&unit.root_path)
        .ok()
        .and_then(|m| device_id(&m))
        .or(Some(phys_id.volume_serial));

    let mut journal = StagingJournalEntry {
        version: 2,
        id,
        original_path: unit.root_path.clone(),
        staged_path: staged_path.clone(),
        mode,
        state: StagingState::Staging,
        created_at: now_utc,
        drive_root: drive_root.to_string_lossy().to_string(),
        unit_label: unit.unit_label.clone(),
        dev_tool: unit.dev_tool,
        note: None,
        staged_dev: Some(phys_id.volume_serial),
        staged_ino: Some(phys_id.file_index),
        unit_kind: Some(unit.kind),
    };

    write_staging_journal(&journal_path, &journal)
        .map_err(|e| format!("Không ghi được nhật ký khu chờ: {e}"))?;

    if let Err(e) = std::fs::rename(&unit.root_path, &staged_path) {
        let _ = std::fs::remove_file(&journal_path);
        return Err(format!(
            "Không đổi tên nguyên tử được {}: {e}",
            unit.root_path.display()
        ));
    }

    journal.state = match mode {
        StagingMode::Vault => StagingState::Staged,
        StagingMode::Direct => StagingState::Deleting,
    };
    let _ = write_staging_journal(&journal_path, &journal);

    let (staged_candidates, staged_to_original) = match mode {
        StagingMode::Vault => collect_staged_tree_files(
            &staged_path,
            &unit.root_path,
            &unit.profile_id,
            expected_dev,
            now_sys,
        ),
        StagingMode::Direct => (Vec::new(), HashMap::new()),
    };

    Ok(StagedDirUnit {
        unit,
        journal_path,
        journal,
        expected_dev,
        staged_candidates,
        staged_to_original,
    })
}

/// Executes a `RestoreStagedDir` job inside the unpackaged `greenc-cli __restore-helper` child
/// process: re-validates `staged_dir` and `original_path`, moves `staged_dir` back to real
/// `original_path` without overwriting an existing destination, and verifies from outside the
/// MSIX package that `original_path` exists on the real filesystem.
pub fn execute_restore_staged_dir_job(
    staged_dir: &Path,
    original_path: &Path,
    expected_dev: Option<u64>,
    expected_ino: Option<u64>,
    running_in_package: bool,
) -> (i32, RestoreHelperResponse) {
    if running_in_package {
        return (
            EXIT_HELPER_STILL_IN_PACKAGE,
            RestoreHelperResponse::still_in_package(current_package_full_name().as_deref()),
        );
    }

    if !staged_dir.is_absolute()
        || path_has_parent_dir(staged_dir)
        || is_windows_appdata_app_dir(staged_dir)
    {
        return (
            2,
            RestoreHelperResponse::failed(
                "invalid_staged_dir",
                format!(
                    "Thư mục khu chờ '{}' không hợp lệ hoặc nằm trong AppData",
                    staged_dir.display()
                ),
            ),
        );
    }

    if let Err(e) = validate_restore_file_path(original_path, "restore_staged_dir") {
        return (
            2,
            RestoreHelperResponse::failed("invalid_original_path", e.to_string()),
        );
    }

    if let Err(why) = is_trusted_user_dir(staged_dir, None) {
        return (
            2,
            RestoreHelperResponse::failed("untrusted_staged_dir", why),
        );
    }

    if let (Some(ed), Some(ei)) = (expected_dev, expected_ino) {
        match query_file_identity(staged_dir) {
            Some(live) if live.volume_serial == ed && live.file_index == ei => {}
            _ => {
                return (
                    2,
                    RestoreHelperResponse::failed(
                        "identity_mismatch",
                        format!(
                            "Định danh vật lý (dev/inode) của '{}' không khớp nhật ký khu chờ",
                            staged_dir.display()
                        ),
                    ),
                );
            }
        }
    }

    if std::fs::symlink_metadata(original_path).is_ok() {
        return (
            2,
            RestoreHelperResponse::failed(
                "restore_conflict",
                format!(
                    "Đường dẫn đích '{}' đã tồn tại trên đĩa thật; từ chối ghi đè",
                    original_path.display()
                ),
            ),
        );
    }

    if let Some(parent) = original_path.parent() {
        if let Ok(pmeta) = std::fs::symlink_metadata(parent) {
            if pmeta.file_type().is_symlink() || !pmeta.is_dir() {
                return (
                    2,
                    RestoreHelperResponse::failed(
                        "invalid_parent",
                        format!(
                            "Thư mục cha '{}' là symlink hoặc không phải thư mục thật",
                            parent.display()
                        ),
                    ),
                );
            }
        } else if let Err(e) = std::fs::create_dir_all(parent) {
            return (
                1,
                RestoreHelperResponse::failed(
                    "create_parent_failed",
                    format!("Không tạo được thư mục cha '{}': {e}", parent.display()),
                ),
            );
        }
    }

    if let Err(e) = std::fs::rename(staged_dir, original_path) {
        return (
            1,
            RestoreHelperResponse::failed(
                "rename_failed",
                format!(
                    "Không thể chuyển '{}' về '{}': {e}",
                    staged_dir.display(),
                    original_path.display()
                ),
            ),
        );
    }

    // Verify from outside the package that `original_path` is now a real directory on disk.
    match std::fs::symlink_metadata(original_path) {
        Ok(m) if !m.file_type().is_symlink() && m.is_dir() => {}
        _ => {
            return (
                1,
                RestoreHelperResponse::failed(
                    "verify_missing",
                    format!(
                        "Xác minh ngoài gói thất bại: không thấy thư mục thật '{}' sau khôi phục khu chờ",
                        original_path.display()
                    ),
                ),
            );
        }
    }

    (
        0,
        RestoreHelperResponse {
            ok: true,
            running_outside_package: true,
            error_code: None,
            error: None,
            files: Vec::new(),
            verified_dir: Some(original_path.to_path_buf()),
            verified_file_count: 0,
            verified_bytes: 0,
        },
    )
}

fn restore_staged_dir_to_real_path(
    staged_path: &Path,
    original_path: &Path,
    expected_dev: Option<u64>,
    expected_ino: Option<u64>,
    client: &dyn BreakawayHelperClient,
) -> std::result::Result<(), String> {
    if cfg!(feature = "store") && client.is_packaged() && is_windows_appdata_app_dir(original_path)
    {
        let job_dir = PrivateBreakawayJobDir::create().map_err(|e| {
            format_breakaway_unavailable_guidance("khôi phục khu chờ", &e.to_string())
        })?;
        let req = RestoreHelperRequest::RestoreStagedDir {
            staged_dir: staged_path.to_path_buf(),
            original_path: original_path.to_path_buf(),
            expected_dev,
            expected_ino,
        };
        let req_bytes = serde_json::to_vec(&req)
            .map_err(|e| format!("Không tuần tự hoá được yêu cầu khôi phục khu chờ: {e}"))?;
        let out = client.execute(&job_dir, &req_bytes).map_err(|e| {
            format_breakaway_unavailable_guidance("khôi phục khu chờ", &e.to_string())
        })?;

        if out.exit_code == EXIT_HELPER_STILL_IN_PACKAGE {
            let detail = serde_json::from_str::<RestoreHelperResponse>(&out.stdout)
                .ok()
                .and_then(|r| r.error)
                .unwrap_or_else(|| "Tiến trình trợ giúp vẫn còn trong gói MSIX".to_string());
            return Err(format_breakaway_unavailable_guidance(
                "khôi phục khu chờ",
                &detail,
            ));
        }

        let resp: RestoreHelperResponse = serde_json::from_str(&out.stdout).map_err(|e| {
            format!(
                "Phản hồi khôi phục khu chờ ngoài gói không hợp lệ (exit={}): {e}",
                out.exit_code
            )
        })?;
        if !resp.running_outside_package || resp.error_code.as_deref() == Some("still_in_package") {
            let detail = resp
                .error
                .unwrap_or_else(|| "Không xác nhận được trạng thái ngoài gói MSIX".to_string());
            return Err(format_breakaway_unavailable_guidance(
                "khôi phục khu chờ",
                &detail,
            ));
        }
        if out.exit_code != 0 || !resp.ok || resp.verified_dir.as_deref() != Some(original_path) {
            return Err(resp.error.unwrap_or_else(|| {
                format!(
                    "Khôi phục khu chờ ngoài gói thất bại cho '{}' (exit={})",
                    original_path.display(),
                    out.exit_code
                )
            }));
        }
        return Ok(());
    }

    std::fs::rename(staged_path, original_path).map_err(|e| e.to_string())
}

/// Rolls back a staged directory unit to its `original_path` ONLY when `original_path` does not
/// already exist. If `original_path` already exists:
/// - For `RegenerableDevCache`: verifies staging `(dev, ino)` identity, deletes the redundant
///   staged copy, removes the journal, and logs the cleanup.
/// - Otherwise: keeps safely in staging, records `RestoreConflict`, and warns.
fn rollback_staged_dir_unit(staged: &mut StagedDirUnit) -> StagingRollbackOutcome {
    if std::fs::symlink_metadata(&staged.journal.original_path).is_ok() {
        if staged.journal.unit_kind == Some(CleanUnitKind::RegenerableDevCache) {
            let id_matches = match (
                staged.journal.staged_dev,
                staged.journal.staged_ino,
                query_file_identity(&staged.journal.staged_path),
            ) {
                (Some(ed), Some(ei), Some(live)) => {
                    live.volume_serial == ed && live.file_index == ei
                }
                _ => false,
            };
            if id_matches
                && safe_remove_path_no_follow(&staged.journal.staged_path, staged.expected_dev)
                    .is_ok()
                && std::fs::symlink_metadata(&staged.journal.staged_path).is_err()
            {
                let _ = std::fs::remove_file(&staged.journal_path);
                let msg = format!(
                    "Đường dẫn {} đã được công cụ tạo lại; đã xoá bản cache tái tạo dư thừa tại khu chờ {}",
                    staged.journal.original_path.display(),
                    staged.journal.staged_path.display()
                );
                tracing::info!("{msg}");
                return StagingRollbackOutcome::DiscardedRedundant(msg);
            }
        }

        let msg = format!(
            "Không thể trả {} về chỗ cũ vì đường dẫn đích đã tồn tại; giữ nguyên tại khu chờ {}",
            staged.journal.original_path.display(),
            staged.journal.staged_path.display()
        );
        staged.journal.state = StagingState::RestoreConflict;
        staged.journal.note = Some(msg.clone());
        let _ = write_staging_journal(&staged.journal_path, &staged.journal);
        return StagingRollbackOutcome::KeptInStaging(msg);
    }

    let client = NativeBreakawayHelperClient;
    match restore_staged_dir_to_real_path(
        &staged.journal.staged_path,
        &staged.journal.original_path,
        staged.journal.staged_dev,
        staged.journal.staged_ino,
        &client,
    ) {
        Ok(()) => {
            let _ = std::fs::remove_file(&staged.journal_path);
            StagingRollbackOutcome::Restored
        }
        Err(e) => {
            let msg = format!(
                "Lỗi khi trả {} về chỗ cũ ({e}); giữ nguyên tại khu chờ {}",
                staged.journal.original_path.display(),
                staged.journal.staged_path.display()
            );
            staged.journal.state = StagingState::RestoreConflict;
            staged.journal.note = Some(msg.clone());
            let _ = write_staging_journal(&staged.journal_path, &staged.journal);
            StagingRollbackOutcome::KeptInStaging(msg)
        }
    }
}

/// Finalizes a staged directory unit after Direct commit or Vault DB commit: deletes the staged
/// directory tree completely (unlimited depth, no symlink follow) and removes its journal file.
fn finalize_staged_dir_unit(staged: &mut StagedDirUnit) -> Result<(u64, u64)> {
    staged.journal.state = StagingState::Deleting;
    let _ = write_staging_journal(&staged.journal_path, &staged.journal);
    let (files_removed, bytes_removed) =
        safe_remove_path_no_follow(&staged.journal.staged_path, staged.expected_dev)?;
    if std::fs::symlink_metadata(&staged.journal.staged_path).is_err() {
        let _ = std::fs::remove_file(&staged.journal_path);
    }
    Ok((files_removed, bytes_removed))
}

fn validate_restore_original_path(
    original_path: &Path,
    staging_dir: &Path,
    expected_dev: u64,
    allowed_roots: &[PathBuf],
) -> std::result::Result<PathBuf, String> {
    if !original_path.is_absolute() {
        return Err(format!(
            "original_path '{}' không phải đường dẫn tuyệt đối",
            original_path.display()
        ));
    }
    if path_has_parent_dir(original_path) {
        return Err(format!(
            "original_path '{}' chứa '..'",
            original_path.display()
        ));
    }
    if is_forbidden_dev_cache_path(original_path) {
        return Err(format!(
            "original_path '{}' thuộc vùng điều khiển bị cấm",
            original_path.display()
        ));
    }
    let file_name = match original_path.components().next_back() {
        Some(Component::Normal(name)) if !name.is_empty() => name.to_os_string(),
        _ => {
            return Err(format!(
                "original_path '{}' không có tên thư mục đích hợp lệ",
                original_path.display()
            ));
        }
    };
    let parent = original_path.parent().ok_or_else(|| {
        format!(
            "original_path '{}' không có thư mục cha",
            original_path.display()
        )
    })?;

    let pmeta = std::fs::symlink_metadata(parent).map_err(|e| {
        format!(
            "Thư mục cha '{}' của original_path không tồn tại hoặc không đọc được: {e}",
            parent.display()
        )
    })?;
    if pmeta.file_type().is_symlink() || !pmeta.is_dir() {
        return Err(format!(
            "Thư mục cha '{}' của original_path là symlink hoặc không phải thư mục thật",
            parent.display()
        ));
    }
    let canon_parent = parent.canonicalize().map_err(|e| {
        format!(
            "Không thể chuẩn hoá (canonicalize) thư mục cha '{}': {e}",
            parent.display()
        )
    })?;

    // Check same volume with staging directory (and matching st_dev on Unix)
    if !is_same_volume(&canon_parent, staging_dir) {
        return Err(format!(
            "original_path '{}' không nằm cùng ổ với khu chờ",
            original_path.display()
        ));
    }
    #[cfg(unix)]
    {
        let cp_meta = std::fs::symlink_metadata(&canon_parent).map_err(|e| {
            format!(
                "Không đọc được siêu dữ liệu thư mục cha '{}': {e}",
                canon_parent.display()
            )
        })?;
        if device_id(&cp_meta) != Some(expected_dev) {
            return Err(format!(
                "original_path '{}' có thiết bị (st_dev) khác khu chờ",
                original_path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = expected_dev;
    }

    // Must lie strictly under one of GreenC's managed developer cache roots for the current user.
    let mut inside_managed = false;
    for root in allowed_roots {
        let Ok(rmeta) = std::fs::symlink_metadata(root) else {
            continue;
        };
        if rmeta.file_type().is_symlink() || !rmeta.is_dir() {
            continue;
        }
        let Ok(canon_root) = root.canonicalize() else {
            continue;
        };
        if canon_parent.starts_with(&canon_root) {
            inside_managed = true;
            break;
        }
    }
    if !inside_managed {
        return Err(format!(
            "original_path '{}' không nằm trong bất kỳ gốc cache developer nào GreenC quản lý",
            original_path.display()
        ));
    }

    Ok(canon_parent.join(file_name))
}

fn record_rejection(report: &mut StagingRecoveryReport, msg: String) {
    tracing::warn!("{msg}");
    report.rejected.push(msg.clone());
    report.conflicts.push(msg);
}

/// Recovers any unfinished staged directory units left behind by a crash or power loss using
/// default strict security rules (current user UID, `0700` staging dir, `PresetProfiles::managed_dev_cache_roots()`).
pub fn recover_staging_dir(
    staging_dir: &Path,
    busy_status: &BusyToolsStatus,
) -> StagingRecoveryReport {
    recover_staging_dir_with_options(staging_dir, busy_status, &StagingRecoveryOptions::default())
}

/// Recovers unfinished staged directory units with configurable security verification options
/// (defaults enforce current user ownership, `0700` Unix permissions, `nlink == 1`, journal
/// `<= 64 KiB`, strict `^unit_[0-9]+_[0-9]+_[0-9]+$` IDs, `staging_dir/<id>` path derivation,
/// `(dev, ino)` matching, and canonicalized managed developer cache root containment).
pub fn recover_staging_dir_with_options(
    staging_dir: &Path,
    busy_status: &BusyToolsStatus,
    opts: &StagingRecoveryOptions,
) -> StagingRecoveryReport {
    let mut report = StagingRecoveryReport::default();

    #[cfg(unix)]
    let expected_uid = opts.expected_uid;
    #[cfg(not(unix))]
    let expected_uid: Option<u32> = None;

    let staging_meta = match std::fs::symlink_metadata(staging_dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return report,
        Err(e) => {
            record_rejection(
                &mut report,
                format!(
                    "Bỏ qua khôi phục khu chờ '{}': không đọc được siêu dữ liệu ({e})",
                    staging_dir.display()
                ),
            );
            return report;
        }
    };

    if let Err(reason) = validate_private_staging_dir(staging_dir, expected_uid) {
        record_rejection(
            &mut report,
            format!("Bỏ qua khôi phục khu chờ không an toàn: {reason}"),
        );
        return report;
    }

    let staging_dev = device_id(&staging_meta);
    let canon_staging_dir = staging_dir.canonicalize().ok();
    let allowed_roots = opts
        .allowed_cache_roots
        .clone()
        .unwrap_or_else(PresetProfiles::managed_dev_cache_roots);

    let Ok(rd) = std::fs::read_dir(staging_dir) else {
        return report;
    };

    for entry in rd.flatten() {
        let jpath = entry.path();
        let fname = entry.file_name().to_string_lossy().to_string();
        let Some(stem_id) = fname.strip_suffix(".journal.json") else {
            continue;
        };

        // 1. Validate filename stem ID matches `^unit_[0-9]+_[0-9]+_[0-9]+$`
        if !is_valid_staging_unit_id(stem_id) {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký khu chờ '{}': tên tệp không khớp mẫu unit_<ts>_<pid>_<seq>",
                    jpath.display()
                ),
            );
            continue;
        }

        // 2. Validate journal file metadata: regular file, not symlink, owned by current user,
        //    not a hardlink (`nlink == 1`), size <= 64 KiB.
        let Ok(jmeta) = std::fs::symlink_metadata(&jpath) else {
            continue;
        };
        if jmeta.file_type().is_symlink() || !jmeta.is_file() {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký khu chờ '{}': là symlink hoặc không phải tệp thường",
                    jpath.display()
                ),
            );
            continue;
        }
        if let Err(why) = check_owner_matches(&jpath, expected_uid) {
            record_rejection(
                &mut report,
                format!("Từ chối nhật ký khu chờ '{}': {why}", jpath.display()),
            );
            continue;
        }
        let nlink_ok = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                jmeta.nlink() == 1
            }
            #[cfg(not(unix))]
            {
                query_file_identity(&jpath).is_some_and(|id| id.number_of_links == 1)
            }
        };
        if !nlink_ok {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký khu chờ '{}': tệp là hardlink (nlink != 1)",
                    jpath.display()
                ),
            );
            continue;
        }
        if jmeta.len() > MAX_STAGING_JOURNAL_BYTES {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký khu chờ '{}': kích thước {} vượt giới hạn {} byte",
                    jpath.display(),
                    jmeta.len(),
                    MAX_STAGING_JOURNAL_BYTES
                ),
            );
            continue;
        }

        // Bounded read so a concurrent writer cannot cause unbounded allocation
        let raw = match std::fs::File::open(&jpath) {
            Ok(f) => {
                let mut buf = Vec::new();
                if f.take(MAX_STAGING_JOURNAL_BYTES + 1)
                    .read_to_end(&mut buf)
                    .is_err()
                {
                    continue;
                }
                if (buf.len() as u64) > MAX_STAGING_JOURNAL_BYTES {
                    record_rejection(
                        &mut report,
                        format!(
                            "Từ chối nhật ký khu chờ '{}': nội dung vượt quá {} byte",
                            jpath.display(),
                            MAX_STAGING_JOURNAL_BYTES
                        ),
                    );
                    continue;
                }
                buf
            }
            Err(_) => continue,
        };

        let Ok(mut journal) = serde_json::from_slice::<StagingJournalEntry>(&raw) else {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký khu chờ '{}': JSON không hợp lệ",
                    jpath.display()
                ),
            );
            continue;
        };

        // 3. NEVER trust JSON `staged_path` for deletion/rename:
        //    `journal.id` must match regex AND filename stem, and `journal.staged_path` must equal
        //    exact `staging_dir/<id>` (no `..`). All subsequent FS ops use `expected_staged_path`.
        if !is_valid_staging_unit_id(&journal.id) || journal.id != stem_id {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký '{}': id '{}' không hợp lệ hoặc không khớp tên tệp '{}'",
                    jpath.display(),
                    journal.id,
                    stem_id
                ),
            );
            continue;
        }

        let expected_staged_path = staging_dir.join(&journal.id);
        let staged_path_matches = !path_has_parent_dir(&journal.staged_path)
            && (journal.staged_path == expected_staged_path
                || canon_staging_dir
                    .as_ref()
                    .is_some_and(|c| journal.staged_path == c.join(&journal.id)));
        if !staged_path_matches {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký '{}': staged_path '{}' không khớp '{}'",
                    jpath.display(),
                    journal.staged_path.display(),
                    expected_staged_path.display()
                ),
            );
            continue;
        }

        // 4. Require recorded `(staged_dev, staged_ino)` in journal.
        let (Some(exp_dev), Some(exp_ino)) = (journal.staged_dev, journal.staged_ino) else {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối nhật ký '{}': thiếu trường dev/inode của đơn vị trong khu chờ",
                    jpath.display()
                ),
            );
            continue;
        };

        // 5. Inspect `expected_staged_path` (NEVER `journal.staged_path`).
        let smeta = match std::fs::symlink_metadata(&expected_staged_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Rename never happened (crash in `Staging`) or staged tree already deleted.
                let _ = std::fs::remove_file(&jpath);
                report.stale_journals_removed += 1;
                continue;
            }
            Err(e) => {
                record_rejection(
                    &mut report,
                    format!(
                        "Từ chối đơn vị '{}': không đọc được siêu dữ liệu ({e})",
                        expected_staged_path.display()
                    ),
                );
                continue;
            }
        };

        if smeta.file_type().is_symlink() || !smeta.is_dir() {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối đơn vị '{}': là symlink hoặc không phải thư mục thật",
                    expected_staged_path.display()
                ),
            );
            continue;
        }
        if let Err(why) = check_owner_matches(&expected_staged_path, expected_uid) {
            record_rejection(
                &mut report,
                format!("Từ chối đơn vị '{}': {why}", expected_staged_path.display()),
            );
            continue;
        }
        if crosses_device(&smeta, staging_dev) {
            record_rejection(
                &mut report,
                format!(
                    "Từ chối đơn vị '{}': khác thiết bị (st_dev) với khu chờ",
                    expected_staged_path.display()
                ),
            );
            continue;
        }
        match query_file_identity(&expected_staged_path) {
            Some(live_id) if live_id.volume_serial == exp_dev && live_id.file_index == exp_ino => {}
            _ => {
                record_rejection(
                    &mut report,
                    format!(
                        "Từ chối đơn vị '{}': dev/inode thực tế không khớp nhật ký (kỳ vọng {exp_dev}:{exp_ino})",
                        expected_staged_path.display()
                    ),
                );
                continue;
            }
        }

        // 6. Execute state-machine recovery using ONLY verified `expected_staged_path`.
        match (journal.mode, journal.state) {
            (StagingMode::Direct, _) | (_, StagingState::Archived | StagingState::Deleting) => {
                let delete_dev = device_id(&smeta).or(Some(exp_dev));
                if safe_remove_path_no_follow(&expected_staged_path, delete_dev).is_ok()
                    && std::fs::symlink_metadata(&expected_staged_path).is_err()
                {
                    let _ = std::fs::remove_file(&jpath);
                    report.completed_deletions += 1;
                }
            }
            (StagingMode::Vault, StagingState::Staging | StagingState::Staged) => {
                let verified_orig = match validate_restore_original_path(
                    &journal.original_path,
                    staging_dir,
                    exp_dev,
                    &allowed_roots,
                ) {
                    Ok(p) => p,
                    Err(why) => {
                        record_rejection(
                            &mut report,
                            format!(
                                "Giữ đơn vị tại khu chờ '{}' vì đường dẫn khôi phục không hợp lệ: {why}",
                                expected_staged_path.display()
                            ),
                        );
                        continue;
                    }
                };

                if let Some(tool) = journal.dev_tool {
                    if busy_status.is_tool_busy(tool) {
                        report.deferred_busy += 1;
                        continue;
                    }
                }

                if std::fs::symlink_metadata(&verified_orig).is_ok() {
                    if journal.unit_kind == Some(CleanUnitKind::RegenerableDevCache) {
                        let delete_dev = device_id(&smeta).or(Some(exp_dev));
                        if safe_remove_path_no_follow(&expected_staged_path, delete_dev).is_ok()
                            && std::fs::symlink_metadata(&expected_staged_path).is_err()
                        {
                            let _ = std::fs::remove_file(&jpath);
                            let msg = format!(
                                "Khôi phục khu chờ: {} đã được công cụ tạo lại; đã xoá bản cache tái tạo dư thừa {}",
                                verified_orig.display(),
                                expected_staged_path.display()
                            );
                            tracing::info!("{msg}");
                            report.redundant_conflicts_cleaned += 1;
                            report.completed_deletions += 1;
                            report.cleaned_conflicts.push(msg);
                        }
                    } else {
                        let msg = format!(
                            "Khôi phục khu chờ sau mất điện: {} đã tồn tại (không phải cache tái tạo được) nên giữ bản cũ trong {}",
                            verified_orig.display(),
                            expected_staged_path.display()
                        );
                        journal.state = StagingState::RestoreConflict;
                        journal.note = Some(msg.clone());
                        let _ = write_staging_journal(&jpath, &journal);
                        report.conflicts.push(msg);
                    }
                } else {
                    let default_client = NativeBreakawayHelperClient;
                    let client: &dyn BreakawayHelperClient =
                        opts.breakaway_client.as_deref().unwrap_or(&default_client);
                    match restore_staged_dir_to_real_path(
                        &expected_staged_path,
                        &verified_orig,
                        Some(exp_dev),
                        Some(exp_ino),
                        client,
                    ) {
                        Ok(()) => {
                            let _ = std::fs::remove_file(&jpath);
                            report.restored_to_original += 1;
                        }
                        Err(e) => {
                            let msg = format!(
                                "Khôi phục khu chờ thất bại cho {} ({e}); giữ tại {}",
                                verified_orig.display(),
                                expected_staged_path.display()
                            );
                            journal.state = StagingState::RestoreConflict;
                            journal.note = Some(msg.clone());
                            let _ = write_staging_journal(&jpath, &journal);
                            report.conflicts.push(msg);
                        }
                    }
                }
            }
            (_, StagingState::RestoreConflict) => {
                if journal.unit_kind == Some(CleanUnitKind::RegenerableDevCache) {
                    match validate_restore_original_path(
                        &journal.original_path,
                        staging_dir,
                        exp_dev,
                        &allowed_roots,
                    ) {
                        Ok(verified_orig) => {
                            if std::fs::symlink_metadata(&verified_orig).is_ok() {
                                let delete_dev = device_id(&smeta).or(Some(exp_dev));
                                if safe_remove_path_no_follow(&expected_staged_path, delete_dev)
                                    .is_ok()
                                    && std::fs::symlink_metadata(&expected_staged_path).is_err()
                                {
                                    let _ = std::fs::remove_file(&jpath);
                                    let msg = format!(
                                        "Đã dọn RestoreConflict dư thừa cho cache tái tạo {}: chỗ cũ đã được tạo lại, xoá {}",
                                        verified_orig.display(),
                                        expected_staged_path.display()
                                    );
                                    tracing::info!("{msg}");
                                    report.redundant_conflicts_cleaned += 1;
                                    report.completed_deletions += 1;
                                    report.cleaned_conflicts.push(msg);
                                }
                            } else {
                                let msg = journal.note.clone().unwrap_or_else(|| {
                                    format!(
                                        "Đơn vị {} đang ở trạng thái RestoreConflict trong khu chờ {}",
                                        verified_orig.display(),
                                        expected_staged_path.display()
                                    )
                                });
                                report.conflicts.push(msg);
                            }
                        }
                        Err(why) => {
                            record_rejection(
                                &mut report,
                                format!(
                                    "Giữ RestoreConflict tại '{}' vì đường dẫn gốc không hợp lệ: {why}",
                                    expected_staged_path.display()
                                ),
                            );
                        }
                    }
                } else {
                    let msg = journal.note.clone().unwrap_or_else(|| {
                        format!(
                            "Đơn vị {} (không phải cache tái tạo được) đang chờ xử lý trong khu chờ {}",
                            journal.original_path.display(),
                            expected_staged_path.display()
                        )
                    });
                    report.conflicts.push(msg);
                }
            }
        }
    }

    report
}

/// Deletes a non-directory unit (single file or ordered multi-file unit such as npm
/// `index-v5` + `content-v2`). For multi-file units, `unit.files[0]` (the index) is ALWAYS
/// removed FIRST; if it fails, content blobs are NEVER touched!
fn direct_delete_file_unit(unit: &CleanUnit, expected_dev: Option<u64>) -> Result<(u64, u64)> {
    let mut files_removed = 0u64;
    let mut bytes_removed = 0u64;
    let is_multi_file = unit.files.len() > 1;

    for (idx, cand) in unit.files.iter().enumerate() {
        match safe_remove_path_no_follow(&cand.path, expected_dev) {
            Ok((fc, bc)) => {
                files_removed += fc;
                bytes_removed = bytes_removed.saturating_add(bc);
            }
            Err(e) => {
                // Multi-file unit (e.g. npm index-v5 + content-v2): NEVER delete content blobs
                // if the primary index file could not be removed!
                if is_multi_file && idx == 0 {
                    return Err(e);
                }
            }
        }
    }
    Ok((files_removed, bytes_removed))
}

/// Core shared drive-quota enforcement engine operating on atomic [`CleanUnit`]s.
#[allow(clippy::too_many_arguments)]
pub fn enforce_drive_quota_units(
    quota: &DriveQuota,
    units: Vec<CleanUnit>,
    vault_dir: &Path,
    db: &GreenCDb,
    volume_api: &dyn Win32VolumeApi,
    lock_probe: &dyn FileLockProbe,
    dev_tool_probe: &dyn DevToolProbe,
    opts: &GuardRunOptions,
    ctrl: &TaskControl,
) -> Result<GuardDriveOutcome> {
    if ctrl.is_cancelled() {
        return Err(GreenCError::Cancelled);
    }

    let drive_root = Path::new(&quota.drive_root);
    let vault_same_drive = is_same_volume(vault_dir, drive_root);
    let mut outcome = GuardDriveOutcome::base(quota, vault_same_drive);
    let atime_rel = detect_atime_reliability(drive_root);
    outcome.atime_unreliable = !atime_rel.is_reliable();

    // 1. Measure free space BEFORE cleanup.
    let (total_before, free_before) = match volume_api.get_disk_free_space(drive_root) {
        Ok((t, f)) if t > 0 => (t, f),
        _ => {
            outcome.status = "unavailable".to_string();
            outcome.level = "unavailable".to_string();
            outcome.target_reached = false;
            outcome
                .reasons
                .push(format!("Không đọc được dung lượng ổ {}", quota.drive_root));
            return Ok(outcome);
        }
    };

    outcome.total_bytes = total_before;
    outcome.free_before_bytes = free_before;
    outcome.free_after_bytes = free_before;
    outcome.usage_before_ratio = quota.usage_ratio(total_before, free_before);
    outcome.usage_after_ratio = outcome.usage_before_ratio;
    let level = quota.level(total_before, free_before);
    outcome.level = level.as_str().to_string();
    let bytes_needed = quota.bytes_to_reclaim(total_before, free_before);
    outcome.bytes_needed = bytes_needed;

    if level != QuotaLevel::Critical || bytes_needed == 0 {
        outcome.status = level.as_str().to_string();
        outcome.target_reached = bytes_needed == 0;
        if !opts.dry_run && level == QuotaLevel::Ok {
            let _ = db.clear_guard_cooldown(&quota.drive_root);
        }
        return Ok(outcome);
    }

    if !quota.auto_clean {
        outcome.status = "auto_clean_disabled".to_string();
        outcome.target_reached = false;
        outcome
            .reasons
            .push("Tự động dọn đang tắt cho ổ này".to_string());
        return Ok(outcome);
    }

    // 2. Check persistent DB cooldown (unless dry-run or explicitly bypassed).
    let prev_cooldown = db.get_guard_cooldown(&quota.drive_root).ok().flatten();
    if let Some(ref cd) = prev_cooldown {
        outcome.consecutive_ineffective = cd.consecutive_ineffective;
        if !opts.dry_run && !opts.ignore_cooldown {
            if let Some(until) = cd.cooldown_until {
                if until > opts.now_utc {
                    outcome.status = "skipped_cooldown".to_string();
                    outcome.cooldown_active = true;
                    outcome.cooldown_until = Some(until);
                    outcome.target_reached = false;
                    outcome.reasons.push(format!(
                        "Đang trong thời gian chờ (cooldown) đến {} (lần không hiệu quả liên tiếp: {})",
                        until.format("%H:%M UTC"),
                        cd.consecutive_ineffective
                    ));
                    return Ok(outcome);
                }
            }
        }
    }

    // 3. Probe busy developer tools once (seeding the <= 1s cache).
    let mut busy_cache =
        CachedBusyToolProbe::new(dev_tool_probe, &opts.extra_busy_tools, BUSY_TOOL_CACHE_TTL);
    let busy_status = busy_cache.get_fresh();
    outcome.busy_tools_unknown = busy_status.is_unknown();
    outcome.busy_tools = busy_status
        .busy_tools()
        .into_iter()
        .map(|t| t.as_str().to_string())
        .collect();

    // 3b. Recover any leftover staged units from an earlier crash or power loss
    //     (strictly checking ONLY user-owned allowed staging locations, NEVER drive_root/.greenc-staging).
    if !opts.dry_run {
        for st_dir in candidate_recovery_staging_dirs(
            drive_root,
            vault_dir,
            opts.staging_dir.as_deref(),
            opts.staging_home_override.as_deref(),
        ) {
            let rec = recover_staging_dir(&st_dir, &busy_status);
            for conflict in rec.conflicts {
                outcome.staging_warnings.push(conflict.clone());
                if !outcome.reasons.contains(&conflict) {
                    outcome.reasons.push(conflict);
                }
            }
        }
    }

    // 4. Filter units strictly to this drive & non-busy tools & non-forbidden paths.
    //    Zalo units (`zalo_*`) are only included in guard runs when explicitly opted in via
    //    `opts.include_zalo_auto_clean` (disabled by default).
    let mut eligible: Vec<CleanUnit> = units
        .into_iter()
        .filter(|u| opts.include_zalo_auto_clean || !u.unit_label.starts_with("zalo_"))
        .filter(|u| is_same_volume(&u.root_path, drive_root))
        .filter(|u| match u.dev_tool {
            Some(tool) => !busy_status.is_tool_busy(tool),
            None => true,
        })
        .filter(|u| {
            u.kind != CleanUnitKind::RegenerableDevCache
                || !is_forbidden_dev_cache_path(&u.root_path)
        })
        .filter(|u| !u.files.is_empty() && u.total_bytes > 0)
        .collect();

    // 5. Sort: regenerable dev caches first, then oldest-unused first, then largest first.
    eligible.sort_by(|a, b| {
        unit_priority(a)
            .cmp(&unit_priority(b))
            .then_with(|| a.last_used.cmp(&b.last_used))
            .then_with(|| b.total_bytes.cmp(&a.total_bytes))
    });

    // 6. Select just enough units to reach `target_ratio`, accounting for same-drive Vault overhead.
    let mut selected: Vec<(CleanUnit, bool)> = Vec::new();
    let mut acc_expected_net = 0u64;
    let mut skipped_incompressible_same_drive = false;

    for unit in eligible {
        if acc_expected_net >= bytes_needed {
            break;
        }
        let will_direct = quota.regenerable_cache_mode == RegenerableCacheMode::Direct
            && unit.is_direct_delete_eligible();
        let expected_net = estimate_unit_net_bytes(&unit, vault_same_drive, will_direct);
        if vault_same_drive && !will_direct && expected_net == 0 {
            skipped_incompressible_same_drive = true;
            continue;
        }
        acc_expected_net = acc_expected_net.saturating_add(expected_net);
        selected.push((unit, will_direct));
    }

    // 7. Dry-run returns planned totals without mutating disk or DB cooldowns.
    if opts.dry_run {
        outcome.status = "dry_run".to_string();
        outcome.cleaned_units = selected.len();
        outcome.cleaned_files = selected.iter().map(|(u, _)| u.files.len() as u64).sum();
        outcome.logical_bytes = selected.iter().map(|(u, _)| u.total_bytes).sum();
        outcome.direct_deleted_bytes = selected
            .iter()
            .filter(|(_, d)| *d)
            .map(|(u, _)| u.total_bytes)
            .sum();
        let vault_logical: u64 = selected
            .iter()
            .filter(|(_, d)| !*d)
            .map(|(u, _)| u.total_bytes)
            .sum();
        let vault_est_compression_savings: u64 = selected
            .iter()
            .filter(|(_, d)| !*d)
            .map(|(u, _)| estimate_unit_net_bytes(u, true, false))
            .sum();
        outcome.vault_bytes = vault_logical.saturating_sub(vault_est_compression_savings);
        outcome.estimated_net_bytes = acc_expected_net as i64;
        outcome.target_reached = acc_expected_net >= bytes_needed;
        populate_miss_reasons(
            &mut outcome,
            &busy_status,
            skipped_incompressible_same_drive,
            opts,
            ctrl,
        );
        return Ok(outcome);
    }

    // 8. Execute Direct deletions (ONLY verified regenerable dev cache units when mode == Direct).
    let mut vault_units: Vec<CleanUnit> = Vec::new();
    let mut stopped_tool: Option<DevTool> = None;
    let mut stopped_unknown: Option<String> = None;

    for (unit, will_direct) in selected {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        if !will_direct {
            vault_units.push(unit);
            continue;
        }

        // Strict double-guard: NEVER direct-delete non-regenerable data.
        debug_assert!(unit.is_direct_delete_eligible());
        if !unit.is_direct_delete_eligible()
            || quota.regenerable_cache_mode != RegenerableCacheMode::Direct
        {
            vault_units.push(unit);
            continue;
        }

        match verify_unit_toctou(&unit, drive_root, opts.now_sys, lock_probe, &mut busy_cache) {
            ToctouVerdict::Ok => {
                let is_dev_dir_unit = unit.kind == CleanUnitKind::RegenerableDevCache
                    && std::fs::symlink_metadata(&unit.root_path)
                        .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir());

                if is_dev_dir_unit {
                    // Fresh busy-tool probe right before atomic rename!
                    if let Some(tool) = unit.dev_tool {
                        let fresh = busy_cache.get_fresh();
                        match fresh {
                            BusyToolsStatus::Unknown { reason } => {
                                stopped_unknown = Some(reason);
                                outcome.busy_tools_unknown = true;
                                break;
                            }
                            BusyToolsStatus::Known(_) if fresh.is_tool_busy(tool) => {
                                stopped_tool = Some(tool);
                                break;
                            }
                            BusyToolsStatus::Known(_) => {}
                        }
                    }

                    match stage_directory_unit(
                        unit,
                        StagingMode::Direct,
                        drive_root,
                        vault_dir,
                        opts.staging_dir.as_deref(),
                        opts.staging_home_override.as_deref(),
                        opts.now_utc,
                        opts.now_sys,
                    ) {
                        Ok(mut staged) => {
                            if let Ok((files_del, bytes_del)) =
                                finalize_staged_dir_unit(&mut staged)
                            {
                                if files_del > 0 {
                                    outcome.cleaned_units += 1;
                                    outcome.cleaned_files =
                                        outcome.cleaned_files.saturating_add(files_del);
                                    outcome.logical_bytes =
                                        outcome.logical_bytes.saturating_add(bytes_del);
                                    outcome.direct_deleted_bytes =
                                        outcome.direct_deleted_bytes.saturating_add(bytes_del);
                                    let _ = db.record_direct_delete(&DirectDeleteRecord {
                                        id: None,
                                        timestamp: opts.now_utc,
                                        drive_root: quota.drive_root.clone(),
                                        profile_id: staged.unit.profile_id.clone(),
                                        unit_kind: staged.unit.unit_label.clone(),
                                        unit_path: staged.journal.original_path.clone(),
                                        file_count: files_del,
                                        logical_bytes: bytes_del,
                                        reason: "direct regenerable developer cache cleanup"
                                            .to_string(),
                                    });
                                }
                            }
                        }
                        Err(why) => {
                            tracing::warn!("Skipped atomic direct stage: {why}");
                            outcome.staging_warnings.push(why.clone());
                            if !outcome.reasons.contains(&why) {
                                outcome.reasons.push(why);
                            }
                        }
                    }
                } else if unit.kind == CleanUnitKind::RegenerableAppCache {
                    let expected_dev = unit.root_identity.map(|id| id.volume_serial);
                    let mut files_del = 0u64;
                    let mut bytes_del = 0u64;
                    for cand in &unit.files {
                        if let Ok((fc, bc)) = safe_remove_path_no_follow(&cand.path, expected_dev) {
                            files_del = files_del.saturating_add(fc);
                            bytes_del = bytes_del.saturating_add(bc);
                        }
                    }
                    if files_del > 0 {
                        let remove_root = unit.unit_label == "zalo_old_version";
                        crate::zalo_cleanup::prune_empty_dirs_no_follow(
                            &unit.root_path,
                            remove_root,
                        );
                        outcome.cleaned_units += 1;
                        outcome.cleaned_files = outcome.cleaned_files.saturating_add(files_del);
                        outcome.logical_bytes = outcome.logical_bytes.saturating_add(bytes_del);
                        outcome.direct_deleted_bytes =
                            outcome.direct_deleted_bytes.saturating_add(bytes_del);
                        let _ = db.record_direct_delete(&DirectDeleteRecord {
                            id: None,
                            timestamp: opts.now_utc,
                            drive_root: quota.drive_root.clone(),
                            profile_id: unit.profile_id.clone(),
                            unit_kind: unit.unit_label.clone(),
                            unit_path: unit.root_path.clone(),
                            file_count: files_del,
                            logical_bytes: bytes_del,
                            reason: "direct regenerable app cache/version cleanup".to_string(),
                        });
                    }
                } else {
                    let expected_dev = unit.root_identity.map(|id| id.volume_serial);
                    if let Ok((files_del, bytes_del)) = direct_delete_file_unit(&unit, expected_dev)
                    {
                        if files_del > 0 {
                            outcome.cleaned_units += 1;
                            outcome.cleaned_files = outcome.cleaned_files.saturating_add(files_del);
                            outcome.logical_bytes = outcome.logical_bytes.saturating_add(bytes_del);
                            outcome.direct_deleted_bytes =
                                outcome.direct_deleted_bytes.saturating_add(bytes_del);
                            let _ = db.record_direct_delete(&DirectDeleteRecord {
                                id: None,
                                timestamp: opts.now_utc,
                                drive_root: quota.drive_root.clone(),
                                profile_id: unit.profile_id.clone(),
                                unit_kind: unit.unit_label.clone(),
                                unit_path: unit.root_path.clone(),
                                file_count: files_del,
                                logical_bytes: bytes_del,
                                reason: "direct regenerable developer cache cleanup".to_string(),
                            });
                        }
                    }
                }
            }
            ToctouVerdict::ToolStarted(tool) => {
                stopped_tool = Some(tool);
                break;
            }
            ToctouVerdict::StatusUnknown(reason) => {
                stopped_unknown = Some(reason);
                outcome.busy_tools_unknown = true;
                break;
            }
            ToctouVerdict::Skip(why) => {
                tracing::debug!(
                    "TOCTOU skipped direct unit {}: {why}",
                    unit.root_path.display()
                );
            }
        }
    }

    // 9. Execute Vault archiving + atomic staging (for dir units) + verified ordered unlinking
    //    (for file / multi-file units), reconciling ZIP + manifest + DB with actual deletions.
    if stopped_tool.is_none() && stopped_unknown.is_none() && !vault_units.is_empty() {
        let mut staged_dir_units: Vec<StagedDirUnit> = Vec::new();
        let mut unstaged_file_units: Vec<CleanUnit> = Vec::new();
        let mut archive_candidates: Vec<CleanCandidate> = Vec::new();
        let mut original_path_overrides: HashMap<PathBuf, PathBuf> = HashMap::new();

        for unit in vault_units {
            if ctrl.is_cancelled() {
                // Roll back any already-staged directory units before returning Cancelled.
                for s in &mut staged_dir_units {
                    match rollback_staged_dir_unit(s) {
                        StagingRollbackOutcome::Restored => {}
                        StagingRollbackOutcome::DiscardedRedundant(msg) => {
                            tracing::info!("{msg}");
                        }
                        StagingRollbackOutcome::KeptInStaging(w) => {
                            outcome.staging_warnings.push(w);
                        }
                    }
                }
                return Err(GreenCError::Cancelled);
            }
            match verify_unit_toctou(&unit, drive_root, opts.now_sys, lock_probe, &mut busy_cache) {
                ToctouVerdict::Ok => {
                    let is_dev_dir_unit = unit.kind == CleanUnitKind::RegenerableDevCache
                        && std::fs::symlink_metadata(&unit.root_path)
                            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir());

                    if is_dev_dir_unit {
                        // Fresh busy-tool probe immediately before atomic rename!
                        if let Some(tool) = unit.dev_tool {
                            let fresh = busy_cache.get_fresh();
                            match fresh {
                                BusyToolsStatus::Unknown { reason } => {
                                    stopped_unknown = Some(reason);
                                    outcome.busy_tools_unknown = true;
                                    break;
                                }
                                BusyToolsStatus::Known(_) if fresh.is_tool_busy(tool) => {
                                    stopped_tool = Some(tool);
                                    break;
                                }
                                BusyToolsStatus::Known(_) => {}
                            }
                        }

                        match stage_directory_unit(
                            unit,
                            StagingMode::Vault,
                            drive_root,
                            vault_dir,
                            opts.staging_dir.as_deref(),
                            opts.staging_home_override.as_deref(),
                            opts.now_utc,
                            opts.now_sys,
                        ) {
                            Ok(staged) => {
                                archive_candidates.extend(staged.staged_candidates.clone());
                                original_path_overrides.extend(staged.staged_to_original.clone());
                                staged_dir_units.push(staged);
                            }
                            Err(why) => {
                                tracing::warn!("Skipped atomic vault stage: {why}");
                                outcome.staging_warnings.push(why.clone());
                                if !outcome.reasons.contains(&why) {
                                    outcome.reasons.push(why);
                                }
                            }
                        }
                    } else {
                        archive_candidates.extend(unit.files.clone());
                        unstaged_file_units.push(unit);
                    }
                }
                ToctouVerdict::ToolStarted(tool) => {
                    stopped_tool = Some(tool);
                    break;
                }
                ToctouVerdict::StatusUnknown(reason) => {
                    stopped_unknown = Some(reason);
                    outcome.busy_tools_unknown = true;
                    break;
                }
                ToctouVerdict::Skip(why) => {
                    tracing::debug!(
                        "TOCTOU skipped vault unit {}: {why}",
                        unit.root_path.display()
                    );
                }
            }
        }

        // If a tool started mid-staging and we have NO unstaged units to archive, roll back or
        // archive already-staged units? Staged units were ALREADY verified & atomically removed
        // before the tool started; archiving them preserves user rollback capability cleanly!
        if !archive_candidates.is_empty() {
            let snapshots: HashMap<PathBuf, (u64, SystemTime)> = unstaged_file_units
                .iter()
                .flat_map(|u| u.files.iter())
                .map(|c| {
                    let snap = std::fs::metadata(&c.path)
                        .map(|m| (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
                        .unwrap_or((c.size_bytes, c.mtime));
                    (c.path.clone(), snap)
                })
                .collect();

            let pipeline = ArchiverPipeline::new(ArchiveOptions {
                vault_dir: vault_dir.to_path_buf(),
                retention_days: opts.retention_days,
                min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                ..Default::default()
            });

            let archive_res = pipeline.archive_candidates_mapped(
                quota.id.unwrap_or(0),
                &format!("Quota ổ {}", quota.drive_root),
                &archive_candidates,
                &original_path_overrides,
                volume_api,
                ctrl,
                |_| {},
            );

            let mut manifest = match archive_res {
                Ok(m) => m,
                Err(e) => {
                    // Archiving failed: restore staged dir units if original path does not exist,
                    // otherwise keep safely in staging and warn.
                    for s in &mut staged_dir_units {
                        match rollback_staged_dir_unit(s) {
                            StagingRollbackOutcome::Restored => {}
                            StagingRollbackOutcome::DiscardedRedundant(msg) => {
                                tracing::info!("{msg}");
                            }
                            StagingRollbackOutcome::KeptInStaging(w) => {
                                tracing::warn!("{w}");
                                outcome.staging_warnings.push(w);
                            }
                        }
                    }
                    return Err(e);
                }
            };

            let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
            let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));

            // O(1) lookup map from logical original_path -> FileManifestItem (eliminates O(n^2) scan!).
            let manifest_by_path: HashMap<PathBuf, FileManifestItem> = manifest
                .files
                .iter()
                .map(|item| (item.original_path.clone(), item.clone()))
                .collect();

            let mut removed_orig_paths: HashSet<PathBuf> = HashSet::new();
            let mut removed_units_count = 0usize;
            let mut removed_files_count = 0u64;
            let mut removed_logical_bytes = 0u64;

            // 9a. All staged directory units have already atomically left their original locations!
            for staged in &staged_dir_units {
                let mut unit_files = 0u64;
                let mut unit_bytes = 0u64;
                for orig in staged.staged_to_original.values() {
                    if let Some(item) = manifest_by_path.get(orig) {
                        if removed_orig_paths.insert(orig.clone()) {
                            unit_files += 1;
                            unit_bytes = unit_bytes.saturating_add(item.size_bytes);
                        }
                    }
                }
                if unit_files > 0 {
                    removed_units_count += 1;
                    removed_files_count = removed_files_count.saturating_add(unit_files);
                    removed_logical_bytes = removed_logical_bytes.saturating_add(unit_bytes);
                }
            }

            // 9b. Unlink unstaged file / multi-file units after a fresh post-archive busy check.
            if !unstaged_file_units.is_empty() {
                let has_dev_unstaged = unstaged_file_units.iter().any(|u| u.dev_tool.is_some());
                let post_archive_busy = if has_dev_unstaged {
                    busy_cache.get_fresh()
                } else {
                    BusyToolsStatus::idle()
                };

                if let BusyToolsStatus::Unknown { ref reason } = post_archive_busy {
                    outcome.busy_tools_unknown = true;
                    if stopped_unknown.is_none() {
                        stopped_unknown = Some(reason.clone());
                    }
                }

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

                for unit in &unstaged_file_units {
                    if let Some(tool) = unit.dev_tool {
                        if post_archive_busy.is_unknown() {
                            continue;
                        }
                        if post_archive_busy.is_tool_busy(tool) {
                            if stopped_tool.is_none() {
                                stopped_tool = Some(tool);
                            }
                            continue;
                        }
                    }

                    let is_ordered_dev_multi_file =
                        unit.kind == CleanUnitKind::RegenerableDevCache && unit.files.len() > 1;
                    let mut unit_deleted_any = false;

                    for (idx, cand) in unit.files.iter().enumerate() {
                        let Some(item) = manifest_by_path.get(&cand.path) else {
                            if is_ordered_dev_multi_file && idx == 0 {
                                break;
                            }
                            continue;
                        };
                        let res = cleaner
                            .unlink_archived_items(std::slice::from_ref(item), &snapshots)?;
                        if res.files_deleted > 0 {
                            unit_deleted_any = true;
                            removed_orig_paths.insert(item.original_path.clone());
                            removed_files_count =
                                removed_files_count.saturating_add(res.files_deleted);
                            removed_logical_bytes =
                                removed_logical_bytes.saturating_add(res.bytes_freed);
                        } else if is_ordered_dev_multi_file && idx == 0 {
                            // Never unlink content-v2 blobs if index-v5 was skipped/failed!
                            break;
                        }
                    }

                    if unit_deleted_any {
                        if unit.kind == CleanUnitKind::RegenerableAppCache {
                            let remove_root = unit.unit_label == "zalo_old_version";
                            crate::zalo_cleanup::prune_empty_dirs_no_follow(
                                &unit.root_path,
                                remove_root,
                            );
                        }
                        removed_units_count += 1;
                    }
                }
            }

            // 9c. Reconcile ZIP + manifest + DB strictly against `removed_orig_paths`:
            // - 0 originals removed -> delete orphan ZIP + manifest, create NO DB row!
            // - Partial originals removed -> rewrite ZIP & manifest to contain ONLY removed files!
            // - All originals removed -> persist manifest & DB, then finalize staged dirs!
            if removed_orig_paths.is_empty() {
                let _ = safe_remove_vault_artifact(&zip_p, vault_dir, ".zip");
                let _ = safe_remove_vault_artifact(&man_p, vault_dir, ".manifest.json");
                outcome.vault_zip = None;
                outcome.vault_bytes = 0;
            } else {
                if removed_orig_paths.len() < manifest.files.len() {
                    manifest
                        .files
                        .retain(|item| removed_orig_paths.contains(&item.original_path));
                    let kept_arcnames: HashSet<String> = manifest
                        .files
                        .iter()
                        .map(|item| item.rel_arcname.clone())
                        .collect();
                    let (new_comp_size, new_sha) =
                        ArchiverPipeline::filter_zip_archive(&zip_p, &kept_arcnames)?;
                    manifest.total_size = manifest.files.iter().map(|i| i.size_bytes).sum();
                    manifest.compressed_size = new_comp_size;
                    manifest.sha256 = new_sha;
                    manifest.write_to_file(&man_p)?;
                }

                if let Err(e) = manifest.persist_to_db(db, &zip_p, &man_p) {
                    // DB persistence failed: remove orphan files and roll back staged dirs.
                    let _ = safe_remove_vault_artifact(&zip_p, vault_dir, ".zip");
                    let _ = safe_remove_vault_artifact(&man_p, vault_dir, ".manifest.json");
                    for s in &mut staged_dir_units {
                        match rollback_staged_dir_unit(s) {
                            StagingRollbackOutcome::Restored => {}
                            StagingRollbackOutcome::DiscardedRedundant(msg) => {
                                tracing::info!("{msg}");
                            }
                            StagingRollbackOutcome::KeptInStaging(w) => {
                                outcome.staging_warnings.push(w);
                            }
                        }
                    }
                    return Err(e);
                }

                // Mark staged directory units archived & delete them cleanly from staging.
                for s in &mut staged_dir_units {
                    s.journal.state = StagingState::Archived;
                    let _ = write_staging_journal(&s.journal_path, &s.journal);
                    let _ = finalize_staged_dir_unit(s);
                }

                let zip_size = std::fs::metadata(&zip_p).map(|m| m.len()).unwrap_or(0);
                let man_size = std::fs::metadata(&man_p).map(|m| m.len()).unwrap_or(0);
                outcome.vault_bytes = zip_size.saturating_add(man_size);
                outcome.vault_zip = Some(zip_p.clone());
                outcome.cleaned_units += removed_units_count;
                outcome.cleaned_files = outcome.cleaned_files.saturating_add(removed_files_count);
                outcome.logical_bytes = outcome.logical_bytes.saturating_add(removed_logical_bytes);

                if quota.auto_backup {
                    match quota.backup_dir.as_deref() {
                        Some(dir) => match backup_vault_archive(&zip_p, drive_root, dir) {
                            Ok(dest) => outcome.backed_up_to = Some(dest),
                            Err(e) => outcome.backup_problem = Some(e),
                        },
                        None => {
                            outcome.backup_problem = Some("Chưa chọn thư mục sao lưu".to_string())
                        }
                    }
                }
            }
        }
    }

    if let Some(tool) = stopped_tool {
        outcome.stopped_mid_run_tool = Some(tool.as_str().to_string());
        outcome.reasons.push(format!(
            "Đã dừng giữa chừng vì {} bắt đầu chạy",
            tool.display_name_vi()
        ));
    } else if let Some(ref unknown_reason) = stopped_unknown {
        outcome.busy_tools_unknown = true;
        outcome.stopped_mid_run_tool = Some("unknown".to_string());
        outcome.reasons.push(format!(
            "Đã dừng giữa chừng vì trạng thái tiến trình công cụ không xác định ({unknown_reason})"
        ));
    }

    // 10. Measure free space AFTER cleanup -> actual `net_freed_bytes` & `target_reached`.
    let (total_after, free_after) = volume_api
        .get_disk_free_space(drive_root)
        .unwrap_or((total_before, free_before));
    outcome.free_after_bytes = free_after;
    outcome.usage_after_ratio = quota.usage_ratio(total_after, free_after);
    outcome.level = quota.level(total_after, free_after).as_str().to_string();
    outcome.net_freed_bytes = (free_after as i64).saturating_sub(free_before as i64);
    outcome.estimated_net_bytes =
        (outcome.logical_bytes as i64).saturating_sub(if vault_same_drive {
            outcome.vault_bytes as i64
        } else {
            0
        });
    outcome.target_reached = quota.bytes_to_reclaim(total_after, free_after) == 0;

    let min_effective_net = ((bytes_needed as f64) * MIN_EFFECTIVE_NET_RATIO).ceil() as i64;
    let made_progress = outcome.net_freed_bytes >= min_effective_net;
    let deferred_for_busy =
        busy_status.is_any_busy() || stopped_tool.is_some() || stopped_unknown.is_some();

    if outcome.target_reached && (made_progress || bytes_needed == 0) {
        outcome.status = "cleaned".to_string();
        outcome.consecutive_ineffective = 0;
        outcome.cooldown_until = None;
        let _ = db.clear_guard_cooldown(&quota.drive_root);
    } else if made_progress {
        // Meaningful progress (>= 5% of needed bytes freed): fixed 30m cooldown, NEVER escalate!
        populate_miss_reasons(
            &mut outcome,
            &busy_status,
            skipped_incompressible_same_drive,
            opts,
            ctrl,
        );
        let cd_dur = chrono::Duration::minutes(FIXED_RETRY_COOLDOWN_MINUTES);
        let until = opts.now_utc + cd_dur;

        outcome.consecutive_ineffective = 0;
        outcome.cooldown_until = Some(until);
        outcome.status = if stopped_tool.is_some() {
            "stopped_tool_started".to_string()
        } else if stopped_unknown.is_some() {
            "stopped_tools_unknown".to_string()
        } else {
            "partial_progress".to_string()
        };

        outcome.warning = Some(format!(
            "Đã giải phóng {:+.1} MiB / cần {:.1} MiB (có tiến triển >= 5%); sẽ tiếp tục sau {} phút lúc {}",
            outcome.net_freed_bytes as f64 / (1024.0 * 1024.0),
            bytes_needed as f64 / (1024.0 * 1024.0),
            cd_dur.num_minutes(),
            until.format("%H:%M UTC")
        ));

        let _ = db.save_guard_cooldown(&GuardCooldownState {
            drive_root: quota.drive_root.clone(),
            consecutive_ineffective: 0,
            cooldown_until: Some(until),
            last_run_at: Some(opts.now_utc),
            last_reason: outcome.reasons.join("; "),
        });
    } else if deferred_for_busy {
        // Deferred because a developer tool is busy / unknown: retry in 30m, NEVER escalate!
        populate_miss_reasons(
            &mut outcome,
            &busy_status,
            skipped_incompressible_same_drive,
            opts,
            ctrl,
        );
        let prev_count = prev_cooldown
            .map(|c| c.consecutive_ineffective)
            .unwrap_or(0);
        let cd_dur = chrono::Duration::minutes(FIXED_RETRY_COOLDOWN_MINUTES);
        let until = opts.now_utc + cd_dur;

        outcome.consecutive_ineffective = prev_count;
        outcome.cooldown_until = Some(until);
        outcome.status = if stopped_tool.is_some() {
            "stopped_tool_started".to_string()
        } else if stopped_unknown.is_some() {
            "stopped_tools_unknown".to_string()
        } else {
            "deferred_busy".to_string()
        };

        outcome.warning = Some(format!(
            "Tạm hoãn dọn ổ {} vì công cụ lập trình đang chạy/không xác định; thử lại sau {} phút lúc {}",
            quota.drive_root,
            cd_dur.num_minutes(),
            until.format("%H:%M UTC")
        ));

        let _ = db.save_guard_cooldown(&GuardCooldownState {
            drive_root: quota.drive_root.clone(),
            consecutive_ineffective: prev_count,
            cooldown_until: Some(until),
            last_run_at: Some(opts.now_utc),
            last_reason: outcome.reasons.join("; "),
        });
    } else {
        // Genuinely ineffective (< 5% freed while tools are idle): escalate 30m -> 2h -> 12h.
        if outcome.target_reached && !made_progress {
            outcome.reasons.push(format!(
                "Thực lợi giải phóng thấp (< 5% lượng cần: {:+.1} MiB / {:.1} MiB)",
                outcome.net_freed_bytes as f64 / (1024.0 * 1024.0),
                bytes_needed as f64 / (1024.0 * 1024.0)
            ));
        }
        populate_miss_reasons(
            &mut outcome,
            &busy_status,
            skipped_incompressible_same_drive,
            opts,
            ctrl,
        );
        let prev_count = prev_cooldown
            .map(|c| c.consecutive_ineffective)
            .unwrap_or(0);
        let next_count = prev_count.saturating_add(1);
        let cd_dur = cooldown_duration_for_failures(next_count);
        let until = opts.now_utc + cd_dur;

        outcome.consecutive_ineffective = next_count;
        outcome.cooldown_until = Some(until);
        outcome.status = "ineffective".to_string();

        let warn_msg = format!(
            "Dọn ổ {} chưa đạt mục tiêu (thực lợi {:+.1} MiB / cần {:.1} MiB); tạm hoãn {} phút đến {}",
            quota.drive_root,
            outcome.net_freed_bytes as f64 / (1024.0 * 1024.0),
            bytes_needed as f64 / (1024.0 * 1024.0),
            cd_dur.num_minutes(),
            until.format("%H:%M UTC")
        );
        outcome.warning = Some(warn_msg);

        let _ = db.save_guard_cooldown(&GuardCooldownState {
            drive_root: quota.drive_root.clone(),
            consecutive_ineffective: next_count,
            cooldown_until: Some(until),
            last_run_at: Some(opts.now_utc),
            last_reason: outcome.reasons.join("; "),
        });
    }

    Ok(outcome)
}

fn populate_miss_reasons(
    outcome: &mut GuardDriveOutcome,
    busy_status: &BusyToolsStatus,
    skipped_incompressible_same_drive: bool,
    opts: &GuardRunOptions,
    ctrl: &TaskControl,
) {
    if outcome.target_reached {
        return;
    }

    if busy_status.is_unknown() {
        let msg =
            "Không xác định được tiến trình công cụ lập trình — bỏ qua cache developer".to_string();
        if !outcome.reasons.contains(&msg) {
            outcome.reasons.push(msg);
        }
    } else {
        for tool in busy_status.busy_tools() {
            let msg = format!("{} đang chạy", tool.display_name_vi());
            if !outcome.reasons.contains(&msg) {
                outcome.reasons.push(msg);
            }
        }
    }

    if outcome.atime_unreliable {
        let msg = "Ổ gắn noatime (atime không tin được): bỏ qua transform phiên bản Gradle đang hoạt động và áp ngưỡng >= 30 ngày cho cache developer".to_string();
        if !outcome.reasons.contains(&msg) {
            outcome.reasons.push(msg);
        }
    }

    if outcome.vault_same_drive
        && outcome.regenerable_cache_mode == RegenerableCacheMode::Vault
        && (skipped_incompressible_same_drive
            || (outcome.logical_bytes > 0 && outcome.net_freed_bytes <= 0))
    {
        outcome.reasons.push(
            "Vault nằm cùng ổ nên giải phóng thực ít; cân nhắc dời Vault hoặc bật regenerable_cache_mode=direct"
                .to_string(),
        );
    }

    // Quick bounded suggest scan to hint large git-ignored build outputs (e.g. ~12 GB app/build).
    if !opts.suggest_roots_on_miss.is_empty() && !ctrl.is_cancelled() {
        let mut s_opts =
            SuggestOptions::with_defaults(opts.suggest_roots_on_miss.clone(), PathBuf::new());
        s_opts.drive_filter = Some(PathBuf::from(&outcome.drive_root));
        s_opts.max_duration = Duration::from_secs(2);
        s_opts.now = opts.now_sys;
        let report = suggest_large_build_outputs_with_busy(&s_opts, busy_status, ctrl);
        if report.total_suggestable_bytes >= 100 * 1024 * 1024 {
            outcome.suggest_hint_bytes = report.total_suggestable_bytes;
            let gb = report.total_suggestable_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            outcome.reasons.push(format!(
                "còn {:.1} GB output build, chạy greenc-cli guard suggest",
                gb
            ));
        }
    }

    if outcome.reasons.is_empty() {
        outcome
            .reasons
            .push("Không còn đủ đơn vị cache cũ an toàn trên ổ này để đạt mục tiêu".to_string());
    }
}
