//! Win32 Restart Manager (`Rstrtmgr.dll`) and Service Control Manager (`Advapi32.dll`) bindings
//! with deterministic RAII session and service handle cleanup (Task T054).

use crate::handle::path_to_wide;
use greenc_core::error::{GreenCError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_MORE_DATA;
use windows::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
    RM_PROCESS_INFO,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    ENUM_SERVICE_STATUS_PROCESSW, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATE_ALL,
    SERVICE_STATUS, SERVICE_WIN32,
};

/// Information about a process holding an active lock on a file or directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockingProcessInfo {
    pub pid: u32,
    pub app_name: String,
    pub process_name: String,
    pub service_short_name: String,
}

/// Trait abstracting Restart Manager and SCM PID-to-service queries for deterministic testing.
pub trait RestartManagerApi: Send + Sync {
    fn get_locking_processes(&self, paths: &[&Path]) -> Result<Vec<LockingProcessInfo>>;
    fn map_pids_to_services(&self) -> Result<HashMap<u32, (String, String)>>;
    fn is_service_running(&self, service_name: &str) -> Result<bool>;
}

/// RAII guard for a Windows Restart Manager session handle.
#[derive(Debug)]
pub struct RmSessionGuard {
    session_handle: u32,
}

impl RmSessionGuard {
    pub fn start() -> Result<Self> {
        let mut handle: u32 = 0;
        let mut session_key = [0u16; (CCH_RM_SESSION_KEY as usize) + 1];
        // SAFETY: `handle` points to a valid u32 and `session_key` has `CCH_RM_SESSION_KEY + 1`
        // wide characters as required by the Windows Restart Manager `RmStartSession` contract.
        let err = unsafe { RmStartSession(&mut handle, 0, PWSTR(session_key.as_mut_ptr())) };
        if err.0 != 0 {
            return Err(GreenCError::Win32 {
                code: err.0,
                message: "RmStartSession failed".to_string(),
            });
        }
        Ok(Self {
            session_handle: handle,
        })
    }

    pub const fn raw(&self) -> u32 {
        self.session_handle
    }
}

impl Drop for RmSessionGuard {
    fn drop(&mut self) {
        // SAFETY: `self.session_handle` was initialized by a successful `RmStartSession` call
        // and is ended exactly once when `RmSessionGuard` drops.
        unsafe {
            let _ = RmEndSession(self.session_handle);
        }
    }
}

/// RAII guard for a Windows Service Control Manager (`SC_HANDLE`).
#[derive(Debug)]
pub struct OwnedServiceHandle(SC_HANDLE);

impl OwnedServiceHandle {
    pub fn new(handle: SC_HANDLE) -> Option<Self> {
        if handle.is_invalid() {
            None
        } else {
            Some(Self(handle))
        }
    }

    pub const fn raw(&self) -> SC_HANDLE {
        self.0
    }
}

impl Drop for OwnedServiceHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a valid non-null `SC_HANDLE` returned by `OpenSCManagerW` or
            // `OpenServiceW` and closed deterministically on drop.
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }
}

fn wide_slice_to_trimmed_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

