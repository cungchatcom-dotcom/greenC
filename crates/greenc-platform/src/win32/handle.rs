use crate::traits::FileLockProbe;
use greenc_core::error::{GreenCError, Result};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// Win32 DELETE access mask (0x00010000).
const DELETE_ACCESS: u32 = 0x0001_0000;

/// Canonicalizes `path` (when existing) and blocks mutations against Windows OS system trees
/// (`\Windows`, `\Program Files`, `\Program Files (x86)`, `\System Volume Information`).
pub fn ensure_not_protected_system_path(path: &Path, operation: &str) -> Result<()> {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let raw = resolved.to_string_lossy();
    let clean = raw
        .strip_prefix(r"\\?\")
        .unwrap_or(&raw)
        .replace('/', "\\")
        .to_ascii_lowercase();

    for forbidden_prefix in [
        r"c:\windows",
        r"c:\program files",
        r"c:\program files (x86)",
        r"c:\system volume information",
    ] {
        if clean == forbidden_prefix || clean.starts_with(&format!("{forbidden_prefix}\\")) {
            return Err(GreenCError::PreconditionBlocked {
                operation: operation.to_string(),
                reason: format!(
                    "System directory '{}' is strictly protected from modification",
                    resolved.display()
                ),
            });
        }
    }
    Ok(())
}

/// RAII wrapper for native Windows HANDLE guaranteeing deterministic cleanup.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub fn new(handle: HANDLE) -> Option<Self> {
        if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
            None
        } else {
            Some(Self(handle))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: `self.0` was validated non-null and not `INVALID_HANDLE_VALUE` on construction,
            // and `OwnedHandle` owns it uniquely so `CloseHandle` runs at most once on drop.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Converts a standard Path into a null-terminated UTF-16 vector for Win32 API.
pub fn path_to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Checks if a file or directory is actively locked by any process on Windows.
/// Bypasses Windows Defender scan overhead by requesting DELETE access with 0 sharing mode.
pub fn is_file_locked(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }

    let wide = path_to_wide(path);
    // SAFETY: `wide` is a live NUL-terminated UTF-16 buffer; any returned valid `HANDLE` is
    // immediately wrapped in `OwnedHandle` and closed deterministically on scope exit.
    let handle_res = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            DELETE_ACCESS,
            FILE_SHARE_MODE(0), // Exclusive access: fail if any process has open handle
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };

    match handle_res {
        Ok(h) => {
            let _guard = OwnedHandle::new(h);
            false
        }
        Err(err) => {
            let code = (err.code().0 as u32) & 0xFFFF;
            code == ERROR_SHARING_VIOLATION.0
                || code == ERROR_LOCK_VIOLATION.0
                || code == ERROR_ACCESS_DENIED.0
        }
    }
}

/// Concrete production implementation of `FileLockProbe`.
#[derive(Default, Clone, Debug)]
pub struct NativeFileLockProbe;

impl FileLockProbe for NativeFileLockProbe {
    fn is_file_locked(&self, path: &Path) -> bool {
        is_file_locked(path)
    }
}

/// Stable Win32 physical file identity (`BY_HANDLE_FILE_INFORMATION`) for NTFS hardlink deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Win32FileIdentity {
    pub volume_serial: u64,
    pub file_index: u64,
    pub number_of_links: u32,
}

/// Queries physical NTFS volume serial, 64-bit file index, and link count via `GetFileInformationByHandle`
/// using `0` desired access (metadata-only).
pub fn query_file_identity(path: &Path) -> Option<Win32FileIdentity> {
    let wide = path_to_wide(path);
    // SAFETY: `wide` is a live NUL-terminated UTF-16 buffer; `dwDesiredAccess = 0` requests
    // attribute-only metadata access, and any valid handle is owned and closed by `OwnedHandle`.
    let handle_res = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle = OwnedHandle::new(handle_res.ok()?)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `handle.raw()` is a valid open handle and `info` is a stack-allocated output struct.
    unsafe { GetFileInformationByHandle(handle.raw(), &mut info).ok()? };
    let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some(Win32FileIdentity {
        volume_serial: u64::from(info.dwVolumeSerialNumber),
        file_index,
        number_of_links: info.nNumberOfLinks,
    })
}
