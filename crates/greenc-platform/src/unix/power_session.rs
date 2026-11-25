//! Unix (macOS & Linux) battery/power telemetry and login autostart management.

#[cfg(target_os = "macos")]
use crate::unix_parsers::parse_pmset_battery;
use greenc_core::error::{GreenCError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Detailed OS-level login autostart state (matches the Windows/MSIX StartupTask surface).
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

/// Queries live AC/battery status on macOS (`pmset -g batt`) or Linux (`/sys/class/power_supply`).
pub fn query_power_session_native() -> Result<PowerSessionState> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("pmset")
            .args(["-g", "batt"])
            .output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let (on_battery, battery_saver_on, battery_percent) = parse_pmset_battery(&stdout);
            return Ok(PowerSessionState {
                on_battery,
                battery_saver_on,
                battery_percent,
                is_fullscreen_or_busy: false,
            });
        }
        Ok(PowerSessionState::default())
    }

    #[cfg(target_os = "linux")]
    {
        let mut on_battery = false;
        let mut battery_percent: u8 = 100;

        if let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") {
            for entry in entries.flatten() {
                let path = entry.path();
                let supply_type = std::fs::read_to_string(path.join("type"))
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                if supply_type == "battery" {
                    let status = std::fs::read_to_string(path.join("status"))
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_lowercase();
                    if status == "discharging" {
                        on_battery = true;
                    }
                    if let Ok(cap_str) = std::fs::read_to_string(path.join("capacity")) {
                        if let Ok(cap) = cap_str.trim().parse::<u8>() {
                            battery_percent = cap.min(100);
                        }
                    }
                }
            }
        }

        let battery_saver_on = on_battery && battery_percent <= 20;
        Ok(PowerSessionState {
            on_battery,
            battery_saver_on,
            battery_percent,
            is_fullscreen_or_busy: false,
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(PowerSessionState::default())
    }
}

fn autostart_file_path(app_name: &str) -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| GreenCError::ConfigError("HOME environment variable not set".into()))?;
    let safe_name = app_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .to_ascii_lowercase();

    #[cfg(target_os = "macos")]
    {
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("com.{safe_name}.app.plist")))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        Ok(config_dir
            .join("autostart")
            .join(format!("{safe_name}.desktop")))
    }
}

/// Enables or disables per-user login autostart via LaunchAgent (macOS) or XDG autostart (Linux).
pub fn set_autostart_registry_native(app_name: &str, exe_path: &Path, enabled: bool) -> Result<()> {
    let target = autostart_file_path(app_name)?;

    if !enabled {
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(target_os = "macos")]
    {
        let exe_str = exe_path.to_string_lossy();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.{label}.app</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--minimized</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
            label = app_name.to_ascii_lowercase(),
            exe = exe_str,
        );
        std::fs::write(&target, plist)?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let exe_str = exe_path.to_string_lossy();
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={app_name}\nExec=\"{exe_str}\" --minimized\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(&target, desktop)?;
    }

    Ok(())
}

/// Returns true if the per-user login autostart entry exists.
pub fn is_autostart_enabled_native(app_name: &str) -> Result<bool> {
    let target = autostart_file_path(app_name)?;
    Ok(target.exists())
}

/// Native Unix implementation of `PowerSessionApi`.
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

/// Deterministic mock implementation of `PowerSessionApi` for unit tests.
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
        let mut map = self.autostart_entries.lock().unwrap();
        if enabled {
            map.insert(
                app_name.to_string(),
                format!("\"{}\" --minimized", exe_path.display()),
            );
        } else {
            map.remove(app_name);
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
