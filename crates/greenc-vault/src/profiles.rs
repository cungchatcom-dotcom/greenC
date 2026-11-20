//! 8 Preset cleanup profiles and candidate discovery engine (Tasks T030, T031).
//!
//! Features:
//! - 8 Preset profiles: windows_temp, browser_cache, chat_media, crash_dumps,
//!   package_managers, app_logs, old_versions, orphan_radar
//! - Exclude pattern filtering against `DEFAULT_PROTECTED_PATTERNS`
//! - Age-based filtering and exclusive file lock evaluation

use greenc_core::config::is_protected_filename;
use greenc_core::models::CleanCandidate;
use greenc_core::task::TaskControl;
use greenc_platform::traits::FileLockProbe;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Category classification for cleanup profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileCategory {
    System,
    Browser,
    Communication,
    Diagnostic,
    Developer,
    Logs,
    Updates,
    Orphan,
}

/// Metadata describing a preset cleanup profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: ProfileCategory,
    pub default_enabled: bool,
    pub min_file_age_seconds: u64,
    pub default_watermark_ratio: f64,
    pub exclude_patterns: Vec<String>,
}

/// Factory and candidate discovery scanner for the 8 preset profiles.
pub struct PresetProfiles;

impl PresetProfiles {
    /// Returns the full list of all 8 preset cleanup profiles.
    pub fn all() -> Vec<CleanupProfile> {
        vec![
            CleanupProfile {
                id: "windows_temp".to_string(),
                name: "Thư mục Temp của Windows".to_string(),
                description: "Tệp tạm thời sinh ra bởi Windows và các ứng dụng".to_string(),
                category: ProfileCategory::System,
                default_enabled: true,
                min_file_age_seconds: 60,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec!["*.lock".to_string(), "*.pid".to_string()],
            },
            CleanupProfile {
                id: "browser_cache".to_string(),
                name: "Bộ nhớ đệm Trình duyệt".to_string(),
                description: "Cache duyệt web Chrome, Edge, Brave, CocCoc, Firefox".to_string(),
                category: ProfileCategory::Browser,
                default_enabled: true,
                min_file_age_seconds: 300,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec![
                    "Cookies*".to_string(),
                    "Login Data*".to_string(),
                    "History*".to_string(),
                    "Bookmarks*".to_string(),
                    "Preferences*".to_string(),
                    "Web Data*".to_string(),
                ],
            },
            CleanupProfile {
                id: "chat_media".to_string(),
                name: "Bộ nhớ đệm Chat (Zalo & DingTalk)".to_string(),
                description: "Tệp đệm hình ảnh, âm thanh và phiên bản cũ Zalo / DingTalk"
                    .to_string(),
                category: ProfileCategory::Communication,
                default_enabled: true,
                min_file_age_seconds: 60,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec![
                    "*.db".to_string(),
                    "*.db-wal".to_string(),
                    "*.db-shm".to_string(),
                    "*.rescache".to_string(),
                    "message".to_string(),
                    "DBFiles".to_string(),
                ],
            },
            CleanupProfile {
                id: "crash_dumps".to_string(),
                name: "Tệp đổ lỗi bộ nhớ & Báo cáo sự cố".to_string(),
                description: "WER Minidumps, Memory.dmp và ứng dụng bị sự cố".to_string(),
                category: ProfileCategory::Diagnostic,
                default_enabled: true,
                min_file_age_seconds: 60,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec![],
            },
            CleanupProfile {
                id: "package_managers".to_string(),
                name: "Bộ nhớ đệm Gói Lập trình".to_string(),
                description: "Cache npm, pip, cargo, uv, pnpm, gradle".to_string(),
                category: ProfileCategory::Developer,
                default_enabled: true,
                min_file_age_seconds: 300,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec![
                    "lockfile".to_string(),
                    "*.toml".to_string(),
                    "*.json".to_string(),
                    "config".to_string(),
                ],
            },
            CleanupProfile {
                id: "app_logs".to_string(),
                name: "Nhật ký hệ thống & Ứng dụng".to_string(),
                description: "Tệp nhật ký .log, CBS log cũ và Panther setup logs".to_string(),
                category: ProfileCategory::Logs,
                default_enabled: true,
                min_file_age_seconds: 86400,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec!["DataStore.edb".to_string()],
            },
            CleanupProfile {
                id: "old_versions".to_string(),
                name: "Phiên bản cũ & Tệp tải về Windows Update".to_string(),
                description: "SoftwareDistribution Download và các tệp nâng cấp cũ".to_string(),
                category: ProfileCategory::Updates,
                default_enabled: false,
                min_file_age_seconds: 86400,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec![],
            },
            CleanupProfile {
                id: "orphan_radar".to_string(),
                name: "Dữ liệu mồ côi ứng dụng đã gỡ bỏ".to_string(),
                description: "Thư mục tàn dư trong AppData của phần mềm đã gỡ cài đặt".to_string(),
                category: ProfileCategory::Orphan,
                default_enabled: true,
                min_file_age_seconds: 86400,
                default_watermark_ratio: 0.85,
                exclude_patterns: vec![
                    "Microsoft".to_string(),
                    "Windows".to_string(),
                    "Packages".to_string(),
                ],
            },
        ]
    }

