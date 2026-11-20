use greenc_core::error::Result;
use greenc_core::models::DriveInfo;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ==========================================
// 1. Volume & Free Space Boundary
// ==========================================

pub trait Win32VolumeApi: Send + Sync {
    fn list_drives(&self) -> Result<Vec<DriveInfo>>;
    fn get_volumes(&self) -> Result<Vec<DriveInfo>> {
        self.list_drives()
    }
    fn get_disk_free_space(&self, root: &Path) -> Result<(u64, u64)>; // (total, free)
    fn get_free_space(&self, drive: char) -> Result<u64> {
        let root = PathBuf::from(format!("{}:\\", drive));
        let (_, free) = self.get_disk_free_space(&root)?;
        Ok(free)
    }
    fn get_volume_info(&self, root: &Path) -> Result<(String, String, u32)>; // (label, fs, drive_type)
    fn is_elevated(&self) -> bool {
        crate::volume::is_process_elevated()
    }
    fn probe_usn_capability(&self, drive_letter: char) -> crate::volume::UsnCapabilityStatus {
        crate::volume::probe_volume_usn_capability(drive_letter)
    }
}

// ==========================================
// 2. File Lock Probe Boundary
// ==========================================

pub trait FileLockProbe: Send + Sync {
    fn is_file_locked(&self, path: &Path) -> bool;
}

// ==========================================
// 3. Reparse Point & Junction Boundary
// ==========================================

pub trait ReparsePointApi: Send + Sync {
    fn is_reparse_point(&self, path: &Path) -> bool;
    fn create_junction(&self, link: &Path, target: &Path) -> Result<()>;
    fn get_junction_target(&self, link: &Path) -> Result<PathBuf>;
    fn delete_junction(&self, link: &Path) -> Result<()>;
}

// ==========================================
// 4. Shell Known Folders Boundary
// ==========================================

pub trait ShellFolderApi: Send + Sync {
    fn get_known_folder(&self, folder_guid: &str) -> Result<PathBuf>;
    fn set_known_folder(&self, folder_guid: &str, new_path: &Path) -> Result<()>;
    fn is_onedrive_path(&self, path: &Path) -> bool;
}

// ==========================================
// 5. Process & Windows Service Probe Boundary
// ==========================================

pub trait ProcessServiceProbe: Send + Sync {
    fn is_process_running(&self, exe_name: &str) -> bool;
    fn is_service_running(&self, service_name: &str) -> Result<bool>;
}

// ==========================================
// 6. External Command Runner Boundary
// ==========================================

pub trait CommandRunner: Send + Sync {
    fn run_command(&self, program: &str, args: &[&str]) -> Result<(i32, String, String)>; // (exit_code, stdout, stderr)
}

// ==========================================
// 7. Compaction & WOF Boundary
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WofAlgorithm {
    Xpress4k,
    Xpress8k,
    Xpress16k,
    Lzx,
    Lznt1,
}

impl WofAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Xpress4k => "XPRESS4K",
            Self::Xpress8k => "XPRESS8K",
            Self::Xpress16k => "XPRESS16K",
            Self::Lzx => "LZX",
            Self::Lznt1 => "LZNT1",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "XPRESS4K" => Self::Xpress4k,
            "XPRESS16K" => Self::Xpress16k,
            "LZX" => Self::Lzx,
            "LZNT1" => Self::Lznt1,
            _ => Self::Xpress8k,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub logical_bytes: u64,
    pub physical_bytes: u64,
    pub algorithm_used: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhdxCompactionResult {
    pub size_before_bytes: u64,
    pub size_after_bytes: u64,
}

pub trait CompactionApi: Send + Sync {
    fn compact_file(&self, path: &Path, algorithm: WofAlgorithm) -> Result<CompactionResult>;
    fn decompress_file(&self, path: &Path) -> Result<()>;
    fn get_compressed_size(&self, path: &Path) -> Result<u64>;
    fn compact_vhdx(&self, path: &Path) -> Result<VhdxCompactionResult>;
}

// =========================================================================
// MOCK IMPLEMENTATIONS (For User-Space Automated Tests & Downstream Crates)
// =========================================================================

