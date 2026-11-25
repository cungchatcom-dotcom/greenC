//! Safe Zalo PC discovery: `config.json` validation, chat data measurement,
//! allowlisted regenerable caches (root + `Partitions\*`), and obsolete `Zalo-x.y.z` versions.

use greenc_core::models::is_same_volume;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Hard cap when reading `%APPDATA%\ZaloData\config.json` (256 KiB).
pub const MAX_ZALO_CONFIG_BYTES: u64 = 256 * 1024;

/// Minimum idle age for obsolete `Zalo-x.y.z` installation directories (3 days).
pub const ZALO_OLD_VERSION_MIN_AGE_SECS: u64 = 3 * 86_400;

/// Safe minimum age for regenerable Zalo cache files (5 minutes).
pub const ZALO_CACHE_MIN_AGE_SECS: u64 = 300;

/// Allowlisted regenerable cache directory names directly under `%APPDATA%\ZaloData`.
pub const ZALO_ROOT_CACHE_ALLOWLIST: &[&str] =
    &["Cache", "Code Cache", "GPUCache", "DawnCache", "resp_cache"];

/// Allowlisted regenerable cache directory names under `%APPDATA%\ZaloData\Partitions\*`.
pub const ZALO_PARTITION_CACHE_ALLOWLIST: &[&str] =
    &["Cache", "Code Cache", "GPUCache", "DawnCache"];

/// Directories that MUST NEVER be touched by automated Zalo cleanup.
pub const ZALO_PROTECTED_DIR_NAMES: &[&str] = &[
    "Local Storage",
    "Session Storage",
    "IndexedDB",
    "WebStorage",
    "Network",
    "databases",
    "shared_proto_db",
    "Service Worker",
    "blob_storage",
    "tf-models",
    "message",
    "media",
];

/// Zalo process executable names on Windows.
pub const ZALO_PROCESS_NAMES: &[&str] = &["Zalo.exe", "ZaloCall.exe", "ZaloCap.exe"];

/// Status of parsing `%APPDATA%\ZaloData\config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ZaloConfigStatus {
    Found,
    Missing,
    Invalid { reason: String },
    RejectedUnsafe { reason: String },
}

/// Discovered allowlisted regenerable cache directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloCacheDirInfo {
    pub path: PathBuf,
    pub label: String,
    pub total_bytes: u64,
    pub file_count: u64,
}

/// Discovered `Zalo-x.y.z` installation directory under `%LOCALAPPDATA%\Programs\Zalo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloVersionDirInfo {
    pub path: PathBuf,
    pub version_str: String,
    pub version_parts: Vec<u64>,
    pub total_bytes: u64,
    pub file_count: u64,
    pub is_running: bool,
    pub is_newest: bool,
    pub is_kept: bool,
    pub keep_reason: Option<String>,
    pub eligible_for_clean: bool,
    pub age_secs: u64,
}

/// Complete discovery snapshot of a Zalo PC installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloDiscovery {
    pub installed: bool,
    pub config_path: PathBuf,
    pub config_status: ZaloConfigStatus,
    pub roaming_dir: PathBuf,
    pub programs_dir: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub data_drive: Option<String>,
    pub data_on_system_drive: bool,
    pub data_is_known_relocation: bool,
    pub chat_data_bytes: u64,
    pub chat_data_file_count: u64,
    pub media_bytes: u64,
    pub media_file_count: u64,
    pub message_bytes: u64,
    pub message_file_count: u64,
    pub cache_dirs: Vec<ZaloCacheDirInfo>,
    pub cache_bytes: u64,
    pub versions: Vec<ZaloVersionDirInfo>,
    pub old_versions_bytes: u64,
    pub old_version_dirs: Vec<PathBuf>,
    pub kept_version_dirs: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Optional parameters for deterministic testing and customization of Zalo discovery.
#[derive(Debug, Clone)]
pub struct ZaloDiscoveryOptions {
    pub config_file: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub system_drive_root: Option<PathBuf>,
    pub running_exe_dirs: HashSet<PathBuf>,
    /// Source paths of active GreenC-managed relocations (`status = Done`), so our own
    /// junction at `Zalo Data` is recognized instead of rejected as a foreign symlink/junction.
    pub known_relocated_sources: HashSet<PathBuf>,
    pub now: SystemTime,
    pub min_old_version_age: Duration,
}

