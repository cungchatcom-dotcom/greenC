use std::path::{Path, PathBuf};

// Application Branding & Metadata
pub const APP_NAME: &str = "GreenC";
pub const APP_ID: &str = "GreenC";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const AUTHOR: &str = "Cùng Chat";
pub const WEBSITE_URL: &str = "https://cungchat.com";
pub const WEBSITE_LABEL: &str = "cungchat.com";

// Folder Names
pub const DATA_FOLDER_NAME: &str = "GreenC";
pub const LEGACY_DATA_FOLDER_NAME: &str = "Green C";
pub const SINGLE_INSTANCE_MUTEX_NAME: &str = "GreenCAppMutex";

// Retention & Watermark Bounds
pub const DEFAULT_WATERMARK_RATIO: f64 = 0.85;
pub const DEFAULT_RETENTION_DAYS: u32 = 7;
pub const MIN_RETENTION_DAYS: u32 = 1;
pub const MAX_RETENTION_DAYS: u32 = 90;
pub const DEFAULT_MIN_FILE_AGE_SECONDS: u64 = 60;

// Disk Safety Reserve Thresholds (Strict Constitution Invariants)
/// Minimum 512 MiB free space strictly preserved on target/vault drives.
pub const MIN_VAULT_RESERVE_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB
/// Recommended minimum free space for secondary data drives (10 GiB).
pub const MIN_RECOMMEND_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB
/// Minimum size of candidates to propose for relocation (200 MiB).
pub const RELOCATION_MIN_CANDIDATE_BYTES: u64 = 200 * 1024 * 1024; // 200 MiB
/// Low disk shield emergency threshold defaults (GiB).
pub const DEFAULT_LOW_DISK_SHIELD_GB: u32 = 10;
pub const MIN_LOW_DISK_SHIELD_GB: u32 = 2;
pub const MAX_LOW_DISK_SHIELD_GB: u32 = 100;

/// Temporary transition suffix for atomic crash-recoverable directory operations.
pub const MOVING_SUFFIX: &str = ".greenc-moving";

/// Default protected patterns that must NEVER be cleaned or modified.
pub const DEFAULT_PROTECTED_PATTERNS: &[&str] = &[
    "*.exe",
    "*.dll",
    "*.sys",
    "*.dylib",
    "*.so",
    "*.plist",
    "*.framework",
    "*.ini",
    "*.cfg",
    "*.config",
    "*.json",
    "*.yaml",
    "*.yml",
    "*.toml",
    "*.db",
    "*.db-wal",
    "*.db-shm",
    "*.sqlite",
    "*.sqlite3",
    "*.mdf",
    "*.ldf",
    "*.rescache",
    "*.lock",
    "*.lck",
    "lockfile",
    "CACHEDIR.TAG",
    "gc.properties",
    "file-access.bin",
    "CURRENT",
    "MANIFEST-*",
];

/// Resolves standard GreenC application home directory per operating system:
/// - Windows: `%LOCALAPPDATA%\GreenC`
/// - macOS: `~/Library/Application Support/GreenC`
/// - Linux/Unix: `$XDG_DATA_HOME/greenc` or `~/.local/share/greenc`
pub fn app_home() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            PathBuf::from(local_app_data).join(DATA_FOLDER_NAME)
        } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
            PathBuf::from(user_profile)
                .join("AppData")
                .join("Local")
                .join(DATA_FOLDER_NAME)
        } else {
            PathBuf::from(r"C:\GreenC")
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(DATA_FOLDER_NAME)
        } else {
            PathBuf::from("/tmp/GreenC")
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
            if !xdg_data.trim().is_empty() {
                return PathBuf::from(xdg_data).join("greenc");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("greenc")
        } else {
            PathBuf::from("/tmp/greenc")
        }
    }
}

/// Resolves legacy Python Green C home (`%LOCALAPPDATA%\Green C` on Windows; falls back to `app_home()` on Unix).
pub fn legacy_green_c_home() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            PathBuf::from(local_app_data).join(LEGACY_DATA_FOLDER_NAME)
        } else {
            PathBuf::from(r"C:\Green C")
        }
    }
    #[cfg(not(windows))]
    {
        app_home()
    }
}

pub fn default_db_path() -> PathBuf {
    app_home().join("greenc.db")
}

pub fn legacy_green_c_db_path() -> PathBuf {
    legacy_green_c_home().join("greenc.db")
}

pub fn legacy_diskguard_db_path() -> PathBuf {
    PathBuf::from(r"C:\DiskGuard\data\diskguard.db")
}

pub fn default_vault_dir() -> PathBuf {
    app_home().join("Vault")
}

pub fn sandbox_demo_dir() -> PathBuf {
    app_home().join("sandbox")
}

/// Resolves the canonical GreenC data directory for a given drive root (e.g. `D:\` -> `D:\GreenC`).
pub fn data_root_for(drive_root: &Path) -> PathBuf {
    drive_root.join(DATA_FOLDER_NAME)
}

/// Helper to check if a filename matches any pattern in the protected patterns list.
pub fn is_protected_filename(filename: &str, custom_patterns: Option<&[String]>) -> bool {
    let lower_name = filename.to_lowercase();
    for &pattern in DEFAULT_PROTECTED_PATTERNS {
        if matches_glob(&lower_name, pattern) {
            return true;
        }
    }
    if let Some(custom) = custom_patterns {
        for pattern in custom {
            if matches_glob(&lower_name, pattern) {
                return true;
            }
        }
    }
    false
}

/// Simple glob matching helper for extension patterns (*.ext, MANIFEST-*, exact).
fn matches_glob(name: &str, pattern: &str) -> bool {
    let p = pattern.to_lowercase();
    if let Some(ext) = p.strip_prefix("*.") {
        name.ends_with(&format!(".{}", ext))
    } else if let Some(prefix) = p.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name == p
    }
}
