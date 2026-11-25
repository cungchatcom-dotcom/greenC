//! Battery-saver, fullscreen presentation/gaming detection, and per-user login autostart:
//! - Standard builds: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` with RAII `OwnedRegKey`.
//! - Microsoft Store (`feature = "store"`) builds: WinRT `Windows.ApplicationModel.StartupTask`
//!   (`desktop:StartupTask`), never writing `HKCU\...\Run`.

#[cfg(feature = "store")]
use greenc_core::config::STORE_STARTUP_TASK_ID;
use greenc_core::error::{GreenCError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(feature = "store")]
use windows::core::HSTRING;
#[cfg(not(feature = "store"))]
use windows::core::PCWSTR;
#[cfg(feature = "store")]
use windows::ApplicationModel::{StartupTask, StartupTaskState};
#[cfg(feature = "store")]
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::{RegCloseKey, HKEY};
#[cfg(not(feature = "store"))]
use windows::Win32::System::Registry::{
    RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};

#[cfg(not(feature = "store"))]
const RUN_REGISTRY_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(not(feature = "store"))]
const STARTUP_APPROVED_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// Detailed OS-level login autostart state (distinguishing user/policy blocks for MSIX StartupTask).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutostartState {
    Enabled,
    Disabled,
    DisabledByUser,
    DisabledByPolicy,
    EnabledByPolicy,
    Unavailable,
}

impl AutostartState {
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled | Self::EnabledByPolicy)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::DisabledByUser => "disabled_by_user",
            Self::DisabledByPolicy => "disabled_by_policy",
            Self::EnabledByPolicy => "enabled_by_policy",
            Self::Unavailable => "unavailable",
        }
    }
}

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
    fn autostart_state(&self, app_name: &str) -> Result<AutostartState> {
        Ok(if self.is_autostart_enabled(app_name)? {
            AutostartState::Enabled
        } else {
            AutostartState::Disabled
        })
    }
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

#[cfg(not(feature = "store"))]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[cfg(feature = "store")]
fn map_winrt_startup_state(state: StartupTaskState) -> AutostartState {
    match state {
        StartupTaskState::Enabled => AutostartState::Enabled,
        StartupTaskState::Disabled => AutostartState::Disabled,
        StartupTaskState::DisabledByUser => AutostartState::DisabledByUser,
        StartupTaskState::DisabledByPolicy => AutostartState::DisabledByPolicy,
        StartupTaskState::EnabledByPolicy => AutostartState::EnabledByPolicy,
        _ => AutostartState::Unavailable,
    }
}

/// RAII guard ensuring COM/WinRT is initialized on the calling thread (e.g. Tokio MTA workers)
/// before invoking `Windows.ApplicationModel.StartupTask`.
#[cfg(feature = "store")]
struct ComMtaGuard {
    should_uninit: bool,
}

#[cfg(feature = "store")]
impl ComMtaGuard {
    fn enter() -> Self {
        // SAFETY: `CoInitializeEx` is thread-local; if it returns `S_OK` or `S_FALSE` (`hr.is_ok()`),
        // we pair it with a matching `CoUninitialize` on drop. If `RPC_E_CHANGED_MODE`, the thread
        // already has an initialized apartment (e.g. STA) and must not be uninitialized by us.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self {
            should_uninit: hr.is_ok(),
        }
    }
}

#[cfg(feature = "store")]
impl Drop for ComMtaGuard {
    fn drop(&mut self) {
        if self.should_uninit {
            // SAFETY: Matches the successful `CoInitializeEx` call on this thread.
            unsafe {
                CoUninitialize();
            }
        }
    }
}

