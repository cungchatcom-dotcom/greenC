use crate::handle::OwnedHandle;
use greenc_core::config::{BUSY_MUTEX_NAME, SINGLE_INSTANCE_MUTEX_NAME};
use greenc_core::error::{GreenCError, Result};
use std::os::windows::ffi::OsStrExt;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
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

/// Deterministic 128-bit FNV-1a hash used to map a canonical DB path to a compact Win32 mutex name.
fn fnv1a_128_hex(bytes: &[u8]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u128::from(b);
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:032x}")
}

/// Machine-wide relocation lock backed by a named Win32 kernel mutex (`Local\GreenC_Relocation_*`).
///
/// Shared across GUI and CLI processes in the user session operating on the same GreenC database.
/// Automatically released/destroyed by the Windows kernel if the holding process exits or crashes.
#[derive(Debug)]
pub struct RelocationLock {
    _handle: OwnedHandle,
}

impl Drop for RelocationLock {
    fn drop(&mut self) {
        // SAFETY: `self._handle` is a valid mutex handle owned by this guard; releasing before
        // `CloseHandle` avoids leaving an abandoned mutex state.
        unsafe {
            let _ = ReleaseMutex(self._handle.raw());
        }
    }
}

impl RelocationLock {
    /// Computes the deterministic session-local Win32 mutex name paired with `db_path`.
    pub fn mutex_name_for_db(db_path: &std::path::Path) -> String {
        let resolved = if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty())
        {
            let canon_parent =
                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            match db_path.file_name() {
                Some(name) => canon_parent.join(name),
                None => canon_parent,
            }
        } else {
            db_path.to_path_buf()
        };
        let raw = resolved.to_string_lossy();
        let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
        let normalized = stripped.replace('/', "\\").to_ascii_lowercase();
        format!(
            "Local\\GreenC_Relocation_{}",
            fnv1a_128_hex(normalized.as_bytes())
        )
    }

    /// Non-blocking attempt to acquire the machine-wide relocation lock for `db_path`.
    /// Returns `Ok(Some(lock))` if acquired, or `Ok(None)` if another operation/process holds it.
    pub fn try_acquire_for_db(db_path: &std::path::Path) -> Result<Option<Self>> {
        let name = Self::mutex_name_for_db(db_path);
        let wide: Vec<u16> = std::ffi::OsStr::new(&name)
            .encode_wide()
            .chain(Some(0))
            .collect();

        // SAFETY: `wide` is a valid NUL-terminated UTF-16 string; the returned `HANDLE` is
        // immediately wrapped in `OwnedHandle` so early-return on `ERROR_ALREADY_EXISTS` closes
        // the duplicate handle without releasing the existing holder's ownership.
        unsafe {
            let handle_res = CreateMutexW(None, true, PCWSTR(wide.as_ptr()));
            let last_error = GetLastError();
            let handle = handle_res.map_err(|e| GreenCError::Win32 {
                code: (e.code().0 as u32) & 0xFFFF,
                message: format!("Failed to create relocation mutex '{}': {}", name, e),
            })?;

            let owned = OwnedHandle::new(handle).ok_or_else(|| GreenCError::Win32 {
                code: 0,
                message: format!("Created relocation mutex handle for '{}' was invalid", name),
            })?;

            if last_error == ERROR_ALREADY_EXISTS {
                return Ok(None);
            }

            Ok(Some(Self { _handle: owned }))
        }
    }

    /// Acquires the machine-wide relocation lock for `db_path`, returning `PreconditionBlocked`
    /// if another GUI/CLI process or thread currently holds it.
    pub fn acquire_for_db(db_path: &std::path::Path) -> Result<Self> {
        Self::try_acquire_for_db(db_path)?.ok_or_else(|| GreenCError::PreconditionBlocked {
            operation: "relocation_lock".to_string(),
            reason: "Đang có thao tác chuyển hoặc khôi phục dữ liệu khác đang chạy. Vui lòng đợi hoàn tất rồi thử lại.".to_string(),
        })
    }

    /// Non-blocking attempt for the default GreenC database location.
    pub fn try_acquire_global() -> Result<Option<Self>> {
        Self::try_acquire_for_db(&greenc_core::config::default_db_path())
    }

    /// Acquires the machine-wide relocation lock for the default GreenC database location.
    pub fn acquire_global() -> Result<Self> {
        Self::acquire_for_db(&greenc_core::config::default_db_path())
    }
}
