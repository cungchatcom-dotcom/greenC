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
