use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ==========================================
// 1. Volume & Drive Models
// ==========================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveInfo {
    pub root: PathBuf,      // e.g. "C:\\" or "D:\\"
    pub drive_letter: char, // 'C'
    pub label: String,      // e.g. "Windows"
    pub filesystem: String, // e.g. "NTFS"
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_system: bool,
    pub is_eligible: bool,
    pub ineligible_reason: Option<String>,
}

impl DriveInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    pub fn usage_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            self.used_bytes() as f64 / self.total_bytes as f64
        }
    }

    pub fn is_critical(&self, threshold_ratio: f64) -> bool {
        self.usage_ratio() >= threshold_ratio
    }
}

// ==========================================
// 2. Snapshot & Write Tracking Models
// ==========================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailySnapshot {
    pub id: Option<i64>,
    pub drive_letter: String,     // "C:\\"
    pub snapshot_date: NaiveDate, // YYYY-MM-DD
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub net_change_bytes: i64, // Signed daily delta
    pub reclaimed_bytes: u64,  // Bytes freed by GreenC
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WriteReasonCode {
    MediaChat,      // MEDIA_CHAT
    EappBundle,     // EAPP_BUNDLE
    CacheTemp,      // CACHE_TEMP
    OldAppVersion,  // OLD_APP_VERSION
    CrashDump,      // CRASH_DUMP
    LogGrowth,      // LOG_GROWTH
    DbStorage,      // DB_STORAGE
    ContainerVm,    // CONTAINER_VM
    MediaDownload,  // MEDIA_DOWNLOAD
    AppData,        // APP_DATA
    OrphanLeftover, // ORPHAN_LEFTOVER (new in v2)
}

impl WriteReasonCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MediaChat => "MEDIA_CHAT",
            Self::EappBundle => "EAPP_BUNDLE",
            Self::CacheTemp => "CACHE_TEMP",
            Self::OldAppVersion => "OLD_APP_VERSION",
            Self::CrashDump => "CRASH_DUMP",
            Self::LogGrowth => "LOG_GROWTH",
            Self::DbStorage => "DB_STORAGE",
            Self::ContainerVm => "CONTAINER_VM",
            Self::MediaDownload => "MEDIA_DOWNLOAD",
            Self::AppData => "APP_DATA",
            Self::OrphanLeftover => "ORPHAN_LEFTOVER",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "MEDIA_CHAT" => Self::MediaChat,
            "EAPP_BUNDLE" => Self::EappBundle,
            "CACHE_TEMP" => Self::CacheTemp,
            "OLD_APP_VERSION" => Self::OldAppVersion,
            "CRASH_DUMP" => Self::CrashDump,
            "LOG_GROWTH" => Self::LogGrowth,
            "DB_STORAGE" => Self::DbStorage,
            "CONTAINER_VM" => Self::ContainerVm,
            "MEDIA_DOWNLOAD" => Self::MediaDownload,
            "APP_DATA" => Self::AppData,
            "ORPHAN_LEFTOVER" => Self::OrphanLeftover,
            _ => Self::CacheTemp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriteEvent {
    pub id: Option<i64>,
    pub event_date: String, // YYYY-MM-DD
    pub timestamp: DateTime<Utc>,
    pub drive_letter: String, // "C:\\"
    pub folder_path: PathBuf,
    pub file_path: PathBuf,
    pub delta_bytes: i64,
    pub process_name: String,
    pub pid: u32,
    pub service_name: String,
    pub service_display_name: String,
    pub reason_code: WriteReasonCode,
    pub reason_detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderGrowthRow {
    pub folder_path: PathBuf,
    pub reason_code: String,
    pub total_delta_bytes: i64,
    pub file_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorWriteRow {
    pub process_name: String,
    pub service_name: String,
    pub total_delta_bytes: i64,
    pub event_count: u64,
}

// ==========================================
// 3. Quota & Cleanup Candidate Models
// ==========================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaRule {
    pub id: Option<i64>,
    pub app_name: String,
    pub process_name: String,
    pub service_name: String,
    pub target_paths: Vec<PathBuf>,
    pub max_bytes: u64,
    pub watermark_ratio: f64,      // default 0.85
    pub retention_days: u32,       // 1..=90
    pub min_file_age_seconds: u64, // default 60
    pub exclude_patterns: Vec<String>,
    pub protected_files: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanCandidate {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub mtime: std::time::SystemTime,
    pub is_locked: bool,
    pub profile_id: String,
    pub is_canary: bool,
    pub is_orphan: bool,
    pub hardlink_siblings: Vec<PathBuf>,
}

// ==========================================
// 4. Canary Vault & Manifest Models
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VaultStatus {
    Quarantined,
    RolledBack,
    Purged,
    /// The archive's ZIP is gone from every known Vault folder: it cannot be restored.
    Missing,
}

impl VaultStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Quarantined => "QUARANTINED",
            Self::RolledBack => "ROLLED_BACK",
            Self::Purged => "PURGED",
            Self::Missing => "MISSING",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "ROLLED_BACK" => Self::RolledBack,
            "PURGED" => Self::Purged,
            "MISSING" => Self::Missing,
            _ => Self::Quarantined,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultArchive {
    pub id: Option<i64>,
    pub rule_id: i64, // 0 for ad-hoc 1-click cleanup
    pub app_name: String,
    pub zip_path: PathBuf,
    pub manifest_path: PathBuf,
    pub file_count: u64,
    pub original_bytes: u64,
    pub compressed_bytes: u64,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: VaultStatus,
    pub status_reason: String,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileManifestItem {
    pub id: Option<i64>,
    pub archive_id: Option<i64>,
    pub original_path: PathBuf,
    pub rel_arcname: String, // item_00001_file.dat
    pub size_bytes: u64,
    pub sha256: String,
    pub mtime: f64,
    pub atime: f64,
    pub hardlink_paths: Vec<PathBuf>,
    pub restored_individually: bool,
}

pub type VaultFileItem = FileManifestItem;
pub type VaultFileEntry = FileManifestItem;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultManifest {
    pub archive_id: String,
    pub created_at: DateTime<Utc>,
    pub original_root: PathBuf,
    pub files: Vec<FileManifestItem>,
    pub total_size: u64,
    pub compressed_size: u64,
    pub sha256: String,
}

// ==========================================
// 5. Relocation Models
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocationStrategyKind {
    KnownFolder,
    ToolConfig,
    Guided,
    Junction,
}

impl RelocationStrategyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::KnownFolder => "known_folder",
            Self::ToolConfig => "tool_config",
            Self::Guided => "guided",
            Self::Junction => "junction",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "known_folder" => Self::KnownFolder,
            "tool_config" => Self::ToolConfig,
            "guided" => Self::Guided,
            _ => Self::Junction,
        }
    }
}

