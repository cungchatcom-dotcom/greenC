//! Pure, deterministic planning & safety rules for 1-click "Make Green Now" autopilot sweeps.

use crate::candidate::RelocationCandidate;
use crate::compress::CompactionCandidate;
use greenc_core::models::RelocationStrategyKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedAppSummary {
    pub app_id: String,
    pub app_name: String,
    pub size_bytes: u64,
    pub process_names: Vec<String>,
    pub reason: String,
}

/// Returns cleanup profile IDs allowed under the given mode.
///
/// - When `has_backup_drive` is `true`, or `single_drive_mode == "local_vault"`, all standard
///   safe profiles are archived into Vault (fully restorable).
/// - When `has_backup_drive` is `false` and `single_drive_mode == "direct_cache"`, ONLY strictly
///   regenerable caches are included (`windows_temp`, `browser_cache`, `chat_media`, `package_managers`)
///   — NEVER `old_versions` (`Windows.old`), `crash_dumps`, `app_logs`, or `orphan_radar`.
pub fn safe_profiles_for_mode(
    has_backup_drive: bool,
    single_drive_mode: Option<&str>,
) -> Vec<&'static str> {
    if !has_backup_drive && single_drive_mode == Some("direct_cache") {
        vec![
            "windows_temp",
            "browser_cache",
            "chat_media",
            "package_managers",
        ]
    } else {
        vec![
            "windows_temp",
            "browser_cache",
            "chat_media",
            "crash_dumps",
            "package_managers",
            "app_logs",
            "old_versions",
        ]
    }
}

/// Filters relocation candidates for 1-click `make_green_now` Phase 4:
/// - Strictly curated allowlist (`!is_advanced`) — never arbitrary AppData folders.
/// - Only `Junction` / `ToolConfig` strategies with `is_eligible == true` and `size_bytes > 0`.
/// - Partitions into `(unlocked_ready_to_move, skipped_locked)`.
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
            // Zalo uses dedicated ZaloRelocator flow handled separately by autopilot.
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

    (ready, locked)
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

/// Excludes compaction candidates that overlap any folder scheduled for Phase 4 relocation,
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
        .filter(|c| !planned_relocate_paths.iter().any(|rp| paths_overlap(&c.path, rp)))
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
    fn curated_filter_excludes_is_advanced_guided_and_partitions_locked() {
        let cands = vec![
            mk_cand("telegram", r"C:\Users\A\AppData\Roaming\Telegram Desktop", RelocationStrategyKind::Junction, false, true, false, 1000),
            mk_cand("discord", r"C:\Users\A\AppData\Roaming\discord", RelocationStrategyKind::Junction, false, true, true, 2000),
            mk_cand("random_adv", r"C:\Users\A\AppData\Local\RandomApp", RelocationStrategyKind::Junction, true, true, false, 5000),
            mk_cand("wsl2", r"C:\Users\A\AppData\Local\Packages", RelocationStrategyKind::Guided, false, true, false, 9000),
            mk_cand("zalo", r"C:\Users\A\Zalo Data", RelocationStrategyKind::Junction, false, true, false, 4000),
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
        let backup = safe_profiles_for_mode(true, None);
        assert!(backup.contains(&"old_versions"));
    }
}
