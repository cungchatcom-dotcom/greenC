//! Windows Shell Known Folders implementation (SHGetKnownFolderPath, SHSetKnownFolderPath, OneDrive guard).

use crate::handle::{ensure_not_protected_system_path, path_to_wide};
use crate::traits::ShellFolderApi;
use greenc_core::error::{GreenCError, Result};
use std::env;
use std::path::{Path, PathBuf};
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{SHGetKnownFolderPath, SHSetKnownFolderPath, KNOWN_FOLDER_FLAG};

pub const FOLDERID_DOWNLOADS: &str = "374DE290-123F-4565-9164-39C4925E467B";
pub const FOLDERID_DOCUMENTS: &str = "FDD39AD0-238F-46AF-ADB4-6C85480369C7";
pub const FOLDERID_PICTURES: &str = "33E28130-4E1E-4676-835A-98395C3BC3BB";
pub const FOLDERID_VIDEOS: &str = "18989B1D-99B5-455B-841C-AB7C74E4DDFC";
pub const FOLDERID_MUSIC: &str = "4BD8D571-6D19-48D3-BE97-422220080E43";
pub const FOLDERID_DESKTOP: &str = "B4BFCC3A-DB2C-424C-B029-7FE99A87C641";

pub fn parse_guid(text: &str) -> Result<GUID> {
    let clean: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if clean.len() != 32 {
        return Err(GreenCError::ConfigError(format!(
            "Invalid GUID string: {}",
            text
        )));
    }
    let d1 = u32::from_str_radix(&clean[0..8], 16)
        .map_err(|e| GreenCError::ConfigError(e.to_string()))?;
    let d2 = u16::from_str_radix(&clean[8..12], 16)
        .map_err(|e| GreenCError::ConfigError(e.to_string()))?;
    let d3 = u16::from_str_radix(&clean[12..16], 16)
        .map_err(|e| GreenCError::ConfigError(e.to_string()))?;
    let mut d4 = [0u8; 8];
    for i in 0..8 {
        d4[i] = u8::from_str_radix(&clean[16 + i * 2..18 + i * 2], 16)
            .map_err(|e| GreenCError::ConfigError(e.to_string()))?;
    }
    Ok(GUID::from_values(d1, d2, d3, d4))
}

/// Native production implementation of `ShellFolderApi` using shell32.dll.
#[derive(Default, Clone, Debug)]
pub struct NativeShellFolderApi;

impl ShellFolderApi for NativeShellFolderApi {
    fn get_known_folder(&self, folder_guid: &str) -> Result<PathBuf> {
        let guid = parse_guid(folder_guid)?;
        // SAFETY: `guid` is a valid stack reference; null token queries current user.
        let path_ptr: PWSTR = unsafe {
            SHGetKnownFolderPath(&guid, KNOWN_FOLDER_FLAG(0), HANDLE(std::ptr::null_mut()))
        }
        .map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("SHGetKnownFolderPath failed for {}: {}", folder_guid, e),
        })?;

        // SAFETY: On `S_OK`, `SHGetKnownFolderPath` allocates a NUL-terminated UTF-16 buffer via
        // `CoTaskMemAlloc`; we copy its UTF-16 code units and free `path_ptr` via `CoTaskMemFree`.
        let res_path = unsafe {
            if path_ptr.is_null() {
                PathBuf::new()
            } else {
                let mut len = 0usize;
                while *path_ptr.0.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(path_ptr.0, len);
                let p = PathBuf::from(String::from_utf16_lossy(slice));
                CoTaskMemFree(Some(path_ptr.0.cast()));
                p
            }
        };

        Ok(res_path)
    }

    fn set_known_folder(&self, folder_guid: &str, new_path: &Path) -> Result<()> {
        ensure_not_protected_system_path(new_path, "SHSetKnownFolderPath")?;
        let guid = parse_guid(folder_guid)?;
        let wide_path = path_to_wide(new_path);
        // SAFETY: `guid` is valid and `wide_path` is a NUL-terminated UTF-16 path alive across the call.
        let hr = unsafe {
            SHSetKnownFolderPath(
                &guid,
                0,
                HANDLE(std::ptr::null_mut()),
                PCWSTR(wide_path.as_ptr()),
            )
        };

        hr.map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!(
                "SHSetKnownFolderPath failed to redirect {} to {}: {}",
                folder_guid,
                new_path.display(),
                e
            ),
        })?;

        Ok(())
    }

    fn is_onedrive_path(&self, path: &Path) -> bool {
        let norm_path = path.to_string_lossy().to_lowercase();
        for key in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
            if let Ok(val) = env::var(key) {
                let norm_val = val.to_lowercase().trim_end_matches(['\\', '/']).to_string();
                if norm_path == norm_val || norm_path.starts_with(&(norm_val + "\\")) {
                    return true;
                }
            }
        }
        false
    }
}
