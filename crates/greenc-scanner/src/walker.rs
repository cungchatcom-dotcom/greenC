//! Parallel Rayon directory walker with NTFS hardlink deduplication and reparse point guards (Task T021).

use crate::space_lens::SpaceNode;
use greenc_core::error::Result;
use greenc_core::task::TaskControl;
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;
#[cfg(windows)]
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x00400000;
#[cfg(windows)]
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x00001000;

/// Unique physical file identity for hardlink deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub volume_serial: u64,
    pub file_index: u64,
}

impl FileIdentity {
    pub fn from_path(path: &Path) -> Option<Self> {
        #[cfg(windows)]
        {
            if let Some(id) = greenc_platform::query_file_identity(path) {
                if id.file_index != 0 {
                    return Some(Self {
                        volume_serial: id.volume_serial,
                        file_index: id.file_index,
                    });
                }
            }
            None
        }
        #[cfg(not(windows))]
        {
            let meta = fs::symlink_metadata(path).ok()?;
            Some(get_file_identity(&meta, path))
        }
    }
}

#[cfg(not(windows))]
fn get_file_identity(meta: &fs::Metadata, path: &Path) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        FileIdentity {
            volume_serial: meta.dev(),
            file_index: meta.ino(),
        }
    }

    #[cfg(not(unix))]
    {
        let _ = meta;
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        FileIdentity {
            volume_serial: 0,
            file_index: hasher.finish(),
        }
    }
}

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn is_cloud_placeholder(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        let attr = meta.file_attributes();
        (attr & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            && ((attr & FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS != 0)
                || (attr & FILE_ATTRIBUTE_OFFLINE != 0))
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        false
    }
}

fn is_excluded_dir_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("$RECYCLE.BIN")
        || name.eq_ignore_ascii_case("System Volume Information")
        || name.eq_ignore_ascii_case(".git")
        || name.eq_ignore_ascii_case(".venv")
}

pub use greenc_core::task::WalkProgressCallback;

/// Traverses the directory tree in parallel using Rayon work-stealing, deduplicating hardlinks
/// and respecting cooperative cancellation via `TaskControl`.
pub fn walk_directory_parallel(
    root: &Path,
    task_control: &TaskControl,
    max_depth: u8,
) -> Result<SpaceNode> {
    walk_directory_parallel_with_progress(root, task_control, max_depth, None)
}

/// Traverses the directory tree in parallel with cooperative cancellation and optional progress streaming.
pub fn walk_directory_parallel_with_progress(
    root: &Path,
    task_control: &TaskControl,
    max_depth: u8,
    custom_cb: Option<WalkProgressCallback>,
) -> Result<SpaceNode> {
    task_control.check_pause_or_cancel()?;

    if let Some(cb) = custom_cb {
        task_control.set_walk_progress_callback(cb);
    }

    let seen_identities = Arc::new(Mutex::new(HashSet::<FileIdentity>::new()));
    let processed_counter = Arc::new(AtomicU64::new(0));
    let bytes_counter = Arc::new(AtomicU64::new(0));

    let node = walk_recursive(
        root,
        0,
        max_depth,
        &seen_identities,
        task_control,
        &processed_counter,
        &bytes_counter,
    )?;

    // Emit final progress milestone
    task_control.report_walk_progress(root, node.file_count, node.physical_bytes);

    Ok(node)
}

/// Convenience synchronous walk using default TaskControl.
pub fn walk_directory(root: &Path, max_depth: u8) -> Result<SpaceNode> {
    let ctrl = TaskControl::new();
    walk_directory_parallel(root, &ctrl, max_depth)
}

fn walk_recursive(
    current_path: &Path,
    current_depth: u8,
    max_depth: u8,
    seen: &Arc<Mutex<HashSet<FileIdentity>>>,
    ctrl: &TaskControl,
    counter: &Arc<AtomicU64>,
    bytes_counter: &Arc<AtomicU64>,
) -> Result<SpaceNode> {
    ctrl.check_pause_or_cancel()?;

    let entries = match fs::read_dir(current_path) {
        Ok(iter) => iter,
        Err(_) => {
            // Unreadable or access denied directory (e.g. System Volume Information)
            return Ok(SpaceNode::empty(current_path));
        }
    };

    let mut direct_physical_bytes = 0u64;
    let mut direct_file_count = 0u64;
    let mut subdirectories = Vec::new();

    for entry_res in entries {
        // Cooperative cancellation & progress check every 64 entries visited (< 100 ms responsiveness SLA)
        if counter.fetch_add(1, Ordering::Relaxed) % 64 == 0 {
            ctrl.check_pause_or_cancel()?;
            let files = counter.load(Ordering::Relaxed);
            let bytes = bytes_counter.load(Ordering::Relaxed);
            ctrl.report_walk_progress(current_path, files, bytes);
        }

        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };

        let file_name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();

        // Query metadata WITHOUT following reparse points/junctions
        let sym_meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // REPARSE POINT & CLOUD PLACEHOLDER GUARD (Attribute 0x400):
        // Never recurse into directory junctions, symlinks, or volume mount points!
        if is_reparse_point(&sym_meta) {
            // Cloud placeholder files (e.g. OneDrive dehydrated) count as 1 file, 0 physical bytes
            if is_cloud_placeholder(&sym_meta) {
                direct_file_count += 1;
            }
            continue;
        }

        if sym_meta.is_dir() {
            if is_excluded_dir_name(&file_name) {
                continue;
            }

            subdirectories.push(path);
        } else {
            let file_size = sym_meta.len();

            // Hardlink deduplication check
            #[cfg(windows)]
            let is_hardlink_dup = if let Some(win_id) = greenc_platform::query_file_identity(&path)
            {
                if win_id.number_of_links > 1 && win_id.file_index != 0 {
                    let id = FileIdentity {
                        volume_serial: win_id.volume_serial,
                        file_index: win_id.file_index,
                    };
                    let mut guard = seen.lock().unwrap();
                    !guard.insert(id)
                } else {
                    false
                }
            } else {
                false
            };

            #[cfg(unix)]
            let is_hardlink_dup = {
                use std::os::unix::fs::MetadataExt;
                let num_links = sym_meta.nlink();
                if num_links > 1 {
                    let id = get_file_identity(&sym_meta, &path);
                    let mut guard = seen.lock().unwrap();
                    !guard.insert(id)
                } else {
                    false
                }
            };

            #[cfg(not(any(windows, unix)))]
            let is_hardlink_dup = false;

            if !is_hardlink_dup {
                direct_physical_bytes += file_size;
                bytes_counter.fetch_add(file_size, Ordering::Relaxed);
            }
            direct_file_count += 1;
        }
    }

    // Recurse into subdirectories in parallel using Rayon if within max_depth limit
    let child_nodes: Vec<SpaceNode> = if current_depth < max_depth && !subdirectories.is_empty() {
        subdirectories
            .into_par_iter()
            .map(|sub_path| {
                walk_recursive(
                    &sub_path,
                    current_depth + 1,
                    max_depth,
                    seen,
                    ctrl,
                    counter,
                    bytes_counter,
                )
            })
            .collect::<Result<Vec<SpaceNode>>>()?
    } else {
        Vec::new()
    };

    let total_bytes =
        direct_physical_bytes + child_nodes.iter().map(|c| c.physical_bytes).sum::<u64>();
    let total_files = direct_file_count + child_nodes.iter().map(|c| c.file_count).sum::<u64>();

    let mut node = SpaceNode::empty(current_path);
    node.physical_bytes = total_bytes;
    node.file_count = total_files;
    node.children = child_nodes;

    Ok(node)
}
