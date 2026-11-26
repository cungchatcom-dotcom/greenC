//! Streaming tree copy with inline SHA-256 verification, `sync_all` durability,
//! timestamp preservation, long-path (`> 260` chars) support, fail-closed error handling,
//! and cooperative cancellation (Tasks T042, GC-ZALO2).

use greenc_core::error::{GreenCError, Result};
use greenc_core::task::TaskControl;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

const CHUNK_SIZE: usize = 65536; // 64 KiB streaming buffer (handles > 4 GiB via u64)

#[derive(Debug, Clone, Default)]
pub struct CopyStats {
    pub files_copied: u64,
    pub bytes_copied: u64,
}

/// One file seen by a copy, with the source metadata observed at that moment.
#[derive(Debug, Clone)]
struct ManifestFile {
    rel: PathBuf,
    len: u64,
    mtime: Option<SystemTime>,
    /// Shell metadata such as `desktop.ini`, which Windows re-creates on its own.
    regenerable: bool,
}

/// What a copy covered. Only files listed here may later be removed from the source.
#[derive(Debug, Clone, Default)]
pub struct CopyManifest {
    files: Vec<ManifestFile>,
    dirs: Vec<PathBuf>,
}

impl CopyManifest {
    /// `true` when `root` still holds exactly the copied files, unchanged.
    pub fn matches_tree(&self, root: &Path) -> bool {
        let known: std::collections::HashMap<&Path, &ManifestFile> =
            self.files.iter().map(|f| (f.rel.as_path(), f)).collect();
        let mut seen = 0usize;
        let mut stack = vec![PathBuf::new()];
        while let Some(rel_dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(to_extended_io_path(&root.join(&rel_dir))) else {
                return false;
            };
            for entry in entries {
                let Ok(entry) = entry else { return false };
                let rel = rel_dir.join(entry.file_name());
                let Ok(meta) = fs::symlink_metadata(to_extended_io_path(&root.join(&rel))) else {
                    return false;
                };
                if is_meta_reparse_or_symlink(&meta) {
                    continue;
                }
                if meta.is_dir() {
                    stack.push(rel);
                } else if meta.is_file() {
                    match known.get(rel.as_path()) {
                        Some(f)
                            if f.regenerable
                                || (f.len == meta.len() && f.mtime == meta.modified().ok()) =>
                        {
                            seen += 1;
                        }
                        _ => return false,
                    }
                }
            }
        }
        seen == self.files.len()
    }

    /// Deletes from `src` only files that are unchanged since the copy and present in `dst`.
    /// New, modified or undeletable files and all links stay. Returns how many copied files remain.
    pub fn remove_copied(&self, src: &Path, dst: &Path) -> usize {
        let mut kept = 0;
        for f in &self.files {
            let src_io = to_extended_io_path(&src.join(&f.rel));
            let Ok(meta) = fs::symlink_metadata(&src_io) else {
                continue;
            };
            let unchanged = meta.is_file()
                && !is_meta_reparse_or_symlink(&meta)
                && meta.len() == f.len
                && meta.modified().ok() == f.mtime;
            let in_dst = fs::symlink_metadata(to_extended_io_path(&dst.join(&f.rel)))
                .is_ok_and(|m| m.is_file() && m.len() == f.len);
            if !(f.regenerable || (unchanged && in_dst)) || fs::remove_file(&src_io).is_err() {
                kept += 1;
            }
        }
        // `remove_dir` refuses non-empty directories, so anything left inside survives.
        for rel in self.dirs.iter().rev() {
            let _ = fs::remove_dir(to_extended_io_path(&src.join(rel)));
        }
        let _ = fs::remove_dir(to_extended_io_path(src));
        kept
    }
}

fn is_regenerable_shell_file(name: &std::ffi::OsStr) -> bool {
    name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db")
}

fn same_content(a: &Path, b: &Path) -> Result<bool> {
    let hash = |p: &Path| -> Result<(u64, Vec<u8>)> {
        let mut file = fs::File::open(p)?;
        let mut hasher = Sha256::new();
        let len = std::io::copy(&mut file, &mut hasher)?;
        Ok((len, hasher.finalize().to_vec()))
    };
    Ok(fs::metadata(a)?.len() == fs::metadata(b)?.len() && hash(a)? == hash(b)?)
}

