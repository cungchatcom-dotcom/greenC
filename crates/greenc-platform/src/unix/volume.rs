//! Unix (macOS & Linux) volume enumeration and disk space telemetry via POSIX `statvfs`.

use crate::traits::Win32VolumeApi;
use crate::unix_parsers::{
    assign_unix_drive_letter, evaluate_unix_drive_eligibility, is_ignored_unix_mount,
    is_supported_unix_fs,
};
use greenc_core::config::MIN_RECOMMEND_FREE_BYTES;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::DriveInfo;
use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const DRIVE_UNKNOWN: u32 = 0;
pub const DRIVE_NO_ROOT_DIR: u32 = 1;
pub const DRIVE_REMOVABLE: u32 = 2;
pub const DRIVE_FIXED: u32 = 3;
pub const DRIVE_REMOTE: u32 = 4;
pub const DRIVE_CDROM: u32 = 5;
pub const DRIVE_RAMDISK: u32 = 6;

/// Status of USN journal accessibility for a volume (NTFS/Windows-only; always Unsupported on Unix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsnCapabilityStatus {
    InsufficientPrivileges,
    UnsupportedFilesystem,
    CannotOpenVolume,
    JournalNotActive,
    Ready,
}

/// Recommendation result struct wrapping drive letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecommendedDrive {
    pub drive_letter: char,
}

impl std::ops::Deref for RecommendedDrive {
    type Target = char;
    fn deref(&self) -> &Self::Target {
        &self.drive_letter
    }
}

impl PartialEq<char> for RecommendedDrive {
    fn eq(&self, other: &char) -> bool {
        self.drive_letter == *other
    }
}

impl PartialEq<RecommendedDrive> for char {
    fn eq(&self, other: &RecommendedDrive) -> bool {
        *self == other.drive_letter
    }
}

impl From<RecommendedDrive> for char {
    fn from(rd: RecommendedDrive) -> Self {
        rd.drive_letter
    }
}

impl From<char> for RecommendedDrive {
    fn from(c: char) -> Self {
        Self { drive_letter: c }
    }
}

impl std::fmt::Display for RecommendedDrive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.drive_letter)
    }
}

/// Queries `(total_bytes, available_bytes)` for a mounted path via POSIX `statvfs`.
fn statvfs_space(path: &Path) -> Result<(u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| GreenCError::InvalidPath(path.to_path_buf()))?;

    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated C string and `stat` is a stack-allocated `statvfs`.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return Err(GreenCError::Io(std::io::Error::last_os_error()));
    }

    let block_size = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };
    let total = (stat.f_blocks as u64).saturating_mul(block_size);
    let free = (stat.f_bavail as u64).saturating_mul(block_size);
    Ok((total, free))
}

/// Returns the system root mount point (`/`).
pub fn get_system_drive_root() -> String {
    "/".to_string()
}

