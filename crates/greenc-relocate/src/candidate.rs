//! Candidate qualification engine and multi-strategy discovery (Tasks T039, T046).

use crate::catalog::{
    get_app_catalog, get_tool_catalog, ADVANCED_SKIP_LIST, ADVANCED_THRESHOLD_BYTES,
};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::RelocationStrategyKind;
use greenc_platform::traits::{FileLockProbe, ProcessServiceProbe, ShellFolderApi, Win32VolumeApi};
use greenc_platform::{
    FOLDERID_DESKTOP, FOLDERID_DOCUMENTS, FOLDERID_DOWNLOADS, FOLDERID_MUSIC, FOLDERID_PICTURES,
    FOLDERID_VIDEOS,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FREE_SPACE_RESERVE_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB minimum reserve

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelocationCandidate {
    pub id: String,
    pub name: String,
    pub strategy: RelocationStrategyKind,
    pub source_path: PathBuf,
    pub default_target_path: PathBuf,
    pub size_bytes: u64,
    pub file_count: u64,
    pub is_locked: bool,
    pub locked_processes: Vec<String>,
    pub how_description: String,
    pub is_advanced: bool,
    pub is_eligible: bool,
    pub ineligible_reason: Option<String>,
}

/// Engine to enforce pre-qualification checks before modifying filesystem or redirecting paths.
pub struct CandidateQualificationEngine;

impl CandidateQualificationEngine {
    /// Verifies that the destination drive is fixed and uses the NTFS filesystem.
    pub fn check_target_volume_eligible(
        target_drive: char,
        volume_api: &dyn Win32VolumeApi,
    ) -> Result<()> {
        let root = PathBuf::from(format!("{}:\\", target_drive.to_ascii_uppercase()));
        let (_, fs, drive_type) = volume_api.get_volume_info(&root)?;

        if drive_type != 3 {
            // 3 = DRIVE_FIXED
            return Err(GreenCError::PreconditionBlocked {
                operation: "check_target_volume_eligible".to_string(),
                reason: format!(
                    "Ổ đĩa đích {}:\\ không phải là ổ đĩa cố định (Fixed Drive)",
                    target_drive
                ),
            });
        }

        if !fs.eq_ignore_ascii_case("NTFS") {
            return Err(GreenCError::PreconditionBlocked {
                operation: "check_target_volume_eligible".to_string(),
                reason: format!(
                    "Ổ đĩa đích {}:\\ sử dụng định dạng {} thay vì NTFS bắt buộc",
                    target_drive, fs
                ),
            });
        }

        Ok(())
    }

    /// Enforces `free_bytes >= required_bytes + 512 MiB` reserve on the destination drive.
    pub fn check_free_space_reserve(
        target_drive: char,
        required_bytes: u64,
        volume_api: &dyn Win32VolumeApi,
    ) -> Result<()> {
        let free_bytes = volume_api.get_free_space(target_drive)?;
        let total_required = required_bytes.saturating_add(FREE_SPACE_RESERVE_BYTES);

        if free_bytes < total_required {
            return Err(GreenCError::InsufficientVaultSpace(format!(
                "Ổ {}:\\ chỉ còn trống {} bytes, cần tối thiểu {} bytes (kèm 512 MiB dự trữ)",
                target_drive, free_bytes, total_required
            )));
        }

        Ok(())
    }

    /// Checks if any processes associated with the candidate are actively running.
    pub fn check_process_running(
        process_names: &[String],
        probe: &dyn ProcessServiceProbe,
    ) -> Result<()> {
        let mut active = Vec::new();
        for proc in process_names {
            if probe.is_process_running(&proc.to_lowercase()) {
                active.push(proc.clone());
            }
        }

        if !active.is_empty() {
            let msg = format!("Cần đóng {}", active.join(", "));
            return Err(GreenCError::FileLocked {
                path: PathBuf::from(active.join(", ")),
                reason: msg,
            });
        }

        Ok(())
    }

    /// Checks if a directory or any of its immediate child files are locked by another process using `FileLockProbe`.
    pub fn check_directory_locked(dir: &Path, lock_probe: &dyn FileLockProbe) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        if lock_probe.is_file_locked(dir) {
            return Err(GreenCError::FileLocked {
                path: dir.to_path_buf(),
                reason: format!("Thư mục đang bị khóa bởi ứng dụng khác: {}", dir.display()),
            });
        }

        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && lock_probe.is_file_locked(&path) {
                        return Err(GreenCError::FileLocked {
                            path: path.clone(),
                            reason: format!(
                                "Tệp tin đang bị khóa bởi ứng dụng khác: {}",
                                path.display()
                            ),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Convenience helper returning true if the directory or any immediate child is locked.
    pub fn is_path_locked(dir: &Path, lock_probe: &dyn FileLockProbe) -> bool {
        Self::check_directory_locked(dir, lock_probe).is_err()
    }

    /// Verifies that the target path does not already exist with conflicting non-empty data.
    pub fn check_target_directory_empty_or_absent(target_path: &Path) -> Result<()> {
        if target_path.exists() {
            if target_path.is_dir() {
                let has_entries = std::fs::read_dir(target_path)
                    .map(|mut r| r.next().is_some())
                    .unwrap_or(false);
                if has_entries {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "check_target_directory".to_string(),
                        reason: format!("Thư mục đích đã có dữ liệu: {}", target_path.display()),
                    });
                }
            } else {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "check_target_directory".to_string(),
                    reason: format!(
                        "Đường dẫn đích đã tồn tại tệp tin: {}",
                        target_path.display()
                    ),
                });
            }
        }
        Ok(())
    }

    /// Verifies that the path is not currently managed by OneDrive Known Folder Move.
    pub fn check_onedrive(path: &Path, shell_api: &dyn ShellFolderApi) -> Result<()> {
        if shell_api.is_onedrive_path(path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "check_onedrive".to_string(),
                reason: "Thư mục đang đồng bộ OneDrive".to_string(),
            });
        }
        Ok(())
    }
}