/// Controls strictness of [`verified_copy_tree_with_options`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CopyTreeOptions {
    /// When `true` (fail-closed mode, mandatory for Zalo chat data), encountering any nested
    /// symlink, NTFS junction, or reparse point aborts the entire copy instead of skipping it.
    pub fail_on_reparse: bool,
    /// When `true`, encountering non-regular special filesystem entries (FIFO/socket/device)
    /// aborts the entire copy instead of skipping them.
    pub fail_on_special_file: bool,
}

impl CopyTreeOptions {
    /// Strict fail-closed mode used for irreplaceable chat data (e.g. Zalo `Zalo Data`).
    pub const fn strict() -> Self {
        Self {
            fail_on_reparse: true,
            fail_on_special_file: true,
        }
    }
}

/// Returns an extended-length verbatim path (`\\?\...`) on Windows when an absolute path
/// approaches or exceeds `MAX_PATH` (260 chars), enabling transparent I/O on long paths.
pub fn to_extended_io_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        if raw.starts_with(r"\\?\") || raw.starts_with(r"\\.\") || raw.starts_with(r"\\") {
            return path.to_path_buf();
        }
        let bytes = raw.as_bytes();
        let is_drive_abs = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        if is_drive_abs && raw.len() >= 240 && !raw.contains("..") {
            let normalized = raw.replace('/', "\\");
            return PathBuf::from(format!(r"\\?\{normalized}"));
        }
        path.to_path_buf()
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