pub type RelocationStrategy = RelocationStrategyKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocationStatus {
    Moving, // Atomic transition state
    Done,
    Failed,
    Undone,
    NeedsReview,
}

impl RelocationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Moving => "moving",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Undone => "undone",
            Self::NeedsReview => "needs_review",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "moving" => Self::Moving,
            "failed" => Self::Failed,
            "undone" => Self::Undone,
            "needs_review" => Self::NeedsReview,
            _ => Self::Done,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZaloRetainedStatus {
    Retained,
    Purged,
    Restored,
    Missing,
    NeedsReview,
}

impl ZaloRetainedStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Retained => "retained",
            Self::Purged => "purged",
            Self::Restored => "restored",
            Self::Missing => "missing",
            Self::NeedsReview => "needs_review",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "purged" => Self::Purged,
            "restored" => Self::Restored,
            "missing" => Self::Missing,
            "needs_review" => Self::NeedsReview,
            _ => Self::Retained,
        }
    }
}

/// Persistent DB record tracking a temporarily retained original `Zalo Data.greenc-original.<uuid>`
/// directory after a verified relocation (kept for 24h by default or until user purges/restores).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloRetainedOriginal {
    pub id: Option<i64>,
    pub relocation_id: i64,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub retained_path: PathBuf,
    pub size_bytes: u64,
    pub file_count: u64,
    pub health_verified: bool,
    pub created_at: DateTime<Utc>,
    pub purge_after: DateTime<Utc>,
    pub status: ZaloRetainedStatus,
    pub resolved_at: Option<DateTime<Utc>>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocationHealthStatus {
    Healthy,
    TargetOffline,
    LinkBroken,
}

