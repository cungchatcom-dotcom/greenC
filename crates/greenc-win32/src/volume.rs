use crate::handle::path_to_wide;
use crate::traits::Win32VolumeApi;
use greenc_core::config::MIN_RECOMMEND_FREE_BYTES;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::DriveInfo;
use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::Diagnostics::Debug::{
    SetThreadErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
pub use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
    DRIVE_UNKNOWN,
};

/// RAII Guard that suppresses modal dialogs ("No disk in drive") for the calling thread
/// and deterministically restores the prior mode on drop.
#[derive(Debug)]
pub struct ThreadErrorModeGuard {
    previous_mode: THREAD_ERROR_MODE,
}

impl ThreadErrorModeGuard {
    pub fn prevent_critical_dialogs() -> Self {
        let mut previous_mode = THREAD_ERROR_MODE(0);
        // SAFETY: `SetThreadErrorMode` only modifies the calling thread's error-mode flags and
        // writes the prior flags into `previous_mode` for deterministic restoration on `Drop`.
        unsafe {
            let _ = SetThreadErrorMode(SEM_FAILCRITICALERRORS, Some(&mut previous_mode));
        }
        Self { previous_mode }
    }
}

impl Drop for ThreadErrorModeGuard {
    fn drop(&mut self) {
        // SAFETY: Restores the exact `THREAD_ERROR_MODE` saved when this guard was constructed.
        unsafe {
            let _ = SetThreadErrorMode(self.previous_mode, None);
        }
    }
}

/// Scans and evaluates all logical drives on the host machine.
pub fn list_drives_native() -> Result<Vec<DriveInfo>> {
    // Suppress "No disk in drive" modal dialogs for empty card readers or optical drives via RAII guard
    let _guard = ThreadErrorModeGuard::prevent_critical_dialogs();

    let system_root = get_system_drive_root();
    let mut drives = Vec::new();

    // SAFETY: `GetLogicalDrives` takes no pointers and returns a 26-bit volume bitmask.
    let mask = unsafe { GetLogicalDrives() };

    for i in 0..26 {
        if (mask & (1 << i)) == 0 {
            continue;
        }

        let drive_letter = (b'A' + i) as char;
        let root_str = format!("{}:\\", drive_letter);
        let root_path = PathBuf::from(&root_str);
        let wide_root = path_to_wide(&root_path);
        let pcwstr = PCWSTR(wide_root.as_ptr());

        // SAFETY: `pcwstr` points to a stack-owned NUL-terminated UTF-16 root string (`"X:\\\0"`).
        let drive_type = unsafe { GetDriveTypeW(pcwstr) };

        let mut vol_name = [0u16; 261];
        let mut fs_name = [0u16; 261];
        let mut serial_number = 0u32;
        let mut max_component_len = 0u32;
        let mut flags = 0u32;

        // SAFETY: `vol_name` and `fs_name` are stack `[u16; 261]` buffers (`MAX_PATH + 1`),
        // and scalar out-pointers reference valid initialized stack `u32` variables.
        let vol_res = unsafe {
            GetVolumeInformationW(
                pcwstr,
                Some(&mut vol_name),
                Some(&mut serial_number),
                Some(&mut max_component_len),
                Some(&mut flags),
                Some(&mut fs_name),
            )
        };

        let label = if vol_res.is_ok() {
            String::from_utf16_lossy(&vol_name)
                .trim_matches('\0')
                .to_string()
        } else {
            String::new()
        };

        let filesystem = if vol_res.is_ok() {
            String::from_utf16_lossy(&fs_name)
                .trim_matches('\0')
                .to_string()
        } else {
            String::new()
        };

        let mut free_bytes_caller = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free_bytes = 0u64;

        // SAFETY: `pcwstr` is NUL-terminated UTF-16 and out-pointers reference stack `u64`s.
        let space_res = unsafe {
            GetDiskFreeSpaceExW(
                pcwstr,
                Some(&mut free_bytes_caller),
                Some(&mut total_bytes),
                Some(&mut total_free_bytes),
            )
        };

        if space_res.is_err() {
            continue;
        }

        let is_system = root_str.eq_ignore_ascii_case(&system_root);

        // Eligibility determination (FR-001) via pure helper
        let (is_eligible, reason) = evaluate_drive_eligibility(drive_type, &filesystem, &label);

        drives.push(DriveInfo {
            root: root_path,
            drive_letter,
            label,
            filesystem,
            total_bytes,
            free_bytes: free_bytes_caller,
            is_system,
            is_eligible,
            ineligible_reason: reason,
        });
    }

    Ok(drives)
}