#[derive(Default, Clone, Debug)]
pub struct MockVolumeApi {
    pub drives: Arc<Mutex<Vec<DriveInfo>>>,
    pub is_elevated: Arc<Mutex<bool>>,
    pub usn_status: Arc<Mutex<HashMap<char, crate::volume::UsnCapabilityStatus>>>,
}

impl MockVolumeApi {
    pub fn new(drives: Vec<DriveInfo>) -> Self {
        Self {
            drives: Arc::new(Mutex::new(drives)),
            is_elevated: Arc::new(Mutex::new(false)),
            usn_status: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_elevated(&self, elevated: bool) {
        *self.is_elevated.lock().unwrap() = elevated;
    }

    pub fn set_usn_status(&self, drive: char, status: crate::volume::UsnCapabilityStatus) {
        self.usn_status.lock().unwrap().insert(drive, status);
    }
}

impl Win32VolumeApi for MockVolumeApi {
    fn list_drives(&self) -> Result<Vec<DriveInfo>> {
        Ok(self.drives.lock().unwrap().clone())
    }

    fn get_disk_free_space(&self, root: &Path) -> Result<(u64, u64)> {
        let drives = self.drives.lock().unwrap();
        for d in drives.iter() {
            if d.root == root || root.starts_with(&d.root) || d.root.starts_with(root) {
                return Ok((d.total_bytes, d.free_bytes));
            }
        }
        if drives.len() == 1 {
            if let Some(d) = drives.first() {
                return Ok((d.total_bytes, d.free_bytes));
            }
        }
        Ok((100 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024))
    }

    fn get_volume_info(&self, root: &Path) -> Result<(String, String, u32)> {
        let drives = self.drives.lock().unwrap();
        for d in drives.iter() {
            if d.root == root {
                return Ok((d.label.clone(), d.filesystem.clone(), 3)); // 3 = DRIVE_FIXED
            }
        }
        Ok(("Data".to_string(), "NTFS".to_string(), 3))
    }

    fn is_elevated(&self) -> bool {
        *self.is_elevated.lock().unwrap()
    }

    fn probe_usn_capability(&self, drive_letter: char) -> crate::volume::UsnCapabilityStatus {
        self.usn_status
            .lock()
            .unwrap()
            .get(&drive_letter)
            .copied()
            .unwrap_or(crate::volume::UsnCapabilityStatus::InsufficientPrivileges)
    }
}

#[derive(Default, Clone, Debug)]
pub struct MockFileLockProbe {
    pub locked_paths: Arc<Mutex<std::collections::HashSet<PathBuf>>>,
}

impl MockFileLockProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock(&self, path: &Path) {
        self.locked_paths.lock().unwrap().insert(path.to_path_buf());
    }

    pub fn unlock(&self, path: &Path) {
        self.locked_paths.lock().unwrap().remove(path);
    }
}

impl FileLockProbe for MockFileLockProbe {
    fn is_file_locked(&self, path: &Path) -> bool {
        self.locked_paths.lock().unwrap().contains(path)
    }
}

#[derive(Default, Clone, Debug)]
pub struct MockReparsePointApi {
    pub junctions: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
    pub should_fail_create: Arc<Mutex<bool>>,
}

impl MockReparsePointApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_fail_create(&self, fail: bool) {
        *self.should_fail_create.lock().unwrap() = fail;
    }
}

impl ReparsePointApi for MockReparsePointApi {
    fn is_reparse_point(&self, path: &Path) -> bool {
        self.junctions.lock().unwrap().contains_key(path)
    }

    fn create_junction(&self, link: &Path, target: &Path) -> Result<()> {
        if *self.should_fail_create.lock().unwrap() {
            return Err(greenc_core::error::GreenCError::Win32 {
                code: 5,
                message: "Access Denied (simulated create_junction failure)".to_string(),
            });
        }
        self.junctions
            .lock()
            .unwrap()
            .insert(link.to_path_buf(), target.to_path_buf());
        Ok(())
    }

    fn get_junction_target(&self, link: &Path) -> Result<PathBuf> {
        self.junctions
            .lock()
            .unwrap()
            .get(link)
            .cloned()
            .ok_or_else(|| greenc_core::error::GreenCError::NotFound {
                entity: "Junction".to_string(),
                id: link.display().to_string(),
            })
    }