impl RelocationHealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::TargetOffline => "target_offline",
            Self::LinkBroken => "link_broken",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "target_offline" => Self::TargetOffline,
            "link_broken" => Self::LinkBroken,
            _ => Self::Healthy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelocationRecord {
    pub id: Option<i64>,
    pub app_id: String,
    pub app_name: String,
    pub strategy: RelocationStrategyKind,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub size_bytes: u64,
    pub status: RelocationStatus,
    pub health_status: RelocationHealthStatus,
    pub process_names: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub detail: Option<String>,
}

// ==========================================
// 6. In-Place Compaction Models
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionKind {
    WofDirectory,
    VhdxTrim,
}

impl CompactionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WofDirectory => "wof_directory",
            Self::VhdxTrim => "vhdx_trim",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "vhdx_trim" => Self::VhdxTrim,
            _ => Self::WofDirectory,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub id: Option<i64>,
    pub target_path: PathBuf,
    pub kind: CompactionKind,
    pub algorithm: String, // XPRESS8K, XPRESS16K, LZNT1, VIRTDISK
    pub logical_bytes: u64,
    pub physical_before_bytes: u64,
    pub physical_after_bytes: u64,
    pub file_count: u64,
    pub status: String, // "compacted", "decompressed"
    pub updated_at: DateTime<Utc>,
}

// ==========================================
// Drive Quota (per-volume capacity budget)
// ==========================================

/// Severity of a drive's usage relative to its configured [`DriveQuota`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLevel {
    /// Usage is below the warning threshold.
    Ok,
    /// Usage reached the warning threshold: notify the user.
    Warning,
    /// Usage reached the clean threshold: auto-clean / auto-backup may run.
    Critical,
}

impl QuotaLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            QuotaLevel::Ok => "ok",
            QuotaLevel::Warning => "warning",
            QuotaLevel::Critical => "critical",
        }
    }
}

/// Controls how verified regenerable developer cache units are handled during drive-quota cleanup.
///
/// - `Vault` (default): every cleaned unit is archived into Canary Safety Vault first.
/// - `Direct`: ONLY verified regenerable developer cache units (`package_managers` atomic units)
///   are deleted directly with full DB audit logging (avoids same-drive Vault overhead).
///   Non-regenerable data ALWAYS goes through the Vault regardless of this setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegenerableCacheMode {
    #[default]
    Vault,
    Direct,
}

impl RegenerableCacheMode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Vault => "vault",
            Self::Direct => "direct",
        }
    }

    pub fn from_str_strict(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "vault" => Some(Self::Vault),
            "direct" => Some(Self::Direct),
            _ => None,
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        Self::from_str_strict(s).unwrap_or(Self::Vault)
    }
}

/// Persistent per-drive guard backoff cooldown state (stored in SQLite `guard_cooldowns`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardCooldownState {
    pub drive_root: String,
    pub consecutive_ineffective: u32,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_reason: String,
}

/// Audit log record for every regenerable developer cache unit deleted directly in `direct` mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectDeleteRecord {
    pub id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub drive_root: String,
    pub profile_id: String,
    pub unit_kind: String,
    pub unit_path: PathBuf,
    pub file_count: u64,
    pub logical_bytes: u64,
    pub reason: String,
}

/// Capacity budget for a whole drive. It is the basis for warnings, automatic backup and
/// automatic cleanup performed by the background guardian.
///
/// Thresholds are fractions of the *effective limit*: `limit_bytes` when set (and smaller than
/// the drive), otherwise the drive's total capacity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveQuota {
    pub id: Option<i64>,
    /// Drive root, e.g. `C:\`.
    pub drive_root: String,
    /// Budget in bytes. `0` means "use the whole drive capacity".
    pub limit_bytes: u64,
    /// Usage ratio at which a warning is raised (default 0.80).
    pub warn_ratio: f64,
    /// Usage ratio at which automatic actions start (default 0.90).
    pub clean_ratio: f64,
    /// Usage ratio automatic cleanup tries to get back under (default 0.85).
    pub target_ratio: f64,
    pub auto_clean: bool,
    pub auto_backup: bool,
    /// Folder that receives a copy of each new Vault archive when `auto_backup` is on.
    pub backup_dir: Option<PathBuf>,
    pub enabled: bool,
    #[serde(default)]
    pub regenerable_cache_mode: RegenerableCacheMode,
    pub created_at: DateTime<Utc>,
}