/// Queries the Windows Restart Manager to identify processes holding open handles to `paths`.
pub fn get_processes_locking_files_native(paths: &[&Path]) -> Result<Vec<LockingProcessInfo>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let session = RmSessionGuard::start()?;
    let wide_buffers: Vec<Vec<u16>> = paths.iter().map(|p| path_to_wide(p)).collect();
    let pcwstr_ptrs: Vec<PCWSTR> = wide_buffers
        .iter()
        .map(|buf| PCWSTR(buf.as_ptr()))
        .collect();

    // SAFETY: Every `PCWSTR` in `pcwstr_ptrs` points into a live null-terminated UTF-16 buffer
    // owned by `wide_buffers` for the entire duration of `RmRegisterResources`.
    let reg_err = unsafe { RmRegisterResources(session.raw(), Some(&pcwstr_ptrs), None, None) };
    if reg_err.0 != 0 {
        return Err(GreenCError::Win32 {
            code: reg_err.0,
            message: "RmRegisterResources failed".to_string(),
        });
    }

    let mut needed: u32 = 0;
    let mut count: u32 = 0;
    let mut reboot_reasons: u32 = 0;

    // Probe required count first, retrying if new processes lock the file concurrently.
    for _ in 0..4 {
        if needed == 0 {
            // SAFETY: Passing `None` with `count = 0` queries the required array length into `needed`.
            let probe_err = unsafe {
                RmGetList(
                    session.raw(),
                    &mut needed,
                    &mut count,
                    None,
                    &mut reboot_reasons,
                )
            };
            if probe_err.0 == 0 || needed == 0 {
                return Ok(Vec::new());
            }
            if probe_err != ERROR_MORE_DATA {
                return Err(GreenCError::Win32 {
                    code: probe_err.0,
                    message: "RmGetList size probe failed".to_string(),
                });
            }
        }

        let capacity = needed.saturating_add(4);
        count = capacity;
        let mut proc_infos = vec![RM_PROCESS_INFO::default(); capacity as usize];

        // SAFETY: `proc_infos` has `capacity` initialized `RM_PROCESS_INFO` elements matching `count`.
        let list_err = unsafe {
            RmGetList(
                session.raw(),
                &mut needed,
                &mut count,
                Some(proc_infos.as_mut_ptr()),
                &mut reboot_reasons,
            )
        };

        if list_err.0 == 0 {
            let mut results = Vec::with_capacity(count as usize);
            for info in proc_infos.into_iter().take(count as usize) {
                let app_name = wide_slice_to_trimmed_string(&info.strAppName);
                let svc_name = wide_slice_to_trimmed_string(&info.strServiceShortName);
                let proc_name = if app_name.to_ascii_lowercase().ends_with(".exe") {
                    app_name.clone()
                } else if !svc_name.is_empty() {
                    format!("svchost.exe ({svc_name})")
                } else {
                    format!("{app_name}.exe")
                };
                results.push(LockingProcessInfo {
                    pid: info.Process.dwProcessId,
                    app_name,
                    process_name: proc_name,
                    service_short_name: svc_name,
                });
            }
            return Ok(results);
        }

        if list_err != ERROR_MORE_DATA {
            return Err(GreenCError::Win32 {
                code: list_err.0,
                message: "RmGetList fetch failed".to_string(),
            });
        }
    }

    Ok(Vec::new())
}

/// Queries the Windows Service Control Manager to check whether `service_name` is `SERVICE_RUNNING`.
pub fn query_service_running_native(service_name: &str) -> Result<bool> {
    if service_name.trim().is_empty() {
        return Ok(false);
    }

    // SAFETY: `OpenSCManagerW` with null machine/database opens the local SCM with read-only connect rights.
    let scm_raw = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(|e| GreenCError::Win32 {
            code: (e.code().0 as u32) & 0xFFFF,
            message: format!("OpenSCManagerW failed: {e}"),
        })?;
    let Some(scm) = OwnedServiceHandle::new(scm_raw) else {
        return Ok(false);
    };

    let wide_name: Vec<u16> = service_name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `scm.raw()` is valid and `wide_name` is null-terminated UTF-16.
    let svc_raw = match unsafe {
        OpenServiceW(scm.raw(), PCWSTR(wide_name.as_ptr()), SERVICE_QUERY_STATUS)
    } {
        Ok(handle) => handle,
        Err(_) => return Ok(false),
    };
    let Some(svc) = OwnedServiceHandle::new(svc_raw) else {
        return Ok(false);
    };

    let mut status = SERVICE_STATUS::default();
    // SAFETY: `svc.raw()` is a valid service handle opened with `SERVICE_QUERY_STATUS`.
    unsafe { QueryServiceStatus(svc.raw(), &mut status) }.map_err(|e| GreenCError::Win32 {
        code: (e.code().0 as u32) & 0xFFFF,
        message: format!("QueryServiceStatus failed for {service_name}: {e}"),
    })?;

    Ok(status.dwCurrentState == SERVICE_RUNNING)
}