    fn delete_junction(&self, link: &Path) -> Result<()> {
        self.junctions.lock().unwrap().remove(link);
        Ok(())
    }
}

#[derive(Default, Clone, Debug)]
pub struct MockShellFolderApi {
    pub folders: Arc<Mutex<HashMap<String, PathBuf>>>,
    pub onedrive_paths: Arc<Mutex<Vec<PathBuf>>>,
    pub should_fail_set: Arc<Mutex<bool>>,
}

impl MockShellFolderApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_folder(&self, guid: &str, path: PathBuf) {
        self.folders.lock().unwrap().insert(guid.to_string(), path);
    }

    pub fn add_onedrive_path(&self, path: PathBuf) {
        self.onedrive_paths.lock().unwrap().push(path);
    }

    pub fn set_fail_set(&self, fail: bool) {
        *self.should_fail_set.lock().unwrap() = fail;
    }
}

impl ShellFolderApi for MockShellFolderApi {
    fn get_known_folder(&self, folder_guid: &str) -> Result<PathBuf> {
        self.folders
            .lock()
            .unwrap()
            .get(folder_guid)
            .cloned()
            .ok_or_else(|| greenc_core::error::GreenCError::NotFound {
                entity: "KnownFolder".to_string(),
                id: folder_guid.to_string(),
            })
    }

    fn set_known_folder(&self, folder_guid: &str, new_path: &Path) -> Result<()> {
        if *self.should_fail_set.lock().unwrap() {
            return Err(greenc_core::error::GreenCError::Win32 {
                code: 5,
                message: "Access Denied (simulated set_known_folder failure)".to_string(),
            });
        }
        self.folders
            .lock()
            .unwrap()
            .insert(folder_guid.to_string(), new_path.to_path_buf());
        Ok(())
    }

    fn is_onedrive_path(&self, path: &Path) -> bool {
        let list = self.onedrive_paths.lock().unwrap();
        list.iter().any(|p| path == p || path.starts_with(p))
    }
}

#[derive(Default, Clone, Debug)]
pub struct MockCompactionApi {
    pub compressed_files: Arc<Mutex<HashMap<PathBuf, CompactionResult>>>,
    pub compression_ratio: Arc<Mutex<f64>>,
    pub should_fail_compact: Arc<Mutex<bool>>,
    pub should_fail_decompress: Arc<Mutex<bool>>,
}

impl MockCompactionApi {
    pub fn new() -> Self {
        Self {
            compressed_files: Arc::new(Mutex::new(HashMap::new())),
            compression_ratio: Arc::new(Mutex::new(0.50)),
            should_fail_compact: Arc::new(Mutex::new(false)),
            should_fail_decompress: Arc::new(Mutex::new(false)),
        }
    }

    pub fn set_fail_decompress(&self, fail: bool) {
        *self.should_fail_decompress.lock().unwrap() = fail;
    }

    pub fn set_compression_ratio(&self, ratio: f64) {
        *self.compression_ratio.lock().unwrap() = ratio;
    }

    pub fn set_fail_compact(&self, fail: bool) {
        *self.should_fail_compact.lock().unwrap() = fail;
    }
}

impl CompactionApi for MockCompactionApi {
    fn compact_file(&self, path: &Path, algorithm: WofAlgorithm) -> Result<CompactionResult> {
        if *self.should_fail_compact.lock().unwrap() {
            return Err(greenc_core::error::GreenCError::Win32 {
                code: 50,
                message: "Not Supported (simulated compaction failure)".to_string(),
            });
        }
        let logical = std::fs::metadata(path)
            .map(|m| m.len())
            .unwrap_or(1024 * 1024);
        let ratio = *self.compression_ratio.lock().unwrap();
        let physical = (logical as f64 * ratio).round() as u64;

        let res = CompactionResult {
            logical_bytes: logical,
            physical_bytes: physical,
            algorithm_used: algorithm.as_str().to_string(),
        };

        self.compressed_files
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), res.clone());
        Ok(res)
    }

    fn decompress_file(&self, path: &Path) -> Result<()> {
        if *self.should_fail_decompress.lock().unwrap() {
            return Err(greenc_core::error::GreenCError::Win32 {
                code: 5,
                message: "Access denied (simulated decompression failure)".to_string(),
            });
        }
        self.compressed_files.lock().unwrap().remove(path);
        Ok(())
    }

    fn get_compressed_size(&self, path: &Path) -> Result<u64> {
        let files = self.compressed_files.lock().unwrap();
        if let Some(res) = files.get(path) {
            Ok(res.physical_bytes)
        } else {
            Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        }
    }

    fn compact_vhdx(&self, path: &Path) -> Result<VhdxCompactionResult> {
        let size_before = std::fs::metadata(path)
            .map(|m| m.len())
            .unwrap_or(10 * 1024 * 1024 * 1024);
        let size_after = (size_before as f64 * 0.70) as u64;
        Ok(VhdxCompactionResult {
            size_before_bytes: size_before,
            size_after_bytes: size_after,
        })
    }
}