pub(crate) fn is_meta_reparse_or_symlink(meta: &fs::Metadata) -> bool {
    let is_link = meta.file_type().is_symlink();
    #[cfg(windows)]
    {
        is_link || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
    #[cfg(not(windows))]
    {
        is_link
    }
}

/// Recursively removes a directory tree, ignoring errors on non-existent files.
pub fn remove_dir_all_safe(path: &Path) {
    let io_path = to_extended_io_path(path);
    if fs::symlink_metadata(&io_path).is_ok() {
        let _ = fs::remove_dir_all(&io_path);
    }
}

/// Copies a directory tree with inline SHA-256 calculation, `sync_all`, destination
/// re-verification, mtime preservation, reparse point tag skipping, and cooperative cancellation.
pub fn verified_copy_tree<F>(
    src: &Path,
    dst: &Path,
    ctrl: &TaskControl,
    on_progress: F,
) -> Result<CopyStats>
where
    F: FnMut(u64, u64), // (done_bytes, done_files)
{
    verified_copy_tree_with_options(src, dst, &CopyTreeOptions::default(), ctrl, on_progress)
}

/// Strict fail-closed variant of [`verified_copy_tree`]: any read error, permission error,
/// symlink/reparse point, or special file aborts the copy and cleans partial destination.
pub fn verified_copy_tree_strict<F>(
    src: &Path,
    dst: &Path,
    ctrl: &TaskControl,
    on_progress: F,
) -> Result<CopyStats>
where
    F: FnMut(u64, u64), // (done_bytes, done_files)
{
    verified_copy_tree_with_options(src, dst, &CopyTreeOptions::strict(), ctrl, on_progress)
}

/// Copies a directory tree with configurable fail-closed behavior.
pub fn verified_copy_tree_with_options<F>(
    src: &Path,
    dst: &Path,
    opts: &CopyTreeOptions,
    ctrl: &TaskControl,
    on_progress: F,
) -> Result<CopyStats>
where
    F: FnMut(u64, u64),
{
    verified_copy_tree_manifest(src, dst, opts, ctrl, on_progress).map(|(stats, _)| stats)
}

/// Like [`verified_copy_tree_with_options`], also returning what was copied. Existing files in `dst`
/// are never overwritten, and a failed copy removes only what this call created.
pub fn verified_copy_tree_manifest<F>(
    src: &Path,
    dst: &Path,
    opts: &CopyTreeOptions,
    ctrl: &TaskControl,
    mut on_progress: F,
) -> Result<(CopyStats, CopyManifest)>
where
    F: FnMut(u64, u64),
{
    let src_io = to_extended_io_path(src);
    let src_meta = match fs::symlink_metadata(&src_io) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(GreenCError::NotFound {
                entity: "SourcePath".to_string(),
                id: src.display().to_string(),
            });
        }
        Err(e) => return Err(GreenCError::Io(e)),
    };

    if opts.fail_on_reparse && is_meta_reparse_or_symlink(&src_meta) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "verified_copy".to_string(),
            reason: format!(
                "Thư mục nguồn là liên kết/reparse point không an toàn: {}",
                src.display()
            ),
        });
    }

    if !src_meta.is_dir() {
        return Err(GreenCError::InvalidPath(src.to_path_buf()));
    }

    if ctrl.is_cancelled() {
        return Err(GreenCError::Cancelled);
    }

    let mut manifest = CopyManifest::default();
    let mut created_files: Vec<PathBuf> = Vec::new();
    let mut created_dirs: Vec<PathBuf> = Vec::new();

    let result = (|| -> Result<CopyStats> {
        let mut stats = CopyStats::default();
        let mut dir_mtimes: Vec<(PathBuf, SystemTime)> = Vec::new();

        let dst_io = to_extended_io_path(dst);
        if fs::symlink_metadata(&dst_io).is_err() {
            fs::create_dir_all(&dst_io)?;
            created_dirs.push(dst_io.clone());
        }
        if let Ok(mtime) = src_meta.modified() {
            dir_mtimes.push((dst.to_path_buf(), mtime));
        }

        let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dst.to_path_buf())];

        while let Some((curr_src, curr_dst)) = stack.pop() {
            if ctrl.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }
            ctrl.check_pause();

            let curr_src_io = to_extended_io_path(&curr_src);
            let entries = fs::read_dir(&curr_src_io)?;
            for entry_res in entries {
                if ctrl.is_cancelled() {
                    return Err(GreenCError::Cancelled);
                }
                ctrl.check_pause();

                // Fail-closed: NEVER swallow read_dir entry errors via `.flatten()`.
                let entry = entry_res?;
                let entry_path = curr_src.join(entry.file_name());
                let dest_path = curr_dst.join(entry.file_name());
                let entry_io = to_extended_io_path(&entry_path);

                let meta = fs::symlink_metadata(&entry_io)?;
                if is_meta_reparse_or_symlink(&meta) {
                    if opts.fail_on_reparse {
                        return Err(GreenCError::PreconditionBlocked {
                            operation: "verified_copy".to_string(),
                            reason: format!(
                                "Phát hiện liên kết/reparse point không an toàn bên trong dữ liệu nguồn: {}",
                                entry_path.display()
                            ),
                        });
                    }
                    tracing::debug!("Skipping nested reparse/symlink: {}", entry_path.display());
                    continue;
                }

                if meta.is_dir() {
                    let dest_io = to_extended_io_path(&dest_path);
                    if fs::symlink_metadata(&dest_io).is_err() {
                        fs::create_dir_all(&dest_io)?;
                        created_dirs.push(dest_io.clone());
                    }
                    if let Ok(rel) = entry_path.strip_prefix(src) {
                        manifest.dirs.push(rel.to_path_buf());
                    }
                    if let Ok(mtime) = meta.modified() {
                        dir_mtimes.push((dest_path.clone(), mtime));
                    }
                    stack.push((entry_path, dest_path));
                } else if meta.is_file() {
                    let dest_io = to_extended_io_path(&dest_path);
                    let regenerable = is_regenerable_shell_file(&entry.file_name());
                    let copied_bytes = if fs::symlink_metadata(&dest_io).is_ok() {
                        if !regenerable && !same_content(&entry_io, &dest_io)? {
                            return Err(GreenCError::PreconditionBlocked {
                                operation: "verified_copy".to_string(),
                                reason: format!(
                                    "Thư mục đích đã có tệp khác cùng tên, GreenC không ghi đè: {}",
                                    dest_path.display()
                                ),
                            });
                        }
                        meta.len()
                    } else {
                        created_files.push(dest_io);
                        copy_and_verify_file(&entry_path, &dest_path, ctrl)?.0
                    };
                    if let Ok(rel) = entry_path.strip_prefix(src) {
                        manifest.files.push(ManifestFile {
                            rel: rel.to_path_buf(),
                            len: meta.len(),
                            mtime: meta.modified().ok(),
                            regenerable,
                        });
                    }
                    stats.files_copied = stats.files_copied.saturating_add(1);
                    stats.bytes_copied = stats.bytes_copied.saturating_add(copied_bytes);
                    on_progress(stats.bytes_copied, stats.files_copied);
                } else if opts.fail_on_special_file {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "verified_copy".to_string(),
                        reason: format!(
                            "Phát hiện tệp hệ thống đặc biệt không hỗ trợ sao chép an toàn: {}",
                            entry_path.display()
                        ),
                    });
                }
            }
        }

        // Restore directory mtimes bottom-up so creating children does not overwrite parent mtime.
        for (dir_path, mtime) in dir_mtimes.into_iter().rev() {
            let dir_io = to_extended_io_path(&dir_path);
            let _ = fs::File::open(&dir_io).and_then(|f| f.set_modified(mtime));
        }

        Ok(stats)
    })();

    match result {
        Ok(stats) => Ok((stats, manifest)),
        Err(err) => {
            tracing::warn!(
                "verified_copy_tree aborted or failed: {}. Cleaning partial target: {}",
                err,
                dst.display()
            );
            for file in created_files.iter().rev() {
                let _ = fs::remove_file(file);
            }
            for dir in created_dirs.iter().rev() {
                let _ = fs::remove_dir(dir);
            }
            Err(err)
        }
    }
}

