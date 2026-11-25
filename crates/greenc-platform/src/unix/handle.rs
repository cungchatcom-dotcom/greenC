//! Unix file lock probe, protected system path guard, and inode-based file identity.

use crate::traits::FileLockProbe;
use crate::unix_parsers::is_protected_unix_path;
use greenc_core::error::{GreenCError, Result};
use std::fs::OpenOptions;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Unique physical file identity backed by POSIX `(st_dev, st_ino)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Win32FileIdentity {
    pub volume_serial: u64,
    pub file_index: u64,
    pub number_of_links: u32,
}

/// Canonicalizes `path` (when existing) and blocks mutations against critical Unix OS trees.
pub fn ensure_not_protected_system_path(path: &Path, operation: &str) -> Result<()> {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if is_protected_unix_path(&resolved) {
        return Err(GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "System directory '{}' is strictly protected from modification",
                resolved.display()
            ),
        });
    }
    Ok(())
}

/// Probes whether a file is currently locked by another process using non-blocking `flock`
/// AND POSIX advisory record lock inspection (`fcntl(F_GETLK)`).
///
/// Important Unix limitation: neither `flock` nor `fcntl(F_GETLK)` detects a process that merely
/// holds a plain open file descriptor without acquiring an OS lock (e.g. a build tool reading
/// cache artifacts while holding a lock only on a sibling `.lock` file). Callers cleaning
/// developer caches MUST also enforce process-level busy-tool detection and atomic cache units.
pub fn is_file_locked(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }

    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.is_dir() || meta.file_type().is_symlink() {
        return false;
    }

    let file = match OpenOptions::new().read(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            return matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
            );
        }
    };

    let fd = file.as_raw_fd();
    // SAFETY: `fd` is a valid open file descriptor owned by `file`. We probe with non-blocking
    // `LOCK_EX | LOCK_NB` and immediately release via `LOCK_UN` if acquired.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        // SAFETY: `fd` holds the exclusive lock we just acquired above; unlock before drop.
        unsafe {
            let _ = libc::flock(fd, libc::LOCK_UN);
        }
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN)
        {
            return true;
        }
    }

    // Secondary check: inspect POSIX `fcntl(F_GETLK)` locks held by OTHER processes (such as JVM
    // `FileChannel.lock()` used by Gradle on Linux/macOS).
    // SAFETY: `fl` is zero-initialized and populated with valid fields for `F_GETLK` on open `fd`.
    unsafe {
        let mut fl: libc::flock = std::mem::zeroed();
        fl.l_type = libc::F_WRLCK as libc::c_short;
        fl.l_whence = libc::SEEK_SET as libc::c_short;
        fl.l_start = 0;
        fl.l_len = 0;
        if libc::fcntl(fd, libc::F_GETLK, &mut fl) == 0
            && fl.l_type != (libc::F_UNLCK as libc::c_short)
        {
            return true;
        }
    }

    false
}

/// Queries physical file identity `(dev, ino, nlink)` for hardlink deduplication.
pub fn query_file_identity(path: &Path) -> Option<Win32FileIdentity> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Win32FileIdentity {
        volume_serial: meta.dev(),
        file_index: meta.ino(),
        number_of_links: meta.nlink() as u32,
    })
}

/// Returns the effective UID of the current process.
pub fn current_user_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, cannot fail, and has no memory safety preconditions.
    unsafe { libc::geteuid() }
}

/// Checks via `lstat` (`symlink_metadata`, never following symlinks) whether `path` is owned
/// by `expected_uid`.
pub fn is_owned_by_uid(path: &Path, expected_uid: u32) -> std::io::Result<bool> {
    let meta = std::fs::symlink_metadata(path)?;
    Ok(meta.uid() == expected_uid)
}

/// Checks via `lstat` (`symlink_metadata`, never following symlinks) whether `path` is owned
/// by the current effective UID.
pub fn is_owned_by_current_user(path: &Path) -> std::io::Result<bool> {
    is_owned_by_uid(path, current_user_uid())
}

/// Verifies that `path` is a real directory (not a symlink), owned by `expected_uid` (or current
/// effective UID if `None`), and NOT world-writable (`mode & 0o002 == 0`).
pub fn is_trusted_user_dir(
    path: &Path,
    expected_uid: Option<u32>,
) -> std::result::Result<(), String> {
    let uid = expected_uid.unwrap_or_else(current_user_uid);
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
    if meta.uid() != uid {
        return Err(format!(
            "Thư mục '{}' thuộc uid {} khác uid hiện tại {}",
            path.display(),
            meta.uid(),
            uid
        ));
    }
    let perm_bits = meta.mode() & 0o7777;
    if (perm_bits & 0o002) != 0 {
        return Err(format!(
            "Thư mục '{}' có quyền ghi công khai ({perm_bits:04o}), từ chối",
            path.display()
        ));
    }
    Ok(())
}

/// Verifies that a GreenC staging directory is a real directory (not a symlink), owned by
/// `expected_uid` (or current effective UID if `None`), with strict `0700` permissions.
pub fn validate_private_staging_dir(
    path: &Path,
    expected_uid: Option<u32>,
) -> std::result::Result<(), String> {
    let uid = expected_uid.unwrap_or_else(current_user_uid);
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        format!(
            "Không đọc được siêu dữ liệu khu chờ '{}': {e}",
            path.display()
        )
    })?;
    if meta.file_type().is_symlink() {
        return Err(format!("Khu chờ '{}' là symlink, từ chối", path.display()));
    }
    if !meta.is_dir() {
        return Err(format!(
            "Khu chờ '{}' không phải thư mục thật",
            path.display()
        ));
    }
    if meta.uid() != uid {
        return Err(format!(
            "Khu chờ '{}' thuộc uid {} khác uid hiện tại {}",
            path.display(),
            meta.uid(),
            uid
        ));
    }
    let perm_bits = meta.mode() & 0o7777;
    if perm_bits != 0o700 {
        return Err(format!(
            "Khu chờ '{}' có quyền {:04o} khác 0700, từ chối",
            path.display(),
            perm_bits
        ));
    }
    Ok(())
}

/// Restricts a real directory (never following symlinks) to current-user-only access (`0700`).
pub fn restrict_dir_to_current_user(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restrict_dir_to_current_user".to_string(),
            reason: format!("Từ chối đặt quyền trên symlink '{}'", path.display()),
        });
    }
    if !meta.is_dir() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restrict_dir_to_current_user".to_string(),
            reason: format!("Đường dẫn '{}' không phải thư mục thật", path.display()),
        });
    }
    if !is_owned_by_current_user(path)? {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restrict_dir_to_current_user".to_string(),
            reason: format!(
                "Thư mục '{}' không thuộc người dùng hiện tại",
                path.display()
            ),
        });
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    let verified = std::fs::symlink_metadata(path)?;
    if (verified.mode() & 0o777) != 0o700 {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restrict_dir_to_current_user".to_string(),
            reason: format!(
                "Không xác minh được quyền 0700 trên thư mục '{}'",
                path.display()
            ),
        });
    }
    Ok(())
}

/// Reads a UTF-8 text file without updating its `atime` when the OS permits (`O_NOATIME` on Linux
/// for files owned by the current UID), falling back to a regular read otherwise.
pub fn read_to_string_noatime(path: &Path) -> std::io::Result<String> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME)
            .open(path)
        {
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            return Ok(buf);
        }
    }
    std::fs::read_to_string(path)
}

/// Native Unix implementation of `FileLockProbe`.
#[derive(Default, Clone, Debug)]
pub struct NativeFileLockProbe;

impl FileLockProbe for NativeFileLockProbe {
    fn is_file_locked(&self, path: &Path) -> bool {
        is_file_locked(path)
    }
}
