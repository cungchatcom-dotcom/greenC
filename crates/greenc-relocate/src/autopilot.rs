//! Pure, deterministic planning & safety rules for Goal-Driven 5-Tier "Make Green Now" sweeps.
//!
//! Tier order (stopping early as soon as the drive goal is reached, unless `max_space` is set):
//! - Tier 0: Compute `bytes_needed` from the user's drive goal (`target_ratio` / `clean_ratio`).
//! - Tier 1: Pure junk (`windows_temp`, `crash_dumps`, `app_logs`, `old_versions`, `orphan_radar`).
//! - Tier 2: Shallow, 100%-confident app caches ordered **least-recently-used first**.
//! - Tier 3: Deep per-app pass (WOF compact + relocate heavy data) ordered **least-recently-used first**.
//! - Tier 4: Removable USB / external drive backup prompt when still short of goal.
//! - Tier 5: Unused-app uninstall suggestions ranked least-used & largest first when still short of goal.

use crate::candidate::RelocationCandidate;
use crate::compress::CompactionCandidate;
use greenc_core::models::{DriveInfo, RelocationStrategyKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MIN_EXTERNAL_BACKUP_FREE_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB minimum

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedAppSummary {
    pub app_id: String,
    pub app_name: String,
    pub size_bytes: u64,
    pub process_names: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalBackupDrive {
    pub root: String,
    pub drive_letter: String,
    pub label: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub is_removable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepMode {
    /// Stop as soon as `bytes_needed` to reach the drive goal has been freed.
    Goal,
    /// Only run Tier 1 (pure junk) — offered when the drive is already green.
    Light,
    /// Run all tiers (Tier 1 → 2 → 3) without stopping early to maximize free space.
    MaxSpace,
}

impl SweepMode {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.to_lowercase()).as_deref() {
            Some("light") => Self::Light,
            Some("max_space") | Some("max") | Some("deep") => Self::MaxSpace,
            _ => Self::Goal,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Light => "light",
            Self::MaxSpace => "max_space",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalBudget {
    pub mode: SweepMode,
    pub clean_ratio: f64,
    pub target_ratio: f64,
    pub bytes_needed: u64,
}

impl GoalBudget {
    /// Computes how many bytes must be freed to bring `used_bytes / total_bytes` down to `target_ratio`.
    pub fn compute(
        total_bytes: u64,
        used_bytes: u64,
        clean_ratio: f64,
        target_ratio: f64,
        mode: SweepMode,
    ) -> Self {
        let clean = clean_ratio.clamp(0.50, 0.98);
        let target = target_ratio.clamp(0.40, clean);
        let target_used = (total_bytes as f64 * target) as u64;
        let bytes_needed = used_bytes.saturating_sub(target_used);
        Self {
            mode,
            clean_ratio: clean,
            target_ratio: target,
            bytes_needed,
        }
    }

    /// `true` once we have freed enough bytes to stop running deeper tiers.
    pub fn is_reached(&self, freed_so_far: u64) -> bool {
        match self.mode {
            SweepMode::MaxSpace => false,
            SweepMode::Light => false,
            SweepMode::Goal => self.bytes_needed > 0 && freed_so_far >= self.bytes_needed,
        }
    }

    /// Remaining bytes still needed to reach the goal after freeing `freed_so_far`.
    pub fn remaining(&self, freed_so_far: u64) -> u64 {
        self.bytes_needed.saturating_sub(freed_so_far)
    }
}

/// Tier 1: Pure junk profiles (least important, genuinely disposable).
pub fn tier1_pure_junk_profiles(
    has_backup_drive: bool,
    single_drive_mode: Option<&str>,
) -> Vec<&'static str> {
    if !has_backup_drive && single_drive_mode == Some("direct_cache") {
        // Never direct-delete `old_versions` (Windows.old) or `orphan_radar` without backup!
        vec!["windows_temp"]
    } else {
        vec![
            "windows_temp",
            "crash_dumps",
            "app_logs",
            "old_versions",
            "orphan_radar",
        ]
    }
}

/// Tier 2: Shallow, 100%-confident regenerable app cache profiles (sorted least-used-first per app).
pub fn tier2_shallow_app_profiles() -> Vec<&'static str> {
    vec!["browser_cache", "chat_media", "package_managers"]
}

/// Legacy combined helper preserved for tests/callers.
pub fn safe_profiles_for_mode(
    has_backup_drive: bool,
    single_drive_mode: Option<&str>,
) -> Vec<&'static str> {
    let mut all = tier1_pure_junk_profiles(has_backup_drive, single_drive_mode);
    all.extend(tier2_shallow_app_profiles());
    all
}

/// Estimates the most recent activity timestamp (Unix seconds) across a set of file/dir paths.
/// Returns `0` if timestamps cannot be read (so unknown/stale items sort with oldest).
pub fn estimate_last_used_epoch(paths: &[PathBuf]) -> u64 {
    let mut latest = 0u64;
    for p in paths {
        if let Ok(meta) = std::fs::metadata(p) {
            if let Ok(mtime) = meta.modified() {
                let secs = mtime
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if secs > latest {
                    latest = secs;
                }
            }
        }
        // Sample immediate children for directories so parent dir mtime alone isn't misleading.
        if p.is_dir() {
            if let Ok(entries) = std::fs::read_dir(p) {
                for entry in entries.flatten().take(48) {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            let secs = mtime
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            if secs > latest {
                                latest = secs;
                            }
                        }
                    }
                }
            }
        }
    }
    latest
}