impl Default for ZaloDiscoveryOptions {
    fn default() -> Self {
        Self {
            config_file: None,
            local_appdata: None,
            system_drive_root: None,
            running_exe_dirs: HashSet::new(),
            known_relocated_sources: HashSet::new(),
            now: SystemTime::now(),
            min_old_version_age: Duration::from_secs(ZALO_OLD_VERSION_MIN_AGE_SECS),
        }
    }
}

/// Returns `true` if `name` is an allowlisted Zalo cache directory under `%APPDATA%\ZaloData`.
pub fn is_allowed_zalo_root_cache_name(name: &str) -> bool {
    ZALO_ROOT_CACHE_ALLOWLIST
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

/// Returns `true` if `name` is an allowlisted Zalo cache directory under `Partitions\*`.
pub fn is_allowed_zalo_partition_cache_name(name: &str) -> bool {
    ZALO_PARTITION_CACHE_ALLOWLIST
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

/// Returns `true` if `name` is a protected Zalo directory or database/config file that must NEVER be cleaned.
pub fn is_protected_zalo_entry(name: &str) -> bool {
    let low = name.trim().to_ascii_lowercase();
    if low.is_empty() || low == "config.json" {
        return true;
    }
    if low.contains(".db")
        || low.ends_with(".sqlite")
        || low.ends_with(".sqlite3")
        || low.ends_with(".rescache")
    {
        return true;
    }
    ZALO_PROTECTED_DIR_NAMES
        .iter()
        .any(|prot| low == prot.to_ascii_lowercase())
}

/// Parses a `Zalo-x.y.z` directory name into numeric version components (e.g. `Zalo-26.11.12` -> `[26, 11, 12]`).
pub fn parse_zalo_version(dir_name: &str) -> Option<(String, Vec<u64>)> {
    let trimmed = dir_name.trim();
    if trimmed.len() <= 5 || !trimmed[..5].eq_ignore_ascii_case("zalo-") {
        return None;
    }
    let ver_str = &trimmed[5..];
    let parts: Vec<&str> = ver_str.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        nums.push(part.parse::<u64>().ok()?);
    }
    Some((ver_str.to_string(), nums))
}

/// Checks whether a path is a symlink or (on Windows) any NTFS reparse point / junction.
pub fn is_symlink_or_reparse(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    let is_link = meta.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        is_link || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
    #[cfg(not(windows))]
    {
        is_link
    }
}

/// Extracts a Windows drive letter prefix like `"C:"` from a path if present.
pub fn extract_drive_label(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    let s_clean = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let mut chars = s_clean.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => {
            Some(format!("{}:", letter.to_ascii_uppercase()))
        }
        _ => None,
    }
}

fn normalize_cmp_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// Validates a raw path string read from `config.json`:
/// - Non-empty, no NUL/control chars
/// - Rejects UNC (`\\server\share`, `//server/share`, `\\?\...`, `\\.\...`)
/// - Rejects `..` path traversal segments
/// - Requires an absolute path (`X:\...` / `X:/...` or Unix `/...`)
pub fn validate_zalo_config_raw_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Đường dẫn trong config.json rỗng".to_string());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("Đường dẫn trong config.json chứa ký tự điều khiển không hợp lệ".to_string());
    }
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err("Từ chối đường dẫn mạng UNC hoặc thiết bị trong config.json".to_string());
    }
    for seg in trimmed.split(['\\', '/']) {
        if seg == ".." {
            return Err("Từ chối đường dẫn chứa '..' trong config.json".to_string());
        }
    }

    let bytes = trimmed.as_bytes();
    let is_win_abs = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    let is_unix_abs = trimmed.starts_with('/') && !trimmed.starts_with("//");
    if !is_win_abs && !is_unix_abs {
        return Err("Đường dẫn trong config.json không phải đường dẫn tuyệt đối".to_string());
    }

    let path = PathBuf::from(trimmed);
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return Err("Từ chối thành phần '..' trong config.json".to_string());
        }
    }

    Ok(path)
}

