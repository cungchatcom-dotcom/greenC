use crate::handle::OwnedHandle;
use greenc_core::config::SINGLE_INSTANCE_MUTEX_NAME;
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