/// Estimates days since `last_used_epoch` relative to `now`.
pub fn inactive_days_from_epoch(last_used_epoch: u64, now: SystemTime) -> u64 {
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now_secs.saturating_sub(last_used_epoch) / 86_400
}

/// Sorts keyed items by **least-recently-used first** (`last_used_epoch` ascending),
/// breaking ties by largest `size_bytes` descending so heavy stale items go first.
pub fn sort_least_used_first<T, F>(items: &mut [T], mut key_fn: F)
where
    F: FnMut(&T) -> (u64, u64), // (last_used_epoch, size_bytes)
{
    items.sort_by(|a, b| {
        let (ts_a, sz_a) = key_fn(a);
        let (ts_b, sz_b) = key_fn(b);
        ts_a.cmp(&ts_b).then_with(|| sz_b.cmp(&sz_a))
    });
}

/// Detects attached removable USB / external drives suitable for ZIP backup (NEVER NTFS junctions).
///
/// Accepts non-system writable drives (removable/exFAT/FAT32 USB sticks AND any additional
/// external NTFS HDD not already used as `exclude_drive`) with at least 512 MiB free space.
pub fn find_external_backup_drives(
    drives: &[DriveInfo],
    exclude_drive: Option<char>,
) -> Vec<ExternalBackupDrive> {
    let excluded = exclude_drive.map(|c| c.to_ascii_uppercase());
    let mut out = Vec::new();
    for d in drives {
        if d.is_system || d.free_bytes < MIN_EXTERNAL_BACKUP_FREE_BYTES {
            continue;
        }
        if excluded == Some(d.drive_letter.to_ascii_uppercase()) {
            continue;
        }
        let label_lower = d.label.to_lowercase();
        if label_lower.contains("recovery") || label_lower.contains("system reserved") {
            continue;
        }
        let fs_lower = d.filesystem.to_lowercase();
        let writable_fs = matches!(
            fs_lower.as_str(),
            "ntfs"
                | "exfat"
                | "fat32"
                | "vfat"
                | "ext4"
                | "ext3"
                | "btrfs"
                | "xfs"
                | "apfs"
                | "hfs+"
        );
        if !writable_fs {
            continue;
        }
        let is_removable = !d.is_eligible;
        out.push(ExternalBackupDrive {
            root: d.root.to_string_lossy().to_string(),
            drive_letter: d.drive_letter.to_string(),
            label: d.label.clone(),
            filesystem: d.filesystem.clone(),
            total_bytes: d.total_bytes,
            free_bytes: d.free_bytes,
            is_removable,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.free_bytes));
    out
}

/// Filters relocation candidates for 1-click `make_green_now` Tier 3:
/// - Strictly curated allowlist (`!is_advanced`) — never arbitrary AppData folders.
/// - Only `Junction` / `ToolConfig` strategies with `is_eligible == true` and `size_bytes > 0`.
/// - Partitions into `(unlocked_ready_to_move, skipped_locked)` sorted **least-recently-used first**.
pub fn select_safe_relocation_candidates(
    candidates: Vec<RelocationCandidate>,
) -> (Vec<RelocationCandidate>, Vec<LockedAppSummary>) {
    let mut ready = Vec::new();
    let mut locked = Vec::new();

    for c in candidates {
        if c.is_advanced || c.size_bytes == 0 || !c.is_eligible {
            continue;
        }
        if !matches!(
            c.strategy,
            RelocationStrategyKind::Junction | RelocationStrategyKind::ToolConfig
        ) {
            continue;
        }
        if c.id.eq_ignore_ascii_case("zalo") {
            continue;
        }
        if c.is_locked {
            locked.push(LockedAppSummary {
                app_id: c.id,
                app_name: c.name.clone(),
                size_bytes: c.size_bytes,
                process_names: c.locked_processes,
                reason: format!("{} đang mở nên tạm bỏ qua", c.name),
            });
        } else {
            ready.push(c);
        }
    }

    // Order unlocked candidates least-recently-used first (tie-break: largest first)
    let mut keyed: Vec<(u64, RelocationCandidate)> = ready
        .into_iter()
        .map(|c| {
            let ts = estimate_last_used_epoch(std::slice::from_ref(&c.source_path));
            (ts, c)
        })
        .collect();
    sort_least_used_first(&mut keyed, |(ts, c)| (*ts, c.size_bytes));
    let sorted_ready = keyed.into_iter().map(|(_, c)| c).collect();

    (sorted_ready, locked)
}

fn norm_path(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    let na = norm_path(a);
    let nb = norm_path(b);
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    na == nb || na.starts_with(&format!("{nb}\\")) || nb.starts_with(&format!("{na}\\"))
}

/// Excludes compaction candidates that overlap any folder scheduled for relocation,
/// preventing wasted CPU compressing files that are immediately being moved off `C:`.
pub fn filter_non_overlapping_compaction_dirs(
    compaction_dirs: Vec<CompactionCandidate>,
    planned_relocate_paths: &[PathBuf],
) -> Vec<CompactionCandidate> {
    if planned_relocate_paths.is_empty() {
        return compaction_dirs;
    }
    compaction_dirs
        .into_iter()
        .filter(|c| {
            !planned_relocate_paths
                .iter()
                .any(|rp| paths_overlap(&c.path, rp))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_cand(
        id: &str,
        path: &str,
        strategy: RelocationStrategyKind,
        is_advanced: bool,
        is_eligible: bool,
        is_locked: bool,
        size: u64,
    ) -> RelocationCandidate {
        RelocationCandidate {
            id: id.to_string(),
            name: id.to_string(),
            strategy,
            source_path: PathBuf::from(path),
            default_target_path: PathBuf::from(format!(r"D:\GreenC\Relocated\{id}")),
            size_bytes: size,
            file_count: 10,
            is_locked,
            locked_processes: if is_locked {
                vec![format!("{id}.exe")]
            } else {
                vec![]
            },
            how_description: String::new(),
            is_advanced,
            is_eligible,
            ineligible_reason: None,
        }
    }

    #[test]
    fn goal_budget_computes_bytes_needed_and_stops_when_reached() {
        let total = 100_000_000_000u64; // 100 GB
        let used = 88_000_000_000u64; // 88 GB (88%)
        let budget = GoalBudget::compute(total, used, 0.80, 0.75, SweepMode::Goal);
        assert_eq!(budget.bytes_needed, 13_000_000_000);
        assert!(!budget.is_reached(5_000_000_000));
        assert!(budget.is_reached(13_000_000_000));
        assert_eq!(budget.remaining(5_000_000_000), 8_000_000_000);

        let max_budget = GoalBudget::compute(total, used, 0.80, 0.75, SweepMode::MaxSpace);
        assert!(!max_budget.is_reached(50_000_000_000));
    }

    #[test]
    fn least_used_sort_orders_oldest_then_largest() {
        let mut apps = vec![
            ("Chrome", 1_700_000_900u64, 5_000u64),
            ("OldAppSmall", 1_600_000_000, 1_000),
            ("OldAppBig", 1_600_000_000, 9_000),
            ("MidApp", 1_650_000_000, 3_000),
        ];
        sort_least_used_first(&mut apps, |x| (x.1, x.2));
        let names: Vec<&str> = apps.iter().map(|x| x.0).collect();
        assert_eq!(names, vec!["OldAppBig", "OldAppSmall", "MidApp", "Chrome"]);
    }

    #[test]
    fn finds_removable_usb_and_exfat_external_drives_only() {
        let drives = vec![
            DriveInfo {
                root: PathBuf::from("C:\\"),
                drive_letter: 'C',
                label: "Windows".to_string(),
                filesystem: "NTFS".to_string(),
                total_bytes: 256_000_000_000,
                free_bytes: 10_000_000_000,
                is_system: true,
                is_eligible: false,
                ineligible_reason: None,
            },
            DriveInfo {
                root: PathBuf::from("D:\\"),
                drive_letter: 'D',
                label: "Data".to_string(),
                filesystem: "NTFS".to_string(),
                total_bytes: 512_000_000_000,
                free_bytes: 200_000_000_000,
                is_system: false,
                is_eligible: true,
                ineligible_reason: None,
            },
            DriveInfo {
                root: PathBuf::from("E:\\"),
                drive_letter: 'E',
                label: "MyUSB".to_string(),
                filesystem: "exFAT".to_string(),
                total_bytes: 128_000_000_000,
                free_bytes: 80_000_000_000,
                is_system: false,
                is_eligible: false,
                ineligible_reason: Some("Ổ di động".to_string()),
            },
            DriveInfo {
                root: PathBuf::from("F:\\"),
                drive_letter: 'F',
                label: "Recovery".to_string(),
                filesystem: "NTFS".to_string(),
                total_bytes: 10_000_000_000,
                free_bytes: 5_000_000_000,
                is_system: false,
                is_eligible: false,
                ineligible_reason: Some("Recovery".to_string()),
            },
        ];

        let ext = find_external_backup_drives(&drives, Some('D'));
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0].drive_letter, "E");
        assert_eq!(ext[0].label, "MyUSB");
        assert!(ext[0].is_removable);

        // Without exclude_drive (e.g. NTFS external HDD plugged into 1-drive PC), D: is also eligible for ZIP backup
        let ext_all = find_external_backup_drives(&drives, None);
        assert_eq!(ext_all.len(), 2);
        assert_eq!(ext_all[0].drive_letter, "D");
        assert_eq!(ext_all[1].drive_letter, "E");
    }

    #[test]
    fn curated_filter_excludes_is_advanced_guided_and_partitions_locked() {
        let cands = vec![
            mk_cand(
                "telegram",
                r"C:\Users\A\AppData\Roaming\Telegram Desktop",
                RelocationStrategyKind::Junction,
                false,
                true,
                false,
                1000,
            ),
            mk_cand(
                "discord",
                r"C:\Users\A\AppData\Roaming\discord",
                RelocationStrategyKind::Junction,
                false,
                true,
                true,
                2000,
            ),
            mk_cand(
                "random_adv",
                r"C:\Users\A\AppData\Local\RandomApp",
                RelocationStrategyKind::Junction,
                true,
                true,
                false,
                5000,
            ),
            mk_cand(
                "wsl2",
                r"C:\Users\A\AppData\Local\Packages",
                RelocationStrategyKind::Guided,
                false,
                true,
                false,
                9000,
            ),
            mk_cand(
                "zalo",
                r"C:\Users\A\Zalo Data",
                RelocationStrategyKind::Junction,
                false,
                true,
                false,
                4000,
            ),
        ];

        let (ready, locked) = select_safe_relocation_candidates(cands);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "telegram");
        assert_eq!(locked.len(), 1);
        assert_eq!(locked[0].app_id, "discord");
        assert_eq!(locked[0].size_bytes, 2000);
    }

    #[test]
    fn phase3_compaction_excludes_folders_overlapping_phase4_relocation() {
        let comp = vec![
            CompactionCandidate {
                path: PathBuf::from(r"C:\Users\A\.cargo"),
                logical_bytes: 10_000,
                file_count: 50,
                estimated_reclaim_bytes: 4_000,
                category: "sdk".to_string(),
            },
            CompactionCandidate {
                path: PathBuf::from(r"C:\Users\A\.rustup"),
                logical_bytes: 20_000,
                file_count: 80,
                estimated_reclaim_bytes: 8_000,
                category: "sdk".to_string(),
            },
        ];
        let relocate_paths = vec![PathBuf::from(r"C:\Users\A\.cargo")];
        let filtered = filter_non_overlapping_compaction_dirs(comp, &relocate_paths);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, PathBuf::from(r"C:\Users\A\.rustup"));
    }

    #[test]
    fn single_drive_direct_cache_never_includes_old_versions_or_orphan() {
        let direct = safe_profiles_for_mode(false, Some("direct_cache"));
        assert!(!direct.contains(&"old_versions"));
        assert!(!direct.contains(&"orphan_radar"));
        assert!(direct.contains(&"browser_cache"));
        assert!(direct.contains(&"chat_media"));

        let vault = safe_profiles_for_mode(false, Some("local_vault"));
        assert!(vault.contains(&"old_versions"));
        assert!(vault.contains(&"orphan_radar"));
    }
}
