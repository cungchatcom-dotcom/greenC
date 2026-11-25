use crate::db::GreenCDb;
use greenc_core::error::{GreenCError, Result};
use greenc_i18n::catalog::is_valid_language_code;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const KEY_DATA_ROOT: &str = "data_root";
pub const KEY_RETENTION: &str = "retention_days_default";
pub const KEY_LANGUAGE: &str = "language";
pub const KEY_THEME_MODE: &str = "theme_mode";
pub const KEY_AUTOSTART: &str = "autostart_enabled";
pub const KEY_LOW_DISK_SHIELD_GB: &str = "low_disk_shield_gb";
pub const KEY_PAUSE_BATTERY_FS: &str = "pause_on_battery_fullscreen";
pub const KEY_ZALO_AUTO_CLEAN: &str = "zalo_auto_clean_enabled";

pub const MIN_RETENTION_DAYS: u32 = 1;
pub const MAX_RETENTION_DAYS: u32 = 90;
pub const DEFAULT_RETENTION_DAYS: u32 = 7;

pub const MIN_LOW_DISK_SHIELD_GB: u32 = 2;
pub const MAX_LOW_DISK_SHIELD_GB: u32 = 100;
pub const DEFAULT_LOW_DISK_SHIELD_GB: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

pub struct AppSettings<'a> {
    db: &'a GreenCDb,
}

impl std::fmt::Debug for AppSettings<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSettings").finish()
    }
}

impl<'a> AppSettings<'a> {
    pub fn new(db: &'a GreenCDb) -> Self {
        Self { db }
    }

    pub fn get_data_root(&self) -> Result<Option<PathBuf>> {
        let val = self.db.get_setting(KEY_DATA_ROOT)?;
        Ok(val.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            }
        }))
    }

    pub fn set_data_root<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let s = path.as_ref().to_string_lossy().to_string();
        self.db.set_setting(KEY_DATA_ROOT, &s)
    }

    pub fn get_retention_days(&self) -> Result<u32> {
        match self.db.get_setting(KEY_RETENTION)? {
            Some(s) => match s.trim().parse::<u32>() {
                Ok(days) => Ok(Self::clamp_retention(days)),
                Err(_) => Ok(DEFAULT_RETENTION_DAYS),
            },
            None => Ok(DEFAULT_RETENTION_DAYS),
        }
    }

    pub fn set_retention_days(&self, days: u32) -> Result<u32> {
        let clamped = Self::clamp_retention(days);
        self.db.set_setting(KEY_RETENTION, &clamped.to_string())?;
        Ok(clamped)
    }

    pub const fn clamp_retention(days: u32) -> u32 {
        if days < MIN_RETENTION_DAYS {
            MIN_RETENTION_DAYS
        } else if days > MAX_RETENTION_DAYS {
            MAX_RETENTION_DAYS
        } else {
            days
        }
    }

    pub fn get_language(&self) -> Result<Option<String>> {
        match self.db.get_setting(KEY_LANGUAGE)? {
            Some(s) => {
                let code = s.trim().to_string();
                if is_valid_language_code(&code) {
                    Ok(Some(code))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    pub fn set_language(&self, code: &str) -> Result<()> {
        let trimmed = code.trim();
        if !is_valid_language_code(trimmed) {
            return Err(GreenCError::I18n(format!(
                "Invalid language code: {}",
                trimmed
            )));
        }
        self.db.set_setting(KEY_LANGUAGE, trimmed)
    }

    pub fn get_theme_mode(&self) -> Result<ThemeMode> {
        match self.db.get_setting(KEY_THEME_MODE)? {
            Some(s) => Ok(ThemeMode::from_str_loose(&s)),
            None => Ok(ThemeMode::System),
        }
    }

    pub fn set_theme_mode(&self, mode: ThemeMode) -> Result<()> {
        self.db.set_setting(KEY_THEME_MODE, mode.as_str())
    }

    pub fn get_autostart_enabled(&self) -> Result<bool> {
        Ok(self.db.get_setting(KEY_AUTOSTART)?.as_deref() == Some("1"))
    }

    pub fn set_autostart_enabled(&self, enabled: bool) -> Result<()> {
        self.db
            .set_setting(KEY_AUTOSTART, if enabled { "1" } else { "0" })
    }

    pub fn get_low_disk_shield_gb(&self) -> Result<u32> {
        match self.db.get_setting(KEY_LOW_DISK_SHIELD_GB)? {
            Some(s) => match s.trim().parse::<u32>() {
                Ok(gb) => Ok(gb.clamp(MIN_LOW_DISK_SHIELD_GB, MAX_LOW_DISK_SHIELD_GB)),
                Err(_) => Ok(DEFAULT_LOW_DISK_SHIELD_GB),
            },
            None => Ok(DEFAULT_LOW_DISK_SHIELD_GB),
        }
    }

    pub fn set_low_disk_shield_gb(&self, gb: u32) -> Result<u32> {
        let clamped = gb.clamp(MIN_LOW_DISK_SHIELD_GB, MAX_LOW_DISK_SHIELD_GB);
        self.db
            .set_setting(KEY_LOW_DISK_SHIELD_GB, &clamped.to_string())?;
        Ok(clamped)
    }

    pub fn get_pause_on_battery_fullscreen(&self) -> Result<bool> {
        Ok(self.db.get_setting(KEY_PAUSE_BATTERY_FS)?.as_deref() != Some("0"))
    }

    pub fn set_pause_on_battery_fullscreen(&self, pause: bool) -> Result<()> {
        self.db
            .set_setting(KEY_PAUSE_BATTERY_FS, if pause { "1" } else { "0" })
    }

    /// Scheduled automatic cleanup of Zalo caches & old versions during drive-quota guard runs.
    /// Strictly OFF (`false`) by default.
    pub fn get_zalo_auto_clean_enabled(&self) -> Result<bool> {
        Ok(self.db.get_setting(KEY_ZALO_AUTO_CLEAN)?.as_deref() == Some("1"))
    }

    pub fn set_zalo_auto_clean_enabled(&self, enabled: bool) -> Result<()> {
        self.db
            .set_setting(KEY_ZALO_AUTO_CLEAN, if enabled { "1" } else { "0" })
    }

    pub fn vault_dir(&self, default_dir: &Path) -> Result<PathBuf> {
        Ok(match self.get_data_root()? {
            Some(root) => root.join("Vault"),
            None => default_dir.to_path_buf(),
        })
    }

    pub fn apps_dir(&self) -> Result<Option<PathBuf>> {
        Ok(self.get_data_root()?.map(|root| root.join("Apps")))
    }

    pub fn needs_first_run(&self) -> Result<bool> {
        Ok(self.get_data_root()?.is_none())
    }
}