/// Queries MSIX `desktop:StartupTask` state via WinRT `Windows.ApplicationModel.StartupTask`.
#[cfg(feature = "store")]
pub fn query_startup_task_state_store(task_id: &str) -> Result<AutostartState> {
    let _com = ComMtaGuard::enter();
    let task_id_h = HSTRING::from(task_id);
    let op = StartupTask::GetAsync(&task_id_h).map_err(|e| GreenCError::Win32 {
        code: (e.code().0 as u32) & 0xFFFF,
        message: format!("StartupTask::GetAsync('{task_id}') failed: {e}"),
    })?;
    let task = op.get().map_err(|e| GreenCError::Win32 {
        code: (e.code().0 as u32) & 0xFFFF,
        message: format!("StartupTask::GetAsync('{task_id}') wait failed: {e}"),
    })?;
    let state = task.State().map_err(|e| GreenCError::Win32 {
        code: (e.code().0 as u32) & 0xFFFF,
        message: format!("StartupTask::State('{task_id}') failed: {e}"),
    })?;
    Ok(map_winrt_startup_state(state))
}

/// Enables or disables MSIX `desktop:StartupTask` via WinRT `Windows.ApplicationModel.StartupTask`.
/// Accurately surfaces `DisabledByUser`, `DisabledByPolicy`, and `EnabledByPolicy` blocks.
#[cfg(feature = "store")]
pub fn set_startup_task_store(task_id: &str, enabled: bool) -> Result<AutostartState> {
    let _com = ComMtaGuard::enter();
    let task_id_h = HSTRING::from(task_id);
    let task = StartupTask::GetAsync(&task_id_h)
        .map_err(|e| GreenCError::Win32 {
            code: (e.code().0 as u32) & 0xFFFF,
            message: format!("StartupTask::GetAsync('{task_id}') failed: {e}"),
        })?
        .get()
        .map_err(|e| GreenCError::Win32 {
            code: (e.code().0 as u32) & 0xFFFF,
            message: format!("StartupTask::GetAsync('{task_id}') wait failed: {e}"),
        })?;

    let current = map_winrt_startup_state(task.State().map_err(|e| GreenCError::Win32 {
        code: (e.code().0 as u32) & 0xFFFF,
        message: format!("StartupTask::State('{task_id}') failed: {e}"),
    })?);

    if enabled {
        match current {
            AutostartState::Enabled | AutostartState::EnabledByPolicy => Ok(current),
            AutostartState::DisabledByUser => Err(GreenCError::PreconditionBlocked {
                operation: "StartupTask".to_string(),
                reason: "Tự khởi động đã bị người dùng tắt trong Windows Settings / Task Manager (DisabledByUser). Hãy bật lại tại Settings → Apps → Startup.".to_string(),
            }),
            AutostartState::DisabledByPolicy => Err(GreenCError::PreconditionBlocked {
                operation: "StartupTask".to_string(),
                reason: "Tự khởi động bị chặn bởi chính sách quản trị hệ thống (DisabledByPolicy).".to_string(),
            }),
            AutostartState::Disabled | AutostartState::Unavailable => {
                let new_state = map_winrt_startup_state(
                    task.RequestEnableAsync()
                        .map_err(|e| GreenCError::Win32 {
                            code: (e.code().0 as u32) & 0xFFFF,
                            message: format!("StartupTask::RequestEnableAsync failed: {e}"),
                        })?
                        .get()
                        .map_err(|e| GreenCError::Win32 {
                            code: (e.code().0 as u32) & 0xFFFF,
                            message: format!("StartupTask::RequestEnableAsync wait failed: {e}"),
                        })?,
                );
                match new_state {
                    AutostartState::Enabled | AutostartState::EnabledByPolicy => Ok(new_state),
                    AutostartState::DisabledByUser => Err(GreenCError::PreconditionBlocked {
                        operation: "StartupTask".to_string(),
                        reason: "Tự khởi động đã bị người dùng tắt trong Windows Settings / Task Manager (DisabledByUser).".to_string(),
                    }),
                    AutostartState::DisabledByPolicy => Err(GreenCError::PreconditionBlocked {
                        operation: "StartupTask".to_string(),
                        reason: "Tự khởi động bị chặn bởi chính sách quản trị hệ thống (DisabledByPolicy).".to_string(),
                    }),
                    _ => Err(GreenCError::PreconditionBlocked {
                        operation: "StartupTask".to_string(),
                        reason: format!(
                            "Windows không cho phép bật StartupTask (trạng thái: {}).",
                            new_state.as_str()
                        ),
                    }),
                }
            }
        }
    } else {
        match current {
            AutostartState::EnabledByPolicy => Err(GreenCError::PreconditionBlocked {
                operation: "StartupTask".to_string(),
                reason: "Tự khởi động đang bị bắt buộc bật bởi chính sách quản trị hệ thống (EnabledByPolicy).".to_string(),
            }),
            AutostartState::Disabled
            | AutostartState::DisabledByUser
            | AutostartState::DisabledByPolicy => Ok(current),
            AutostartState::Enabled | AutostartState::Unavailable => {
                task.Disable().map_err(|e| GreenCError::Win32 {
                    code: (e.code().0 as u32) & 0xFFFF,
                    message: format!("StartupTask::Disable failed: {e}"),
                })?;
                Ok(AutostartState::Disabled)
            }
        }
    }
}

