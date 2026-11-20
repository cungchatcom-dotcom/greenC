//! NTFS WOF CompactOS (XPRESS4K/8K/16K/LZX), LZNT1 fallback, and VHDX virtual disk compaction.

use crate::handle::{ensure_not_protected_system_path, path_to_wide, OwnedHandle};
use crate::traits::{CompactionApi, CompactionResult, VhdxCompactionResult, WofAlgorithm};
use greenc_core::error::{GreenCError, Result};
use std::ffi::c_void;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetCompressedFileSizeW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING,
};
use windows::Win32::Storage::Vhd::{
    CompactVirtualDisk, OpenVirtualDisk, COMPACT_VIRTUAL_DISK_FLAG_NONE,
    COMPACT_VIRTUAL_DISK_PARAMETERS, COMPACT_VIRTUAL_DISK_PARAMETERS_0,
    COMPACT_VIRTUAL_DISK_PARAMETERS_0_0, COMPACT_VIRTUAL_DISK_VERSION_1,
    OPEN_VIRTUAL_DISK_FLAG_NONE, OPEN_VIRTUAL_DISK_PARAMETERS, OPEN_VIRTUAL_DISK_PARAMETERS_0,
    OPEN_VIRTUAL_DISK_PARAMETERS_0_0, OPEN_VIRTUAL_DISK_VERSION_1, VIRTUAL_DISK_ACCESS_METAOPS,
    VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_VHDX, VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
};
use windows::Win32::System::Ioctl::{
    FSCTL_DELETE_EXTERNAL_BACKING, FSCTL_SET_COMPRESSION, FSCTL_SET_EXTERNAL_BACKING,
};
use windows::Win32::System::IO::DeviceIoControl;

pub const WOF_CURRENT_VERSION: u32 = 1;
pub const WOF_PROVIDER_FILE: u32 = 2;
pub const FILE_PROVIDER_CURRENT_VERSION: u32 = 1;

pub const FILE_PROVIDER_COMPRESSION_XPRESS4K: u32 = 0;
pub const FILE_PROVIDER_COMPRESSION_LZX: u32 = 1;
pub const FILE_PROVIDER_COMPRESSION_XPRESS8K: u32 = 2;
pub const FILE_PROVIDER_COMPRESSION_XPRESS16K: u32 = 3;