/// Identifies the system drive root (typically `C:\`).
///
/// Strips trailing slashes and colons from `%SystemDrive%` to prevent double-colon formatting (`C::\`),
/// falls back to `%SystemRoot%` / `%windir%` drive letter, and defaults to `C:\`.
pub fn get_system_drive_root() -> String {
    if let Ok(sys_drive) = std::env::var("SystemDrive") {
        let clean = sys_drive.trim().trim_end_matches(['\\', '/', ':']).trim();
        if !clean.is_empty() {
            return format!("{}:\\", clean.to_uppercase());
        }
    }

    if let Ok(sys_root) = std::env::var("SystemRoot").or_else(|_| std::env::var("windir")) {
        let clean = sys_root.trim();
        if clean.len() >= 2 && clean.as_bytes()[1] == b':' {
            let drive_char = (clean.as_bytes()[0] as char).to_ascii_uppercase();
            if drive_char.is_ascii_alphabetic() {
                return format!("{}:\\", drive_char);
            }
        }
    }

    "C:\\".to_string()
}

/// Evaluates drive eligibility and ineligibility reason according to FR-001.
///
/// Returns `(true, None)` if eligible, or `(false, Some(reason))` if ineligible.
///
/// Rules:
/// 1. Must be a fixed drive (`drive_type == DRIVE_FIXED` / 3).
/// 2. Must be formatted with NTFS (`filesystem.eq_ignore_ascii_case("NTFS")`).
/// 3. Must not be a system recovery partition (`!label.to_lowercase().contains("recovery")`).
pub fn evaluate_drive_eligibility(
    drive_type: u32,
    filesystem: &str,
    label: &str,
) -> (bool, Option<String>) {
    if drive_type != DRIVE_FIXED {
        (
            false,
            Some("Ổ di động hoặc ổ mạng không được hỗ trợ".to_string()),
        )
    } else if !filesystem.eq_ignore_ascii_case("NTFS") {
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
    } else if label.to_lowercase().contains("recovery") {
        (false, Some("Phân vùng khôi phục của hệ thống".to_string()))
    } else {
        (true, None)
    }
}

/// Status of USN journal accessibility for a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsnCapabilityStatus {
    /// Process is running with standard user rights; opening raw volume handles requires Administrator elevation.
    InsufficientPrivileges,
    /// Target drive is not formatted with an NTFS filesystem.
    UnsupportedFilesystem,
    /// Failed to open volume handle `\\.\<drive>:` (e.g. invalid drive letter or sharing violation).
    CannotOpenVolume,
    /// USN change journal is not active or disabled on this volume.
    JournalNotActive,
    /// USN journal is active and accessible with current privileges.
    Ready,
}