pub const DEFAULT_DRIVE_WARN_RATIO: f64 = 0.80;
pub const DEFAULT_DRIVE_CLEAN_RATIO: f64 = 0.90;
pub const DEFAULT_DRIVE_TARGET_RATIO: f64 = 0.85;

impl DriveQuota {
    pub fn new(drive_root: impl Into<String>) -> Self {
        Self {
            id: None,
            drive_root: drive_root.into(),
            limit_bytes: 0,
            warn_ratio: DEFAULT_DRIVE_WARN_RATIO,
            clean_ratio: DEFAULT_DRIVE_CLEAN_RATIO,
            target_ratio: DEFAULT_DRIVE_TARGET_RATIO,
            auto_clean: false,
            auto_backup: false,
            backup_dir: None,
            enabled: true,
            regenerable_cache_mode: RegenerableCacheMode::Vault,
            created_at: Utc::now(),
        }
    }

    /// Returns a copy with ratios clamped into a consistent order:
    /// `0.10 <= target < clean <= 0.99` and `warn <= clean`.
    pub fn normalized(mut self) -> Self {
        self.clean_ratio = self.clean_ratio.clamp(0.20, 0.99);
        self.warn_ratio = self.warn_ratio.clamp(0.10, self.clean_ratio);
        self.target_ratio = self.target_ratio.clamp(0.10, self.clean_ratio - 0.01);
        self
    }

    /// Budget in bytes for a drive of `total_bytes`.
    pub fn effective_limit(&self, total_bytes: u64) -> u64 {
        if self.limit_bytes == 0 || self.limit_bytes > total_bytes {
            total_bytes
        } else {
            self.limit_bytes
        }
    }

    /// Used bytes (`total - free`) as a share of the effective limit. May exceed `1.0`.
    pub fn usage_ratio(&self, total_bytes: u64, free_bytes: u64) -> f64 {
        let limit = self.effective_limit(total_bytes);
        if limit == 0 {
            return 0.0;
        }
        total_bytes.saturating_sub(free_bytes) as f64 / limit as f64
    }

    pub fn level(&self, total_bytes: u64, free_bytes: u64) -> QuotaLevel {
        let ratio = self.usage_ratio(total_bytes, free_bytes);
        if ratio >= self.clean_ratio {
            QuotaLevel::Critical
        } else if ratio >= self.warn_ratio {
            QuotaLevel::Warning
        } else {
            QuotaLevel::Ok
        }
    }

    /// Bytes that must be freed to get back to `target_ratio` (0 when already under it).
    pub fn bytes_to_reclaim(&self, total_bytes: u64, free_bytes: u64) -> u64 {
        let limit = self.effective_limit(total_bytes);
        let used = total_bytes.saturating_sub(free_bytes);
        let target = (limit as f64 * self.target_ratio) as u64;
        used.saturating_sub(target)
    }
}

/// Extracts an uppercase Windows drive letter if `raw` begins with `<letter>:`, including the
/// verbatim form returned by `std::fs::canonicalize` (`\\?\C:\...`).
fn windows_drive_letter(raw: &str) -> Option<char> {
    let raw = raw
        .strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix(r"\\.\"))
        .unwrap_or(raw);
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

/// Compares two Windows-style paths for equality, ignoring case, separator style, trailing separators
/// and the verbatim / NT prefixes (`\\?\`, `\\.\`, `\??\`) that `get_junction_target` and
/// `std::fs::canonicalize` may add.
pub fn same_windows_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| {
        let raw = p.to_string_lossy().replace('/', "\\");
        let stripped = [r"\\?\", r"\\.\", r"\??\"]
            .iter()
            .find_map(|prefix| raw.strip_prefix(prefix))
            .unwrap_or(&raw);
        stripped.trim_end_matches('\\').to_lowercase()
    };
    if norm(a) == norm(b) {
        return true;
    }
    // Junctions store the canonical target, while the recorded path may use an 8.3 short name
    // (`RUNNER~1`) or a SUBST drive. Both must resolve, so a missing target never matches.
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => norm(&ca) == norm(&cb),
        _ => false,
    }
}

/// Normalizes a user-supplied drive root:
/// - Windows drives (`C`, `c:`, `C:\`, `C:/`) -> `C:\`
/// - Unix mount points (`/`, `/home`, `/home/`, `/Volumes/Data`) -> `/` or `/home`
///
/// Returns `None` for non-root Windows paths (`C:\Users`), relative paths, or invalid paths.
pub fn normalize_drive_root(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }

    if t.starts_with('/') {
        if t.starts_with("//") || t.contains('\\') || t.contains('\0') {
            return None;
        }
        let stripped = t.trim_end_matches('/');
        if stripped.is_empty() {
            return Some("/".to_string());
        }
        if stripped[1..]
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        {
            return None;
        }
        return Some(stripped.to_string());
    }

    let mut chars = t.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let rest: String = chars.collect();
    match rest.as_str() {
        "" | ":" | ":\\" | ":/" => Some(format!("{}:\\", letter.to_ascii_uppercase())),
        _ => None,
    }
}

