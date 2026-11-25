use std::path::{Path, PathBuf};

// Application Branding & Metadata
pub const APP_NAME: &str = "GreenC";
pub const APP_ID: &str = "GreenC";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const AUTHOR: &str = "Cùng Chat";
pub const WEBSITE_URL: &str = "https://cungchat.com";
pub const WEBSITE_LABEL: &str = "cungchat.com";

/// True when compiled with the `store` feature (Microsoft Store / MSIX edition).
pub const IS_STORE_BUILD: bool = cfg!(feature = "store");
/// TaskId declared in `packaging/msix/AppxManifest.xml.in` (`desktop:StartupTask`).
pub const STORE_STARTUP_TASK_ID: &str = "GreenCStartupTask";
/// Microsoft Store Downloads & Updates URI used by Store builds instead of GitHub Releases.
pub const MICROSOFT_STORE_UPDATES_URL: &str = "ms-windows-store://downloadsandupdates";

// Folder Names
pub const DATA_FOLDER_NAME: &str = "GreenC";
/// Separate AppData folder name for the Microsoft Store (MSIX) edition so it never collides with
/// or mutates an existing unpackaged EXE install's `%LOCALAPPDATA%\GreenC` data on Windows 10 1903+.
pub const STORE_DATA_FOLDER_NAME: &str = "GreenC-Store";
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
/// - Windows (standard): `%LOCALAPPDATA%\GreenC`
/// - Windows (`store` feature / MSIX): `%LOCALAPPDATA%\GreenC-Store` (virtualized per-package,
///   isolated from any unpackaged EXE installation)
/// - macOS: `~/Library/Application Support/GreenC` (or `GreenC-Store` when `store` enabled)
/// - Linux/Unix: `$XDG_DATA_HOME/greenc` or `~/.local/share/greenc` (`greenc-store` when `store` enabled)
pub fn app_home() -> PathBuf {
    #[cfg(feature = "store")]
    let (win_folder, unix_folder) = (STORE_DATA_FOLDER_NAME, "greenc-store");
    #[cfg(not(feature = "store"))]
    let (win_folder, unix_folder) = (DATA_FOLDER_NAME, "greenc");

    #[cfg(windows)]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            PathBuf::from(local_app_data).join(win_folder)
        } else if let Ok(user_profile) = std::env::var("USERPROFILE") {
            PathBuf::from(user_profile)
                .join("AppData")
                .join("Local")
                .join(win_folder)
        } else {
            PathBuf::from(r"C:\").join(win_folder)
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = unix_folder;
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(win_folder)
        } else {
            PathBuf::from("/tmp").join(win_folder)
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = win_folder;
        if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
            if !xdg_data.trim().is_empty() {
                return PathBuf::from(xdg_data).join(unix_folder);
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(unix_folder)
        } else {
            PathBuf::from("/tmp").join(unix_folder)
        }
    }
}

/// Returns `true` if `path` resides under Windows `%APPDATA%` or `%LOCALAPPDATA%` (excluding
/// `%TEMP%` / `\AppData\Local\Temp`, where test sandboxes and transient temp files live).
///
/// Used by `store` (MSIX) builds to prevent operations whose new filesystem entries would be
/// silently redirected into the MSIX package's virtualized `LocalCache` (such as NTFS junctions
/// or same-drive atomic staging).
pub fn is_windows_appdata_app_dir(path: &Path) -> bool {
    let norm = normalize_windows_like_path(path);

    // Explicitly exclude AppData\Local\Temp and %TEMP%/%TMP%
    if norm.contains("/appdata/local/temp/") || norm.ends_with("/appdata/local/temp") {
        return false;
    }
    for env_key in ["TEMP", "TMP"] {
        if let Ok(val) = std::env::var(env_key) {
            let env_norm = normalize_windows_like_path(Path::new(&val));
            if !env_norm.is_empty()
                && (norm == env_norm || norm.starts_with(&format!("{env_norm}/")))
            {
                return false;
            }
        }
    }

    for env_key in ["LOCALAPPDATA", "APPDATA"] {
        if let Ok(val) = std::env::var(env_key) {
            let env_norm = normalize_windows_like_path(Path::new(&val));
            if !env_norm.is_empty()
                && (norm == env_norm || norm.starts_with(&format!("{env_norm}/")))
            {
                return true;
            }
        }
    }

    norm.contains("/appdata/local/")
        || norm.ends_with("/appdata/local")
        || norm.contains("/appdata/roaming/")
        || norm.ends_with("/appdata/roaming")
        || norm.contains("/appdata/locallow/")
        || norm.ends_with("/appdata/locallow")
}

/// Returns `true` if `path` targets a Windows Store / MSIX protected location such as
/// `WindowsApps` (`C:\Program Files\WindowsApps`, `D:\WindowsApps`,
/// `%LOCALAPPDATA%\Microsoft\WindowsApps`) or `%LOCALAPPDATA%\Packages`.
pub fn is_windows_apps_or_packages_path(path: &Path) -> bool {
    let norm = normalize_windows_like_path(path);
    norm.split('/').any(|seg| seg == "windowsapps")
        || norm.contains("/appdata/local/packages/")
        || norm.ends_with("/appdata/local/packages")
}

fn normalize_windows_like_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
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