/// Enables or disables per-user Windows login autostart (`--minimized`) without requiring Admin.
/// In `store` (MSIX) builds, registry Run keys are never touched; callers must use `StartupTask`.
pub fn set_autostart_registry_native(app_name: &str, exe_path: &Path, enabled: bool) -> Result<()> {
    #[cfg(feature = "store")]
    {
        let _ = (app_name, exe_path, enabled);
        return Err(GreenCError::PreconditionBlocked {
            operation: "set_autostart_registry_native".to_string(),
            reason:
                "Bản Microsoft Store dùng desktop:StartupTask và không ghi khoá HKCU\\...\\Run."
                    .to_string(),
        });
    }

    #[cfg(not(feature = "store"))]
    {
        let subkey_w = to_wide_null(RUN_REGISTRY_SUBKEY);
        let mut raw_key = HKEY::default();
        // SAFETY: Opens or creates `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` for the
        // current user only. The key can be missing on fresh profiles.
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
}

/// Checks whether `app_name` is registered for login autostart.
pub fn is_autostart_enabled_native(app_name: &str) -> Result<bool> {
    #[cfg(feature = "store")]
    {
        let _ = app_name;
        Ok(query_startup_task_state_store(STORE_STARTUP_TASK_ID)?.is_enabled())
    }

    #[cfg(not(feature = "store"))]
    {
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
}

/// Production implementation of `PowerSessionApi`.
#[derive(Default, Clone, Debug)]
pub struct NativePowerSessionApi;

impl PowerSessionApi for NativePowerSessionApi {
    fn query_state(&self) -> Result<PowerSessionState> {
        query_power_session_native()
    }

    fn set_autostart(&self, app_name: &str, exe_path: &Path, enabled: bool) -> Result<()> {
        #[cfg(feature = "store")]
        {
            let _ = (app_name, exe_path);
            let _ = set_startup_task_store(STORE_STARTUP_TASK_ID, enabled)?;
            Ok(())
        }
        #[cfg(not(feature = "store"))]
        {
            set_autostart_registry_native(app_name, exe_path, enabled)
        }
    }

    fn is_autostart_enabled(&self, app_name: &str) -> Result<bool> {
        is_autostart_enabled_native(app_name)
    }

    fn autostart_state(&self, app_name: &str) -> Result<AutostartState> {
        #[cfg(feature = "store")]
        {
            let _ = app_name;
            query_startup_task_state_store(STORE_STARTUP_TASK_ID)
        }
        #[cfg(not(feature = "store"))]
        {
            Ok(if is_autostart_enabled_native(app_name)? {
                AutostartState::Enabled
            } else {
                AutostartState::Disabled
            })
        }
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
