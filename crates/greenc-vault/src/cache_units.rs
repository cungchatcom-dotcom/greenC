//! Atomic developer-cache unit discovery (`GC-GUARD` Item 2).
//!
//! Instead of picking arbitrary files inside `~/.gradle/caches`, `~/.npm/_cacache`,
//! `~/.cargo/registry/cache`, or `~/.cache/{pip,uv}`, this module discovers **atomic cache units**
//! (a single Gradle transform directory, an obsolete Gradle version, a Cargo `.crate`, an npm
//! `index-v5` entry with exclusively-owned `content-v2` blobs, a pip/uv cache entry):
//! - Unit age = newest `max(mtime, reliable_atime)` across the entire unit (default >= 7 days).
//! - Forbidden control files/directories (`*.lock`, `*.lck`, `journal*`, `gc.properties`,
//!   `CACHEDIR.TAG`, `fileHashes`, `modules-2/metadata-*`, `file-access.bin`, `_cacache/tmp`,
//!   active/latest Gradle version control level) are NEVER selected.
//! - If a tool is busy (or status is `BusyToolsStatus::Unknown`), ALL units belonging to that
//!   tool are skipped.
//! - Symlinks are never traversed; mount-point crossings (`st_dev` changes) are rejected.

use greenc_core::models::CleanCandidate;
use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool};
use greenc_platform::handle::{query_file_identity, Win32FileIdentity};
use greenc_platform::traits::FileLockProbe;
use greenc_platform::{detect_atime_reliability, AtimeReliability};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Default minimum idle age for developer cache units: 7 days (604,800 seconds).
pub const DEFAULT_DEV_CACHE_MIN_AGE_SECS: u64 = 7 * 86_400;

/// Conservative minimum age (30 days) applied when filesystem `atime` is unreliable (e.g. `noatime`).
/// Active Gradle versions are skipped completely when `atime` is unreliable.
pub const NOATIME_FALLBACK_MIN_AGE_SECS: u64 = 30 * 86_400;

/// Defensive maximum directory depth for iterative timestamp inspection (never silently `break`s).
const MAX_TIMESTAMP_WALK_DEPTH: usize = 512;

/// Classification of a cleanup unit for Vault vs Direct deletion policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanUnitKind {
    /// Verified atomic regenerable developer cache unit (Item 2).
    /// Eligible for `direct` deletion ONLY when the drive quota sets `regenerable_cache_mode = direct`.
    RegenerableDevCache,
    /// Verified regenerable desktop-app cache or obsolete reinstallable app version (e.g. Zalo
    /// allowlisted caches and obsolete `Zalo-x.y.z` installations).
    /// Goes through Vault in `vault` mode; eligible for `direct` deletion in `direct` mode.
    RegenerableAppCache,
    /// Standard cleanup file (browser_cache, windows_temp, chat_media, crash_dumps, app_logs, old_versions).
    /// MUST ALWAYS go through the Canary Safety Vault.
    StandardFile,
}

/// An atomic unit of disk space eligible for cleanup.
#[derive(Debug, Clone)]
pub struct CleanUnit {
    /// Primary path of the unit (directory for transform/old Gradle version, or file for crate/npm index).
    pub root_path: PathBuf,
    /// Allowed parent cache root that `root_path` was discovered under (used for containment checks).
    pub allowed_parent: PathBuf,
    /// Constituent files belonging to this atomic unit.
    pub files: Vec<CleanCandidate>,
    /// Total logical bytes across `files`.
    pub total_bytes: u64,
    /// Newest `max(mtime, reliable_atime)` timestamp across the entire unit.
    pub last_used: SystemTime,
    /// Cleanup profile ID (`package_managers`, `app_logs`, `browser_cache`, ...).
    pub profile_id: String,
    /// Whether this unit is a verified regenerable developer cache or standard file.
    pub kind: CleanUnitKind,
    /// Associated developer tool (always `Some(_)` for `RegenerableDevCache`).
    pub dev_tool: Option<DevTool>,
    /// Human/log classification label (e.g. `gradle_transform`, `gradle_old_version`, `cargo_crate`, `npm_cache_entry`).
    pub unit_label: String,
    /// Physical identity `(volume_serial / st_dev, file_index / st_ino)` captured at scan time for TOCTOU checks.
    pub root_identity: Option<Win32FileIdentity>,
    /// Minimum age requirement enforced at scan and re-verified before deletion.
    pub min_age: Duration,
}

impl CleanUnit {
    /// Returns `true` ONLY for verified regenerable cache/reinstallable units eligible for
    /// `direct` deletion when `regenerable_cache_mode = direct`.
    pub fn is_direct_delete_eligible(&self) -> bool {
        match self.kind {
            CleanUnitKind::RegenerableDevCache => {
                self.profile_id == "package_managers" && self.dev_tool.is_some()
            }
            CleanUnitKind::RegenerableAppCache => {
                self.profile_id == "chat_media"
                    && (self.unit_label.starts_with("zalo_cache")
                        || self.unit_label == "zalo_old_version")
            }
            CleanUnitKind::StandardFile => false,
        }
    }
}

