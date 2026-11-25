//! Battery-saver, fullscreen presentation/gaming detection, and per-user Registry Autostart
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) with RAII `OwnedRegKey` (Task T059).

use greenc_core::error::{GreenCError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use windows::core::PCWSTR;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};

const RUN_REGISTRY_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// Snapshot of system power and fullscreen/presentation session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerSessionState {
    pub on_battery: bool,
    pub battery_saver_on: bool,
    pub battery_percent: u8,
    pub is_fullscreen_or_busy: bool,
}

impl Default for PowerSessionState {
    fn default() -> Self {
        Self {
            on_battery: false,
            battery_saver_on: false,
            battery_percent: 100,
            is_fullscreen_or_busy: false,
        }
    }
}

impl PowerSessionState {
    /// Returns true if heavy background compression should be deferred.
    pub const fn should_defer_background_work(&self) -> bool {
        self.on_battery || self.battery_saver_on || self.is_fullscreen_or_busy
    }
}

/// Trait abstracting power/fullscreen telemetry and per-user autostart registration.
pub trait PowerSessionApi: Send + Sync {
    fn query_state(&self) -> Result<PowerSessionState>;
    fn set_autostart(&self, app_name: &str, exe_path: &Path, enabled: bool) -> Result<()>;
    fn is_autostart_enabled(&self, app_name: &str) -> Result<bool>;
}

/// RAII wrapper around a Win32 Registry `HKEY` guaranteeing `RegCloseKey` on drop.
#[derive(Debug)]
pub struct OwnedRegKey(HKEY);

impl OwnedRegKey {
    pub fn new(key: HKEY) -> Option<Self> {
        if key.is_invalid() {
            None
        } else {
            Some(Self(key))
        }
    }

    pub const fn raw(&self) -> HKEY {
        self.0
    }
}

impl Drop for OwnedRegKey {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` was opened via `RegOpenKeyExW` and is closed exactly once on drop.
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }
}

/// Queries live Windows AC/battery status and D3D fullscreen / presentation notification state.
pub fn query_power_session_native() -> Result<PowerSessionState> {
    let mut power = SYSTEM_POWER_STATUS::default();
    // SAFETY: `power` is a stack-allocated `SYSTEM_POWER_STATUS` valid for writing.
    let power_ok = unsafe { GetSystemPowerStatus(&mut power) }.is_ok();

    let (on_battery, battery_saver_on, battery_percent) = if power_ok {
        // ACLineStatus == 0 means offline (running on battery); 1 = AC online; 255 = unknown (desktop PC).
        let on_bat = power.ACLineStatus == 0;
        // SystemStatusFlag == 1 indicates Windows Battery Saver is active.
        let saver = power.SystemStatusFlag == 1;
        let pct = if power.BatteryLifePercent <= 100 {
            power.BatteryLifePercent
        } else {
            100
        };
        (on_bat, saver, pct)
    } else {
        (false, false, 100)
    };

    // SAFETY: `SHQueryUserNotificationState` writes into a single valid enum output reference.
    let is_fullscreen_or_busy = unsafe { SHQueryUserNotificationState() }
        .map(|state| {
            state == QUNS_RUNNING_D3D_FULL_SCREEN
                || state == QUNS_PRESENTATION_MODE
                || state == QUNS_BUSY
        })
        .unwrap_or(false);

    Ok(PowerSessionState {
        on_battery,
        battery_saver_on,
        battery_percent,
        is_fullscreen_or_busy,
    })
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Enables or disables per-user Windows login autostart (`--minimized`) without requiring Admin.
pub fn set_autostart_registry_native(app_name: &str, exe_path: &Path, enabled: bool) -> Result<()> {
    let subkey_w = to_wide_null(RUN_REGISTRY_SUBKEY);
    let mut raw_key = HKEY::default();
    // SAFETY: Opens or creates `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for the current
    // user only. The key can be missing on fresh profiles, so it must be created on demand.
    let open_err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut raw_key,
            None,
        )
    };
    if open_err.is_err() {
        return Err(GreenCError::Win32 {
            code: open_err.0,
            message: "Failed to open HKCU Run registry key".to_string(),
        });
    }
    let Some(reg_key) = OwnedRegKey::new(raw_key) else {
        return Err(GreenCError::Win32 {
            code: 6,
            message: "Invalid HKCU Run registry handle".to_string(),
        });
    };

    let value_name_w = to_wide_null(app_name);
    if enabled {
        let command = format!("\"{}\" --minimized", exe_path.display());
        let cmd_wide = to_wide_null(&command);
        // SAFETY: `cmd_wide` is a valid Vec<u16>; viewing its initialized bytes as `&[u8]`
        // (`len * 2` bytes including the UTF-16 NUL terminator) satisfies `REG_SZ` layout.
        let byte_slice = unsafe {
            std::slice::from_raw_parts(cmd_wide.as_ptr().cast::<u8>(), cmd_wide.len() * 2)
        };
        // SAFETY: `reg_key.raw()` is open with `KEY_SET_VALUE` and `value_name_w` is NUL-terminated.
        let set_err = unsafe {
            RegSetValueExW(
                reg_key.raw(),
                PCWSTR(value_name_w.as_ptr()),
                0,
                REG_SZ,
                Some(byte_slice),
            )
        };
        if set_err.is_err() {
            return Err(GreenCError::Win32 {
                code: set_err.0,
                message: "RegSetValueExW failed for autostart".to_string(),
            });
        }
        // Clear any Task Manager "StartupApproved" disabled override so Windows honors the Run entry.
        let approved_subkey_w = to_wide_null(STARTUP_APPROVED_SUBKEY);
        let mut approved_raw = HKEY::default();
        // SAFETY: Best-effort open & delete of per-user StartupApproved override; missing key/value is ignored.
        let approved_open = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(approved_subkey_w.as_ptr()),
                0,
                KEY_SET_VALUE,
                &mut approved_raw,
            )
        };
        if approved_open.is_ok() {
            if let Some(approved_key) = OwnedRegKey::new(approved_raw) {
                unsafe {
                    let _ = RegDeleteValueW(approved_key.raw(), PCWSTR(value_name_w.as_ptr()));
                }
            }
        }
    } else {
        // SAFETY: Deleting a missing value returns `ERROR_FILE_NOT_FOUND` (2), which is harmless.
        let _ = unsafe { RegDeleteValueW(reg_key.raw(), PCWSTR(value_name_w.as_ptr())) };
    }

    Ok(())
}

