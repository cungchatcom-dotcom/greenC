//! Pure, platform-independent parsers and guards for Unix (macOS & Linux) telemetry.
//!
//! Compiled unconditionally on all OS targets so unit tests can verify macOS/Linux parsing
//! logic even when executed on Windows hosts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Reliability of filesystem access-time (`atime`) metadata for cache-age heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtimeReliability {
    /// Filesystem updates `atime` on reads (`relatime`, `strictatime`, or default `atime`).
    Reliable,
    /// Filesystem is mounted with `noatime`; reads never update `atime`.
    UnreliableNoAtime,
    /// Mount options could not be determined; callers should treat `atime` conservatively.
    Unknown,
}

impl AtimeReliability {
    pub fn is_reliable(self) -> bool {
        self == Self::Reliable
    }
}

/// Determines `atime` reliability for `path` by matching the most specific mount point
/// in a Linux `/proc/mounts` snapshot.
///
/// Pure function so all mount-option cases (`noatime`, `relatime`, `strictatime`, nested mounts,
/// bind mounts overriding an earlier mount at the same path) are unit-testable without root.
pub fn detect_atime_reliability_from_proc_mounts(
    path: &Path,
    proc_mounts_content: &str,
) -> AtimeReliability {
    let entries = parse_proc_mounts(proc_mounts_content);
    if entries.is_empty() {
        return AtimeReliability::Unknown;
    }

    let raw_path = path.to_string_lossy();
    let norm_path = if raw_path.len() > 1 {
        raw_path.trim_end_matches('/')
    } else {
        raw_path.as_ref()
    };

    let mut best: Option<&UnixMountEntry> = None;
    let mut best_len: usize = 0;

    for entry in &entries {
        let mnt = if entry.mount_point.len() > 1 {
            entry.mount_point.trim_end_matches('/')
        } else {
            entry.mount_point.as_str()
        };

        let matches = if mnt == "/" {
            norm_path.starts_with('/')
        } else {
            norm_path == mnt
                || (norm_path.starts_with(mnt)
                    && norm_path.as_bytes().get(mnt.len()) == Some(&b'/'))
        };

        // Use `>=` so a later mount stacked on the exact same mount point wins (Linux order).
        if matches && mnt.len() >= best_len {
            best = Some(entry);
            best_len = mnt.len();
        }
    }

    let Some(matched) = best else {
        return AtimeReliability::Unknown;
    };

    for opt in matched.options.split(',') {
        let trimmed = opt.trim();
        if trimmed == "noatime" {
            return AtimeReliability::UnreliableNoAtime;
        }
    }

    AtimeReliability::Reliable
}

/// Inspects the real filesystem mount options for `path` to determine whether `atime` is reliable.
pub fn detect_atime_reliability(path: &Path) -> AtimeReliability {
    #[cfg(target_os = "linux")]
    {
        // Canonicalize the path or its nearest existing ancestor so symlinks resolve to real mount.
        let mut probe = path.to_path_buf();
        let resolved = loop {
            if let Ok(c) = std::fs::canonicalize(&probe) {
                break c;
            }
            if !probe.pop() {
                break path.to_path_buf();
            }
        };
        let Ok(content) = std::fs::read_to_string("/proc/mounts") else {
            return AtimeReliability::Unknown;
        };
        detect_atime_reliability_from_proc_mounts(&resolved, &content)
    }

    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let mut probe = path.to_path_buf();
        while !probe.exists() {
            if !probe.pop() {
                return AtimeReliability::Unknown;
            }
        }
        let Ok(c_path) = CString::new(probe.as_os_str().as_bytes()) else {
            return AtimeReliability::Unknown;
        };
        // SAFETY: `statfs` is a plain C struct zero-initialized before passing to `libc::statfs`.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut st) };
        if rc != 0 {
            return AtimeReliability::Unknown;
        }
        if (st.f_flags & (libc::MNT_NOATIME as u32)) != 0 {
            AtimeReliability::UnreliableNoAtime
        } else {
            AtimeReliability::Reliable
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = path;
        AtimeReliability::Reliable
    }
}

/// Parsed entry from Linux `/proc/mounts` or macOS volume enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixMountEntry {
    pub device: String,
    pub mount_point: String,
    pub filesystem: String,
    pub options: String,
}

