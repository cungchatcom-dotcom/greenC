//! Unix stub for Windows-only WOF/VHDX compaction (returns clear PreconditionBlocked).

use crate::traits::{CompactionApi, CompactionResult, VhdxCompactionResult, WofAlgorithm};
use greenc_core::error::{GreenCError, Result};
use std::path::Path;

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

/// Native Unix implementation of `CompactionApi` returning explicit unsupported status
/// for Windows-specific NTFS WOF and Hyper-V VHDX compaction.
#[derive(Default, Clone, Debug)]
pub struct NativeCompactionApi;

impl CompactionApi for NativeCompactionApi {
    fn compact_file(&self, _path: &Path, _algorithm: WofAlgorithm) -> Result<CompactionResult> {
        Err(GreenCError::PreconditionBlocked {
            operation: "compact_file".to_string(),
            reason: "In-place NTFS WOF compaction is Windows-only; use Vault zstd archiving on macOS/Linux".to_string(),
        })
    }

    fn decompress_file(&self, _path: &Path) -> Result<()> {
        Err(GreenCError::PreconditionBlocked {
            operation: "decompress_file".to_string(),
            reason: "NTFS WOF decompression is Windows-only".to_string(),
        })
    }

    fn get_compressed_size(&self, path: &Path) -> Result<u64> {
        Ok(std::fs::metadata(path)?.len())
    }

    fn compact_vhdx(&self, _path: &Path) -> Result<VhdxCompactionResult> {
        Err(GreenCError::PreconditionBlocked {
            operation: "compact_vhdx".to_string(),
            reason: "VHDX virtual disk compaction is Windows-only".to_string(),
        })
    }
}