/// Computes `max(mtime, reliable_atime)` for a filesystem metadata record.
///
/// `atime` is considered reliable only when `accessed()` succeeds, is strictly after `UNIX_EPOCH`,
/// and is not implausibly far in the future (`<= now + 300s`). Under Linux `relatime`, reads bump
/// `atime > mtime`, preventing premature deletion of recently read caches. Under `noatime`,
/// `atime <= mtime` so this safely falls back to `mtime`.
pub fn effective_last_used(meta: &std::fs::Metadata, now: SystemTime) -> SystemTime {
    let mtime = meta.modified().unwrap_or(now);
    let max_reasonable = now + Duration::from_secs(300);
    match meta.accessed() {
        Ok(atime) if atime > SystemTime::UNIX_EPOCH && atime <= max_reasonable => {
            if atime > mtime {
                atime
            } else {
                mtime
            }
        }
        _ => mtime,
    }
}

/// Returns `true` if `file_name` is a strictly forbidden developer cache control/lock entry.
pub fn is_forbidden_dev_cache_name(file_name: &str) -> bool {
    let low = file_name.trim().to_ascii_lowercase();
    if low.is_empty() {
        return true;
    }
    if low.ends_with(".lock") || low.ends_with(".lck") {
        return true;
    }
    if matches!(
        low.as_str(),
        "lockfile"
            | "gc.properties"
            | "cachedir.tag"
            | "file-access.bin"
            | "filehashes"
            | "journal"
            | ".package-cache"
            | ".package-cache-mutate"
            | "selfcheck.json"
    ) {
        return true;
    }
    if low.starts_with("journal-") || low.starts_with("metadata-") {
        return true;
    }
    false
}

/// Returns `true` if any component of `path` matches a strictly forbidden developer cache control
/// file or directory (including `modules-2/metadata-*`, `fileHashes`, `journal-*`, `*.lock`,
/// `*.lck`, `gc.properties`, `CACHEDIR.TAG`, `file-access.bin`, `_cacache/tmp`).
pub fn is_forbidden_dev_cache_path(path: &Path) -> bool {
    let mut prev_low = String::new();
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy();
        let low = s.to_ascii_lowercase();
        if is_forbidden_dev_cache_name(&low) {
            return true;
        }
        if prev_low == "modules-2" && (low.starts_with("metadata-") || low == "file-access.bin") {
            return true;
        }
        if prev_low == "_cacache" && low == "tmp" {
            return true;
        }
        prev_low = low;
    }
    false
}

/// Strict filename filter for Linux `app_logs` (`~/.local/state`): ONLY `*.log` and `*.log.*`.
pub fn is_linux_app_log_file(file_name: &str) -> bool {
    let low = file_name.trim().to_ascii_lowercase();
    if low.is_empty() || low.ends_with(".lock") || low.ends_with(".lck") {
        return false;
    }
    // Never delete GreenC's own active guardian log.
    if low == "guardian.log" {
        return false;
    }
    low.ends_with(".log") || low.contains(".log.")
}

