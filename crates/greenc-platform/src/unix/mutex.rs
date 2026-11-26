//! Single-instance application lock using POSIX `flock(LOCK_EX | LOCK_NB)`.

use greenc_core::config::{app_home, SINGLE_INSTANCE_MUTEX_NAME};
use greenc_core::error::{GreenCError, Result};
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

/// Reports to the user that another instance of GreenC is already running.
pub fn notify_already_running(message: &str) {
    eprintln!("{message}");
}

/// RAII single-instance guard backed by an advisory exclusive `flock` on Unix.
///
/// The kernel automatically releases the lock when the file descriptor is closed or the
/// process terminates (even on crash).
#[derive(Debug)]
pub struct GreenCAppMutex {
    file: File,
}

impl Drop for GreenCAppMutex {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is a valid open file descriptor owned by `self.file`.
        unsafe {
            let _ = libc::flock(fd, libc::LOCK_UN);
        }
    }
}

impl GreenCAppMutex {
    /// Attempts to acquire the global single-instance application lock.
    pub fn acquire() -> Result<Self> {
        Self::acquire_named(SINGLE_INSTANCE_MUTEX_NAME)
    }

    /// Attempts to acquire a named single-instance lock.
    pub fn acquire_named(name: &str) -> Result<Self> {
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();

        let lock_dir: PathBuf = if name == SINGLE_INSTANCE_MUTEX_NAME {
            app_home()
        } else {
            std::env::temp_dir().join("greenc-locks")
        };

        std::fs::create_dir_all(&lock_dir)?;
        let lock_path = lock_dir.join(format!("{sanitized}.lock"));

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;

        let fd = file.as_raw_fd();
        // SAFETY: `fd` is a valid open file descriptor owned by `file`.
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(GreenCError::PreconditionBlocked {
                operation: "Application Startup".to_string(),
                reason: format!(
                    "Another instance of GreenC (lock '{}') is already running",
                    name
                ),
            });
        }

        Ok(Self { file })
    }
}

/// Windows installers look for this marker; other platforms have no equivalent.
#[derive(Debug)]
pub struct BusyMarker(());

impl BusyMarker {
    pub fn hold() -> Option<Self> {
        Some(Self(()))
    }
}

/// Machine-wide relocation lock backed by an exclusive advisory POSIX `flock(LOCK_EX | LOCK_NB)`.
///
/// Shared across GUI and CLI processes operating on the same GreenC database. Automatically
/// released by the kernel if the holding process exits or crashes abruptly.
#[derive(Debug)]
pub struct RelocationLock {
    file: File,
}

impl Drop for RelocationLock {
    fn drop(&mut self) {
        let fd = self.file.as_raw_fd();
        // SAFETY: `fd` is a valid open file descriptor owned by `self.file`.
        unsafe {
            let _ = libc::flock(fd, libc::LOCK_UN);
        }
    }
}

impl RelocationLock {
    /// Computes the deterministic lock file path paired with `db_path` so GUI and CLI
    /// processes pointing at the same database contend on the exact same inode.
    pub fn lock_path_for_db(db_path: &std::path::Path) -> PathBuf {
        let db_name = db_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("greenc.db");
        let sanitized_name: String = db_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();

        let parent = db_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
            .unwrap_or_else(|| std::env::temp_dir().join("greenc-locks"));

        parent.join(format!(".greenc-relocation-{sanitized_name}.lock"))
    }

    /// Non-blocking attempt to acquire the machine-wide relocation lock for `db_path`.
    /// Returns `Ok(Some(lock))` if acquired, or `Ok(None)` if another operation/process holds it.
    pub fn try_acquire_for_db(db_path: &std::path::Path) -> Result<Option<Self>> {
        let lock_path = Self::lock_path_for_db(db_path);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;

        let fd = file.as_raw_fd();
        // SAFETY: `fd` is a valid open file descriptor owned by `file`.
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK)
                || err.raw_os_error() == Some(libc::EAGAIN)
            {
                return Ok(None);
            }
            return Err(GreenCError::Io(err));
        }

        // Write PID for diagnostic visibility (best-effort; kernel flock is authoritative).
        use std::io::Write;
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();

        Ok(Some(Self { file }))
    }

    /// Acquires the machine-wide relocation lock for `db_path`, returning `PreconditionBlocked`
    /// if another GUI/CLI process or thread currently holds it.
    pub fn acquire_for_db(db_path: &std::path::Path) -> Result<Self> {
        Self::try_acquire_for_db(db_path)?.ok_or_else(|| GreenCError::PreconditionBlocked {
            operation: "relocation_lock".to_string(),
            reason: "Đang có thao tác chuyển hoặc khôi phục dữ liệu khác đang chạy. Vui lòng đợi hoàn tất rồi thử lại.".to_string(),
        })
    }
}