fn resolve_default_paths(
    config_file: Option<&Path>,
    local_appdata: Option<&Path>,
) -> Option<(PathBuf, PathBuf, PathBuf)> {
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

    let roaming_dir = cfg_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("ZaloData"));

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

    let programs_dir = local.join("Programs").join("Zalo");
    Some((cfg_path, roaming_dir, programs_dir))
}

/// Safely reads and validates Zalo `config.json`, returning `(status, Option<(data_dir, is_known_relocation)>)`.
fn parse_zalo_config_safe(
    cfg_path: &Path,
    known_relocated_norm: &HashSet<String>,
) -> (ZaloConfigStatus, Option<(PathBuf, bool)>) {
    let Ok(meta) = fs::symlink_metadata(cfg_path) else {
        return (ZaloConfigStatus::Missing, None);
    };
    if is_symlink_or_reparse(cfg_path) {
        return (
            ZaloConfigStatus::RejectedUnsafe {
                reason: "config.json là symlink/junction không an toàn".to_string(),
            },
            None,
        );
    }
    if !meta.is_file() {
        return (
            ZaloConfigStatus::Invalid {
                reason: "config.json không phải tệp tin hợp lệ".to_string(),
            },
            None,
        );
    }
    if meta.len() > MAX_ZALO_CONFIG_BYTES {
        return (
            ZaloConfigStatus::RejectedUnsafe {
                reason: format!(
                    "config.json vượt giới hạn kích thước ({} > {} bytes)",
                    meta.len(),
                    MAX_ZALO_CONFIG_BYTES
                ),
            },
            None,
        );
    }

    let Ok(file) = fs::File::open(cfg_path) else {
        return (
            ZaloConfigStatus::Invalid {
                reason: "Không đọc được config.json".to_string(),
            },
            None,
        );
    };
    let mut raw = Vec::new();
    if file
        .take(MAX_ZALO_CONFIG_BYTES + 1)
        .read_to_end(&mut raw)
        .is_err()
    {
        return (
            ZaloConfigStatus::Invalid {
                reason: "Lỗi đọc nội dung config.json".to_string(),
            },
            None,
        );
    }
    if raw.len() as u64 > MAX_ZALO_CONFIG_BYTES {
        return (
            ZaloConfigStatus::RejectedUnsafe {
                reason: "config.json vượt giới hạn kích thước an toàn".to_string(),
            },
            None,
        );
    }

    let Ok(text) = String::from_utf8(raw) else {
        return (
            ZaloConfigStatus::Invalid {
                reason: "config.json không phải UTF-8 hợp lệ".to_string(),
            },
            None,
        );
    };
    let clean = text.trim_start_matches('\u{feff}');
    let Ok(json) = serde_json::from_str::<serde_json::Value>(clean) else {
        return (
            ZaloConfigStatus::Invalid {
                reason: "config.json không đúng định dạng JSON".to_string(),
            },
            None,
        );
    };

    // Primary field: `db_path.location` = `<ROOT>\Zalo Data\message`
    let candidate_data_dir: Option<PathBuf> = if let Some(db_loc) = json
        .get("db_path")
        .and_then(|v| v.get("location"))
        .and_then(|v| v.as_str())
    {
        match validate_zalo_config_raw_path(db_loc) {
            Ok(db_path) => {
                if is_symlink_or_reparse(&db_path) {
                    return (
                        ZaloConfigStatus::RejectedUnsafe {
                            reason: "db_path.location là symlink/junction lạ".to_string(),
                        },
                        None,
                    );
                }
                let is_msg = db_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case("message"));
                if is_msg {
                    db_path.parent().map(Path::to_path_buf)
                } else {
                    Some(db_path)
                }
            }
            Err(reason) => return (ZaloConfigStatus::RejectedUnsafe { reason }, None),
        }
    } else if let Some(res_loc) = json
        .get("resource")
        .and_then(|v| v.get("location"))
        .and_then(|v| v.get("path"))
        .and_then(|v| v.as_str())
    {
        match validate_zalo_config_raw_path(res_loc) {
            Ok(root_path) => {
                if is_symlink_or_reparse(&root_path) {
                    return (
                        ZaloConfigStatus::RejectedUnsafe {
                            reason: "resource.location.path là symlink/junction lạ".to_string(),
                        },
                        None,
                    );
                }
                let zalo_data = root_path.join("Zalo Data");
                if zalo_data.exists() {
                    Some(zalo_data)
                } else {
                    None
                }
            }
            Err(reason) => return (ZaloConfigStatus::RejectedUnsafe { reason }, None),
        }
    } else {
        None
    };

    let Some(data_dir) = candidate_data_dir else {
        return (
            ZaloConfigStatus::Invalid {
                reason: "config.json thiếu trường db_path.location".to_string(),
            },
            None,
        );
    };

    // Validate `data_dir` itself against unexpected symlinks/junctions.
    let data_norm = normalize_cmp_path(&data_dir);
    let is_known_reloc = known_relocated_norm.contains(&data_norm);
    if is_symlink_or_reparse(&data_dir) && !is_known_reloc {
        return (
            ZaloConfigStatus::RejectedUnsafe {
                reason: "Thư mục Zalo Data là symlink/junction lạ".to_string(),
            },
            None,
        );
    }

    // Validate critical child folders `message` and `media` are never symlinks/junctions.
    let msg_dir = data_dir.join("message");
    let media_dir = data_dir.join("media");
    if is_symlink_or_reparse(&msg_dir) || is_symlink_or_reparse(&media_dir) {
        return (
            ZaloConfigStatus::RejectedUnsafe {
                reason: "Thư mục message hoặc media trong Zalo Data là symlink/junction lạ"
                    .to_string(),
            },
            None,
        );
    }

    (ZaloConfigStatus::Found, Some((data_dir, is_known_reloc)))
}