/// Checks whether `app_name` is registered under `HKCU\...\Run`.
pub fn is_autostart_enabled_native(app_name: &str) -> Result<bool> {
    let subkey_w = to_wide_null(RUN_REGISTRY_SUBKEY);
    let mut raw_key = HKEY::default();
    // SAFETY: Read-only query against `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
    let open_err = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut raw_key,
        )
    };
    if open_err.is_err() {
        return Ok(false);
    }
    let Some(reg_key) = OwnedRegKey::new(raw_key) else {
        return Ok(false);
    };

    let value_name_w = to_wide_null(app_name);
    let mut data_len: u32 = 0;
    // SAFETY: Passing `None` buffer probes existence and byte length of the `app_name` value.
    let query_err = unsafe {
        RegQueryValueExW(
            reg_key.raw(),
            PCWSTR(value_name_w.as_ptr()),
            None,
            None,
            None,
            Some(&mut data_len),
        )
    };
    Ok(query_err.is_ok() && data_len > 2)
}

/// Production implementation of `PowerSessionApi`.
#[derive(Default, Clone, Debug)]
pub struct NativePowerSessionApi;

impl PowerSessionApi for NativePowerSessionApi {
    fn query_state(&self) -> Result<PowerSessionState> {
        query_power_session_native()
    }

    fn set_autostart(&self, app_name: &str, exe_path: &Path, enabled: bool) -> Result<()> {
        set_autostart_registry_native(app_name, exe_path, enabled)
    }

    fn is_autostart_enabled(&self, app_name: &str) -> Result<bool> {
        is_autostart_enabled_native(app_name)
    }
}

/// In-memory mock implementation of `PowerSessionApi` for deterministic unit tests.
#[derive(Default, Clone, Debug)]
pub struct MockPowerSessionApi {
    pub state: Arc<Mutex<PowerSessionState>>,
    pub autostart_entries: Arc<Mutex<std::collections::HashMap<String, String>>>,
}

impl MockPowerSessionApi {
    pub fn new(state: PowerSessionState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
            autostart_entries: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn set_state(&self, state: PowerSessionState) {
        *self.state.lock().unwrap() = state;
    }
}

impl PowerSessionApi for MockPowerSessionApi {
    fn query_state(&self) -> Result<PowerSessionState> {
        Ok(*self.state.lock().unwrap())
    }

    fn set_autostart(&self, app_name: &str, exe_path: &Path, enabled: bool) -> Result<()> {
        let mut guard = self.autostart_entries.lock().unwrap();
        if enabled {
            guard.insert(
                app_name.to_string(),
                format!("\"{}\" --minimized", exe_path.display()),
            );
        } else {
            guard.remove(app_name);
        }
        Ok(())
    }

    fn is_autostart_enabled(&self, app_name: &str) -> Result<bool> {
        Ok(self
            .autostart_entries
            .lock()
            .unwrap()
            .contains_key(app_name))
    }
}