/// Copies a single file while calculating SHA-256 on the fly, flushing with `sync_all`,
/// re-verifying the destination hash from disk, preserving original mtime, and verifying
/// the source file did not change during copy.
fn copy_and_verify_file(src: &Path, dst: &Path, ctrl: &TaskControl) -> Result<(u64, String)> {
    let src_io = to_extended_io_path(src);
    let dst_io = to_extended_io_path(dst);

    let mut src_file = fs::File::open(&src_io)?;
    let src_meta_before = src_file.metadata()?;
    let expected_len: u64 = src_meta_before.len();
    let src_mtime_before = src_meta_before.modified().ok();

    let mut hasher_src = Sha256::new();
    let mut buffer = [0u8; CHUNK_SIZE];
    let mut total_copied: u64 = 0;

    {
        let mut dst_file = fs::File::create_new(&dst_io)?;
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
            total_copied = total_copied.saturating_add(n as u64);
        }

        dst_file.flush()?;
        dst_file.sync_all()?;

        if let Some(mtime) = src_mtime_before {
            let _ = dst_file.set_modified(mtime);
        }
    }

    if total_copied != expected_len {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: format!("{expected_len} bytes"),
            actual: format!("{total_copied} bytes"),
        });
    }

    // Verify source was not modified or replaced while we were reading it.
    let src_meta_after_path = fs::symlink_metadata(&src_io)?;
    let src_meta_after_fd = src_file.metadata()?;
    if src_meta_after_path.len() != expected_len
        || src_meta_after_fd.len() != expected_len
        || src_meta_after_path.modified().ok() != src_mtime_before
        || src_meta_after_fd.modified().ok() != src_mtime_before
    {
        return Err(GreenCError::IntegrityMismatch {
            path: src.to_path_buf(),
            expected: format!("unchanged ({expected_len} bytes)"),
            actual: format!(
                "source changed during copy ({} bytes)",
                src_meta_after_path.len()
            ),
        });
    }

    let hash_src = format!("{:x}", hasher_src.finalize());

    // Re-verify destination file SHA-256 directly from disk after `sync_all`.
    let mut dst_read = fs::File::open(&dst_io)?;
    let mut hasher_dst = Sha256::new();
    let mut read_back: u64 = 0;
    loop {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        ctrl.check_pause();

        let n = dst_read.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher_dst.update(&buffer[..n]);
        read_back = read_back.saturating_add(n as u64);
    }

    if read_back != expected_len {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: format!("{expected_len} bytes"),
            actual: format!("{read_back} bytes"),
        });
    }

    let hash_dst = format!("{:x}", hasher_dst.finalize());
    if hash_src != hash_dst {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: hash_src,
            actual: hash_dst,
        });
    }

    Ok((total_copied, hash_src))
}
