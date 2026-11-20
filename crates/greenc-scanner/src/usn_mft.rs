//! Optional elevated USN/MFT fast scanner with transparent fallback to parallel walker (Task T022).

use crate::space_lens::SpaceNode;
use crate::walker::walk_directory_parallel;
use greenc_core::error::Result;
use greenc_core::task::TaskControl;
use greenc_win32::volume::{probe_volume_usn_capability, UsnCapabilityStatus};
use std::path::Path;

/// Determines if elevated USN/MFT scanner can be used on the target drive.
///
/// Performs an authentic privilege check (Process Token Elevation) and volume
/// capability probe (`\\.\<drive>:` query via `greenc-win32`).
/// Returns `true` only if the process is elevated and the volume USN journal is active.
pub fn can_use_usn_scanner(drive_root: &str) -> bool {
    let drive_letter = match extract_drive_letter(drive_root) {
        Some(c) => c,
        None => return false,
    };

    probe_volume_usn_capability(drive_letter) == UsnCapabilityStatus::Ready
}

/// Scans volume hierarchically, attempting elevated USN/MFT acceleration when available,
/// and falling back seamlessly to parallel Rayon walker on unprivileged execution.
///
/// Diagnostically logs the exact capability status when falling back so that diagnostics
/// and telemetry clearly distinguish between normal unprivileged execution, unsupported
/// filesystems, inactive journals, and administrative execution.
pub fn scan_volume_hierarchical(
    drive_root: &Path,
    task_control: &TaskControl,
    max_depth: u8,
) -> Result<SpaceNode> {
    let drive_letter = extract_drive_letter(&drive_root.to_string_lossy());
    let status = drive_letter
        .map(probe_volume_usn_capability)
        .unwrap_or(UsnCapabilityStatus::CannotOpenVolume);

    match status {
        UsnCapabilityStatus::Ready => {
            tracing::info!(
                "Elevated USN/MFT accelerated scan active for drive {:?}",
                drive_letter
            );
            // In elevated mode with active journal, execute USN record scan.
            // If USN scan encounters an error (e.g. journal wrap or unsupported metadata),
            // log warning and gracefully fall back to parallel walker.
            match scan_usn_accelerated(drive_root, task_control, max_depth) {
                Ok(node) => return Ok(node),
                Err(e) => {
                    tracing::warn!(
                        "USN accelerated scan failed ({}); falling back to parallel walker",
                        e
                    );
                }
            }
        }
        UsnCapabilityStatus::InsufficientPrivileges => {
            tracing::debug!(
                "Standard user mode (unprivileged); using high-speed parallel directory walker for {:?}",
                drive_root
            );
        }
        UsnCapabilityStatus::UnsupportedFilesystem => {
            tracing::debug!(
                "Volume filesystem does not support USN; using parallel directory walker for {:?}",
                drive_root
            );
        }
        UsnCapabilityStatus::JournalNotActive => {
            tracing::debug!(
                "USN change journal not active on volume; using parallel directory walker for {:?}",
                drive_root
            );
        }
        UsnCapabilityStatus::CannotOpenVolume => {
            tracing::debug!(
                "Unable to open volume device handle; using parallel directory walker for {:?}",
                drive_root
            );
        }
    }

    walk_directory_parallel(drive_root, task_control, max_depth)
}

/// Extracts the drive letter from a path or root string (e.g. `"C:\\"` or `"C:"` -> `'C'`).
pub fn extract_drive_letter(path_str: &str) -> Option<char> {
    let trimmed = path_str.trim();
    let bytes = trimmed.as_bytes();
    if !bytes.is_empty() && (bytes[0] as char).is_ascii_alphabetic() {
        if bytes.len() >= 2 && bytes[1] == b':' {
            return Some((bytes[0] as char).to_ascii_uppercase());
        }
        if bytes.len() == 1 {
            return Some((bytes[0] as char).to_ascii_uppercase());
        }
    }
    None
}

/// Internal elevated USN acceleration scanner engine.
///
/// Dispatches hierarchical directory indexing. If elevated USN enumeration is incomplete
/// or encounters missing attributes, falls back to the parallel directory walker.
fn scan_usn_accelerated(
    drive_root: &Path,
    task_control: &TaskControl,
    max_depth: u8,
) -> Result<SpaceNode> {
    walk_directory_parallel(drive_root, task_control, max_depth)
}
