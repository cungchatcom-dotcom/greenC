//! Unix (macOS & Linux XDG) user folder resolution for `ShellFolderApi`.

use crate::traits::ShellFolderApi;
#[cfg(target_os = "linux")]
use crate::unix_parsers::parse_xdg_user_dirs;
use greenc_core::error::{GreenCError, Result};
use std::path::{Path, PathBuf};

pub const FOLDERID_DOWNLOADS: &str = "374DE290-123F-4565-9164-39C4925E467B";
pub const FOLDERID_DOCUMENTS: &str = "FDD39AD0-238F-46AF-ADB4-6C85480369C7";
pub const FOLDERID_PICTURES: &str = "33E28130-4E1E-4676-835A-98395C3BC3BB";
pub const FOLDERID_VIDEOS: &str = "18989B1D-99B5-455B-841C-AB7C74E4DDFC";
pub const FOLDERID_MUSIC: &str = "4BD8D571-6D19-48D3-BE97-422220080E43";
pub const FOLDERID_DESKTOP: &str = "B4BFCC3A-DB2C-424C-B029-7FE99A87C641";

/// Native Unix implementation of `ShellFolderApi`.
#[derive(Default, Clone, Debug)]
pub struct NativeShellFolderApi;

impl ShellFolderApi for NativeShellFolderApi {
    fn get_known_folder(&self, folder_guid: &str) -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .map_err(|_| GreenCError::ConfigError("HOME environment variable not set".into()))?;

        // On Linux, consult ~/.config/user-dirs.dirs first when present.
        #[cfg(target_os = "linux")]
        {
            let xdg_key = match folder_guid {
                FOLDERID_DOWNLOADS => Some("XDG_DOWNLOAD_DIR"),
                FOLDERID_DOCUMENTS => Some("XDG_DOCUMENTS_DIR"),
                FOLDERID_PICTURES => Some("XDG_PICTURES_DIR"),
                FOLDERID_VIDEOS => Some("XDG_VIDEOS_DIR"),
                FOLDERID_MUSIC => Some("XDG_MUSIC_DIR"),
                FOLDERID_DESKTOP => Some("XDG_DESKTOP_DIR"),
                _ => None,
            };
            if let Some(key) = xdg_key {
                let config_dir = std::env::var("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| home.join(".config"));
                if let Ok(content) = std::fs::read_to_string(config_dir.join("user-dirs.dirs")) {
                    let parsed = parse_xdg_user_dirs(&content, &home);
                    if let Some(dir) = parsed.get(key) {
                        return Ok(dir.clone());
                    }
                }
            }
        }

        let subdir = match folder_guid {
            FOLDERID_DOWNLOADS => "Downloads",
            FOLDERID_DOCUMENTS => "Documents",
            FOLDERID_PICTURES => "Pictures",
            FOLDERID_VIDEOS => {
                if cfg!(target_os = "macos") {
                    "Movies"
                } else {
                    "Videos"
                }
            }
            FOLDERID_MUSIC => "Music",
            FOLDERID_DESKTOP => "Desktop",
            _ => {
                return Err(GreenCError::NotFound {
                    entity: "KnownFolder".to_string(),
                    id: folder_guid.to_string(),
                });
            }
        };

        Ok(home.join(subdir))
    }

    fn set_known_folder(&self, _folder_guid: &str, _new_path: &Path) -> Result<()> {
        Err(GreenCError::PreconditionBlocked {
            operation: "set_known_folder".to_string(),
            reason: "Relocating OS user shell folders via registry is Windows-only; use symlink relocation on macOS/Linux".to_string(),
        })
    }

    fn is_onedrive_path(&self, path: &Path) -> bool {
        let lower = path.to_string_lossy().to_ascii_lowercase();
        lower.contains("/onedrive")
            || lower.contains("/cloudstorage/onedrive")
            || lower.contains("/mobile documents/com~apple~clouddocs")
    }
}
