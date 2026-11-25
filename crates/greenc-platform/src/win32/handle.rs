use crate::traits::FileLockProbe;
use greenc_core::error::{GreenCError, Result};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, BOOL, ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::{
    EqualSid, GetKernelObjectSecurity, GetSecurityDescriptorOwner, GetTokenInformation, TokenOwner,
    TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_OWNER, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_MODE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Win32 DELETE access mask (0x00010000).
const DELETE_ACCESS: u32 = 0x0001_0000;
/// Win32 READ_CONTROL access mask (0x00020000) for querying object security descriptors.
const READ_CONTROL_ACCESS: u32 = 0x0002_0000;

/// Canonicalizes `path` (when existing) and blocks mutations against Windows OS system trees
/// (`\Windows`, `\Program Files`, `\Program Files (x86)`, `\System Volume Information`,
/// `WindowsApps`, and `%LOCALAPPDATA%\Packages`).
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
    if greenc_core::config::is_windows_apps_or_packages_path(&resolved) {
        // In standard (non-Store) builds, allow explicit VHDX compaction of WSL2 ext4.vhdx
        // images living under `%LOCALAPPDATA%\Packages\<Distro>\LocalState\ext4.vhdx`, while
        // still strictly blocking `WindowsApps` and all other mutations inside `Packages`.
        let is_standard_wsl_vhdx = !cfg!(feature = "store")
            && operation == "compact_vhdx"
            && !clean.split('\\').any(|seg| seg == "windowsapps")
            && clean.ends_with(".vhdx");
        if !is_standard_wsl_vhdx {
            return Err(GreenCError::PreconditionBlocked {
                operation: operation.to_string(),
                reason: format!(
                    "Windows Store / MSIX package path '{}' is strictly protected from modification",
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

/// Reads a UTF-8 text file and puts the previous last-access time back (NTFS may bump it on read),
/// matching the Unix `O_NOATIME` helper. Without this, scanning a cache index would make every entry
/// look recently used. Falls back to a plain read when attributes cannot be written.
pub fn read_to_string_noatime(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_READ_DATA: u32 = 0x0001;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;

    let opened = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .open(path);
    let Ok(mut file) = opened else {
        return std::fs::read_to_string(path);
    };
    let original_access = file.metadata().and_then(|m| m.accessed()).ok();
    let mut text = String::new();
    let result = file.read_to_string(&mut text);
    if let Some(accessed) = original_access {
        let _ = file.set_times(std::fs::FileTimes::new().set_accessed(accessed));
    }
    result.map(|_| text)
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

/// Checks (without following reparse points/symlinks) whether `path` is owned by the current
/// Windows user SID.
pub fn is_owned_by_current_user(path: &Path) -> std::io::Result<bool> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess()` returns the pseudo-handle for the current process, and
    // `token` receives a real handle wrapped in `OwnedHandle` below.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| std::io::Error::other(format!("OpenProcessToken failed: {e}")))?;
    }
    let token_guard = OwnedHandle::new(token)
        .ok_or_else(|| std::io::Error::other("Invalid process token handle"))?;

    let mut token_len = 0u32;
    // SAFETY: First call queries required buffer size for `TokenUser`.
    let _ = unsafe { GetTokenInformation(token_guard.raw(), TokenUser, None, 0, &mut token_len) };
    if token_len == 0 {
        return Err(std::io::Error::other(
            "GetTokenInformation returned zero length",
        ));
    }
    let mut token_buf = vec![0u8; token_len as usize];
    // SAFETY: `token_buf` has exact length `token_len` allocated for `TOKEN_USER`.
    unsafe {
        GetTokenInformation(
            token_guard.raw(),
            TokenUser,
            Some(token_buf.as_mut_ptr().cast()),
            token_len,
            &mut token_len,
        )
        .map_err(|e| std::io::Error::other(format!("GetTokenInformation failed: {e}")))?;
    }
    // SAFETY: Buffer was filled by `GetTokenInformation(TokenUser)` and starts with `TOKEN_USER`.
    let current_sid: PSID =
        unsafe { std::ptr::read_unaligned(token_buf.as_ptr().cast::<TOKEN_USER>()) }
            .User
            .Sid;

    let wide = path_to_wide(path);
    // SAFETY: `wide` is NUL-terminated UTF-16; `FILE_FLAG_OPEN_REPARSE_POINT` prevents following
    // symlinks/junctions, and any returned handle is owned by `OwnedHandle`.
    let obj_handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            READ_CONTROL_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
        .map_err(|e| std::io::Error::other(format!("CreateFileW(READ_CONTROL) failed: {e}")))?
    };
    let obj_guard = OwnedHandle::new(obj_handle)
        .ok_or_else(|| std::io::Error::other("Invalid object handle"))?;

    let mut sd_len = 0u32;
    // SAFETY: Queries required buffer size for owner security descriptor on open handle.
    let _ = unsafe {
        GetKernelObjectSecurity(
            obj_guard.raw(),
            OWNER_SECURITY_INFORMATION.0,
            PSECURITY_DESCRIPTOR::default(),
            0,
            &mut sd_len,
        )
    };
    if sd_len == 0 {
        return Err(std::io::Error::other(
            "GetKernelObjectSecurity returned zero length",
        ));
    }
    let mut sd_buf = vec![0u8; sd_len as usize];
    let sd_ptr = PSECURITY_DESCRIPTOR(sd_buf.as_mut_ptr().cast());
    // SAFETY: `sd_buf` has exact length `sd_len` for `OWNER_SECURITY_INFORMATION`.
    unsafe {
        GetKernelObjectSecurity(
            obj_guard.raw(),
            OWNER_SECURITY_INFORMATION.0,
            sd_ptr,
            sd_len,
            &mut sd_len,
        )
        .map_err(|e| std::io::Error::other(format!("GetKernelObjectSecurity failed: {e}")))?;
    }

    let mut owner_sid = PSID::default();
    let mut defaulted = BOOL(0);
    // SAFETY: `sd_ptr` points to a valid self-relative security descriptor in `sd_buf`.
    unsafe {
        GetSecurityDescriptorOwner(sd_ptr, &mut owner_sid, &mut defaulted).map_err(|e| {
            std::io::Error::other(format!("GetSecurityDescriptorOwner failed: {e}"))
        })?;
    }
    if owner_sid.is_invalid() || current_sid.is_invalid() {
        return Ok(false);
    }

    // SAFETY: Both SIDs are valid pointers living inside `sd_buf` and `token_buf`.
    if unsafe { EqualSid(owner_sid, current_sid).is_ok() } {
        return Ok(true);
    }

    // Elevated administrators create objects owned by the token's default owner (usually
    // BUILTIN\Administrators), so accept exactly that SID too. Non-elevated tokens report the user
    // SID here, which keeps the check strict for normal users.
    let mut owner_len = 0u32;
    // SAFETY: Queries the required buffer size for `TokenOwner`.
    let _ = unsafe { GetTokenInformation(token_guard.raw(), TokenOwner, None, 0, &mut owner_len) };
    if owner_len == 0 {
        return Ok(false);
    }
    let mut owner_buf = vec![0u8; owner_len as usize];
    // SAFETY: `owner_buf` has exact length `owner_len` allocated for `TOKEN_OWNER`.
    if unsafe {
        GetTokenInformation(
            token_guard.raw(),
            TokenOwner,
            Some(owner_buf.as_mut_ptr().cast()),
            owner_len,
            &mut owner_len,
        )
    }
    .is_err()
    {
        return Ok(false);
    }
    // SAFETY: Buffer was filled by `GetTokenInformation(TokenOwner)` and starts with `TOKEN_OWNER`.
    let default_owner: PSID =
        unsafe { std::ptr::read_unaligned(owner_buf.as_ptr().cast::<TOKEN_OWNER>()) }.Owner;
    if default_owner.is_invalid() {
        return Ok(false);
    }
    // SAFETY: Both SIDs are valid pointers living inside `sd_buf` and `owner_buf`.
    Ok(unsafe { EqualSid(owner_sid, default_owner).is_ok() })
}

/// Verifies that `path` is a real directory (not a symlink/reparse point) owned by the current
/// Windows user.
pub fn is_trusted_user_dir(
    path: &Path,
    _expected_uid: Option<u32>,
) -> std::result::Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        format!(
            "Không đọc được siêu dữ liệu thư mục '{}': {e}",
            path.display()
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(format!("Thư mục '{}' là symlink, từ chối", path.display()));
    }
    if !meta.is_dir() {
        return Err(format!(
            "Đường dẫn '{}' không phải thư mục thật",
            path.display()
        ));
    }
    match is_owned_by_current_user(path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "Thư mục '{}' không thuộc người dùng hiện tại, từ chối",
            path.display()
        )),
        Err(e) => Err(format!(
            "Không xác minh được chủ sở hữu thư mục '{}': {e}",
            path.display()
        )),
    }
}

/// Verifies that a GreenC staging directory is a real directory owned by the current Windows user.
pub fn validate_private_staging_dir(
    path: &Path,
    expected_uid: Option<u32>,
) -> std::result::Result<(), String> {
    is_trusted_user_dir(path, expected_uid)
}