/// Decodes octal escape sequences (such as `\040` for space) used by Linux `/proc/mounts`.
pub fn unescape_proc_mount_field(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &raw[i + 1..i + 4];
            if oct.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
                if let Ok(val) = u8::from_str_radix(oct, 8) {
                    out.push(val as char);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Parses the contents of Linux `/proc/mounts` into structured mount entries.
pub fn parse_proc_mounts(content: &str) -> Vec<UnixMountEntry> {
    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let (Some(dev), Some(mnt), Some(fs), Some(opts)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        entries.push(UnixMountEntry {
            device: unescape_proc_mount_field(dev),
            mount_point: unescape_proc_mount_field(mnt),
            filesystem: fs.to_string(),
            options: opts.to_string(),
        });
    }
    entries
}

/// Returns true if a Unix filesystem type represents a real local or external storage volume.
pub fn is_supported_unix_fs(fs: &str) -> bool {
    matches!(
        fs.trim().to_ascii_lowercase().as_str(),
        "apfs"
            | "hfs"
            | "hfs+"
            | "ext4"
            | "ext3"
            | "ext2"
            | "btrfs"
            | "xfs"
            | "zfs"
            | "f2fs"
            | "ntfs"
            | "ntfs3"
            | "fuseblk"
            | "exfat"
            | "vfat"
            | "msdos"
    )
}

/// Returns true if a Unix mount point is a virtual, boot, snapshot, or system helper mount
/// that should be hidden from user-facing drive lists.
pub fn is_ignored_unix_mount(mount_point: &str) -> bool {
    let m = mount_point.trim();
    if m.is_empty() {
        return true;
    }
    if m == "/" {
        return false;
    }

    const IGNORED_PREFIXES: &[&str] = &[
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/snap",
        "/boot",
        "/var/lib/docker",
        "/var/lib/containers",
        "/private/var/vm",
        "/System/Volumes/VM",
        "/System/Volumes/Preboot",
        "/System/Volumes/Update",
        "/System/Volumes/xarts",
        "/System/Volumes/iSCPreboot",
        "/System/Volumes/Hardware",
        "/System/Volumes/Recovery",
        "/System/Volumes/Data",
        "/Volumes/Recovery",
        "/Volumes/Preboot",
        "/Volumes/VM",
    ];

    IGNORED_PREFIXES
        .iter()
        .any(|prefix| m == *prefix || m.starts_with(&format!("{prefix}/")))
}

/// Evaluates whether a Unix volume is eligible for GreenC Vault/Relocation operations.
pub fn evaluate_unix_drive_eligibility(
    filesystem: &str,
    mount_point: &str,
    label: &str,
) -> (bool, Option<String>) {
    let fs_low = filesystem.trim().to_ascii_lowercase();
    let label_low = label.to_ascii_lowercase();

    if label_low.contains("recovery") || mount_point.contains("Recovery") {
        return (false, Some("Phân vùng khôi phục của hệ thống".to_string()));
    }

    if matches!(
        fs_low.as_str(),
        "apfs"
            | "hfs"
            | "hfs+"
            | "ext4"
            | "ext3"
            | "btrfs"
            | "xfs"
            | "zfs"
            | "f2fs"
            | "ntfs"
            | "ntfs3"
            | "fuseblk"
    ) {
        (true, None)
    } else {
        let fs_display = if filesystem.trim().is_empty() {
            "không rõ"
        } else {
            filesystem.trim()
        };
        (
            false,
            Some(format!(
                "Định dạng {} không hỗ trợ liên kết thư mục và tệp lớn",
                fs_display
            )),
        )
    }
}

/// Assigns a stable single-character identifier for `DriveInfo.drive_letter` on Unix volumes.
pub fn assign_unix_drive_letter(mount_point: &str) -> char {
    if mount_point == "/" {
        return '/';
    }
    let trimmed = mount_point.trim_end_matches('/');
    if let Some(last) = trimmed.rsplit('/').next() {
        if let Some(ch) = last.chars().find(|c| c.is_ascii_alphabetic()) {
            return ch.to_ascii_uppercase();
        }
    }
    'V'
}

/// Checks whether a Unix path targets a protected OS system directory that must never be modified.
pub fn is_protected_unix_path(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    let clean = raw.trim_end_matches('/');
    if clean.is_empty() || clean == "/" {
        return true;
    }

    const FORBIDDEN_ROOTS: &[&str] = &[
        "/System",
        "/Library/System",
        "/Applications",
        "/usr",
        "/bin",
        "/sbin",
        "/etc",
        "/var/db",
        "/proc",
        "/sys",
        "/dev",
        "/boot",
        "/lib",
        "/lib64",
    ];

    FORBIDDEN_ROOTS
        .iter()
        .any(|prefix| clean == *prefix || clean.starts_with(&format!("{prefix}/")))
}

/// Parses macOS `pmset -g batt` output into `(on_battery, battery_saver_on, battery_percent)`.
pub fn parse_pmset_battery(output: &str) -> (bool, bool, u8) {
    let lower = output.to_ascii_lowercase();
    let on_battery = lower.contains("'battery power'");
    let mut percent: u8 = 100;

    for token in output.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '%');
        if let Some(num_str) = cleaned.strip_suffix('%') {
            if let Ok(val) = num_str.parse::<u8>() {
                percent = val.min(100);
                break;
            }
        }
    }

    let battery_saver_on = on_battery && percent <= 20;
    (on_battery, battery_saver_on, percent)
}

/// Parses Linux `~/.config/user-dirs.dirs` into a map of XDG folder keys to resolved paths.
pub fn parse_xdg_user_dirs(content: &str, home: &Path) -> HashMap<String, PathBuf> {
    let mut dirs = HashMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, val)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim().trim_matches('"');
        let resolved = if let Some(rest) = val.strip_prefix("$HOME/") {
            home.join(rest)
        } else if val == "$HOME" {
            home.to_path_buf()
        } else {
            PathBuf::from(val)
        };
        dirs.insert(key.to_string(), resolved);
    }
    dirs
}
