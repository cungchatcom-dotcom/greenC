//! Deep application storage profiles (Zalo multi-drive `config.json`, inactive `Zalo-*` versions,
//! DingTalk `*_v2/EAppFiles`, browser & developer tool caches) with HardLink deduplication (Task T030).

use crate::walker::walk_directory;
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

/// Parses Zalo PC `%APPDATA%\ZaloData\config.json` to discover cross-drive media folders,
/// roaming cache, and obsolete `Zalo-*` version directories not matching any running `Zalo.exe`.
pub fn discover_zalo_profile(
    config_file: Option<&Path>,
    local_appdata: Option<&Path>,
    running_exe_dirs: &HashSet<PathBuf>,
) -> Option<DiscoveredAppProfile> {
    let cfg_path = config_file.map(Path::to_path_buf).or_else(|| {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| {
                    let home = PathBuf::from(h);
                    if cfg!(target_os = "macos") {
                        home.join("Library").join("Application Support")
                    } else {
                        home.join(".config")
                    }
                })
            })
            .map(|base| base.join("ZaloData").join("config.json"))
    })?;
    let local = local_appdata
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let home = PathBuf::from(h);
                if cfg!(target_os = "macos") {
                    home.join("Library").join("Application Support")
                } else {
                    home.join(".local").join("share")
                }
            })
        })?;

    let mut reclaimable = Vec::new();
    let mut protected = Vec::new();

    if cfg_path.is_file() {
        if let Ok(raw) = fs::read_to_string(&cfg_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(db_loc) = json
                    .get("db_path")
                    .and_then(|v| v.get("location"))
                    .and_then(|v| v.as_str())
                {
                    if let Some(db_root) = Path::new(db_loc).parent() {
                        let media_dir = db_root.join("media");
                        let msg_dir = db_root.join("message");
                        if media_dir.is_dir() {
                            reclaimable.push(media_dir);
                        }
                        if msg_dir.is_dir() {
                            protected.push(msg_dir);
                        }
                    }
                }
            }
        }
    }

    if let Some(cfg_parent) = cfg_path.parent() {
        let cache_dir = cfg_parent.join("Cache");
        if cache_dir.is_dir() {
            reclaimable.push(cache_dir);
        }
    }

    let running_norm: HashSet<String> = running_exe_dirs
        .iter()
        .map(|p| p.to_string_lossy().to_lowercase())
        .collect();

    let programs_zalo = local.join("Programs").join("Zalo");
    if let Ok(entries) = fs::read_dir(&programs_zalo) {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if p.is_dir() && name.starts_with("zalo-") {
                if running_norm.contains(&p.to_string_lossy().to_lowercase()) {
                    protected.push(p);
                } else {
                    reclaimable.push(p);
                }
            }
        }
    }

    let pending = local.join("Zalo").join("pending");
    if pending.is_dir() {
        reclaimable.push(pending);
    }

    if reclaimable.is_empty() {
        return None;
    }

    let physical_bytes = sum_physical_bytes(&reclaimable);
    let mut excludes: Vec<String> = DEFAULT_PROTECTED_PATTERNS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    excludes.extend([
        "*.rescache".to_string(),
        "*\\Database\\*".to_string(),
        "*\\message\\*".to_string(),
    ]);

    Some(DiscoveredAppProfile {
        app_id: "zalo_pc".to_string(),
        app_name: "Zalo Desktop (Media, Cache & Old Versions)".to_string(),
        process_name: "Zalo.exe".to_string(),
        service_name: String::new(),
        description: "Quản lý kho Media Zalo (ZaloDownloads/resource, fileNoise hỗ trợ NTFS HardLink), Zalo Cache và các bản cài đặt Zalo cũ bị bỏ quên.".to_string(),
        reclaimable_paths: reclaimable,
        protected_paths: protected,
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
