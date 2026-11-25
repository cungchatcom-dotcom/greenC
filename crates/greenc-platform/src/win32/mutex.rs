use crate::handle::OwnedHandle;
use greenc_core::config::{BUSY_MUTEX_NAME, SINGLE_INSTANCE_MUTEX_NAME};
use greenc_core::error::{GreenCError, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    LockFileEx, UnlockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows::Win32::System::IO::OVERLAPPED;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

/// Displays a native informational dialog when another instance of GreenC (including a
/// side-by-side EXE or Microsoft Store installation) already holds `GreenCAppMutex`.
pub fn notify_already_running(message: &str) {
    let text_w: Vec<u16> = std::ffi::OsStr::new(message)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let title_w: Vec<u16> = std::ffi::OsStr::new("GreenC")
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: Both `text_w` and `title_w` are valid NUL-terminated UTF-16 strings alive across the call.
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

#[derive(Debug)]
pub struct GreenCAppMutex {
    _handle: OwnedHandle,
}

impl Drop for GreenCAppMutex {
    fn drop(&mut self) {
        // SAFETY: `self._handle` is a valid mutex handle acquired with `bInitialOwner = true`
        // by the current thread/process; `ReleaseMutex` releases ownership before `_handle` drops.
        unsafe {
            let _ = ReleaseMutex(self._handle.raw());
        }
    }
}

impl GreenCAppMutex {
    /// Attempts to acquire the global single-instance application mutex.
    /// Returns `Err(GreenCError::PreconditionBlocked)` if another instance is already running.
    pub fn acquire() -> Result<Self> {
        Self::acquire_named(SINGLE_INSTANCE_MUTEX_NAME)
    }

    /// Attempts to acquire a single-instance mutex with a custom or parameterized name.
    ///
    /// Returns `Err(GreenCError::PreconditionBlocked)` if another instance with the same name is already running.
    pub fn acquire_named(name: &str) -> Result<Self> {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();

        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string; the returned `HANDLE` is
        // immediately wrapped inside `OwnedHandle` so early-return on `ERROR_ALREADY_EXISTS`
        // closes the duplicate handle without leaking kernel resources.
        unsafe {
            let handle_res = CreateMutexW(None, true, PCWSTR(wide.as_ptr()));
            let last_error = GetLastError();
            let handle = handle_res.map_err(|e| GreenCError::Win32 {
                code: (e.code().0 as u32) & 0xFFFF,
                message: format!("Failed to create single-instance mutex '{}': {}", name, e),
            })?;

            let owned = OwnedHandle::new(handle).ok_or_else(|| GreenCError::Win32 {
                code: 0,
                message: format!("Created mutex handle for '{}' was invalid", name),
            })?;

            if last_error == ERROR_ALREADY_EXISTS {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "Application Startup".to_string(),
                    reason: format!(
                        "Another instance of GreenC (mutex '{}') is already running",
                        name
                    ),
                });
            }

            Ok(Self { _handle: owned })
        }
    }
}

/// Named mutex whose existence tells the installer that files are being changed.
#[derive(Debug)]
pub struct BusyMarker {
    _handle: OwnedHandle,
}

// SAFETY: The mutex is created unowned and is never waited on or released, so no thread affinity
// applies; a kernel handle may be closed from any thread.
unsafe impl Send for BusyMarker {}

impl BusyMarker {
    pub fn hold() -> Option<Self> {
        let wide: Vec<u16> = std::ffi::OsStr::new(BUSY_MUTEX_NAME)
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the handle is owned immediately.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }.ok()?;
        Some(Self {
            _handle: OwnedHandle::new(handle)?,
        })
    }
}

/// Machine-wide, cross-session relocation lock backed by an exclusive Win32 `LockFileEx` byte-range
/// lock on a lock file adjacent to the GreenC database (F12).
///
/// Unlike a `Local\` kernel mutex (which is isolated to a single Terminal Services session), a lock
/// file next to the DB enforces mutual exclusion across ALL Windows sessions (console, RDP,
/// elevated/non-elevated, scheduled tasks, GUI and CLI) operating on the same database without
/// needing `SeCreateGlobalPrivilege`. The Windows kernel automatically releases the lock if the
/// holding process exits or crashes abruptly.
#[derive(Debug)]
pub struct RelocationLock {
    file: File,
}

impl Drop for RelocationLock {
    fn drop(&mut self) {
        let mut overlapped = OVERLAPPED::default();
        let handle = HANDLE(self.file.as_raw_handle());
        // SAFETY: `handle` is a valid open file handle owned by `self.file` and `overlapped` is a
        // valid zero-initialized `OVERLAPPED` matching the `[0, u32::MAX:u32::MAX]` range locked on
        // acquisition.
        unsafe {
            let _ = UnlockFileEx(handle, 0, u32::MAX, u32::MAX, &mut overlapped);
        }
    }
}

impl RelocationLock {
    /// Computes the deterministic lock file path adjacent to `db_path` so GUI, CLI, and
    /// cross-session processes pointing at the same database contend on the exact same file.
    pub fn lock_path_for_db(db_path: &Path) -> PathBuf {
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

    /// Non-blocking attempt to acquire the cross-session relocation lock for `db_path`.
    /// Returns `Ok(Some(lock))` if acquired, or `Ok(None)` if another operation/process/session
    /// currently holds it.
    pub fn try_acquire_for_db(db_path: &Path) -> Result<Option<Self>> {
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

        let mut overlapped = OVERLAPPED::default();
        let handle = HANDLE(file.as_raw_handle());
        // SAFETY: `handle` is a valid open file handle owned by `file`, `overlapped` is a valid
        // zero-initialized `OVERLAPPED` starting at offset 0, and `LOCKFILE_EXCLUSIVE_LOCK |
        // LOCKFILE_FAIL_IMMEDIATELY` performs a non-blocking exclusive byte-range lock.
        let lock_res = unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };

        if let Err(err) = lock_res {
            let code = (err.code().0 as u32) & 0xFFFF;
            if code == ERROR_LOCK_VIOLATION.0 || code == ERROR_SHARING_VIOLATION.0 {
                return Ok(None);
            }
            return Err(GreenCError::Win32 {
                code,
                message: format!(
                    "Failed to lock relocation file '{}': {}",
                    lock_path.display(),
                    err
                ),
            });
        }

        // Write PID for diagnostic visibility (best-effort; kernel LockFileEx is authoritative).
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();

        Ok(Some(Self { file }))
    }

    /// Acquires the cross-session relocation lock for `db_path`, returning `PreconditionBlocked`
    /// if another GUI/CLI process, thread, or Windows session currently holds it.
    pub fn acquire_for_db(db_path: &Path) -> Result<Self> {
        Self::try_acquire_for_db(db_path)?.ok_or_else(|| GreenCError::PreconditionBlocked {
            operation: "relocation_lock".to_string(),
            reason: "Đang có thao tác chuyển hoặc khôi phục dữ liệu khác đang chạy. Vui lòng đợi hoàn tất rồi thử lại.".to_string(),
        })
    }

    /// Non-blocking attempt for the default GreenC database location.
    pub fn try_acquire_global() -> Result<Option<Self>> {
        Self::try_acquire_for_db(&greenc_core::config::default_db_path())
    }

    /// Acquires the cross-session relocation lock for the default GreenC database location.
    pub fn acquire_global() -> Result<Self> {
        Self::acquire_for_db(&greenc_core::config::default_db_path())
    }
}
