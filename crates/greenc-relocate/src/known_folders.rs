//! Windows Shell Known Folders relocation orchestrator with OneDrive protection (Tasks T040, T043).

use crate::candidate::{CandidateQualificationEngine, CandidateScanner};
use crate::verified_copy::{remove_dir_all_safe, verified_copy_tree};
use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    NativeReparsePointApi, ReparsePointApi, ShellFolderApi, Win32VolumeApi,
};
use greenc_platform::{
    FOLDERID_DESKTOP, FOLDERID_DOCUMENTS, FOLDERID_DOWNLOADS, FOLDERID_MUSIC, FOLDERID_PICTURES,
    FOLDERID_VIDEOS,
};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

fn normalized_path_segments(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy();
    let stripped = raw
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| raw.strip_prefix(r"\\?\"))
        .unwrap_or(&raw);
    stripped
        .split(['\\', '/'])
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn path_is_equal_or_inside(candidate: &Path, root: &Path) -> bool {
    let c_segs = normalized_path_segments(candidate);
    let r_segs = normalized_path_segments(root);
    !r_segs.is_empty() && c_segs.len() >= r_segs.len() && c_segs[..r_segs.len()] == r_segs[..]
}

/// Checks whether `root` equals or contains any Zalo junction (either tracked in `db` or
/// discovered directly on disk via reparse-point inspection without following symlinks).
fn find_zalo_junction_in_tree(root: &Path, db: &GreenCDb) -> Option<PathBuf> {
    // 1. Check active Zalo relocations recorded in DB.
    if let Ok(relocs) = db.list_relocations(None) {
        for rec in relocs {
            if rec.app_id.eq_ignore_ascii_case("zalo")
                && matches!(
                    rec.status,
                    RelocationStatus::Done
                        | RelocationStatus::NeedsReview
                        | RelocationStatus::Moving
                )
                && path_is_equal_or_inside(&rec.source_path, root)
            {
                return Some(rec.source_path);
            }
        }
    }

    // 2. Walk `root` without following symlinks/reparse points to catch untracked Zalo junctions.
    let reparse = NativeReparsePointApi;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            let is_link = meta.file_type().is_symlink() || reparse.is_reparse_point(&path);
            if is_link {
                let name_lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
                let target_is_zalo = reparse
                    .get_junction_target(&path)
                    .or_else(|_| fs::read_link(&path).map_err(GreenCError::Io))
                    .is_ok_and(|tgt| {
                        greenc_scanner::zalo::has_valid_zalo_data_structure(&tgt)
                            || tgt.to_string_lossy().to_ascii_lowercase().contains("zalo")
                    });
                if name_lower.contains("zalo")
                    || target_is_zalo
                    || greenc_scanner::zalo::has_valid_zalo_data_structure(&path)
                {
                    return Some(path);
                }
                // Never recurse into symlinks/junctions.
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownFolderInfo {
    pub folder_id: String,
    pub guid: String,
    pub name: String,
    pub current_path: PathBuf,
    pub default_target_path: PathBuf,
    pub size_bytes: u64,
    pub file_count: u64,
    pub is_redirected: bool,
    pub is_onedrive: bool,
}

pub struct KnownFolderService;

impl KnownFolderService {
    /// Returns the 6 standard Windows Shell Known Folders with their current physical paths and sizes.
    pub fn list_known_folders(
        target_drive: char,
        shell_api: &dyn ShellFolderApi,
    ) -> Result<Vec<KnownFolderInfo>> {
        let defs = [
            ("downloads", "Downloads", FOLDERID_DOWNLOADS),
            ("documents", "Documents", FOLDERID_DOCUMENTS),
            ("pictures", "Pictures", FOLDERID_PICTURES),
            ("videos", "Videos", FOLDERID_VIDEOS),
            ("music", "Music", FOLDERID_MUSIC),
            ("desktop", "Desktop", FOLDERID_DESKTOP),
        ];

        let mut list = Vec::new();

        for (id, name, guid) in defs {
            if let Ok(current_path) = shell_api.get_known_folder(guid) {
                let (size, files) = CandidateScanner::compute_dir_size(&current_path);
                let is_onedrive = shell_api.is_onedrive_path(&current_path);

                let is_redirected = current_path
                    .to_string_lossy()
                    .to_uppercase()
                    .starts_with(&format!("{}:", target_drive.to_ascii_uppercase()));

                let default_target =
                    PathBuf::from(format!("{}:\\{}", target_drive.to_ascii_uppercase(), name));

                list.push(KnownFolderInfo {
                    folder_id: id.to_string(),
                    guid: guid.to_string(),
                    name: name.to_string(),
                    current_path,
                    default_target_path: default_target,
                    size_bytes: size,
                    file_count: files,
                    is_redirected,
                    is_onedrive,
                });
            }
        }

        Ok(list)
    }

    /// Redirects a Windows Shell Known Folder to a secondary volume via `SHSetKnownFolderPath`.
    pub fn relocate_known_folder<F>(
        folder_id: &str,
        target_drive: char,
        custom_target_path: Option<PathBuf>,
        db: &GreenCDb,
        shell_api: &dyn ShellFolderApi,
        volume_api: &dyn Win32VolumeApi,
        ctrl: &TaskControl,
        on_progress: F,
    ) -> Result<RelocationRecord>
    where
        F: FnMut(u64, u64),
    {
        let guid = match folder_id {
            "downloads" => FOLDERID_DOWNLOADS,
            "documents" => FOLDERID_DOCUMENTS,
            "pictures" => FOLDERID_PICTURES,
            "videos" => FOLDERID_VIDEOS,
            "music" => FOLDERID_MUSIC,
            "desktop" => FOLDERID_DESKTOP,
            other => other,
        };

        let current_path = shell_api.get_known_folder(guid)?;
        CandidateQualificationEngine::check_onedrive(&current_path, shell_api)?;

        // Item 12: Refuse relocating a Known Folder whose tree contains a Zalo junction
        // (tracked or untracked) so `verified_copy_tree` + `remove_dir_all_safe` can never break
        // or orphan relocated Zalo chat data.
        if let Some(zalo_link) = find_zalo_junction_in_tree(&current_path, db) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "relocate_known_folder".to_string(),
                reason: format!(
                    "Thư mục '{}' đang chứa liên kết dữ liệu Zalo ({}); GreenC từ chối di chuyển thư mục này để bảo vệ dữ liệu chat Zalo.",
                    current_path.display(),
                    zalo_link.display()
                ),
            });
        }

        let target_path = custom_target_path.unwrap_or_else(|| {
            let folder_name = current_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(folder_id);
            PathBuf::from(format!(
                "{}:\\{}",
                target_drive.to_ascii_uppercase(),
                folder_name
            ))
        });

        // Pre-qualification checks
        let (size_bytes, _) = CandidateScanner::compute_dir_size(&current_path);
        CandidateQualificationEngine::check_target_volume_eligible(target_drive, volume_api)?;
        CandidateQualificationEngine::check_free_space_reserve(
            target_drive,
            size_bytes,
            volume_api,
        )?;
        CandidateQualificationEngine::check_target_directory_empty_or_absent(&target_path)?;

        // Ensure target directory exists
        fs::create_dir_all(&target_path)?;

        // Copy files with streaming SHA-256 verification
        let copy_stats = verified_copy_tree(&current_path, &target_path, ctrl, on_progress)?;

        // Redirect Windows Shell location
        if let Err(set_err) = shell_api.set_known_folder(guid, &target_path) {
            tracing::error!(
                "SHSetKnownFolderPath failed to redirect {} to {}: {}. Rolling back target.",
                folder_id,
                target_path.display(),
                set_err
            );
            remove_dir_all_safe(&target_path);
            return Err(set_err);
        }

        // Clean original files from C: if copy and redirect succeeded
        remove_dir_all_safe(&current_path);

        let mut record = RelocationRecord {
            id: None,
            app_id: format!("known_folder_{}", folder_id),
            app_name: format!("Thư mục {}", folder_id),
            strategy: RelocationStrategyKind::KnownFolder,
            source_path: current_path,
            target_path,
            size_bytes: copy_stats.bytes_copied,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: Vec::new(),
            created_at: Utc::now(),
            detail: Some("Chuyển thư mục Windows Shell thành công".to_string()),
            moving_path: None,
            target_user_repaired: false,
        };

        let id = db.save_relocation(&record)?;
        record.id = Some(id);

        Ok(record)
    }
}