/// Parses a Gradle cache version directory name (e.g. `"8.11.1"`, `"7.6.4-rc-1"`) into numeric parts.
pub fn parse_gradle_version(name: &str) -> Option<Vec<u64>> {
    let core = name.split('-').next()?;
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for p in parts {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        nums.push(p.parse::<u64>().ok()?);
    }
    Some(nums)
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

#[cfg(unix)]
fn nlink_count(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink()
}

#[cfg(not(unix))]
fn nlink_count(_meta: &std::fs::Metadata) -> u64 {
    1
}

/// Inspects a single directory unit recursively without following symlinks or crossing devices.
struct UnitTreeStats {
    files: Vec<CleanCandidate>,
    total_bytes: u64,
    newest_used: SystemTime,
    has_locked: bool,
    has_forbidden: bool,
    has_symlink_or_cross_dev: bool,
}

fn inspect_directory_unit(
    unit_root: &Path,
    expected_dev: Option<u64>,
    profile_id: &str,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Option<UnitTreeStats> {
    let root_meta = std::fs::symlink_metadata(unit_root).ok()?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return None;
    }
    if expected_dev.is_some() && device_id(&root_meta) != expected_dev {
        return None;
    }

    let mut stats = UnitTreeStats {
        files: Vec::new(),
        total_bytes: 0,
        // Use mtime for directories (readdir itself bumps directory atime under Linux relatime);
        // use max(mtime, reliable_atime) for regular files.
        newest_used: root_meta.modified().unwrap_or(now),
        has_locked: false,
        has_forbidden: false,
        has_symlink_or_cross_dev: false,
    };

    let mut stack: Vec<(PathBuf, usize)> = vec![(unit_root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if ctrl.is_cancelled() || depth > 32 {
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            stats.has_locked = true;
            return Some(stats);
        };
        for entry in rd.flatten() {
            if ctrl.is_cancelled() {
                return None;
            }
            let path = entry.path();
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            if is_forbidden_dev_cache_name(&fname_str) {
                stats.has_forbidden = true;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                stats.has_symlink_or_cross_dev = true;
                continue;
            }
            if expected_dev.is_some() && device_id(&meta) != expected_dev {
                stats.has_symlink_or_cross_dev = true;
                continue;
            }

            if meta.is_dir() {
                let dir_mtime = meta.modified().unwrap_or(now);
                if dir_mtime > stats.newest_used {
                    stats.newest_used = dir_mtime;
                }
                stack.push((path, depth + 1));
            } else if meta.is_file() {
                let used = effective_last_used(&meta, now);
                if used > stats.newest_used {
                    stats.newest_used = used;
                }
                if lock_probe.is_file_locked(&path) {
                    stats.has_locked = true;
                }
                let size = meta.len();
                stats.total_bytes = stats.total_bytes.saturating_add(size);
                stats.files.push(CleanCandidate {
                    path: path.clone(),
                    size_bytes: size,
                    mtime: meta.modified().unwrap_or(now),
                    is_locked: false,
                    profile_id: profile_id.to_string(),
                    is_canary: false,
                    is_orphan: false,
                    hardlink_siblings: vec![path],
                });
            }
        }
    }

    Some(stats)
}

/// Computes the newest `max(mtime, reliable_atime)` across files (and `mtime` for dirs) inside
/// `path` without following symlinks or crossing filesystem devices. Used for TOCTOU age checks.
pub fn compute_unit_latest_used(path: &Path, now: SystemTime) -> Option<SystemTime> {
    let root_meta = std::fs::symlink_metadata(path).ok()?;
    if root_meta.file_type().is_symlink() {
        return None;
    }
    let root_dev = device_id(&root_meta);
    if !root_meta.is_dir() {
        return Some(effective_last_used(&root_meta, now));
    }
    let mut newest = root_meta.modified().unwrap_or(now);

    let mut stack: Vec<(PathBuf, usize)> = vec![(path.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_TIMESTAMP_WALK_DEPTH {
            // Reject rather than silently truncating and returning a stale timestamp.
            return None;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return None;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if root_dev.is_some() && device_id(&meta) != root_dev {
                continue;
            }
            if meta.is_dir() {
                let dir_mtime = meta.modified().unwrap_or(now);
                if dir_mtime > newest {
                    newest = dir_mtime;
                }
                stack.push((p, depth + 1));
            } else {
                let used = effective_last_used(&meta, now);
                if used > newest {
                    newest = used;
                }
            }
        }
    }
    Some(newest)
}

/// Identifies which developer tool owns a given `package_managers` cache root directory.
pub fn classify_dev_cache_root(dir: &Path) -> Option<DevTool> {
    let norm = dir
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if norm.contains(".gradle") || norm.ends_with("/gradle/caches") {
        Some(DevTool::Gradle)
    } else if norm.contains(".cargo") {
        Some(DevTool::Cargo)
    } else if norm.contains("_cacache")
        || norm.contains("npm-cache")
        || norm.contains("/pnpm")
        || norm.contains("/yarn")
    {
        Some(DevTool::Npm)
    } else if norm.contains("/pip") || norm.contains("/uv") {
        Some(DevTool::Pip)
    } else {
        None
    }
}

/// Scans a single developer cache root directory into atomic [`CleanUnit`] items.
///
/// Automatically inspects the real filesystem mount options (`detect_atime_reliability`):
/// - When `atime` is reliable (`relatime` / `strictatime`), applies `min_age` (default 7 days).
/// - When `atime` is unreliable (`noatime` or `Unknown`), skips active Gradle version transforms
///   completely and raises the minimum age threshold for other dev-cache units to >= 30 days
///   (`NOATIME_FALLBACK_MIN_AGE_SECS`), without ever parsing Gradle's binary journal format.
///
/// Respects `busy_status`: if the owning tool is busy (or status is `Unknown`), returns empty!
pub fn scan_dev_cache_root(
    cache_root: &Path,
    min_age: Duration,
    now: SystemTime,
    busy_status: &BusyToolsStatus,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let atime_reliability = detect_atime_reliability(cache_root);
    scan_dev_cache_root_with_atime_reliability(
        cache_root,
        min_age,
        now,
        busy_status,
        lock_probe,
        ctrl,
        atime_reliability,
    )
}

/// Scans a developer cache root with an explicitly supplied [`AtimeReliability`] (used by tests
/// and callers that already probed the mount options).
pub fn scan_dev_cache_root_with_atime_reliability(
    cache_root: &Path,
    min_age: Duration,
    now: SystemTime,
    busy_status: &BusyToolsStatus,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
    atime_reliability: AtimeReliability,
) -> Vec<CleanUnit> {
    if ctrl.is_cancelled() {
        return Vec::new();
    }
    let Ok(root_meta) = std::fs::symlink_metadata(cache_root) else {
        return Vec::new();
    };
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Vec::new();
    }
    let Some(tool) = classify_dev_cache_root(cache_root) else {
        return Vec::new();
    };
    if busy_status.is_tool_busy(tool) {
        return Vec::new();
    }

    let atime_reliable = atime_reliability.is_reliable();
    let effective_min_age = if atime_reliable {
        min_age
    } else {
        min_age.max(Duration::from_secs(NOATIME_FALLBACK_MIN_AGE_SECS))
    };

    let expected_dev = device_id(&root_meta);
    let norm = cache_root
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();

    match tool {
        DevTool::Gradle => scan_gradle_caches(
            cache_root,
            expected_dev,
            effective_min_age,
            atime_reliable,
            now,
            lock_probe,
            ctrl,
        ),
        DevTool::Cargo => scan_cargo_cache(
            cache_root,
            expected_dev,
            effective_min_age,
            now,
            lock_probe,
            ctrl,
        ),
        DevTool::Npm => {
            if norm.contains("_cacache") {
                scan_npm_cacache(
                    cache_root,
                    expected_dev,
                    effective_min_age,
                    now,
                    lock_probe,
                    ctrl,
                )
            } else if norm.contains("/pnpm") {
                scan_pnpm_store(
                    cache_root,
                    expected_dev,
                    effective_min_age,
                    now,
                    lock_probe,
                    ctrl,
                )
            } else {
                Vec::new()
            }
        }
        DevTool::Pip => {
            if norm.contains("/uv") {
                scan_uv_cache(
                    cache_root,
                    expected_dev,
                    effective_min_age,
                    now,
                    lock_probe,
                    ctrl,
                )
            } else {
                scan_pip_cache(
                    cache_root,
                    expected_dev,
                    effective_min_age,
                    now,
                    lock_probe,
                    ctrl,
                )
            }
        }
    }
}

/// Scans `~/.gradle/caches` for atomic units:
/// 1. Inside the active/latest Gradle version(s):
///    - When filesystem `atime` is reliable: ONLY stale `transforms/<hash>`, `transforms-3/<hash>`,
///      `transforms-4/<hash>` directories (never touching control-level `fileHashes`, `kotlin-dsl`,
///      `executionHistory`, `gc.properties`, `*.lock`, `CACHEDIR.TAG`).
///    - When filesystem `atime` is unreliable (`noatime` / `Unknown`): SKIPPED completely so
///      daily-used transforms with old creation `mtime` are never evicted.
/// 2. Obsolete older Gradle version directories (strictly older than the highest semver AND not
///    the most recently used version): cleaned as a single atomic `gradle_old_version` unit if it
///    contains no forbidden files, or else its non-forbidden cache subdirectories as atomic units
///    (subject to >= 30d threshold when `atime` is unreliable).
/// 3. Top-level `transforms-3/<hash>` and `transforms-4/<hash>` directories.
/// 4. Top-level `journal-*`, `modules-2`, `*.lock`, `*.lck`, `gc.properties`, `CACHEDIR.TAG`,
///    `file-access.bin` are ALWAYS untouched.
fn scan_gradle_caches(
    gradle_caches_root: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    atime_reliable: bool,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let Ok(rd) = std::fs::read_dir(gradle_caches_root) else {
        return Vec::new();
    };

    let mut version_dirs: Vec<(PathBuf, Vec<u64>, SystemTime)> = Vec::new();
    let mut top_transform_parents: Vec<PathBuf> = Vec::new();

    for entry in rd.flatten() {
        if ctrl.is_cancelled() {
            return Vec::new();
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let low = name.to_ascii_lowercase();

        // Never touch top-level control/lock/journal/modules-2 entries.
        if is_forbidden_dev_cache_name(&low)
            || low == "modules-2"
            || low.starts_with("jars-")
            || low.starts_with("build-cache-")
        {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        if expected_dev.is_some() && device_id(&meta) != expected_dev {
            continue;
        }

        if let Some(ver) = parse_gradle_version(&name) {
            let newest = compute_unit_latest_used(&path, now)
                .unwrap_or_else(|| effective_last_used(&meta, now));
            version_dirs.push((path, ver, newest));
        } else if low == "transforms" || low.starts_with("transforms-") {
            top_transform_parents.push(path);
        }
    }

    // Protect BOTH highest semver version AND most-recently-touched version at control level.
    let highest_semver_path = version_dirs
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|v| v.0.clone());
    let most_recent_path = version_dirs.iter().max_by_key(|v| v.2).map(|v| v.0.clone());

    let mut units = Vec::new();

    // 1. Scan top-level transforms-3 / transforms-4 directories.
    for tparent in top_transform_parents {
        collect_gradle_transform_units(
            &tparent,
            gradle_caches_root,
            expected_dev,
            min_age,
            now,
            lock_probe,
            ctrl,
            &mut units,
        );
    }

    // 2. Scan each Gradle version directory.
    for (ver_path, _ver_tuple, _newest) in version_dirs {
        if ctrl.is_cancelled() {
            break;
        }
        let is_active_version = highest_semver_path.as_ref() == Some(&ver_path)
            || most_recent_path.as_ref() == Some(&ver_path);

        if is_active_version {
            // On `noatime` / unknown-atime filesystems, transforms used daily still have old
            // creation mtime. Skip active Gradle version units completely rather than risking
            // evicting active transforms or parsing Gradle's binary journal format.
            if !atime_reliable {
                continue;
            }
            // Active/latest Gradle version: ONLY stale transform hash subdirectories are eligible!
            for sub_name in ["transforms", "transforms-3", "transforms-4"] {
                let tdir = ver_path.join(sub_name);
                if tdir.is_dir() {
                    collect_gradle_transform_units(
                        &tdir,
                        gradle_caches_root,
                        expected_dev,
                        min_age,
                        now,
                        lock_probe,
                        ctrl,
                        &mut units,
                    );
                }
            }
        } else {
            // Older Gradle version: inspect as an atomic unit first.
            let Some(stats) = inspect_directory_unit(
                &ver_path,
                expected_dev,
                "package_managers",
                now,
                lock_probe,
                ctrl,
            ) else {
                continue;
            };
            if stats.has_locked || stats.has_symlink_or_cross_dev || stats.files.is_empty() {
                continue;
            }
            let age = now
                .duration_since(stats.newest_used)
                .unwrap_or(Duration::ZERO);
            if age < min_age {
                continue;
            }

            if !stats.has_forbidden {
                // Entire obsolete Gradle version directory is clean -> select as ONE atomic unit!
                units.push(CleanUnit {
                    root_path: ver_path.clone(),
                    allowed_parent: gradle_caches_root.to_path_buf(),
                    files: stats.files,
                    total_bytes: stats.total_bytes,
                    last_used: stats.newest_used,
                    profile_id: "package_managers".to_string(),
                    kind: CleanUnitKind::RegenerableDevCache,
                    dev_tool: Some(DevTool::Gradle),
                    unit_label: "gradle_old_version".to_string(),
                    root_identity: query_file_identity(&ver_path),
                    min_age,
                });
            } else {
                // Old version contains internal control/lock files (e.g. gc.properties/fileHashes):
                // never touch those control files; select only its non-forbidden cache sub-units.
                for sub_name in ["transforms", "transforms-3", "transforms-4"] {
                    let tdir = ver_path.join(sub_name);
                    if tdir.is_dir() {
                        collect_gradle_transform_units(
                            &tdir,
                            gradle_caches_root,
                            expected_dev,
                            min_age,
                            now,
                            lock_probe,
                            ctrl,
                            &mut units,
                        );
                    }
                }
                for sub_name in [
                    "generated-gradle-jars",
                    "scripts",
                    "kotlin-dsl",
                    "dependencies-accessors",
                ] {
                    let sub = ver_path.join(sub_name);
                    if let Some(sub_stats) = inspect_directory_unit(
                        &sub,
                        expected_dev,
                        "package_managers",
                        now,
                        lock_probe,
                        ctrl,
                    ) {
                        let sub_age = now
                            .duration_since(sub_stats.newest_used)
                            .unwrap_or(Duration::ZERO);
                        if !sub_stats.has_locked
                            && !sub_stats.has_forbidden
                            && !sub_stats.has_symlink_or_cross_dev
                            && !sub_stats.files.is_empty()
                            && sub_age >= min_age
                        {
                            units.push(CleanUnit {
                                root_path: sub.clone(),
                                allowed_parent: gradle_caches_root.to_path_buf(),
                                files: sub_stats.files,
                                total_bytes: sub_stats.total_bytes,
                                last_used: sub_stats.newest_used,
                                profile_id: "package_managers".to_string(),
                                kind: CleanUnitKind::RegenerableDevCache,
                                dev_tool: Some(DevTool::Gradle),
                                unit_label: "gradle_old_version".to_string(),
                                root_identity: query_file_identity(&sub),
                                min_age,
                            });
                        }
                    }
                }
            }
        }
    }

    units
}

#[allow(clippy::too_many_arguments)]
fn collect_gradle_transform_units(
    transforms_dir: &Path,
    allowed_parent: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
    out: &mut Vec<CleanUnit>,
) {
    let Ok(rd) = std::fs::read_dir(transforms_dir) else {
        return;
    };
    for entry in rd.flatten() {
        if ctrl.is_cancelled() {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_forbidden_dev_cache_name(&name) {
            continue;
        }
        let Some(stats) = inspect_directory_unit(
            &path,
            expected_dev,
            "package_managers",
            now,
            lock_probe,
            ctrl,
        ) else {
            continue;
        };
        if stats.has_locked
            || stats.has_forbidden
            || stats.has_symlink_or_cross_dev
            || stats.files.is_empty()
        {
            continue;
        }
        let age = now
            .duration_since(stats.newest_used)
            .unwrap_or(Duration::ZERO);
        if age < min_age {
            continue;
        }

        out.push(CleanUnit {
            root_path: path.clone(),
            allowed_parent: allowed_parent.to_path_buf(),
            files: stats.files,
            total_bytes: stats.total_bytes,
            last_used: stats.newest_used,
            profile_id: "package_managers".to_string(),
            kind: CleanUnitKind::RegenerableDevCache,
            dev_tool: Some(DevTool::Gradle),
            unit_label: "gradle_transform".to_string(),
            root_identity: query_file_identity(&path),
            min_age,
        });
    }
}

/// Scans `~/.cargo/registry/cache/<index>/*.crate` — each `.crate` archive is one atomic unit.
fn scan_cargo_cache(
    cargo_cache_root: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let Ok(indices) = std::fs::read_dir(cargo_cache_root) else {
        return Vec::new();
    };
    let mut units = Vec::new();

    for idx_entry in indices.flatten() {
        if ctrl.is_cancelled() {
            break;
        }
        let idx_path = idx_entry.path();
        let Ok(idx_meta) = std::fs::symlink_metadata(&idx_path) else {
            continue;
        };
        if idx_meta.file_type().is_symlink() || !idx_meta.is_dir() {
            continue;
        }
        if expected_dev.is_some() && device_id(&idx_meta) != expected_dev {
            continue;
        }

        let Ok(crates_rd) = std::fs::read_dir(&idx_path) else {
            continue;
        };
        for crate_entry in crates_rd.flatten() {
            if ctrl.is_cancelled() {
                break;
            }
            let cpath = crate_entry.path();
            let fname = crate_entry.file_name().to_string_lossy().to_string();
            if is_forbidden_dev_cache_name(&fname) || !fname.ends_with(".crate") {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&cpath) else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            if expected_dev.is_some() && device_id(&meta) != expected_dev {
                continue;
            }
            let used = effective_last_used(&meta, now);
            let age = now.duration_since(used).unwrap_or(Duration::ZERO);
            if age < min_age || lock_probe.is_file_locked(&cpath) {
                continue;
            }

            let size = meta.len();
            units.push(CleanUnit {
                root_path: cpath.clone(),
                allowed_parent: cargo_cache_root.to_path_buf(),
                files: vec![CleanCandidate {
                    path: cpath.clone(),
                    size_bytes: size,
                    mtime: meta.modified().unwrap_or(now),
                    is_locked: false,
                    profile_id: "package_managers".to_string(),
                    is_canary: false,
                    is_orphan: false,
                    hardlink_siblings: vec![cpath.clone()],
                }],
                total_bytes: size,
                last_used: used,
                profile_id: "package_managers".to_string(),
                kind: CleanUnitKind::RegenerableDevCache,
                dev_tool: Some(DevTool::Cargo),
                unit_label: "cargo_crate".to_string(),
                root_identity: query_file_identity(&cpath),
                min_age,
            });
        }
    }

    units
}

/// Decodes standard base64 (RFC 4648) into raw bytes without external dependencies.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }

    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks_exact(4) {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = val(chunk[2])?;
            out.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' {
                let d = val(chunk[3])?;
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}

fn resolve_cacache_content_path(cacache_root: &Path, integrity: &str) -> Option<PathBuf> {
    let (algo, b64) = integrity.trim().split_once('-')?;
    if !matches!(algo, "sha512" | "sha256" | "sha1") {
        return None;
    }
    let raw = decode_base64(b64)?;
    let mut hex = String::with_capacity(raw.len() * 2);
    for byte in raw {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    if hex.len() < 6 {
        return None;
    }
    Some(
        cacache_root
            .join("content-v2")
            .join(algo)
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(&hex[4..]),
    )
}

fn parse_npm_index_content_paths(cacache_root: &Path, index_file: &Path) -> Vec<PathBuf> {
    let Ok(raw) = greenc_platform::handle::read_to_string_noatime(index_file) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let json_part = match trimmed.split_once('\t') {
            Some((_, rhs)) => rhs,
            None => trimmed,
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(json_part) else {
            continue;
        };
        if let Some(integrity) = val.get("integrity").and_then(|v| v.as_str()) {
            if let Some(p) = resolve_cacache_content_path(cacache_root, integrity) {
                if seen.insert(p.clone()) {
                    paths.push(p);
                }
            }
        }
    }
    paths
}

/// Scans `~/.npm/_cacache` through `index-v5` entries and pairs each stale index file with its
/// exclusively-referenced `content-v2` blobs (refcount == 1). Never touches `_cacache/tmp`.
fn scan_npm_cacache(
    cacache_root: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let index_root = cacache_root.join("index-v5");
    if !index_root.is_dir() {
        return Vec::new();
    }

    // Pass 1: collect all index files and build global refcounts for content-v2 blobs.
    let mut index_entries: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    let mut content_refcount: HashMap<PathBuf, usize> = HashMap::new();

    let mut stack: Vec<(PathBuf, usize)> = vec![(index_root, 0)];
    while let Some((dir, depth)) = stack.pop() {
        if ctrl.is_cancelled() || depth > 8 {
            return Vec::new();
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            if is_forbidden_dev_cache_name(&fname) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if expected_dev.is_some() && device_id(&meta) != expected_dev {
                continue;
            }
            if meta.is_dir() {
                stack.push((p, depth + 1));
            } else if meta.is_file() {
                let referenced = parse_npm_index_content_paths(cacache_root, &p);
                for c in &referenced {
                    *content_refcount.entry(c.clone()).or_insert(0) += 1;
                }
                index_entries.push((p, referenced));
            }
        }
    }

    // Pass 2: build atomic units for stale index entries whose exclusive content blobs are also stale & unlocked.
    let mut units = Vec::new();
    for (idx_path, referenced) in index_entries {
        if ctrl.is_cancelled() {
            break;
        }
        let Ok(idx_meta) = std::fs::symlink_metadata(&idx_path) else {
            continue;
        };
        if idx_meta.file_type().is_symlink() || lock_probe.is_file_locked(&idx_path) {
            continue;
        }

        let mut newest_used = effective_last_used(&idx_meta, now);
        let mut unit_files = vec![CleanCandidate {
            path: idx_path.clone(),
            size_bytes: idx_meta.len(),
            mtime: idx_meta.modified().unwrap_or(now),
            is_locked: false,
            profile_id: "package_managers".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: vec![idx_path.clone()],
        }];
        let mut total_bytes = idx_meta.len();
        let mut unit_blocked = false;

        for cpath in referenced {
            // Only remove content blobs exclusively owned by this index entry!
            if content_refcount.get(&cpath).copied().unwrap_or(0) != 1 {
                continue;
            }
            let Ok(cmeta) = std::fs::symlink_metadata(&cpath) else {
                continue;
            };
            if cmeta.file_type().is_symlink() || !cmeta.is_file() {
                unit_blocked = true;
                break;
            }
            if expected_dev.is_some() && device_id(&cmeta) != expected_dev {
                unit_blocked = true;
                break;
            }
            if lock_probe.is_file_locked(&cpath) {
                unit_blocked = true;
                break;
            }
            let cused = effective_last_used(&cmeta, now);
            if cused > newest_used {
                newest_used = cused;
            }
            total_bytes = total_bytes.saturating_add(cmeta.len());
            unit_files.push(CleanCandidate {
                path: cpath.clone(),
                size_bytes: cmeta.len(),
                mtime: cmeta.modified().unwrap_or(now),
                is_locked: false,
                profile_id: "package_managers".to_string(),
                is_canary: false,
                is_orphan: false,
                hardlink_siblings: vec![cpath],
            });
        }

        if unit_blocked {
            continue;
        }
        let age = now.duration_since(newest_used).unwrap_or(Duration::ZERO);
        if age < min_age {
            continue;
        }

        units.push(CleanUnit {
            root_path: idx_path.clone(),
            allowed_parent: cacache_root.to_path_buf(),
            files: unit_files,
            total_bytes,
            last_used: newest_used,
            profile_id: "package_managers".to_string(),
            kind: CleanUnitKind::RegenerableDevCache,
            dev_tool: Some(DevTool::Npm),
            unit_label: "npm_cache_entry".to_string(),
            root_identity: query_file_identity(&idx_path),
            min_age,
        });
    }

    units
}

/// Scans pnpm store conservatively: skips any file with `nlink > 1` (actively hardlinked to a project).
fn scan_pnpm_store(
    pnpm_root: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let mut units = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(pnpm_root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if ctrl.is_cancelled() {
            break;
        }
        if depth > 6 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            let low = fname.to_ascii_lowercase();
            if is_forbidden_dev_cache_name(&low)
                || low.starts_with("metadata")
                || low.ends_with(".json")
            {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if expected_dev.is_some() && device_id(&meta) != expected_dev {
                continue;
            }
            if meta.is_dir() {
                stack.push((p, depth + 1));
            } else if meta.is_file() {
                // Never delete pnpm store blobs still hardlinked into any project's node_modules
                // (works on both Unix via st_nlink and Windows via BY_HANDLE_FILE_INFORMATION).
                let identity = query_file_identity(&p);
                let nlinks = identity
                    .map(|id| u64::from(id.number_of_links))
                    .unwrap_or_else(|| nlink_count(&meta));
                if nlinks > 1 {
                    continue;
                }
                let used = effective_last_used(&meta, now);
                let age = now.duration_since(used).unwrap_or(Duration::ZERO);
                if age < min_age || lock_probe.is_file_locked(&p) {
                    continue;
                }
                let size = meta.len();
                units.push(CleanUnit {
                    root_path: p.clone(),
                    allowed_parent: pnpm_root.to_path_buf(),
                    files: vec![CleanCandidate {
                        path: p.clone(),
                        size_bytes: size,
                        mtime: meta.modified().unwrap_or(now),
                        is_locked: false,
                        profile_id: "package_managers".to_string(),
                        is_canary: false,
                        is_orphan: false,
                        hardlink_siblings: vec![p.clone()],
                    }],
                    total_bytes: size,
                    last_used: used,
                    profile_id: "package_managers".to_string(),
                    kind: CleanUnitKind::RegenerableDevCache,
                    dev_tool: Some(DevTool::Npm),
                    unit_label: "pnpm_store_blob".to_string(),
                    root_identity: identity,
                    min_age,
                });
            }
        }
    }
    units
}

/// Scans `~/.cache/pip` (`http-v2`, `http`, `wheels`) into atomic units.
fn scan_pip_cache(
    pip_root: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let mut units = Vec::new();
    for sub in ["http-v2", "http", "wheels"] {
        let sub_dir = pip_root.join(sub);
        if !sub_dir.is_dir() {
            continue;
        }
        let mut stack: Vec<(PathBuf, usize)> = vec![(sub_dir, 0)];
        while let Some((dir, depth)) = stack.pop() {
            if ctrl.is_cancelled() || depth > 6 {
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                let fname = entry.file_name().to_string_lossy().to_string();
                if is_forbidden_dev_cache_name(&fname) {
                    continue;
                }
                let Ok(meta) = std::fs::symlink_metadata(&p) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    continue;
                }
                if expected_dev.is_some() && device_id(&meta) != expected_dev {
                    continue;
                }
                if meta.is_dir() {
                    stack.push((p, depth + 1));
                } else if meta.is_file() {
                    let used = effective_last_used(&meta, now);
                    let age = now.duration_since(used).unwrap_or(Duration::ZERO);
                    if age < min_age || lock_probe.is_file_locked(&p) {
                        continue;
                    }
                    let size = meta.len();
                    units.push(CleanUnit {
                        root_path: p.clone(),
                        allowed_parent: pip_root.to_path_buf(),
                        files: vec![CleanCandidate {
                            path: p.clone(),
                            size_bytes: size,
                            mtime: meta.modified().unwrap_or(now),
                            is_locked: false,
                            profile_id: "package_managers".to_string(),
                            is_canary: false,
                            is_orphan: false,
                            hardlink_siblings: vec![p.clone()],
                        }],
                        total_bytes: size,
                        last_used: used,
                        profile_id: "package_managers".to_string(),
                        kind: CleanUnitKind::RegenerableDevCache,
                        dev_tool: Some(DevTool::Pip),
                        unit_label: "pip_cache_entry".to_string(),
                        root_identity: query_file_identity(&p),
                        min_age,
                    });
                }
            }
        }
    }
    units
}

/// Scans `~/.cache/uv` (`archive-v0/<hash>` atomic dirs). Never touches `simple-v*` metadata or lock files.
fn scan_uv_cache(
    uv_root: &Path,
    expected_dev: Option<u64>,
    min_age: Duration,
    now: SystemTime,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Vec<CleanUnit> {
    let mut units = Vec::new();
    let archive_dir = uv_root.join("archive-v0");
    if let Ok(rd) = std::fs::read_dir(&archive_dir) {
        for entry in rd.flatten() {
            if ctrl.is_cancelled() {
                break;
            }
            let p = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            if is_forbidden_dev_cache_name(&fname) {
                continue;
            }
            let Some(stats) =
                inspect_directory_unit(&p, expected_dev, "package_managers", now, lock_probe, ctrl)
            else {
                continue;
            };
            if stats.has_locked
                || stats.has_forbidden
                || stats.has_symlink_or_cross_dev
                || stats.files.is_empty()
            {
                continue;
            }
            let age = now
                .duration_since(stats.newest_used)
                .unwrap_or(Duration::ZERO);
            if age < min_age {
                continue;
            }
            units.push(CleanUnit {
                root_path: p.clone(),
                allowed_parent: uv_root.to_path_buf(),
                files: stats.files,
                total_bytes: stats.total_bytes,
                last_used: stats.newest_used,
                profile_id: "package_managers".to_string(),
                kind: CleanUnitKind::RegenerableDevCache,
                dev_tool: Some(DevTool::Pip),
                unit_label: "uv_archive".to_string(),
                root_identity: query_file_identity(&p),
                min_age,
            });
        }
    }
    units
}
