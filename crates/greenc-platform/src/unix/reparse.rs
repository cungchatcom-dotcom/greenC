//! Unix symlink-based implementation of `ReparsePointApi` for directory relocation.

use crate::traits::ReparsePointApi;
use crate::unix::handle::ensure_not_protected_system_path;
use greenc_core::error::{GreenCError, Result};
use std::path::{Path, PathBuf};

pub const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
pub const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
pub const FSCTL_DELETE_REPARSE_POINT: u32 = 0x0009_00AC;
pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
pub const MAXIMUM_REPARSE_DATA_BUFFER_SIZE: usize = 16384;

/// Native Unix implementation of `ReparsePointApi` backed by POSIX symbolic links.
#[derive(Default, Clone, Debug)]
pub struct NativeReparsePointApi;

impl ReparsePointApi for NativeReparsePointApi {
    fn is_reparse_point(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    fn create_junction(&self, link: &Path, target: &Path) -> Result<()> {
        ensure_not_protected_system_path(link, "create_symlink(link)")?;
        ensure_not_protected_system_path(target, "create_symlink(target)")?;

        if !target.exists() {
            return Err(GreenCError::NotFound {
                entity: "SymlinkTarget".to_string(),
                id: target.display().to_string(),
            });
        }

        if let Some(parent) = link.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        // Remove an existing empty directory or stale symlink at `link` if present.
        if let Ok(meta) = std::fs::symlink_metadata(link) {
            if meta.file_type().is_symlink() {
                std::fs::remove_file(link)?;
            } else if meta.is_dir() {
                std::fs::remove_dir(link)?;
            }
        }

        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }

    fn get_junction_target(&self, link: &Path) -> Result<PathBuf> {
        let meta = std::fs::symlink_metadata(link).map_err(|_| GreenCError::NotFound {
            entity: "Symlink".to_string(),
            id: link.display().to_string(),
        })?;
        if !meta.file_type().is_symlink() {
            return Err(GreenCError::NotFound {
                entity: "Symlink".to_string(),
                id: link.display().to_string(),
            });
        }
        Ok(std::fs::read_link(link)?)
    }

    fn delete_junction(&self, link: &Path) -> Result<()> {
        ensure_not_protected_system_path(link, "delete_symlink")?;
        let meta = match std::fs::symlink_metadata(link) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        if !meta.file_type().is_symlink() {
            return Err(GreenCError::PreconditionBlocked {
                operation: "delete_symlink".to_string(),
                reason: format!("Path '{}' is not a symbolic link", link.display()),
            });
        }

        // Strictly remove the symlink entry itself, never the target directory's contents.
        std::fs::remove_file(link)?;
        Ok(())
    }
}