/// Candidate scanner discovering movable data matching all 4 relocation strategies.
pub struct CandidateScanner;

impl CandidateScanner {
    /// Computes recursive size and file count for a directory.
    pub fn compute_dir_size(dir: &Path) -> (u64, u64) {
        if !dir.exists() || !dir.is_dir() {
            return (0, 0);
        }

        let mut total_bytes = 0u64;
        let mut total_files = 0u64;
        let mut stack = vec![dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Ok(ft) = entry.file_type() {
                        if ft.is_dir() {
                            if !ft.is_symlink() {
                                stack.push(path);
                            }
                        } else if ft.is_file() {
                            total_files += 1;
                            if let Ok(meta) = entry.metadata() {
                                total_bytes += meta.len();
                            }
                        }
                    }
                }
            }
        }

        (total_bytes, total_files)
    }

    /// Discovers all candidates eligible for relocation.
    pub fn discover_all(
        target_drive: char,
        volume_api: &dyn Win32VolumeApi,
        shell_api: &dyn ShellFolderApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
    ) -> Vec<RelocationCandidate> {
        let mut results = Vec::new();
        let target_root = PathBuf::from(format!(
            "{}:\\GreenC\\Relocated",
            target_drive.to_ascii_uppercase()
        ));

        // 1. Shell Known Folders
        let known_folders = [
            ("downloads", "Downloads", FOLDERID_DOWNLOADS),
            ("documents", "Documents", FOLDERID_DOCUMENTS),
            ("pictures", "Pictures", FOLDERID_PICTURES),
            ("videos", "Videos", FOLDERID_VIDEOS),
            ("music", "Music", FOLDERID_MUSIC),
            ("desktop", "Desktop", FOLDERID_DESKTOP),
        ];

        for (id, name, guid) in &known_folders {
            if let Ok(path) = shell_api.get_known_folder(guid) {
                if path.exists() {
                    let (size, files) = Self::compute_dir_size(&path);
                    let is_onedrive = shell_api.is_onedrive_path(&path);
                    let is_locked = CandidateQualificationEngine::is_path_locked(&path, lock_probe);
                    let (eligible, reason) = if is_onedrive {
                        (false, Some("Thư mục đang đồng bộ OneDrive".to_string()))
                    } else if size == 0 {
                        (false, Some("Thư mục trống".to_string()))
                    } else if is_locked {
                        (
                            false,
                            Some("Tệp tin đang bị khóa bởi ứng dụng khác".to_string()),
                        )
                    } else {
                        (true, None)
                    };

                    let default_target =
                        PathBuf::from(format!("{}:\\{}", target_drive.to_ascii_uppercase(), name));

                    results.push(RelocationCandidate {
                        id: format!("known_folder_{}", id),
                        name: format!("Thư mục {}", name),
                        strategy: RelocationStrategyKind::KnownFolder,
                        source_path: path,
                        default_target_path: default_target,
                        size_bytes: size,
                        file_count: files,
                        is_locked,
                        locked_processes: Vec::new(),
                        how_description: "Chuyển vị trí chính thức của Windows Shell".to_string(),
                        is_advanced: false,
                        is_eligible: eligible,
                        ineligible_reason: reason,
                    });
                }
            }
        }

        // 2. Application Catalog (Telegram, Discord, Slack, CapCut, JetBrains)
        for profile in get_app_catalog() {
            if profile.source_path.exists() {
                let (size, files) = Self::compute_dir_size(&profile.source_path);
                if size > 0 {
                    let mut locked = false;
                    let mut running_procs = Vec::new();
                    for proc in &profile.process_names {
                        if proc_probe.is_process_running(&proc.to_lowercase()) {
                            locked = true;
                            running_procs.push(proc.clone());
                        }
                    }

                    if CandidateQualificationEngine::is_path_locked(
                        &profile.source_path,
                        lock_probe,
                    ) {
                        locked = true;
                    }

                    let default_target = target_root.join(&profile.app_id);
                    results.push(RelocationCandidate {
                        id: profile.app_id.clone(),
                        name: profile.name.clone(),
                        strategy: profile.strategy,
                        source_path: profile.source_path.clone(),
                        default_target_path: default_target,
                        size_bytes: size,
                        file_count: files,
                        is_locked: locked,
                        locked_processes: running_procs,
                        how_description: profile.how_description.clone(),
                        is_advanced: false,
                        is_eligible: true,
                        ineligible_reason: None,
                    });
                }
            }
        }

        // 3. Developer Tool Caches
        for tool in get_tool_catalog() {
            if tool.source_path.exists() {
                let (size, files) = Self::compute_dir_size(&tool.source_path);
                if size > 0 {
                    let is_locked =
                        CandidateQualificationEngine::is_path_locked(&tool.source_path, lock_probe);
                    let default_target = target_root.join(&tool.tool_id);
                    results.push(RelocationCandidate {
                        id: format!("tool_{}", tool.tool_id),
                        name: tool.name.clone(),
                        strategy: RelocationStrategyKind::ToolConfig,
                        source_path: tool.source_path.clone(),
                        default_target_path: default_target,
                        size_bytes: size,
                        file_count: files,
                        is_locked,
                        locked_processes: Vec::new(),
                        how_description: tool.how_description.clone(),
                        is_advanced: false,
                        is_eligible: true,
                        ineligible_reason: None,
                    });
                }
            }
        }

        // 4. Advanced User Folders in %APPDATA% and %LOCALAPPDATA% (>= 200 MiB)
        let mut app_dirs = Vec::new();
        if let Ok(appdata) = std::env::var("APPDATA") {
            app_dirs.push(PathBuf::from(appdata));
        }
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            app_dirs.push(PathBuf::from(local_appdata));
        }

        for base in app_dirs {
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let folder_name = entry.file_name().to_string_lossy().to_string();
                        let folder_lower = folder_name.to_lowercase();

                        // Skip protected system folders
                        if ADVANCED_SKIP_LIST
                            .iter()
                            .any(|skip| folder_lower == *skip || folder_lower.starts_with(skip))
                        {
                            continue;
                        }

                        // Avoid duplicate candidate if already captured in catalog
                        if results.iter().any(|r| r.source_path == path) {
                            continue;
                        }

                        let (size, files) = Self::compute_dir_size(&path);
                        if size >= ADVANCED_THRESHOLD_BYTES {
                            let is_locked =
                                CandidateQualificationEngine::is_path_locked(&path, lock_probe);
                            let default_target =
                                target_root.join(format!("Advanced_{}", folder_name));
                            results.push(RelocationCandidate {
                                id: format!("advanced_{}", folder_name.to_lowercase()),
                                name: format!("Thư mục {}", folder_name),
                                strategy: RelocationStrategyKind::Junction,
                                source_path: path,
                                default_target_path: default_target,
                                size_bytes: size,
                                file_count: files,
                                is_locked,
                                locked_processes: Vec::new(),
                                how_description:
                                    "Liên kết thư mục người dùng nâng cao (NTFS Junction)"
                                        .to_string(),
                                is_advanced: true,
                                is_eligible: true,
                                ineligible_reason: None,
                            });
                        }
                    }
                }
            }
        }

        // Validate MSIX AppData junction restrictions and free space against destination drive
        for cand in &mut results {
            if cand.is_eligible
                && cfg!(feature = "store")
                && matches!(
                    cand.strategy,
                    RelocationStrategyKind::Junction | RelocationStrategyKind::ToolConfig
                )
                && greenc_core::config::is_windows_appdata_app_dir(&cand.source_path)
            {
                cand.is_eligible = false;
                cand.ineligible_reason = Some(
                    "Bản Microsoft Store không tạo liên kết (NTFS Junction) trong AppData do cơ chế ảo hoá AppData của MSIX."
                        .to_string(),
                );
            }
            if cand.is_eligible {
                if let Err(e) = CandidateQualificationEngine::check_free_space_reserve(
                    target_drive,
                    cand.size_bytes,
                    volume_api,
                ) {
                    cand.is_eligible = false;
                    cand.ineligible_reason = Some(e.to_string());
                }
            }
        }

        results
    }
}
