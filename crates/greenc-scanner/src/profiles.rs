//! Deep application storage profiles (Zalo multi-drive `config.json`, inactive `Zalo-*` versions,
//! DingTalk `*_v2/EAppFiles`, browser & developer tool caches) with HardLink deduplication (Task T030).

use crate::walker::walk_directory;
use crate::zalo::{discover_zalo_installation, ZaloDiscoveryOptions};
use greenc_core::config::DEFAULT_PROTECTED_PATTERNS;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Discovered storage footprint for an installed desktop application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredAppProfile {
    pub app_id: String,
    pub app_name: String,
    pub process_name: String,
    pub service_name: String,
    pub description: String,
    pub reclaimable_paths: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
    pub exclude_patterns: Vec<String>,
    pub recommended_max_bytes: u64,
    pub physical_bytes: u64,
}

/// Parses Zalo PC `%APPDATA%\ZaloData\config.json` safely to discover chat data (ALWAYS protected:
/// `message` and `media` are never cleaned), allowlisted regenerable roaming caches (including
/// `Partitions\*`), and obsolete `Zalo-*` version directories not matching any running `Zalo.exe`
/// and older than 3 days (keeping running + newest, or 2 newest when Zalo is not running).
pub fn discover_zalo_profile(
    config_file: Option<&Path>,
    local_appdata: Option<&Path>,
    running_exe_dirs: &HashSet<PathBuf>,
) -> Option<DiscoveredAppProfile> {
    let opts = ZaloDiscoveryOptions {
        config_file: config_file.map(Path::to_path_buf),
        local_appdata: local_appdata.map(Path::to_path_buf),
        running_exe_dirs: running_exe_dirs.clone(),
        ..Default::default()
    };
    let discovery = discover_zalo_installation(&opts)?;

    let mut reclaimable: Vec<PathBuf> = discovery
        .cache_dirs
        .iter()
        .map(|c| c.path.clone())
        .collect();
    reclaimable.extend(discovery.old_version_dirs.iter().cloned());

    if reclaimable.is_empty() && discovery.data_dir.is_none() {
        return None;
    }

    let physical_bytes = discovery
        .cache_bytes
        .saturating_add(discovery.old_versions_bytes);

    let mut excludes: Vec<String> = DEFAULT_PROTECTED_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    excludes.extend([
        "*.rescache".to_string(),
        "*\\Database\\*".to_string(),
        "*\\message\\*".to_string(),
        "*\\media\\*".to_string(),
        "config.json".to_string(),
    ]);

    Some(DiscoveredAppProfile {
        app_id: "zalo_pc".to_string(),
        app_name: "Zalo PC (Bộ đệm & Bản cũ)".to_string(),
        process_name: "Zalo.exe".to_string(),
        service_name: String::new(),
        description: "Bộ đệm tái tạo được và các phiên bản Zalo cũ bị bỏ quên; giữ nguyên hoàn toàn tin nhắn, ảnh và đăng nhập.".to_string(),
        reclaimable_paths: reclaimable,
        protected_paths: discovery.protected_paths,
        exclude_patterns: excludes,
        recommended_max_bytes: (2.5 * 1024.0 * 1024.0 * 1024.0) as u64,
        physical_bytes,
    })
}

/// Discovers DingTalk `*_v2\EAppFiles`, `resource_cache`, logs, and crash dumps while protecting `DBFiles`.
pub fn discover_dingtalk_profile(
    roaming_dingtalk: Option<&Path>,
    local_dingtalk: Option<&Path>,
) -> Option<DiscoveredAppProfile> {
    let roaming_dt = roaming_dingtalk
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("DingTalk")))?;
    let local_dt = local_dingtalk
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("DingTalk")))?;

    let mut reclaimable = Vec::new();
    let mut protected = Vec::new();

    if let Ok(entries) = fs::read_dir(&roaming_dt) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with("_v2") {
                for rel in [
                    Path::new("EAppFiles").join("download"),
                    Path::new("EAppFiles").join("unziped"),
                    PathBuf::from("resource_cache"),
                    PathBuf::from("AvatarFiles"),
                ] {
                    let cand = p.join(rel);
                    if cand.is_dir() {
                        reclaimable.push(cand);
                    }
                }
                for prot in ["DBFiles", "dtnest_db", "nest_module_shared_db"] {
                    let prot_p = p.join(prot);
                    if prot_p.is_dir() {
                        protected.push(prot_p);
                    }
                }
            } else if matches!(
                name.to_lowercase().as_str(),
                "log" | "holmeslogs" | "updaterlogs"
            ) {
                reclaimable.push(p);
            }
        }
    }

    let dumps = local_dt.join("dumps");
    if dumps.is_dir() {
        reclaimable.push(dumps);
    }

    if reclaimable.is_empty() {
        return None;
    }

    let physical_bytes = sum_physical_bytes(&reclaimable);
    let mut excludes: Vec<String> = DEFAULT_PROTECTED_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    excludes.extend(["*\\DBFiles\\*".to_string(), "*\\dtnest_db\\*".to_string()]);

    Some(DiscoveredAppProfile {
        app_id: "dingtalk_pc".to_string(),
        app_name: "DingTalk Desktop (EApp Bundles, Resource Cache & Dumps)".to_string(),
        process_name: "DingTalk.exe".to_string(),
        service_name: String::new(),
        description: "Quản lý các gói Mini-App (.amr) trong EAppFiles/download, EAppFiles/unziped, kho ảnh resource_cache và Crash Dumps của DingTalk.".to_string(),
        reclaimable_paths: reclaimable,
        protected_paths: protected,
        exclude_patterns: excludes,
        recommended_max_bytes: 3 * 1024 * 1024 * 1024,
        physical_bytes,
    })
}

fn sum_physical_bytes(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .filter_map(|p| walk_directory(p, u8::MAX).ok())
        .map(|n| n.physical_bytes)
        .sum()
}