    /// Looks up a profile by its unique string ID.
    pub fn get_by_id(id: &str) -> Option<CleanupProfile> {
        Self::all().into_iter().find(|p| p.id == id)
    }

    /// Resolves target directory paths on disk for a given profile (OS-aware: Windows, macOS, Linux).
    pub fn resolve_profile_paths(profile_id: &str) -> Vec<PathBuf> {
        #[cfg(windows)]
        let paths = Self::resolve_profile_paths_windows(profile_id);

        #[cfg(target_os = "macos")]
        let paths = Self::resolve_profile_paths_macos(profile_id);

        #[cfg(all(unix, not(target_os = "macos")))]
        let paths = Self::resolve_profile_paths_linux(profile_id);

        // Return only paths that exist on disk (silently skips TCC/permission-denied paths)
        paths.into_iter().filter(|p| p.exists()).collect()
    }

    #[cfg(windows)]
    fn resolve_profile_paths_windows(profile_id: &str) -> Vec<PathBuf> {
        let local_app_data = std::env::var("LOCALAPPDATA").map(PathBuf::from).ok();
        let app_data = std::env::var("APPDATA").map(PathBuf::from).ok();
        let user_profile = std::env::var("USERPROFILE").map(PathBuf::from).ok();
        let windir = std::env::var("windir")
            .or_else(|_| std::env::var("SystemRoot"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"C:\Windows"));

        let mut paths = Vec::new();

        match profile_id {
            "windows_temp" => {
                if let Some(ref lad) = local_app_data {
                    paths.push(lad.join("Temp"));
                }
                paths.push(windir.join("Temp"));
            }
            "browser_cache" => {
                if let Some(ref lad) = local_app_data {
                    // Chrome
                    paths.push(lad.join(r"Google\Chrome\User Data\Default\Cache"));
                    paths.push(lad.join(r"Google\Chrome\User Data\Default\Code Cache"));
                    paths.push(lad.join(r"Google\Chrome\User Data\Default\GPUCache"));
                    // Edge
                    paths.push(lad.join(r"Microsoft\Edge\User Data\Default\Cache"));
                    paths.push(lad.join(r"Microsoft\Edge\User Data\Default\Code Cache"));
                    paths.push(lad.join(r"Microsoft\Edge\User Data\Default\GPUCache"));
                    // Brave
                    paths.push(lad.join(r"BraveSoftware\Brave-Browser\User Data\Default\Cache"));
                    // CocCoc
                    paths.push(lad.join(r"CocCoc\Browser\User Data\Default\Cache"));
                }
            }
            "chat_media" => {
                if let Some(ref ad) = app_data {
                    paths.push(ad.join(r"ZaloData\Cache"));
                    paths.push(ad.join(r"ZaloData\fileNoise"));
                }
                if let Some(ref lad) = local_app_data {
                    paths.push(lad.join(r"Zalo\pending"));
                    paths.push(lad.join(r"DingTalk\dumps"));
                }
            }
            "crash_dumps" => {
                if let Some(ref lad) = local_app_data {
                    paths.push(lad.join("CrashDumps"));
                    paths.push(lad.join(r"Microsoft\Windows\WER\ReportArchive"));
                    paths.push(lad.join(r"Microsoft\Windows\WER\ReportQueue"));
                }
                paths.push(windir.join("Minidump"));
            }
            "package_managers" => {
                if let Some(ref lad) = local_app_data {
                    paths.push(lad.join(r"npm-cache\_cacache"));
                    paths.push(lad.join(r"pip\cache"));
                    paths.push(lad.join(r"uv\cache"));
                    paths.push(lad.join(r"pnpm\store"));
                }
                if let Some(ref up) = user_profile {
                    paths.push(up.join(r".cargo\registry\cache"));
                    paths.push(up.join(r".gradle\caches"));
                }
            }
            "app_logs" => {
                paths.push(windir.join(r"Logs\CBS"));
                paths.push(windir.join(r"Logs\DISM"));
                paths.push(windir.join("Panther"));
            }
            "old_versions" => {
                paths.push(windir.join(r"SoftwareDistribution\Download"));
                // `Windows.old` is a sibling of %windir% (normally `C:\Windows.old`). Deriving it from
                // windir keeps sandboxed runs (windir redirected) away from the real one.
                if let Some(parent) = windir.parent() {
                    paths.push(parent.join("Windows.old"));
                }
            }
            "orphan_radar" => {
                // Scanned dynamically in AppData remnants
            }
            _ => {}
        }

        paths
    }

