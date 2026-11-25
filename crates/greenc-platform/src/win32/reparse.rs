//! Win32 Reparse Point Junction implementations (FSCTL_SET_REPARSE_POINT, FSCTL_GET_REPARSE_POINT).

use crate::handle::{ensure_not_protected_system_path, path_to_wide, OwnedHandle};
use crate::traits::ReparsePointApi;
use greenc_core::error::{GreenCError, Result};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileAttributesW, RemoveDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
pub use windows::Win32::System::Ioctl::{
    FSCTL_DELETE_REPARSE_POINT, FSCTL_GET_REPARSE_POINT, FSCTL_SET_REPARSE_POINT,
};
use windows::Win32::System::IO::DeviceIoControl;

pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
pub const MAXIMUM_REPARSE_DATA_BUFFER_SIZE: usize = 16384;

/// Native production implementation of `ReparsePointApi` via Win32 `DeviceIoControl`.
#[derive(Default, Clone, Debug)]
pub struct NativeReparsePointApi;

impl ReparsePointApi for NativeReparsePointApi {
    fn is_reparse_point(&self, path: &Path) -> bool {
        let wide = path_to_wide(path);
        // SAFETY: `wide` is a live NUL-terminated UTF-16 path buffer; `GetFileAttributesW` is read-only.
        let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
        if attrs == u32::MAX {
            false
        } else {
            (attrs & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0
        }
    }

    fn create_junction(&self, link: &Path, target: &Path) -> Result<()> {
        ensure_not_protected_system_path(link, "create_junction(link)")?;
        ensure_not_protected_system_path(target, "create_junction(target)")?;
        #[cfg(feature = "store")]
        if greenc_core::config::is_windows_appdata_app_dir(link) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "create_junction(link)".to_string(),
                reason: format!(
                    "Bản Microsoft Store không tạo NTFS Junction trong AppData ('{}') do cơ chế ảo hoá AppData của MSIX.",
                    link.display()
                ),
            });
        }

        if !target.exists() {
            return Err(GreenCError::NotFound {
                entity: "JunctionTarget".to_string(),
                id: target.display().to_string(),
            });
        }

        if !link.exists() {
            std::fs::create_dir_all(link)?;
        }

        let link_wide = path_to_wide(link);
        // SAFETY: `link_wide` is NUL-terminated UTF-16; `FILE_FLAG_OPEN_REPARSE_POINT` opens the
        // directory entry itself, and `OwnedHandle` closes the handle on every exit path.
        let raw_handle = unsafe {
            CreateFileW(
                PCWSTR(link_wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("Failed to open junction directory for writing: {}", e),
        })?;
        let handle = OwnedHandle::new(raw_handle).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid junction directory handle".to_string(),
        })?;

        let canonical_target = target
            .canonicalize()
            .unwrap_or_else(|_| target.to_path_buf());
        let target_str = canonical_target.to_string_lossy();
        let target_clean = target_str.strip_prefix(r"\\?\").unwrap_or(&target_str);

        // NT Object Manager path syntax required for SubstituteName (\??\D:\Path)
        let sub_name = format!(r"\??\{}", target_clean);
        let print_name = target_clean.to_string();

        let sub_wide: Vec<u16> = sub_name.encode_utf16().collect();
        let print_wide: Vec<u16> = print_name.encode_utf16().collect();

        let sub_bytes = (sub_wide.len() * 2) as u16;
        let print_bytes = (print_wide.len() * 2) as u16;

        let path_buffer_len_bytes = (sub_wide.len() + 1 + print_wide.len() + 1) * 2;
        let reparse_data_len = 8 + path_buffer_len_bytes as u16;
        let total_size = 8 + reparse_data_len as usize;

        let mut buffer = vec![0u8; total_size];

        // Header: ReparseTag, ReparseDataLength, Reserved
        buffer[0..4].copy_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buffer[4..6].copy_from_slice(&reparse_data_len.to_le_bytes());
        buffer[6..8].copy_from_slice(&0u16.to_le_bytes());

        // MountPointReparseBuffer: SubNameOffset, SubNameLen, PrintNameOffset, PrintNameLen
        let sub_offset = 0u16;
        let print_offset = sub_bytes + 2;

        buffer[8..10].copy_from_slice(&sub_offset.to_le_bytes());
        buffer[10..12].copy_from_slice(&sub_bytes.to_le_bytes());
        buffer[12..14].copy_from_slice(&print_offset.to_le_bytes());
        buffer[14..16].copy_from_slice(&print_bytes.to_le_bytes());

        let mut path_offset = 16;
        for &ch in &sub_wide {
            buffer[path_offset..path_offset + 2].copy_from_slice(&ch.to_le_bytes());
            path_offset += 2;
        }
        buffer[path_offset..path_offset + 2].copy_from_slice(&0u16.to_le_bytes());
        path_offset += 2;

        for &ch in &print_wide {
            buffer[path_offset..path_offset + 2].copy_from_slice(&ch.to_le_bytes());
            path_offset += 2;
        }
        buffer[path_offset..path_offset + 2].copy_from_slice(&0u16.to_le_bytes());

        let mut bytes_returned = 0u32;
        // SAFETY: `handle.raw()` is open with `GENERIC_WRITE`, and `buffer` contains a well-formed
        // `REPARSE_DATA_BUFFER` (`IO_REPARSE_TAG_MOUNT_POINT`) of exact length `buffer.len()`.
        let ok = unsafe {
            DeviceIoControl(
                handle.raw(),
                FSCTL_SET_REPARSE_POINT,
                Some(buffer.as_ptr().cast::<c_void>()),
                buffer.len() as u32,
                None,
                0,
                Some(&mut bytes_returned),
                None,
            )
        };

        ok.map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("DeviceIoControl(FSCTL_SET_REPARSE_POINT) failed: {}", e),
        })?;

        Ok(())
    }

    fn get_junction_target(&self, link: &Path) -> Result<PathBuf> {
        let link_wide = path_to_wide(link);
        // SAFETY: Opens the reparse point tag itself (`FILE_FLAG_OPEN_REPARSE_POINT`) read-only;
        // `OwnedHandle` closes the handle deterministically on return.
        let raw_handle = unsafe {
            CreateFileW(
                PCWSTR(link_wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("Failed to open reparse point for reading: {}", e),
        })?;
        let handle = OwnedHandle::new(raw_handle).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid reparse point handle".to_string(),
        })?;

        let mut buffer = vec![0u8; MAXIMUM_REPARSE_DATA_BUFFER_SIZE];
        let mut bytes_returned = 0u32;

        // SAFETY: `buffer` has `MAXIMUM_REPARSE_DATA_BUFFER_SIZE` (`16 KiB`) writable bytes.
        let ok = unsafe {
            DeviceIoControl(
                handle.raw(),
                FSCTL_GET_REPARSE_POINT,
                None,
                0,
                Some(buffer.as_mut_ptr().cast::<c_void>()),
                buffer.len() as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        ok.map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("DeviceIoControl(FSCTL_GET_REPARSE_POINT) failed: {}", e),
        })?;

        if (bytes_returned as usize) < 16 || buffer.len() < 16 {
            return Err(GreenCError::Relocation(
                "Reparse data buffer returned is too short".to_string(),
            ));
        }

        let tag = u32::from_le_bytes(buffer[0..4].try_into().unwrap());
        if tag != IO_REPARSE_TAG_MOUNT_POINT {
            return Err(GreenCError::Relocation(format!(
                "Reparse tag is not a Mount Point: 0x{:08X}",
                tag
            )));
        }

        let print_offset = u16::from_le_bytes(buffer[12..14].try_into().unwrap()) as usize;
        let print_len = u16::from_le_bytes(buffer[14..16].try_into().unwrap()) as usize;

        if print_len >= 2 && 16 + print_offset + print_len <= buffer.len() {
            let start = 16 + print_offset;
            let end = start + print_len;
            let u16_vec: Vec<u16> = buffer[start..end]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            return Ok(PathBuf::from(String::from_utf16_lossy(&u16_vec)));
        }

        let sub_offset = u16::from_le_bytes(buffer[8..10].try_into().unwrap()) as usize;
        let sub_len = u16::from_le_bytes(buffer[10..12].try_into().unwrap()) as usize;
        if sub_len >= 2 && 16 + sub_offset + sub_len <= buffer.len() {
            let start = 16 + sub_offset;
            let end = start + sub_len;
            let u16_vec: Vec<u16> = buffer[start..end]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let mut raw_target = String::from_utf16_lossy(&u16_vec);
            if let Some(stripped) = raw_target.strip_prefix(r"\??\") {
                raw_target = stripped.to_string();
            }
            return Ok(PathBuf::from(raw_target));
        }

        Err(GreenCError::Relocation(
            "Could not parse valid target from reparse buffer".to_string(),
        ))
    }

    fn delete_junction(&self, link: &Path) -> Result<()> {
        ensure_not_protected_system_path(link, "delete_junction")?;
        let link_wide = path_to_wide(link);
        // SAFETY: `RemoveDirectoryW` on an NTFS mount-point reparse directory removes ONLY the
        // junction link itself and never traverses or deletes files in the target directory.
        let ok = unsafe { RemoveDirectoryW(PCWSTR(link_wide.as_ptr())) };
        if let Err(err) = ok {
            return Err(GreenCError::Win32 {
                code: err.code().0 as u32,
                message: format!("RemoveDirectoryW on junction failed: {}", err),
            });
        }
        Ok(())
    }
}