/// Enumerates active Win32 services and maps `PID -> (service_short_name, display_name)`.
pub fn map_pids_to_services_native() -> Result<HashMap<u32, (String, String)>> {
    // SAFETY: Opens local Service Control Manager with enumeration rights only.
    let scm_raw = match unsafe {
        OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
        )
    } {
        Ok(handle) => handle,
        Err(_) => return Ok(HashMap::new()),
    };
    let Some(scm) = OwnedServiceHandle::new(scm_raw) else {
        return Ok(HashMap::new());
    };

    let mut bytes_needed: u32 = 0;
    let mut services_returned: u32 = 0;
    let mut resume_handle: u32 = 0;

    // SAFETY: First call with `None` buffer probes required byte size into `bytes_needed`.
    let _ = unsafe {
        EnumServicesStatusExW(
            scm.raw(),
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut bytes_needed,
            &mut services_returned,
            Some(&mut resume_handle),
            PCWSTR::null(),
        )
    };

    if bytes_needed == 0 {
        return Ok(HashMap::new());
    }

    let mut buffer = vec![0u8; (bytes_needed + 4096) as usize];
    resume_handle = 0;
    // SAFETY: `buffer` is properly sized for `EnumServicesStatusExW` output records.
    let ok = unsafe {
        EnumServicesStatusExW(
            scm.raw(),
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            Some(&mut buffer),
            &mut bytes_needed,
            &mut services_returned,
            Some(&mut resume_handle),
            PCWSTR::null(),
        )
    };
    if ok.is_err() || services_returned == 0 {
        return Ok(HashMap::new());
    }

    let mut map = HashMap::new();
    // SAFETY: `EnumServicesStatusExW` populated `services_returned` contiguous
    // `ENUM_SERVICE_STATUS_PROCESSW` structs at the beginning of `buffer`, and all
    // embedded `PWSTR` pointers point into the trailing region of `buffer`.
    unsafe {
        let entries = std::slice::from_raw_parts(
            buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
            services_returned as usize,
        );
        for entry in entries {
            let pid = entry.ServiceStatusProcess.dwProcessId;
            if pid == 0 {
                continue;
            }
            let short_name = if entry.lpServiceName.is_null() {
                String::new()
            } else {
                entry.lpServiceName.to_string().unwrap_or_default()
            };
            let display_name = if entry.lpDisplayName.is_null() {
                short_name.clone()
            } else {
                entry
                    .lpDisplayName
                    .to_string()
                    .unwrap_or_else(|_| short_name.clone())
            };
            if !short_name.is_empty() {
                map.insert(pid, (short_name, display_name));
            }
        }
    }

    Ok(map)
}

/// Production implementation of `RestartManagerApi`.
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

/// Deterministic mock implementation of `RestartManagerApi` for automated tests.
#[derive(Default, Clone, Debug)]
pub struct MockRestartManagerApi {
    pub locked_by_path: Arc<Mutex<HashMap<PathBuf, Vec<LockingProcessInfo>>>>,
    pub pid_services: Arc<Mutex<HashMap<u32, (String, String)>>>,
    pub running_services: Arc<Mutex<HashMap<String, bool>>>,
}

impl MockRestartManagerApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_lock(&self, path: &Path, info: LockingProcessInfo) {
        self.locked_by_path
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_default()
            .push(info);
    }

    pub fn register_pid_service(&self, pid: u32, short_name: &str, display_name: &str) {
        self.pid_services
            .lock()
            .unwrap()
            .insert(pid, (short_name.to_string(), display_name.to_string()));
    }

    pub fn set_service_running(&self, short_name: &str, running: bool) {
        self.running_services
            .lock()
            .unwrap()
            .insert(short_name.to_ascii_lowercase(), running);
    }
}

impl RestartManagerApi for MockRestartManagerApi {
    fn get_locking_processes(&self, paths: &[&Path]) -> Result<Vec<LockingProcessInfo>> {
        let guard = self.locked_by_path.lock().unwrap();
        let mut out = Vec::new();
        for p in paths {
            if let Some(items) = guard.get(*p) {
                out.extend(items.iter().cloned());
            }
        }
        Ok(out)
    }

    fn map_pids_to_services(&self) -> Result<HashMap<u32, (String, String)>> {
        Ok(self.pid_services.lock().unwrap().clone())
    }

    fn is_service_running(&self, service_name: &str) -> Result<bool> {
        Ok(self
            .running_services
            .lock()
            .unwrap()
            .get(&service_name.to_ascii_lowercase())
            .copied()
            .unwrap_or(false))
    }
}