    #[cfg(target_os = "macos")]
    fn resolve_profile_paths_macos(profile_id: &str) -> Vec<PathBuf> {
        let home = match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h),
            Err(_) => return Vec::new(),
        };
        let lib = home.join("Library");
        let caches = lib.join("Caches");
        let app_support = lib.join("Application Support");
        let mut paths = Vec::new();

        match profile_id {
            "windows_temp" => {
                if let Ok(tmpdir) = std::env::var("TMPDIR") {
                    if !tmpdir.trim().is_empty() {
                        paths.push(PathBuf::from(tmpdir));
                    }
                }
            }
            "browser_cache" => {
                paths.push(caches.join("Google/Chrome/Default/Cache"));
                paths.push(caches.join("Google/Chrome/Default/Code Cache"));
                paths.push(caches.join("Microsoft Edge/Default/Cache"));
                paths.push(caches.join("BraveSoftware/Brave-Browser/Default/Cache"));
                paths.push(caches.join("Firefox/Profiles"));
                paths.push(caches.join("com.apple.Safari"));
            }
            "chat_media" => {
                paths.push(app_support.join("ZaloData/Cache"));
                paths.push(app_support.join("ZaloData/fileNoise"));
                paths.push(app_support.join("discord/Cache"));
                paths.push(app_support.join("discord/Code Cache"));
                paths.push(app_support.join("Slack/Cache"));
                paths.push(app_support.join("Slack/Code Cache"));
            }
            "crash_dumps" => {
                paths.push(lib.join("Logs/DiagnosticReports"));
            }
            "package_managers" => {
                paths.push(caches.join("Homebrew"));
                paths.push(lib.join("Developer/Xcode/DerivedData"));
                paths.push(home.join(".npm/_cacache"));
                paths.push(caches.join("pip"));
                paths.push(caches.join("uv"));
                paths.push(caches.join("pnpm"));
                paths.push(home.join(".cargo/registry/cache"));
                paths.push(home.join(".gradle/caches"));
            }
            "app_logs" => {
                paths.push(lib.join("Logs"));
            }
            "old_versions" => {
                paths.push(lib.join("Developer/Xcode/iOS DeviceSupport"));
                paths.push(home.join(".Trash"));
            }
            "orphan_radar" => {}
            _ => {}
        }

        paths
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn resolve_profile_paths_linux(profile_id: &str) -> Vec<PathBuf> {
        let home = match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h),
            Err(_) => return Vec::new(),
        };
        let xdg_cache = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".cache"));
        let xdg_config = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        let xdg_data = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/share"));
        let mut paths = Vec::new();

        match profile_id {
            "windows_temp" => {
                paths.push(xdg_cache.join("thumbnails"));
            }
            "browser_cache" => {
                paths.push(xdg_cache.join("google-chrome/Default/Cache"));
                paths.push(xdg_cache.join("google-chrome/Default/Code Cache"));
                paths.push(xdg_cache.join("chromium/Default/Cache"));
                paths.push(xdg_cache.join("BraveSoftware/Brave-Browser/Default/Cache"));
                paths.push(xdg_cache.join("mozilla/firefox"));
            }
            "chat_media" => {
                paths.push(xdg_config.join("discord/Cache"));
                paths.push(xdg_config.join("discord/Code Cache"));
                paths.push(xdg_config.join("Slack/Cache"));
                paths.push(xdg_data.join("TelegramDesktop/tdata/user_data/cache"));
            }
            "crash_dumps" => {
                paths.push(xdg_data.join("apport"));
            }
            "package_managers" => {
                paths.push(home.join(".npm/_cacache"));
                paths.push(xdg_cache.join("pip"));
                paths.push(xdg_cache.join("uv"));
                paths.push(xdg_data.join("pnpm/store"));
                paths.push(home.join(".cargo/registry/cache"));
                paths.push(home.join(".gradle/caches"));
            }
            "app_logs" => {
                paths.push(home.join(".local/state"));
            }
            "old_versions" => {
                paths.push(xdg_data.join("Trash/files"));
            }
            "orphan_radar" => {}
            _ => {}
        }

        paths
    }

    /// Discovers candidate cleanup files for a specified profile.
    pub fn scan_profile_candidates(
        profile_id: &str,
        task_control: &TaskControl,
        lock_probe: &dyn FileLockProbe,
    ) -> Vec<CleanCandidate> {
        let profile = match Self::get_by_id(profile_id) {
            Some(p) => p,
            None => return Vec::new(),
        };

        let target_dirs = Self::resolve_profile_paths(profile_id);
        let mut candidates = Vec::new();
        let now = SystemTime::now();
        let min_age = Duration::from_secs(profile.min_file_age_seconds);

        for dir in target_dirs {
            if task_control.is_cancelled() {
                break;
            }
            Self::scan_directory_recursive(
                &dir,
                &profile,
                now,
                min_age,
                task_control,
                lock_probe,
                &mut candidates,
                0,
                5, // Max depth 5
            );
        }

        candidates
    }

    /// Helper for recursive directory traversal with filtering and lock inspection.
    fn scan_directory_recursive(
        dir: &Path,
        profile: &CleanupProfile,
        now: SystemTime,
        min_age: Duration,
        task_control: &TaskControl,
        lock_probe: &dyn FileLockProbe,
        out: &mut Vec<CleanCandidate>,
        depth: usize,
        max_depth: usize,
    ) {
        if depth > max_depth || task_control.is_cancelled() {
            return;
        }

        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };

        for entry in read_dir.flatten() {
            if task_control.is_cancelled() {
                break;
            }

            let path = entry.path();
            let file_name = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Check if protected by global constants or custom profile excludes
            if is_protected_filename(file_name, Some(&profile.exclude_patterns)) {
                continue;
            }

            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_symlink() {
                continue;
            }

            if file_type.is_dir() {
                Self::scan_directory_recursive(
                    &path,
                    profile,
                    now,
                    min_age,
                    task_control,
                    lock_probe,
                    out,
                    depth + 1,
                    max_depth,
                );
            } else if file_type.is_file() {
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                // Age filter
                let mtime = meta.modified().unwrap_or(now);
                if let Ok(age) = now.duration_since(mtime) {
                    if age < min_age {
                        continue;
                    }
                }

                // Lock evaluation
                let is_locked = lock_probe.is_file_locked(&path);

                out.push(CleanCandidate {
                    path: path.clone(),
                    size_bytes: meta.len(),
                    mtime,
                    is_locked,
                    profile_id: profile.id.clone(),
                    is_canary: false,
                    is_orphan: profile.id == "orphan_radar",
                    hardlink_siblings: vec![path],
                });
            }
        }
    }
}