/// Checks if the calling process is running with root (`euid == 0`) privileges.
pub fn is_process_elevated() -> bool {
    // SAFETY: `geteuid` takes no arguments and has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// USN journal is NTFS/Windows-specific; on Unix always report `UnsupportedFilesystem`
/// so the scanner uses the parallel Rayon walker.
pub fn probe_volume_usn_capability(_drive_letter: char) -> UsnCapabilityStatus {
    UsnCapabilityStatus::UnsupportedFilesystem
}

/// Evaluates drive eligibility (shared signature with Windows backend).
pub fn evaluate_drive_eligibility(
    drive_type: u32,
    filesystem: &str,
    label: &str,
) -> (bool, Option<String>) {
    if drive_type != DRIVE_FIXED {
        return (
            false,
            Some("Ổ di động hoặc ổ mạng không được hỗ trợ".to_string()),
        );
    }
    evaluate_unix_drive_eligibility(filesystem, "/", label)
}

/// Lists real mounted volumes on macOS (`getfsstat`) or Linux (`/proc/mounts`).
pub fn list_drives_native() -> Result<Vec<DriveInfo>> {
    #[cfg(target_os = "macos")]
    {
        list_drives_macos()
    }
    #[cfg(target_os = "linux")]
    {
        list_drives_linux()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let (total_bytes, free_bytes) = statvfs_space(Path::new("/"))?;
        Ok(vec![DriveInfo {
            root: PathBuf::from("/"),
            drive_letter: '/',
            label: "Root".to_string(),
            filesystem: "unix".to_string(),
            total_bytes,
            free_bytes,
            is_system: true,
            is_eligible: true,
            ineligible_reason: None,
        }])
    }
}

#[cfg(target_os = "macos")]
fn list_drives_macos() -> Result<Vec<DriveInfo>> {
    use std::ffi::CStr;

    // SAFETY: Querying mount count with null buffer is documented by `getfsstat(2)`.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        let (total_bytes, free_bytes) = statvfs_space(Path::new("/"))?;
        return Ok(vec![DriveInfo {
            root: PathBuf::from("/"),
            drive_letter: '/',
            label: "Macintosh HD".to_string(),
            filesystem: "APFS".to_string(),
            total_bytes,
            free_bytes,
            is_system: true,
            is_eligible: true,
            ineligible_reason: None,
        }]);
    }

    let capacity = (count as usize).saturating_add(4);
    // SAFETY: `libc::statfs` is a plain C struct safe to zero-initialize before `getfsstat`.
    let mut entries: Vec<libc::statfs> = (0..capacity)
        .map(|_| unsafe { std::mem::zeroed() })
        .collect();
    let buf_size = (capacity * std::mem::size_of::<libc::statfs>()) as libc::c_int;

    // SAFETY: `entries` has capacity `buf_size` bytes for `getfsstat` to populate.
    let actual = unsafe { libc::getfsstat(entries.as_mut_ptr(), buf_size, libc::MNT_NOWAIT) };
    if actual < 0 {
        return Err(GreenCError::Io(std::io::Error::last_os_error()));
    }

    let mut drives = Vec::new();
    let mut seen_mounts = HashSet::new();

    for entry in entries.iter().take(actual as usize) {
        // SAFETY: Kernel NUL-terminates `f_mntonname` and `f_fstypename` in `statfs`.
        let mount_point = unsafe { CStr::from_ptr(entry.f_mntonname.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let fs_type = unsafe { CStr::from_ptr(entry.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        if is_ignored_unix_mount(&mount_point) || !is_supported_unix_fs(&fs_type) {
            continue;
        }
        // Only expose `/` and `/Volumes/*` to avoid duplicate APFS synthetic mounts.
        if mount_point != "/" && !mount_point.starts_with("/Volumes/") {
            continue;
        }
        if !seen_mounts.insert(mount_point.clone()) {
            continue;
        }

        let root_path = PathBuf::from(&mount_point);
        let Ok((total_bytes, free_bytes)) = statvfs_space(&root_path) else {
            continue;
        };
        if total_bytes == 0 {
            continue;
        }

        let is_system = mount_point == "/";
        let label = if is_system {
            "Macintosh HD".to_string()
        } else {
            root_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| mount_point.clone())
        };

        let (is_eligible, reason) = evaluate_unix_drive_eligibility(&fs_type, &mount_point, &label);

        drives.push(DriveInfo {
            root: root_path,
            drive_letter: assign_unix_drive_letter(&mount_point),
            label,
            filesystem: fs_type.to_ascii_uppercase(),
            total_bytes,
            free_bytes,
            is_system,
            is_eligible,
            ineligible_reason: reason,
        });
    }

    Ok(drives)
}

#[cfg(target_os = "linux")]
fn list_drives_linux() -> Result<Vec<DriveInfo>> {
    use crate::unix_parsers::parse_proc_mounts;

    let content = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let parsed = parse_proc_mounts(&content);

    let mut drives = Vec::new();
    let mut seen_mounts = HashSet::new();

    for entry in parsed {
        if is_ignored_unix_mount(&entry.mount_point) || !is_supported_unix_fs(&entry.filesystem) {
            continue;
        }
        if !seen_mounts.insert(entry.mount_point.clone()) {
            continue;
        }

        let root_path = PathBuf::from(&entry.mount_point);
        let Ok((total_bytes, free_bytes)) = statvfs_space(&root_path) else {
            continue;
        };
        if total_bytes == 0 {
            continue;
        }

        let is_system = entry.mount_point == "/";
        let label = if is_system {
            "System Root".to_string()
        } else {
            root_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.mount_point.clone())
        };

        let (is_eligible, reason) =
            evaluate_unix_drive_eligibility(&entry.filesystem, &entry.mount_point, &label);

        drives.push(DriveInfo {
            root: root_path,
            drive_letter: assign_unix_drive_letter(&entry.mount_point),
            label,
            filesystem: entry.filesystem.to_ascii_uppercase(),
            total_bytes,
            free_bytes,
            is_system,
            is_eligible,
            ineligible_reason: reason,
        });
    }

    if drives.is_empty() {
        let (total_bytes, free_bytes) = statvfs_space(Path::new("/"))?;
        drives.push(DriveInfo {
            root: PathBuf::from("/"),
            drive_letter: '/',
            label: "System Root".to_string(),
            filesystem: "EXT4".to_string(),
            total_bytes,
            free_bytes,
            is_system: true,
            is_eligible: true,
            ineligible_reason: None,
        });
    }

    Ok(drives)
}

pub fn recommend_drive(drives: &[DriveInfo]) -> Option<RecommendedDrive> {
    recommend_drive_info(drives).map(|d| RecommendedDrive {
        drive_letter: d.drive_letter,
    })
}

pub fn recommend_drive_letter(drives: &[DriveInfo]) -> Option<char> {
    recommend_drive_info(drives).map(|d| d.drive_letter)
}

pub fn recommend_drive_info(drives: &[DriveInfo]) -> Option<&DriveInfo> {
    let eligible: Vec<&DriveInfo> = drives.iter().filter(|d| d.is_eligible).collect();
    if eligible.is_empty() {
        return None;
    }

    let roomy: Vec<&DriveInfo> = eligible
        .iter()
        .copied()
        .filter(|d| !d.is_system && d.free_bytes >= MIN_RECOMMEND_FREE_BYTES)
        .collect();

    let pool = if !roomy.is_empty() { roomy } else { eligible };
    pool.into_iter().max_by_key(|d| d.free_bytes)
}

#[derive(Default, Clone, Debug)]
pub struct NativeWin32VolumeApi;

impl Win32VolumeApi for NativeWin32VolumeApi {
    fn list_drives(&self) -> Result<Vec<DriveInfo>> {
        list_drives_native()
    }

    fn get_disk_free_space(&self, root: &Path) -> Result<(u64, u64)> {
        statvfs_space(root)
    }

    fn get_free_space(&self, drive: char) -> Result<u64> {
        if drive == '/' {
            return Ok(statvfs_space(Path::new("/"))?.1);
        }
        let drives = self.list_drives()?;
        if let Some(found) = drives
            .iter()
            .find(|d| d.drive_letter.eq_ignore_ascii_case(&drive))
        {
            return Ok(statvfs_space(&found.root)?.1);
        }
        Ok(statvfs_space(Path::new("/"))?.1)
    }

    fn get_volume_info(&self, root: &Path) -> Result<(String, String, u32)> {
        let drives = self.list_drives().unwrap_or_default();
        if let Some(found) = drives.iter().find(|d| d.root == root) {
            return Ok((found.label.clone(), found.filesystem.clone(), DRIVE_FIXED));
        }
        let label = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Root".to_string());
        let fs = if cfg!(target_os = "macos") {
            "APFS".to_string()
        } else {
            "EXT4".to_string()
        };
        Ok((label, fs, DRIVE_FIXED))
    }
}
