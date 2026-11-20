//! Unix process & service locking probe (`RestartManagerApi` equivalent).

use greenc_core::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Information about a process holding an active lock on a file or directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockingProcessInfo {
    pub pid: u32,
    pub app_name: String,
    pub process_name: String,
    pub service_short_name: String,
}

/// Trait abstracting locking-process and service queries for deterministic testing.
pub trait RestartManagerApi: Send + Sync {
    fn get_locking_processes(&self, paths: &[&Path]) -> Result<Vec<LockingProcessInfo>>;
    fn map_pids_to_services(&self) -> Result<HashMap<u32, (String, String)>>;
    fn is_service_running(&self, service_name: &str) -> Result<bool>;
}

/// Queries processes that currently have any of `paths` open using `lsof` when available.
pub fn get_processes_locking_files_native(paths: &[&Path]) -> Result<Vec<LockingProcessInfo>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut cmd = std::process::Command::new("lsof");
    cmd.args(["-F", "pc"]);
    for p in paths {
        if p.exists() {
            cmd.arg(p);
        }
    }

    let Ok(output) = cmd.output() else {
        return Ok(Vec::new());
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let mut current_pid: Option<u32> = None;

    for line in stdout.lines() {
        if let Some(pid_str) = line.strip_prefix('p') {
            current_pid = pid_str.parse::<u32>().ok();
        } else if let Some(cmd_name) = line.strip_prefix('c') {
            if let Some(pid) = current_pid {
                if !results.iter().any(|r: &LockingProcessInfo| r.pid == pid) {
                    results.push(LockingProcessInfo {
                        pid,
                        app_name: cmd_name.to_string(),
                        process_name: cmd_name.to_string(),
                        service_short_name: String::new(),
                    });
                }
            }
        }
    }

    Ok(results)
}

/// Maps PIDs to system services (empty best-effort map on Unix).
pub fn map_pids_to_services_native() -> Result<HashMap<u32, (String, String)>> {
    Ok(HashMap::new())
}

/// Checks if a system service is active via `launchctl` (macOS) or `systemctl` (Linux).
pub fn query_service_running_native(service_name: &str) -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("launchctl")
            .args(["list", service_name])
            .output();
        Ok(out.map(|o| o.status.success()).unwrap_or(false))
    }

    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", service_name])
            .output();
        Ok(out.map(|o| o.status.success()).unwrap_or(false))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = service_name;
        Ok(false)
    }
}

/// Native Unix implementation of `RestartManagerApi`.
#[derive(Default, Clone, Debug)]
pub struct NativeRestartManagerApi;

impl RestartManagerApi for NativeRestartManagerApi {
    fn get_locking_processes(&self, paths: &[&Path]) -> Result<Vec<LockingProcessInfo>> {
        get_processes_locking_files_native(paths)
    }

    fn map_pids_to_services(&self) -> Result<HashMap<u32, (String, String)>> {
        map_pids_to_services_native()
    }

    fn is_service_running(&self, service_name: &str) -> Result<bool> {
        query_service_running_native(service_name)
    }
}

/// Deterministic mock implementation of `RestartManagerApi`.
#[derive(Default, Clone, Debug)]
pub struct MockRestartManagerApi {
    pub locked_by_path: Arc<Mutex<HashMap<PathBuf, Vec<LockingProcessInfo>>>>,
    pub pid_services: Arc<Mutex<HashMap<u32, (String, String)>>>,
    pub running_services: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl MockRestartManagerApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_lock(&self, path: PathBuf, info: LockingProcessInfo) {
        self.locked_by_path
            .lock()
            .unwrap()
            .entry(path)
            .or_default()
            .push(info);
    }

    pub fn add_pid_service(&self, pid: u32, short_name: String, display_name: String) {
        self.pid_services
            .lock()
            .unwrap()
            .insert(pid, (short_name, display_name));
    }

    pub fn set_service_running(&self, service_name: &str, running: bool) {
        let mut set = self.running_services.lock().unwrap();
        if running {
            set.insert(service_name.to_ascii_lowercase());
        } else {
            set.remove(&service_name.to_ascii_lowercase());
        }
    }
}

impl RestartManagerApi for MockRestartManagerApi {
    fn get_locking_processes(&self, paths: &[&Path]) -> Result<Vec<LockingProcessInfo>> {
        let map = self.locked_by_path.lock().unwrap();
        let mut results = Vec::new();
        for p in paths {
            if let Some(entries) = map.get(*p) {
                results.extend(entries.clone());
            }
        }
        Ok(results)
    }

    fn map_pids_to_services(&self) -> Result<HashMap<u32, (String, String)>> {
        Ok(self.pid_services.lock().unwrap().clone())
    }

    fn is_service_running(&self, service_name: &str) -> Result<bool> {
        Ok(self
            .running_services
            .lock()
            .unwrap()
            .contains(&service_name.to_ascii_lowercase()))
    }
}