pub const COMPRESSION_FORMAT_NONE: u16 = 0;
pub const COMPRESSION_FORMAT_DEFAULT: u16 = 1;
pub const COMPRESSION_FORMAT_LZNT1: u16 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WofExternalInfo {
    pub version: u32,
    pub provider: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FileProviderExternalInfoV1 {
    pub version: u32,
    pub algorithm: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WofFileCompressionInfo {
    pub wof_info: WofExternalInfo,
    pub file_info: FileProviderExternalInfoV1,
}

/// Native production implementation of `CompactionApi`.
#[derive(Default, Clone, Debug)]
pub struct NativeCompactionApi;

impl CompactionApi for NativeCompactionApi {
    fn compact_file(&self, path: &Path, algorithm: WofAlgorithm) -> Result<CompactionResult> {
        ensure_not_protected_system_path(path, "compact_file")?;
        let wide = path_to_wide(path);
        let logical = std::fs::metadata(path)?.len();

        // SAFETY: `wide` is a live NUL-terminated UTF-16 path; `OwnedHandle` closes the opened
        // file handle deterministically on scope exit or error propagation.
        let raw_handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("Failed to open file for compaction: {}", e),
        })?;
        let handle = OwnedHandle::new(raw_handle).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid file handle for compaction".to_string(),
        })?;

        let mut bytes_returned = 0u32;
        let mut used_algo = algorithm.as_str().to_string();

        if algorithm == WofAlgorithm::Lznt1 {
            let format = COMPRESSION_FORMAT_LZNT1;
            // SAFETY: `format` is a valid 2-byte `u16` (`COMPRESSION_FORMAT_LZNT1`) passed to `FSCTL_SET_COMPRESSION`.
            let ok = unsafe {
                DeviceIoControl(
                    handle.raw(),
                    FSCTL_SET_COMPRESSION,
                    Some((&raw const format).cast::<c_void>()),
                    std::mem::size_of::<u16>() as u32,
                    None,
                    0,
                    Some(&mut bytes_returned),
                    None,
                )
            };
            ok.map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("FSCTL_SET_COMPRESSION failed: {}", e),
            })?;
        } else {
            let algo_id = match algorithm {
                WofAlgorithm::Xpress4k => FILE_PROVIDER_COMPRESSION_XPRESS4K,
                WofAlgorithm::Lzx => FILE_PROVIDER_COMPRESSION_LZX,
                WofAlgorithm::Xpress8k => FILE_PROVIDER_COMPRESSION_XPRESS8K,
                WofAlgorithm::Xpress16k => FILE_PROVIDER_COMPRESSION_XPRESS16K,
                WofAlgorithm::Lznt1 => unreachable!(),
            };

            let info = WofFileCompressionInfo {
                wof_info: WofExternalInfo {
                    version: WOF_CURRENT_VERSION,
                    provider: WOF_PROVIDER_FILE,
                },
                file_info: FileProviderExternalInfoV1 {
                    version: FILE_PROVIDER_CURRENT_VERSION,
                    algorithm: algo_id,
                    flags: 0,
                },
            };

            // SAFETY: `info` is `#[repr(C)]` matching `WOF_EXTERNAL_INFO` + `FILE_PROVIDER_EXTERNAL_INFO_V1`.
            let ok = unsafe {
                DeviceIoControl(
                    handle.raw(),
                    FSCTL_SET_EXTERNAL_BACKING,
                    Some((&raw const info).cast::<c_void>()),
                    std::mem::size_of::<WofFileCompressionInfo>() as u32,
                    None,
                    0,
                    Some(&mut bytes_returned),
                    None,
                )
            };

            if ok.is_err() {
                let format = COMPRESSION_FORMAT_LZNT1;
                // SAFETY: Fallback to standard NTFS `COMPRESSION_FORMAT_LZNT1` (2-byte `u16`).
                let fb = unsafe {
                    DeviceIoControl(
                        handle.raw(),
                        FSCTL_SET_COMPRESSION,
                        Some((&raw const format).cast::<c_void>()),
                        std::mem::size_of::<u16>() as u32,
                        None,
                        0,
                        Some(&mut bytes_returned),
                        None,
                    )
                };
                fb.map_err(|e| GreenCError::Win32 {
                    code: e.code().0 as u32,
                    message: format!("Both WOF and NTFS compression failed: {}", e),
                })?;
                used_algo = "LZNT1".to_string();
            }
        }

        // Drop `handle` before querying compressed size so NTFS flushes metadata cleanly.
        drop(handle);

        let physical = self.get_compressed_size(path)?;
        Ok(CompactionResult {
            logical_bytes: logical,
            physical_bytes: physical,
            algorithm_used: used_algo,
        })
    }

    fn decompress_file(&self, path: &Path) -> Result<()> {
        ensure_not_protected_system_path(path, "decompress_file")?;
        let wide = path_to_wide(path);
        // SAFETY: Opens `path` with `GENERIC_READ | GENERIC_WRITE`; `OwnedHandle` closes on drop.
        let raw_handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("Failed to open file for decompression: {}", e),
        })?;
        let handle = OwnedHandle::new(raw_handle).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid file handle for decompression".to_string(),
        })?;

        let mut bytes_returned = 0u32;
        // SAFETY: Removes WOF external backing if present; harmless error if file is uncompressed.
        let _ = unsafe {
            DeviceIoControl(
                handle.raw(),
                FSCTL_DELETE_EXTERNAL_BACKING,
                None,
                0,
                None,
                0,
                Some(&mut bytes_returned),
                None,
            )
        };

        let format = COMPRESSION_FORMAT_NONE;
        // SAFETY: Clears NTFS LZNT1 compression flag using a valid 2-byte `COMPRESSION_FORMAT_NONE`.
        let _ = unsafe {
            DeviceIoControl(
                handle.raw(),
                FSCTL_SET_COMPRESSION,
                Some((&raw const format).cast::<c_void>()),
                std::mem::size_of::<u16>() as u32,
                None,
                0,
                Some(&mut bytes_returned),
                None,
            )
        };

        Ok(())
    }

    fn get_compressed_size(&self, path: &Path) -> Result<u64> {
        let wide = path_to_wide(path);
        let mut high = 0u32;
        // SAFETY: `wide` is NUL-terminated UTF-16 and `high` is a valid stack `u32` output pointer.
        let low = unsafe { GetCompressedFileSizeW(PCWSTR(wide.as_ptr()), Some(&mut high)) };
        if low == u32::MAX {
            Ok(std::fs::metadata(path)?.len())
        } else {
            Ok((u64::from(high) << 32) | u64::from(low))
        }
    }

    fn compact_vhdx(&self, path: &Path) -> Result<VhdxCompactionResult> {
        ensure_not_protected_system_path(path, "compact_vhdx")?;
        let size_before = std::fs::metadata(path)?.len();
        let wide = path_to_wide(path);

        let storage_type = VIRTUAL_STORAGE_TYPE {
            DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
            VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
        };

        let params = OPEN_VIRTUAL_DISK_PARAMETERS {
            Version: OPEN_VIRTUAL_DISK_VERSION_1,
            Anonymous: OPEN_VIRTUAL_DISK_PARAMETERS_0 {
                Version1: OPEN_VIRTUAL_DISK_PARAMETERS_0_0 { RWDepth: 1 },
            },
        };

        let mut raw_handle = HANDLE::default();
        // SAFETY: `storage_type`, `wide`, `params`, and `raw_handle` satisfy `OpenVirtualDisk` V1 contract.
        let open_res = unsafe {
            OpenVirtualDisk(
                &storage_type,
                PCWSTR(wide.as_ptr()),
                VIRTUAL_DISK_ACCESS_METAOPS,
                OPEN_VIRTUAL_DISK_FLAG_NONE,
                Some(&params),
                &mut raw_handle,
            )
        };

        if open_res != ERROR_SUCCESS {
            return Err(GreenCError::Win32 {
                code: open_res.0,
                message: format!("OpenVirtualDisk failed with Win32 error {}", open_res.0),
            });
        }
        let vhd_handle = OwnedHandle::new(raw_handle).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "OpenVirtualDisk returned invalid handle".to_string(),
        })?;

        let compact_params = COMPACT_VIRTUAL_DISK_PARAMETERS {
            Version: COMPACT_VIRTUAL_DISK_VERSION_1,
            Anonymous: COMPACT_VIRTUAL_DISK_PARAMETERS_0 {
                Version1: COMPACT_VIRTUAL_DISK_PARAMETERS_0_0 { Reserved: 0 },
            },
        };

        // SAFETY: `vhd_handle.raw()` was opened with `VIRTUAL_DISK_ACCESS_METAOPS` and is closed by `OwnedHandle`.
        let compact_res = unsafe {
            CompactVirtualDisk(
                vhd_handle.raw(),
                COMPACT_VIRTUAL_DISK_FLAG_NONE,
                Some(&compact_params),
                None,
            )
        };
        drop(vhd_handle);

        if compact_res != ERROR_SUCCESS {
            return Err(GreenCError::Win32 {
                code: compact_res.0,
                message: format!(
                    "CompactVirtualDisk failed with Win32 error {}",
                    compact_res.0
                ),
            });
        }

        let size_after = std::fs::metadata(path)?.len();
        Ok(VhdxCompactionResult {
            size_before_bytes: size_before,
            size_after_bytes: size_after,
        })
    }
}
