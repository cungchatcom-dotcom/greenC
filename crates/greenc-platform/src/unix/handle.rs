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

/// Probes whether a file is currently locked by another process using non-blocking `flock`.
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
        false
    } else {
        let err = std::io::Error::last_os_error();
        err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN)
    }
}

/// Queries physical file identity `(dev, ino)` for hardlink deduplication.
pub fn query_file_identity(path: &Path) -> Option<Win32FileIdentity> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Win32FileIdentity {
        volume_serial: meta.dev(),
        file_index: meta.ino(),
    })
}

/// Native Unix implementation of `FileLockProbe`.
#[derive(Default, Clone, Debug)]
pub struct NativeFileLockProbe;

impl FileLockProbe for NativeFileLockProbe {
    fn is_file_locked(&self, path: &Path) -> bool {
        is_file_locked(path)
    }
}
