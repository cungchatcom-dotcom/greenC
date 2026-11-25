//! Orphaned App Leftover Radar: identifies `%APPDATA%` / `%LOCALAPPDATA%` directories left behind
//! by uninstalled applications by cross-referencing installed app names, shortcuts, and processes (Task T031).

use crate::walker::walk_directory;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// System and vendor umbrella folders that must never be flagged as orphaned app leftovers.
pub const PROTECTED_UMBRELLA_FOLDERS: &[&str] = &[
    "microsoft",
    "windows",
    "packages",
    "programs",
    "temp",
    "greenc",
    "greenc-store",
    "green c",
    "windowsapps",
    "google",
    "mozilla",
    "apple",
    "nvidia",
    "nvidia corporation",
    "amd",
    "intel",
    "valve",
    "steam",
    "docker",
    "npm-cache",
    "pip",
    "uv",
    "cargo",
    "rustup",
    "connecteddevicesplatform",
    "d3dscache",
    "crashdumps",
    "comms",
];

/// Minimum size (`20 MiB`) and inactivity age (`30 days`) before flagging a directory as an orphan candidate.
pub const DEFAULT_MIN_ORPHAN_BYTES: u64 = 20 * 1024 * 1024;
pub const DEFAULT_MIN_INACTIVE_DAYS: u64 = 30;

/// Discovered leftover directory from a previously uninstalled application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanLeftoverCandidate {
    pub id: String,
    pub folder_name: String,
    pub path: PathBuf,
    pub physical_bytes: u64,
    pub file_count: u64,
    pub inactive_days: u64,
}

/// Evaluates user AppData directories against known installed application tokens and active processes.
pub struct OrphanLeftoverRadar {
    pub min_bytes: u64,
    pub min_inactive_days: u64,
}

impl Default for OrphanLeftoverRadar {
    fn default() -> Self {
        Self {
            min_bytes: DEFAULT_MIN_ORPHAN_BYTES,
            min_inactive_days: DEFAULT_MIN_INACTIVE_DAYS,
        }
    }
}

impl OrphanLeftoverRadar {
    pub fn new(min_bytes: u64, min_inactive_days: u64) -> Self {
        Self {
            min_bytes,
            min_inactive_days,
        }
    }

    /// Scans `appdata_roots` and returns directories that do not match any installed app token,
    /// Start Menu shortcut token, active process name, or protected vendor umbrella.
    pub fn scan(
        &self,
        appdata_roots: &[PathBuf],
        installed_app_names: &[String],
        running_processes: &[String],
    ) -> Vec<OrphanLeftoverCandidate> {
        let protected: HashSet<&str> = PROTECTED_UMBRELLA_FOLDERS.iter().copied().collect();
        let active_tokens = Self::build_normalized_tokens(installed_app_names, running_processes);
        let now = SystemTime::now();
        let min_age = Duration::from_secs(self.min_inactive_days * 86_400);

        let mut results = Vec::new();

        for root in appdata_roots {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    continue;
                }

                let folder_name = entry.file_name().to_string_lossy().to_string();
                let lower = folder_name.to_lowercase();
                if protected.contains(lower.as_str()) {
                    continue;
                }

                let slug = Self::normalize_token(&lower);
                if slug.len() < 3 || Self::matches_any_installed(&slug, &active_tokens) {
                    continue;
                }

                let inactive_dur = meta
                    .modified()
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .unwrap_or(Duration::ZERO);
                if inactive_dur < min_age {
                    continue;
                }

                let Ok(space_node) = walk_directory(&path, u8::MAX) else {
                    continue;
                };
                if space_node.physical_bytes < self.min_bytes {
                    continue;
                }

                results.push(OrphanLeftoverCandidate {
                    id: format!("orphan-{slug}"),
                    folder_name,
                    path,
                    physical_bytes: space_node.physical_bytes,
                    file_count: space_node.file_count,
                    inactive_days: inactive_dur.as_secs() / 86_400,
                });
            }
        }

        results.sort_by_key(|b| std::cmp::Reverse(b.physical_bytes));
        results
    }

    /// Scans `appdata_roots` after automatically collecting installed Start Menu `.lnk` names.
    pub fn scan_roots(
        &self,
        appdata_roots: &[PathBuf],
        running_processes: &[String],
    ) -> Vec<OrphanLeftoverCandidate> {
        let mut start_dirs = Vec::new();
        if let Ok(appdata) = std::env::var("APPDATA") {
            start_dirs.push(
                PathBuf::from(appdata)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs"),
            );
        }
        if let Ok(progdata) = std::env::var("ProgramData") {
            start_dirs.push(
                PathBuf::from(progdata)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs"),
            );
        }
        let dir_refs: Vec<&Path> = start_dirs.iter().map(PathBuf::as_path).collect();
        let installed = Self::collect_start_menu_names(&dir_refs);
        self.scan(appdata_roots, &installed, running_processes)
    }

    fn normalize_token(raw: &str) -> String {
        raw.trim_end_matches(".exe")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    }

    fn build_normalized_tokens(installed: &[String], running: &[String]) -> Vec<String> {
        installed
            .iter()
            .chain(running.iter())
            .map(|s| Self::normalize_token(s))
            .filter(|t| t.len() >= 3)
            .collect()
    }

    fn matches_any_installed(folder_slug: &str, installed_tokens: &[String]) -> bool {
        installed_tokens
            .iter()
            .any(|tok| tok.contains(folder_slug) || folder_slug.contains(tok))
    }

    /// Collects Start Menu `.lnk` stem names from user and common Start Menu Programs directories.
    pub fn collect_start_menu_names(start_menu_dirs: &[&Path]) -> Vec<String> {
        let mut names = Vec::new();
        let mut stack: Vec<PathBuf> = start_menu_dirs.iter().map(|p| p.to_path_buf()).collect();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
                {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
        names
    }
}
