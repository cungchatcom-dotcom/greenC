//! Virtual disk (.vhdx) slack compaction service (Tasks T050, T051).

use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{CompactionKind, CompactionRecord};
use greenc_storage::db::GreenCDb;
use greenc_win32::traits::{CompactionApi, ProcessServiceProbe};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const WSL_DOCKER_PROCESSES: &[&str] = &[
    "wsl.exe",
    "wslhost.exe",
    "vmmem.exe",
    "vmmemwsl.exe",
    "vmwp.exe",
    "docker.exe",
    "docker desktop.exe",
    "com.docker.backend.exe",
    "com.docker.proxy.exe",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VhdxImageCandidate {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub is_locked: bool,
    pub running_processes: Vec<String>,
}

pub struct VhdxCompactor;

impl VhdxCompactor {
    /// Discovers common WSL2 and Docker Desktop `.vhdx` images.
    pub fn discover_vhdx_images(proc_probe: &dyn ProcessServiceProbe) -> Vec<VhdxImageCandidate> {
        let mut results = Vec::new();

        let mut running = Vec::new();
        for proc in WSL_DOCKER_PROCESSES {
            let proc_lower = proc.to_lowercase();
            if proc_probe.is_process_running(&proc_lower) {
                running.push((*proc).to_string());
            }
        }
        let is_locked = !running.is_empty();

        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            let base = PathBuf::from(local_appdata);

            // Docker Desktop WSL2 data disk
            let docker_vhdx = base
                .join("Docker")
                .join("wsl")
                .join("data")
                .join("ext4.vhdx");
            if docker_vhdx.exists() {
                let size = fs::metadata(&docker_vhdx).map(|m| m.len()).unwrap_or(0);
                results.push(VhdxImageCandidate {
                    path: docker_vhdx,
                    name: "Docker Desktop WSL2 Data".to_string(),
                    size_bytes: size,
                    is_locked,
                    running_processes: running.clone(),
                });
            }

            // Standard WSL2 Distro Packages
            let packages_dir = base.join("Packages");
            if packages_dir.exists() && packages_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&packages_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            let vhdx = path.join("LocalState").join("ext4.vhdx");
                            if vhdx.exists() {
                                let size = fs::metadata(&vhdx).map(|m| m.len()).unwrap_or(0);
                                let pkg_name = entry.file_name().to_string_lossy().to_string();
                                results.push(VhdxImageCandidate {
                                    path: vhdx,
                                    name: format!("WSL2 Linux Distro ({})", pkg_name),
                                    size_bytes: size,
                                    is_locked,
                                    running_processes: running.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }

        results
    }

    /// Verifies virtual machine processes are stopped, then compacts the virtual disk image.
    pub fn compact_vhdx_image(
        path: &Path,
        db: &GreenCDb,
        compaction_api: &dyn CompactionApi,
        proc_probe: &dyn ProcessServiceProbe,
    ) -> Result<CompactionRecord> {
        if !path.exists() {
            return Err(GreenCError::NotFound {
                entity: "VhdxImage".to_string(),
                id: path.display().to_string(),
            });
        }

        // Verify WSL2 / Docker processes are not running
        let mut active = Vec::new();
        for proc in WSL_DOCKER_PROCESSES {
            let proc_lower = proc.to_lowercase();
            if proc_probe.is_process_running(&proc_lower) {
                active.push(*proc);
            }
        }

        if !active.is_empty() {
            return Err(GreenCError::PreconditionBlocked {
                operation: "compact_vhdx".to_string(),
                reason: format!(
                    "Cần đóng WSL2 / Docker (chạy 'wsl --shutdown' trong PowerShell) trước khi nén ổ đĩa ảo: đang chạy {}",
                    active.join(", ")
                ),
            });
        }

        let res = compaction_api.compact_vhdx(path)?;

        let mut record = CompactionRecord {
            id: None,
            target_path: path.to_path_buf(),
            kind: CompactionKind::VhdxTrim,
            algorithm: "VIRTDISK".to_string(),
            logical_bytes: res.size_before_bytes,
            physical_before_bytes: res.size_before_bytes,
            physical_after_bytes: res.size_after_bytes,
            file_count: 1,
            status: "compacted".to_string(),
            updated_at: Utc::now(),
        };

        let id = db.save_compaction_record(&record)?;
        record.id = Some(id);

        Ok(record)
    }
}
