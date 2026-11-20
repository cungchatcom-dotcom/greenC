//! Windows Shell Known Folders relocation orchestrator with OneDrive protection (Tasks T040, T043).

use crate::candidate::{CandidateQualificationEngine, CandidateScanner};
use crate::verified_copy::{remove_dir_all_safe, verified_copy_tree};
use chrono::Utc;
use greenc_core::error::Result;
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{ShellFolderApi, Win32VolumeApi};
use greenc_platform::{
    FOLDERID_DESKTOP, FOLDERID_DOCUMENTS, FOLDERID_DOWNLOADS, FOLDERID_MUSIC, FOLDERID_PICTURES,
    FOLDERID_VIDEOS,
};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
        };

        let id = db.save_relocation(&record)?;
        record.id = Some(id);

        Ok(record)
    }
}