/// Returns `true` when `path` resides on the same filesystem/volume as `drive_root`.
///
/// Handles Windows drive letters case-insensitively (`C:\` vs `c:/foo`) and Unix mount points
/// using POSIX device IDs (`st_dev`) when paths exist on disk, falling back to segment-aware
/// path prefix matching when paths are synthetic/nonexistent.
pub fn is_same_volume(path: &std::path::Path, drive_root: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    let root_str = drive_root.to_string_lossy();

    let win_p = windows_drive_letter(&path_str);
    let win_r = windows_drive_letter(&root_str);
    if win_p.is_some() || win_r.is_some() {
        return win_p.is_some() && win_p == win_r;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(root_meta) = std::fs::metadata(drive_root) {
            let mut cur = Some(path);
            while let Some(candidate) = cur {
                if let Ok(cand_meta) = std::fs::metadata(candidate) {
                    // Do not let a nonexistent path under a fake mount collapse to `/`.
                    if candidate != std::path::Path::new("/")
                        || drive_root == std::path::Path::new("/")
                    {
                        return cand_meta.dev() == root_meta.dev();
                    }
                    break;
                }
                cur = candidate.parent();
            }
        }
    }

    let norm = |s: &str| {
        let replaced = s.trim().replace('\\', "/");
        let trimmed = replaced.trim_end_matches('/');
        if trimmed.is_empty() && replaced.starts_with('/') {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let p_norm = norm(&path_str);
    let r_norm = norm(&root_str);
    if r_norm.is_empty() || p_norm.is_empty() {
        return false;
    }
    if r_norm == "/" {
        p_norm == "/" || p_norm.starts_with('/')
    } else {
        p_norm == r_norm || p_norm.starts_with(&format!("{r_norm}/"))
    }
}

impl DriveQuota {
    /// Checks the backup settings: a backup folder is required when `auto_backup` is on, and it must
    /// not live on the drive being protected (it would consume the very space we try to free).
    pub fn validate_backup(&self) -> std::result::Result<(), String> {
        if !self.auto_backup {
            return Ok(());
        }
        let dir = self
            .backup_dir
            .as_ref()
            .ok_or_else(|| "Cần chọn thư mục sao lưu".to_string())?;
        if is_same_volume(dir, std::path::Path::new(&self.drive_root)) {
            return Err("Thư mục sao lưu phải nằm trên ổ khác với ổ được bảo vệ".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod drive_quota_tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn levels_follow_thresholds() {
        let q = DriveQuota::new("C:\\");
        assert_eq!(q.level(100 * GB, 50 * GB), QuotaLevel::Ok); // 50%
        assert_eq!(q.level(100 * GB, 20 * GB), QuotaLevel::Warning); // 80%
        assert_eq!(q.level(100 * GB, 10 * GB), QuotaLevel::Critical); // 90%
    }

    #[test]
    fn explicit_limit_is_the_budget() {
        let mut q = DriveQuota::new("D:\\");
        q.limit_bytes = 50 * GB;
        // 40 GB used of a 50 GB budget on a 100 GB drive = 80%
        assert_eq!(q.level(100 * GB, 60 * GB), QuotaLevel::Warning);
        // a limit larger than the drive falls back to the drive size
        q.limit_bytes = 500 * GB;
        assert_eq!(q.effective_limit(100 * GB), 100 * GB);
    }

    #[test]
    fn reclaim_targets_target_ratio() {
        let q = DriveQuota::new("C:\\");
        // 95 GB used, target 85% of 100 GB => reclaim 10 GB
        assert_eq!(q.bytes_to_reclaim(100 * GB, 5 * GB), 10 * GB);
        assert_eq!(q.bytes_to_reclaim(100 * GB, 50 * GB), 0);
    }

    #[test]
    fn normalization_keeps_order_and_handles_zero_total() {
        let mut q = DriveQuota::new("C:\\");
        q.warn_ratio = 0.99;
        q.clean_ratio = 0.50;
        q.target_ratio = 0.95;
        let q = q.normalized();
        assert!(q.target_ratio < q.clean_ratio && q.warn_ratio <= q.clean_ratio);
        assert_eq!(q.level(0, 0), QuotaLevel::Ok);
    }

    #[test]
    fn normalize_drive_root_supports_windows_and_unix_mounts() {
        assert_eq!(normalize_drive_root("c"), Some("C:\\".to_string()));
        assert_eq!(normalize_drive_root(" d: "), Some("D:\\".to_string()));
        assert_eq!(normalize_drive_root("E:/"), Some("E:\\".to_string()));
        assert_eq!(normalize_drive_root("/"), Some("/".to_string()));
        assert_eq!(normalize_drive_root("///"), None);
        assert_eq!(normalize_drive_root("/home"), Some("/home".to_string()));
        assert_eq!(normalize_drive_root("/home/"), Some("/home".to_string()));
        assert_eq!(
            normalize_drive_root("/Volumes/Data/"),
            Some("/Volumes/Data".to_string())
        );

        assert_eq!(normalize_drive_root("C:\\Users"), None);
        assert_eq!(normalize_drive_root("C:/Users"), None);
        assert_eq!(normalize_drive_root("/home/../etc"), None);
        assert_eq!(normalize_drive_root("//server/share"), None);
        assert_eq!(normalize_drive_root("/bad\\path"), None);
        assert_eq!(normalize_drive_root("1:"), None);
        assert_eq!(normalize_drive_root(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn same_windows_path_resolves_aliases_but_never_matches_missing_targets() {
        let base = std::env::temp_dir().join(format!("greenc-alias-{}", std::process::id()));
        let real = base.join("real");
        let other = base.join("other");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let alias = base.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        assert!(same_windows_path(&alias, &real));
        assert!(!same_windows_path(&alias, &other));
        std::fs::remove_dir(&real).unwrap();
        assert!(!same_windows_path(&alias, &real));

        std::fs::remove_file(&alias).unwrap();
        std::fs::remove_dir(&other).unwrap();
        std::fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn is_same_volume_handles_windows_and_unix_paths() {
        use std::path::Path;

        assert!(same_windows_path(
            Path::new(r"\??\D:\GreenC\zalo"),
            Path::new("d:/greenc/Zalo/")
        ));
        assert!(!same_windows_path(
            Path::new(r"D:\GreenC\zalo2"),
            Path::new(r"D:\GreenC\zalo")
        ));
        assert!(is_same_volume(Path::new("C:/bk"), Path::new("C:\\")));
        assert!(is_same_volume(Path::new("c:\\Users\\a"), Path::new("C:\\")));
        assert!(!is_same_volume(Path::new("D:\\bk"), Path::new("C:\\")));
        assert!(is_same_volume(
            Path::new(r"\\?\C:\Users\a"),
            Path::new(r"C:\Users\a\staging")
        ));
        assert!(!is_same_volume(Path::new(r"\\?\D:\x"), Path::new(r"C:\")));
        assert!(!is_same_volume(Path::new("/tmp/bk"), Path::new("Q:/")));

        // Synthetic Unix mounts: segment-boundary prefix match prevents /mnt/a matching /mnt/a2
        assert!(is_same_volume(
            Path::new("/nonexistent_mnt/disk1/bk"),
            Path::new("/nonexistent_mnt/disk1")
        ));
        assert!(!is_same_volume(
            Path::new("/nonexistent_mnt/disk10/bk"),
            Path::new("/nonexistent_mnt/disk1")
        ));
        assert!(!is_same_volume(
            Path::new("/nonexistent_mnt/disk2/bk"),
            Path::new("/nonexistent_mnt/disk1")
        ));

        let mut q = DriveQuota::new("/nonexistent_mnt/disk1");
        q.auto_backup = true;
        q.backup_dir = Some(PathBuf::from("/nonexistent_mnt/disk1/bk"));
        assert!(q.validate_backup().is_err());

        q.backup_dir = Some(PathBuf::from("/nonexistent_mnt/disk2/bk"));
        assert!(q.validate_backup().is_ok());
    }
}