/// Measures recursive regular-file bytes, file count, and newest mtime without following symlinks.
pub fn measure_dir_safe(dir: &Path, now: SystemTime) -> (u64, u64, SystemTime) {
    let Ok(root_meta) = fs::symlink_metadata(dir) else {
        return (0, 0, now);
    };
    if is_symlink_or_reparse(dir) && !dir.is_dir() {
        return (0, 0, now);
    }
    let mut total_bytes = 0u64;
    let mut file_count = 0u64;
    let mut newest_mtime = root_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let mut stack = vec![dir.to_path_buf()];

    while let Some(curr) = stack.pop() {
        let Ok(entries) = fs::read_dir(&curr) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if is_symlink_or_reparse(&p) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if let Ok(mt) = meta.modified() {
                if mt > newest_mtime {
                    newest_mtime = mt;
                }
            }
            if meta.is_dir() {
                stack.push(p);
            } else if meta.is_file() {
                file_count = file_count.saturating_add(1);
                total_bytes = total_bytes.saturating_add(meta.len());
            }
        }
    }

    if newest_mtime == SystemTime::UNIX_EPOCH {
        newest_mtime = now;
    }
    (total_bytes, file_count, newest_mtime)
}

/// Discovers a Zalo PC installation using default OS paths and currently running processes.
pub fn discover_zalo_installation(options: &ZaloDiscoveryOptions) -> Option<ZaloDiscovery> {
    let (cfg_path, roaming_dir, programs_dir) = resolve_default_paths(
        options.config_file.as_deref(),
        options.local_appdata.as_deref(),
    )?;

    let known_reloc_norm: HashSet<String> = options
        .known_relocated_sources
        .iter()
        .map(|p| normalize_cmp_path(p))
        .collect();

    let mut warnings = Vec::new();
    let mut protected_paths = Vec::new();

    // 1. Parse config.json safely (missing -> skip, never guess!).
    let (config_status, data_pair) = if is_symlink_or_reparse(&roaming_dir) {
        (
            ZaloConfigStatus::RejectedUnsafe {
                reason: "Thư mục ZaloData là symlink/junction lạ".to_string(),
            },
            None,
        )
    } else {
        parse_zalo_config_safe(&cfg_path, &known_reloc_norm)
    };

    if cfg_path.is_file() {
        protected_paths.push(cfg_path.clone());
    }

    if let ZaloConfigStatus::RejectedUnsafe { ref reason }
    | ZaloConfigStatus::Invalid { ref reason } = config_status
    {
        warnings.push(reason.clone());
    }

    let (data_dir, data_is_known_relocation) = match data_pair {
        Some((dir, known)) => (Some(dir), known),
        None => (None, false),
    };

    // Measure chat data (`Zalo Data`, `media`, `message`) — NEVER reclaimable!
    let mut chat_data_bytes = 0u64;
    let mut chat_data_file_count = 0u64;
    let mut media_bytes = 0u64;
    let mut media_file_count = 0u64;
    let mut message_bytes = 0u64;
    let mut message_file_count = 0u64;
    let mut data_drive = None;
    let mut data_on_system_drive = false;

    if let Some(ref ddir) = data_dir {
        data_drive = extract_drive_label(ddir);
        let sys_root = options
            .system_drive_root
            .clone()
            .unwrap_or_else(|| PathBuf::from(greenc_platform::volume::get_system_drive_root()));
        data_on_system_drive = is_path_on_system_drive(ddir, &sys_root);

        if ddir.exists() && ddir.is_dir() {
            let (total_b, total_f, _) = measure_dir_safe(ddir, options.now);
            chat_data_bytes = total_b;
            chat_data_file_count = total_f;

            let msg_dir = ddir.join("message");
            if msg_dir.is_dir() && !is_symlink_or_reparse(&msg_dir) {
                let (mb, mf, _) = measure_dir_safe(&msg_dir, options.now);
                message_bytes = mb;
                message_file_count = mf;
                protected_paths.push(msg_dir);
            }
            let media_dir = ddir.join("media");
            if media_dir.is_dir() && !is_symlink_or_reparse(&media_dir) {
                let (mb, mf, _) = measure_dir_safe(&media_dir, options.now);
                media_bytes = mb;
                media_file_count = mf;
                protected_paths.push(media_dir);
            }
        }
    }

    // 2. Discover allowlisted regenerable caches under `%APPDATA%\ZaloData` and `Partitions\*`.
    let mut cache_dirs = Vec::new();
    let mut cache_bytes = 0u64;

    if roaming_dir.is_dir() && !is_symlink_or_reparse(&roaming_dir) {
        if let Ok(entries) = fs::read_dir(&roaming_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if is_symlink_or_reparse(&p) || !p.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if is_allowed_zalo_root_cache_name(&name) {
                    let (bytes, files, _) = measure_dir_safe(&p, options.now);
                    cache_bytes = cache_bytes.saturating_add(bytes);
                    cache_dirs.push(ZaloCacheDirInfo {
                        path: p,
                        label: name,
                        total_bytes: bytes,
                        file_count: files,
                    });
                } else if name.eq_ignore_ascii_case("Partitions") {
                    if let Ok(part_entries) = fs::read_dir(&p) {
                        for part in part_entries.flatten() {
                            let part_path = part.path();
                            if is_symlink_or_reparse(&part_path) || !part_path.is_dir() {
                                continue;
                            }
                            let part_name = part.file_name().to_string_lossy().to_string();
                            if let Ok(sub_entries) = fs::read_dir(&part_path) {
                                for sub in sub_entries.flatten() {
                                    let sub_path = sub.path();
                                    if is_symlink_or_reparse(&sub_path) || !sub_path.is_dir() {
                                        continue;
                                    }
                                    let sub_name = sub.file_name().to_string_lossy().to_string();
                                    if is_allowed_zalo_partition_cache_name(&sub_name) {
                                        let (bytes, files, _) =
                                            measure_dir_safe(&sub_path, options.now);
                                        cache_bytes = cache_bytes.saturating_add(bytes);
                                        cache_dirs.push(ZaloCacheDirInfo {
                                            path: sub_path,
                                            label: format!("Partitions/{part_name}/{sub_name}"),
                                            total_bytes: bytes,
                                            file_count: files,
                                        });
                                    } else if is_protected_zalo_entry(&sub_name) {
                                        protected_paths.push(sub_path);
                                    }
                                }
                            }
                        }
                    }
                } else if is_protected_zalo_entry(&name) {
                    protected_paths.push(p);
                }
            }
        }
    }

    cache_dirs.sort_by(|a, b| a.label.cmp(&b.label));

    // 3. Discover `Zalo-x.y.z` installations under `%LOCALAPPDATA%\Programs\Zalo`.
    let running_norm: HashSet<String> = options
        .running_exe_dirs
        .iter()
        .map(|p| normalize_cmp_path(p))
        .collect();

    let mut versions: Vec<ZaloVersionDirInfo> = Vec::new();
    if programs_dir.is_dir() && !is_symlink_or_reparse(&programs_dir) {
        if let Ok(entries) = fs::read_dir(&programs_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if is_symlink_or_reparse(&p) || !p.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let Some((ver_str, ver_parts)) = parse_zalo_version(&name) else {
                    continue;
                };
                let (bytes, files, newest_mtime) = measure_dir_safe(&p, options.now);
                let age_secs = options
                    .now
                    .duration_since(newest_mtime)
                    .unwrap_or(Duration::ZERO)
                    .as_secs();
                let is_running = running_norm.contains(&normalize_cmp_path(&p));
                versions.push(ZaloVersionDirInfo {
                    path: p,
                    version_str: ver_str,
                    version_parts: ver_parts,
                    total_bytes: bytes,
                    file_count: files,
                    is_running,
                    is_newest: false,
                    is_kept: false,
                    keep_reason: None,
                    eligible_for_clean: false,
                    age_secs,
                });
            }
        }
    }

    // Sort newest first by numeric version tuple.
    versions.sort_by(|a, b| b.version_parts.cmp(&a.version_parts));
    let any_running = versions.iter().any(|v| v.is_running);
    let min_age_secs = options.min_old_version_age.as_secs();

    let mut old_versions_bytes = 0u64;
    let mut old_version_dirs = Vec::new();
    let mut kept_version_dirs = Vec::new();

    for (idx, ver) in versions.iter_mut().enumerate() {
        if idx == 0 {
            ver.is_newest = true;
        }
        if ver.is_running {
            ver.is_kept = true;
            ver.keep_reason = Some("running".to_string());
        } else if ver.is_newest {
            ver.is_kept = true;
            ver.keep_reason = Some("newest".to_string());
        } else if !any_running && idx == 1 {
            // When no Zalo process is running, always keep the 2 newest versions.
            ver.is_kept = true;
            ver.keep_reason = Some("second_newest_no_running_process".to_string());
        } else if ver.age_secs < min_age_secs {
            ver.is_kept = true;
            ver.keep_reason = Some("too_recent".to_string());
        } else {
            ver.is_kept = false;
            ver.eligible_for_clean = true;
            old_versions_bytes = old_versions_bytes.saturating_add(ver.total_bytes);
            old_version_dirs.push(ver.path.clone());
        }

        if ver.is_kept {
            kept_version_dirs.push(ver.path.clone());
            protected_paths.push(ver.path.clone());
        }
    }

    let installed = data_dir.is_some()
        || !cache_dirs.is_empty()
        || !versions.is_empty()
        || roaming_dir.is_dir();

    if !installed {
        return None;
    }

    Some(ZaloDiscovery {
        installed,
        config_path: cfg_path,
        config_status,
        roaming_dir,
        programs_dir,
        data_dir,
        data_drive,
        data_on_system_drive,
        data_is_known_relocation,
        chat_data_bytes,
        chat_data_file_count,
        media_bytes,
        media_file_count,
        message_bytes,
        message_file_count,
        cache_dirs,
        cache_bytes,
        versions,
        old_versions_bytes,
        old_version_dirs,
        kept_version_dirs,
        protected_paths,
        warnings,
    })
}

fn is_path_on_system_drive(path: &Path, sys_root: &Path) -> bool {
    if let (Some(p_drv), Some(s_drv)) = (extract_drive_label(path), extract_drive_label(sys_root)) {
        return p_drv.eq_ignore_ascii_case(&s_drv);
    }
    if path.starts_with(sys_root) {
        return true;
    }
    is_same_volume(path, sys_root)
}
