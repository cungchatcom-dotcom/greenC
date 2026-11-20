//! Streaming tree copy with inline SHA-256 verification, timestamp preservation, and cooperative cancellation (Task T042).

use greenc_core::error::{GreenCError, Result};
use greenc_core::task::TaskControl;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;

const CHUNK_SIZE: usize = 65536; // 64 KiB buffer

#[derive(Debug, Clone, Default)]
pub struct CopyStats {
    pub files_copied: u64,
    pub bytes_copied: u64,
}

/// Recursively removes a directory tree, ignoring errors on non-existent files.
pub fn remove_dir_all_safe(path: &Path) {
    if path.exists() {
        let _ = fs::remove_dir_all(path);
    }
}

/// Copies a directory tree with inline SHA-256 calculation, destination re-verification,
/// mtime preservation, reparse point tag skipping, and cooperative cancellation.
pub fn verified_copy_tree<F>(
    src: &Path,
    dst: &Path,
    ctrl: &TaskControl,
    mut on_progress: F,
) -> Result<CopyStats>
where
    F: FnMut(u64, u64), // (done_bytes, done_files)
{
    if !src.exists() {
        return Err(GreenCError::NotFound {
            entity: "SourcePath".to_string(),
            id: src.display().to_string(),
        });
    }

    if ctrl.is_cancelled() {
        return Err(GreenCError::Cancelled);
    }

    // Attempt to copy tree. On failure or cancellation, cleanup partial destination.
    let result = (|| -> Result<CopyStats> {
        let mut stats = CopyStats::default();

        if !dst.exists() {
            fs::create_dir_all(dst)?;
        }

        // Gather all entries to copy (skipping reparse points)
        let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];

        while let Some((curr_src, curr_dst)) = stack.pop() {
            if ctrl.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }

            ctrl.check_pause();

            let entries = fs::read_dir(&curr_src)?;
            for entry in entries.flatten() {
                if ctrl.is_cancelled() {
                    return Err(GreenCError::Cancelled);
                }

                ctrl.check_pause();

                let entry_path = entry.path();
                let file_name = entry.file_name();
                let dest_path = curr_dst.join(&file_name);

                let ft = entry.file_type()?;

                // Reparse Tag Guard (0x400): skip symlinks, junctions, and non-symlink reparse points
                // (e.g. OneDrive cloud dehydrated placeholders 0x9000001A, App Execution Aliases 0x8000001B).
                #[cfg(windows)]
                let is_reparse = if let Ok(meta) = entry.metadata() {
                    (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                } else if let Ok(meta) = fs::symlink_metadata(&entry_path) {
                    (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                } else {
                    ft.is_symlink()
                };

                #[cfg(not(windows))]
                let is_reparse = ft.is_symlink();

                if is_reparse || ft.is_symlink() {
                    tracing::debug!("Skipping nested reparse/symlink: {}", entry_path.display());
                    continue;
                }

                if ft.is_dir() {
                    if !dest_path.exists() {
                        fs::create_dir_all(&dest_path)?;
                    }
                    // Preserve directory mtime if possible
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            let _ = fs::File::open(&dest_path).and_then(|f| f.set_modified(mtime));
                        }
                    }
                    stack.push((entry_path, dest_path));
                } else if ft.is_file() {
                    let (copied_bytes, _) = copy_and_verify_file(&entry_path, &dest_path, ctrl)?;
                    stats.files_copied += 1;
                    stats.bytes_copied += copied_bytes;
                    on_progress(stats.bytes_copied, stats.files_copied);
                }
            }
        }

        Ok(stats)
    })();

    match result {
        Ok(stats) => Ok(stats),
        Err(err) => {
            tracing::warn!(
                "verified_copy_tree aborted or failed: {}. Cleaning partial target: {}",
                err,
                dst.display()
            );
            remove_dir_all_safe(dst);
            Err(err)
        }
    }
}

/// Copies a single file while calculating SHA-256 on the fly, re-verifying the destination,
/// and preserving original timestamps.
fn copy_and_verify_file(src: &Path, dst: &Path, ctrl: &TaskControl) -> Result<(u64, String)> {
    let mut src_file = fs::File::open(src)?;
    let src_meta = src_file.metadata()?;
    let expected_len = src_meta.len();

    let mut dst_file = fs::File::create(dst)?;
    let mut hasher_src = Sha256::new();
    let mut buffer = [0u8; CHUNK_SIZE];
    let mut total_copied = 0u64;

    loop {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        ctrl.check_pause();

        let n = src_file.read(&mut buffer)?;
        if n == 0 {
            break;
        }

        hasher_src.update(&buffer[..n]);
        dst_file.write_all(&buffer[..n])?;
        total_copied += n as u64;
    }

    dst_file.flush()?;

    if total_copied != expected_len {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: format!("{} bytes", expected_len),
            actual: format!("{} bytes", total_copied),
        });
    }

    let hash_src = format!("{:x}", hasher_src.finalize());

    // Re-verify destination file hash
    let mut dst_read = fs::File::open(dst)?;
    let mut hasher_dst = Sha256::new();
    loop {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        let n = dst_read.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher_dst.update(&buffer[..n]);
    }

    let hash_dst = format!("{:x}", hasher_dst.finalize());

    if hash_src != hash_dst {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: hash_src,
            actual: hash_dst,
        });
    }

    // Preserve original modified timestamp
    if let Ok(mtime) = src_meta.modified() {
        let _ = dst_file.set_modified(mtime);
    }

    Ok((total_copied, hash_src))
}
