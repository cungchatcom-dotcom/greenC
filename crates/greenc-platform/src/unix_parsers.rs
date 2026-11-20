//! Pure, platform-independent parsers and guards for Unix (macOS & Linux) telemetry.
//!
//! Compiled unconditionally on all OS targets so unit tests can verify macOS/Linux parsing
//! logic even when executed on Windows hosts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