/// Checks if the calling process has elevated Administrator privileges.
pub fn is_process_elevated() -> bool {
    // SAFETY: Queries `TokenElevation` on the current process token with `TOKEN_QUERY` read-only
    // rights; `OwnedHandle` closes the opened token handle deterministically on scope exit.
    unsafe {
        let mut raw_token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token).is_err() {
            return false;
        }
        let Some(token_guard) = crate::handle::OwnedHandle::new(raw_token) else {
            return false;
        };

        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let res = GetTokenInformation(
            token_guard.raw(),
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size,
            &mut size,
        );

        res.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// Probes whether the USN journal on the given drive can be queried by the current process.
pub fn probe_volume_usn_capability(drive_letter: char) -> UsnCapabilityStatus {
    // 1. Process elevation check
    if !is_process_elevated() {
        return UsnCapabilityStatus::InsufficientPrivileges;
    }

    let letter = drive_letter.to_ascii_uppercase();
    let root_str = format!("{}:\\", letter);
    let root_path = PathBuf::from(&root_str);
    let wide_root = path_to_wide(&root_path);
    let pcwstr = PCWSTR(wide_root.as_ptr());

    // 2. Filesystem check
    let mut fs_name = [0u16; 261];
    // SAFETY: `pcwstr` is NUL-terminated UTF-16 and `fs_name` has `MAX_PATH + 1` capacity.
    let vol_res =
        unsafe { GetVolumeInformationW(pcwstr, None, None, None, None, Some(&mut fs_name)) };
    if vol_res.is_err() {
        return UsnCapabilityStatus::CannotOpenVolume;
    }

    let fs = String::from_utf16_lossy(&fs_name)
        .trim_matches('\0')
        .to_string();
    if !fs.eq_ignore_ascii_case("NTFS") {
        return UsnCapabilityStatus::UnsupportedFilesystem;
    }

    // 3. Device handle probe: attempt to open \\.\<drive>:
    let dev_path_str = format!("\\\\.\\{}:", letter);
    let dev_path = PathBuf::from(&dev_path_str);
    let wide_dev = path_to_wide(&dev_path);

    // SAFETY: Opens raw volume read-only (`GENERIC_READ | FILE_READ_ATTRIBUTES`) with full read/write sharing.
    let handle_res = unsafe {
        windows::Win32::Storage::FileSystem::CreateFileW(
            PCWSTR(wide_dev.as_ptr()),
            0x8000_0000 | 0x0008, // GENERIC_READ | FILE_READ_ATTRIBUTES
            windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
            None,
            windows::Win32::Storage::FileSystem::OPEN_EXISTING,
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };

    let raw_handle = match handle_res {
        Ok(h) => h,
        Err(e) => {
            let code = (e.code().0 as u32) & 0xFFFF;
            return if code == windows::Win32::Foundation::ERROR_ACCESS_DENIED.0 {
                UsnCapabilityStatus::InsufficientPrivileges
            } else {
                UsnCapabilityStatus::CannotOpenVolume
            };
        }
    };
    let Some(vol_guard) = crate::handle::OwnedHandle::new(raw_handle) else {
        return UsnCapabilityStatus::CannotOpenVolume;
    };

    // 4. Query USN journal status via FSCTL_QUERY_USN_JOURNAL
    let mut journal_data = [0u8; 64];
    let mut bytes_returned = 0u32;
    // SAFETY: `journal_data` is 64 writable bytes (`>= sizeof(USN_JOURNAL_DATA_V0)`), and
    // `vol_guard` deterministically closes the volume handle when leaving scope.
    let ioctl_res = unsafe {
        windows::Win32::System::IO::DeviceIoControl(
            vol_guard.raw(),
            windows::Win32::System::Ioctl::FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(journal_data.as_mut_ptr().cast()),
            journal_data.len() as u32,
            Some(&mut bytes_returned),
            None,
        )
    };

    if ioctl_res.is_ok() {
        UsnCapabilityStatus::Ready
    } else {
        UsnCapabilityStatus::JournalNotActive
    }
}

/// Recommendation result struct wrapping drive letter.
/// Compatible with both direct char comparisons (Some('D')) and field access (.drive_letter).
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

/// Selects the optimal data drive recommendation according to FR-002:
/// 1. Non-system eligible drive with >= 10 GiB free space, picking the one with max free space.
/// 2. If none, any eligible drive with max free space.
pub fn recommend_drive(drives: &[DriveInfo]) -> Option<RecommendedDrive> {
    recommend_drive_info(drives).map(|d| RecommendedDrive {
        drive_letter: d.drive_letter,
    })
}

/// Recommends the optimal data drive letter returning Option<char>.
pub fn recommend_drive_letter(drives: &[DriveInfo]) -> Option<char> {
    recommend_drive_info(drives).map(|d| d.drive_letter)
}

/// Returns a reference to the recommended DriveInfo.
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
        let wide = path_to_wide(root);
        let mut free_caller = 0u64;
        let mut total = 0u64;
        let mut total_free = 0u64;

        // SAFETY: `wide` is NUL-terminated UTF-16 and out-pointers reference valid stack `u64`s.
        unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(wide.as_ptr()),
                Some(&mut free_caller),
                Some(&mut total),
                Some(&mut total_free),
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("GetDiskFreeSpaceExW failed for {:?}: {}", root, e),
            })?;
        }
        Ok((total, free_caller))
    }

    fn get_volume_info(&self, root: &Path) -> Result<(String, String, u32)> {
        let wide = path_to_wide(root);
        let pcwstr = PCWSTR(wide.as_ptr());
        // SAFETY: `pcwstr` points into live NUL-terminated `wide`.
        let drive_type = unsafe { GetDriveTypeW(pcwstr) };

        let mut vol_name = [0u16; 261];
        let mut fs_name = [0u16; 261];

        // SAFETY: `vol_name` and `fs_name` are stack `[u16; 261]` buffers (`MAX_PATH + 1`).
        unsafe {
            GetVolumeInformationW(
                pcwstr,
                Some(&mut vol_name),
                None,
                None,
                None,
                Some(&mut fs_name),
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("GetVolumeInformationW failed for {:?}: {}", root, e),
            })?;
        }

        let label = String::from_utf16_lossy(&vol_name)
            .trim_matches('\0')
            .to_string();
        let fs = String::from_utf16_lossy(&fs_name)
            .trim_matches('\0')
            .to_string();

        Ok((label, fs, drive_type))
    }
}