#[derive(Default, Clone, Debug)]
pub struct MockProcessServiceProbe {
    pub running_processes: Arc<Mutex<std::collections::HashSet<String>>>,
    pub running_services: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl MockProcessServiceProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_process_running(&self, exe_name: &str, running: bool) {
        let mut procs = self.running_processes.lock().unwrap();
        if running {
            procs.insert(exe_name.to_lowercase());
        } else {
            procs.remove(&exe_name.to_lowercase());
        }
    }

    pub fn set_service_running(&self, service_name: &str, running: bool) {
        let mut svcs = self.running_services.lock().unwrap();
        if running {
            svcs.insert(service_name.to_lowercase());
        } else {
            svcs.remove(&service_name.to_lowercase());
        }
    }
}

impl ProcessServiceProbe for MockProcessServiceProbe {
    fn is_process_running(&self, exe_name: &str) -> bool {
        self.running_processes
            .lock()
            .unwrap()
            .contains(&exe_name.to_lowercase())
    }

    fn is_service_running(&self, service_name: &str) -> Result<bool> {
        Ok(self
            .running_services
            .lock()
            .unwrap()
            .contains(&service_name.to_lowercase()))
    }
}

/// Native implementation of ProcessServiceProbe using ToolHelp32 snapshot APIs.
#[derive(Default, Clone, Debug)]
pub struct NativeProcessServiceProbe;

impl ProcessServiceProbe for NativeProcessServiceProbe {
    fn is_process_running(&self, exe_name: &str) -> bool {
        let target = exe_name.to_lowercase();
        // SAFETY: `CreateToolhelp32Snapshot` returns a snapshot `HANDLE` wrapped immediately in
        // `OwnedHandle` so `CloseHandle` runs deterministically on every return path, and `entry`
        // has `dwSize` initialized to `size_of::<PROCESSENTRY32W>()` per Win32 contract.
        unsafe {
            let Ok(raw_handle) =
                windows::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot(
                    windows::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS,
                    0,
                )
            else {
                return false;
            };
            let Some(snap_guard) = crate::handle::OwnedHandle::new(raw_handle) else {
                return false;
            };
            let mut entry = windows::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W {
                dwSize: std::mem::size_of::<
                    windows::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W,
                >() as u32,
                ..Default::default()
            };
            if windows::Win32::System::Diagnostics::ToolHelp::Process32FirstW(
                snap_guard.raw(),
                &mut entry,
            )
            .is_ok()
            {
                loop {
                    let name_len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name =
                        String::from_utf16_lossy(&entry.szExeFile[..name_len]).to_lowercase();
                    if name == target {
                        return true;
                    }
                    if windows::Win32::System::Diagnostics::ToolHelp::Process32NextW(
                        snap_guard.raw(),
                        &mut entry,
                    )
                    .is_err()
                    {
                        break;
                    }
                }
            }
        }
        false
    }

    fn is_service_running(&self, service_name: &str) -> Result<bool> {
        crate::restart_mgr::query_service_running_native(service_name)
    }
}

pub use crate::handle::NativeFileLockProbe;
pub use crate::known_folders::NativeShellFolderApi;
pub use crate::reparse::NativeReparsePointApi;
pub use crate::volume::NativeWin32VolumeApi;
pub use crate::wof_compact::NativeCompactionApi;
