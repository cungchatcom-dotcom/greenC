//! Application lifecycle control for safe relocation (graceful Restart Manager shutdown,
//! active call window detection, exact running-executable discovery, and reopen).
//!
//! Safety guarantees:
//! - NEVER force-kills (`RmForceShutdown` / `TerminateProcess` / `SIGKILL` are strictly forbidden).
//! - Refuses shutdown if a call window (`ZaloCall.exe` visible window) is active.
//! - Uses Windows Restart Manager (`RmStartSession` + `RmRegisterResources` + `RmShutdown(0)`)
//!   with a bounded wait (default <= 30s), falling back to a clear user instruction if the app
//!   remains open in the system tray.

use greenc_core::error::{GreenCError, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default maximum wait time for graceful Restart Manager shutdown (30 seconds).
pub const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Plain Vietnamese guidance returned when Zalo cannot be closed gracefully automatically.
pub const ZALO_MANUAL_QUIT_MESSAGE: &str =
    "Hãy thoát Zalo từ biểu tượng ở góc màn hình rồi bấm Thử lại";

/// Plain Vietnamese refusal message when an active Zalo call window is detected.
pub const ZALO_ACTIVE_CALL_MESSAGE: &str =
    "Zalo đang có cuộc gọi, không thể đóng để chuyển dữ liệu";

/// Handle returned after a successful graceful shutdown so the application can be reopened
/// cleanly via `RmRestart` or by relaunching the exact executable that was running.
#[derive(Debug, Default)]
pub struct AppShutdownSession {
    /// Executable paths that were running before shutdown (e.g. `...\Zalo-26.11.12\Zalo.exe`).
    pub captured_exes: Vec<PathBuf>,
    /// Whether Restart Manager was used to shut down the app.
    pub used_restart_manager: bool,
    #[cfg(windows)]
    rm_guard: Option<crate::win32::restart_mgr::RmSessionGuard>,
}

impl AppShutdownSession {
    pub fn no_op() -> Self {
        Self::default()
    }

    pub fn with_captured_exes(captured_exes: Vec<PathBuf>, used_restart_manager: bool) -> Self {
        Self {
            captured_exes,
            used_restart_manager,
            #[cfg(windows)]
            rm_guard: None,
        }
    }
}

/// Cross-platform, mockable boundary for inspecting running app executables, detecting visible
/// call windows, gracefully shutting down an app via Restart Manager, and reopening it.
pub trait AppLifecycleApi: Send + Sync {
    /// Returns full executable paths for currently running processes matching `exe_names`
    /// (case-insensitive).
    fn find_running_exe_paths(&self, exe_names: &[&str]) -> Vec<PathBuf>;

    /// Returns parent directories of currently running processes matching `exe_names`.
    fn find_running_exe_dirs(&self, exe_names: &[&str]) -> HashSet<PathBuf> {
        self.find_running_exe_paths(exe_names)
            .into_iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect()
    }

    /// Returns `true` if any process matching `exe_names` owns a currently visible top-level window
    /// (used to detect an active `ZaloCall.exe` call).
    fn has_visible_window(&self, exe_names: &[&str]) -> bool;

    /// Gracefully requests shutdown of applications locking `resource_paths` (using Restart
    /// Manager `RmShutdown` with NO force flag on Windows) and waits up to `timeout` until all
    /// processes matching `process_names` exit. NEVER force-kills.
    fn graceful_shutdown(
        &self,
        resource_paths: &[PathBuf],
        process_names: &[&str],
        timeout: Duration,
    ) -> Result<AppShutdownSession>;

    /// Reopens the application after a successful move (via `RmRestart` if available, else by
    /// relaunching the exact captured executable path). Returns `Ok(true)` if reopened.
    fn restart_app(&self, session: &mut AppShutdownSession) -> Result<bool>;
}

/// Production implementation backed by Win32 ToolHelp32 / Restart Manager / User32 on Windows,
/// and read-only `/proc` inspection on Unix.
#[derive(Default, Clone, Debug)]
pub struct NativeAppLifecycleApi;

#[cfg(windows)]
mod win_impl {
    use super::*;
    use crate::handle::{path_to_wide, OwnedHandle};
    use crate::win32::restart_mgr::RmSessionGuard;
    use std::time::Instant;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::RestartManager::{RmRegisterResources, RmRestart, RmShutdown};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    fn query_process_image_path(pid: u32) -> Option<PathBuf> {
        // SAFETY: `OpenProcess` with read-only `PROCESS_QUERY_LIMITED_INFORMATION` returns a
        // handle immediately wrapped in `OwnedHandle` so `CloseHandle` always runs.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let guard = OwnedHandle::new(raw)?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` is writable UTF-16 storage of length `len`.
        unsafe {
            QueryFullProcessImageNameW(
                guard.raw(),
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        }
        .ok()?;
        let s = String::from_utf16_lossy(&buf[..len as usize]);
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }

    pub fn find_running_exe_paths_native(exe_names: &[&str]) -> Vec<PathBuf> {
        let targets: HashSet<String> = exe_names
            .iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();
        // SAFETY: Snapshot handle wrapped immediately in `OwnedHandle`; `dwSize` initialized.
        unsafe {
            let Ok(raw_snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            let Some(snap) = OwnedHandle::new(raw_snap) else {
                return out;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap.raw(), &mut entry).is_ok() {
                loop {
                    let end = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..end])
                        .trim()
                        .to_ascii_lowercase();
                    if targets.contains(&name) {
                        if let Some(full) = query_process_image_path(entry.th32ProcessID) {
                            if !out.contains(&full) {
                                out.push(full);
                            }
                        }
                    }
                    if Process32NextW(snap.raw(), &mut entry).is_err() {
                        break;
                    }
                }
            }
        }
        out
    }

    struct EnumVisibleCtx {
        targets: HashSet<String>,
        found: bool,
    }

    unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut EnumVisibleCtx);
        if IsWindowVisible(hwnd).as_bool() {
            let mut pid: u32 = 0;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid != 0 {
                if let Some(img) = query_process_image_path(pid) {
                    if let Some(fname) = img.file_name().and_then(|n| n.to_str()) {
                        if ctx.targets.contains(&fname.to_ascii_lowercase()) {
                            ctx.found = true;
                            return BOOL(0); // stop enumeration
                        }
                    }
                }
            }
        }
        BOOL(1)
    }

    pub fn has_visible_window_native(exe_names: &[&str]) -> bool {
        let targets: HashSet<String> = exe_names
            .iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        if targets.is_empty() {
            return false;
        }
        let mut ctx = EnumVisibleCtx {
            targets,
            found: false,
        };
        let ptr = &mut ctx as *mut EnumVisibleCtx as isize;
        // SAFETY: `EnumWindows` synchronously invokes `enum_windows_cb` on the current thread
        // before returning, so `&mut ctx` stays alive for the entire call.
        unsafe {
            let _ = EnumWindows(Some(enum_windows_cb), LPARAM(ptr));
        }
        ctx.found
    }

    fn collect_rm_resource_files(
        resource_paths: &[PathBuf],
        running_exes: &[PathBuf],
    ) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for exe in running_exes {
            if exe.is_file() && !files.contains(exe) {
                files.push(exe.clone());
            }
        }
        for root in resource_paths {
            if files.len() >= 128 {
                break;
            }
            if root.is_file() {
                if !files.contains(root) {
                    files.push(root.clone());
                }
            } else if root.is_dir() {
                // Register immediate files and `message/**` (up to 1 subdir deep) so Restart
                // Manager identifies all Zalo processes holding handles to account DB files.
                let mut scan_dirs = vec![root.clone()];
                let msg_dir = root.join("message");
                if msg_dir.is_dir() {
                    scan_dirs.push(msg_dir.clone());
                    if let Ok(rd) = std::fs::read_dir(&msg_dir) {
                        for entry in rd.flatten() {
                            let p = entry.path();
                            if p.is_dir() {
                                scan_dirs.push(p);
                            }
                        }
                    }
                }
                for sub in scan_dirs {
                    if files.len() >= 128 {
                        break;
                    }
                    if let Ok(rd) = std::fs::read_dir(&sub) {
                        for entry in rd.flatten() {
                            if files.len() >= 128 {
                                break;
                            }
                            let p = entry.path();
                            if p.is_file() && !files.contains(&p) {
                                files.push(p);
                            }
                        }
                    }
                }
            }
        }
        files
    }

    pub fn graceful_shutdown_native(
        resource_paths: &[PathBuf],
        process_names: &[&str],
        timeout: Duration,
    ) -> Result<AppShutdownSession> {
        let captured_exes = find_running_exe_paths_native(process_names);
        if captured_exes.is_empty() {
            return Ok(AppShutdownSession::no_op());
        }

        let rm_files = collect_rm_resource_files(resource_paths, &captured_exes);
        let session = RmSessionGuard::start()?;
        let wide_bufs: Vec<Vec<u16>> = rm_files.iter().map(|p| path_to_wide(p)).collect();
        let ptrs: Vec<PCWSTR> = wide_bufs.iter().map(|b| PCWSTR(b.as_ptr())).collect();

        if !ptrs.is_empty() {
            // SAFETY: Every `PCWSTR` points into `wide_bufs` alive for `RmRegisterResources`.
            let reg_err = unsafe { RmRegisterResources(session.raw(), Some(&ptrs), None, None) };
            if reg_err.0 != 0 {
                return Err(GreenCError::FileLocked {
                    path: resource_paths.first().cloned().unwrap_or_default(),
                    reason: ZALO_MANUAL_QUIT_MESSAGE.to_string(),
                });
            }
        }

        // SAFETY: `lActionFlags = 0` requests graceful shutdown ONLY (NEVER `RmForceShutdown`).
        let shut_err = unsafe { RmShutdown(session.raw(), 0, None) };
        if shut_err.0 != 0 {
            return Err(GreenCError::FileLocked {
                path: resource_paths.first().cloned().unwrap_or_default(),
                reason: ZALO_MANUAL_QUIT_MESSAGE.to_string(),
            });
        }

        let deadline = Instant::now() + timeout;
        loop {
            if find_running_exe_paths_native(process_names).is_empty() {
                return Ok(AppShutdownSession {
                    captured_exes,
                    used_restart_manager: true,
                    rm_guard: Some(session),
                });
            }
            if Instant::now() >= deadline {
                return Err(GreenCError::FileLocked {
                    path: resource_paths.first().cloned().unwrap_or_default(),
                    reason: ZALO_MANUAL_QUIT_MESSAGE.to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn restart_app_native(session: &mut AppShutdownSession) -> Result<bool> {
        let mut restarted = false;
        if let Some(ref rm_guard) = session.rm_guard {
            // SAFETY: `rm_guard.raw()` is a live Restart Manager session that performed `RmShutdown`.
            let err = unsafe { RmRestart(rm_guard.raw(), 0, None) };
            if err.0 == 0 {
                restarted = true;
            }
        }
        // Drop RM session cleanly after attempting RmRestart.
        session.rm_guard = None;

        if !restarted {
            // Fallback: relaunch the exact primary `Zalo.exe` captured before shutdown.
            if let Some(primary_exe) = session
                .captured_exes
                .iter()
                .find(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("zalo.exe"))
                })
                .or_else(|| session.captured_exes.first())
            {
                if primary_exe.is_file() {
                    let _ = std::process::Command::new(primary_exe).spawn()?;
                    restarted = true;
                }
            }
        }
        Ok(restarted)
    }
}

impl AppLifecycleApi for NativeAppLifecycleApi {
    fn find_running_exe_paths(&self, exe_names: &[&str]) -> Vec<PathBuf> {
        #[cfg(windows)]
        {
            win_impl::find_running_exe_paths_native(exe_names)
        }
        #[cfg(not(windows))]
        {
            let _ = exe_names;
            Vec::new()
        }
    }

    fn has_visible_window(&self, exe_names: &[&str]) -> bool {
        #[cfg(windows)]
        {
            win_impl::has_visible_window_native(exe_names)
        }
        #[cfg(not(windows))]
        {
            let _ = exe_names;
            false
        }
    }

    fn graceful_shutdown(
        &self,
        resource_paths: &[PathBuf],
        process_names: &[&str],
        timeout: Duration,
    ) -> Result<AppShutdownSession> {
        #[cfg(windows)]
        {
            win_impl::graceful_shutdown_native(resource_paths, process_names, timeout)
        }
        #[cfg(not(windows))]
        {
            let _ = (resource_paths, process_names, timeout);
            Ok(AppShutdownSession::no_op())
        }
    }

    fn restart_app(&self, session: &mut AppShutdownSession) -> Result<bool> {
        #[cfg(windows)]
        {
            win_impl::restart_app_native(session)
        }
        #[cfg(not(windows))]
        {
            let _ = session;
            Ok(false)
        }
    }
}

/// Configurable shutdown outcome for [`MockAppLifecycleApi`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MockShutdownBehavior {
    #[default]
    Succeed,
    Timeout,
    Fail(String),
}

type RestartCallback = Box<dyn Fn() + Send + Sync>;

/// Deterministic mock implementation of [`AppLifecycleApi`] for Linux/CI unit & integration tests.
#[derive(Default, Clone)]
pub struct MockAppLifecycleApi {
    pub running_exes: Arc<Mutex<Vec<PathBuf>>>,
    pub visible_window_exes: Arc<Mutex<HashSet<String>>>,
    pub shutdown_behavior: Arc<Mutex<MockShutdownBehavior>>,
    pub shutdown_calls: Arc<Mutex<usize>>,
    pub restart_calls: Arc<Mutex<usize>>,
    pub restarted_exes: Arc<Mutex<Vec<PathBuf>>>,
    on_restart: Arc<Mutex<Option<RestartCallback>>>,
}

impl std::fmt::Debug for MockAppLifecycleApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockAppLifecycleApi")
            .field("running_exes", &self.running_exes)
            .field("visible_window_exes", &self.visible_window_exes)
            .field("shutdown_behavior", &self.shutdown_behavior)
            .finish()
    }
}

impl MockAppLifecycleApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_running_exe(&self, exe_path: PathBuf) {
        self.running_exes.lock().unwrap().push(exe_path);
    }

    pub fn clear_running_exes(&self) {
        self.running_exes.lock().unwrap().clear();
    }

    pub fn set_visible_window(&self, exe_name: &str, visible: bool) {
        let mut set = self.visible_window_exes.lock().unwrap();
        if visible {
            set.insert(exe_name.to_ascii_lowercase());
        } else {
            set.remove(&exe_name.to_ascii_lowercase());
        }
    }

    pub fn set_shutdown_behavior(&self, behavior: MockShutdownBehavior) {
        *self.shutdown_behavior.lock().unwrap() = behavior;
    }

    /// Registers a hook invoked immediately when `restart_app` succeeds (e.g. to simulate
    /// a junction breaking after Zalo reopens in tests).
    pub fn set_on_restart_hook<F: Fn() + Send + Sync + 'static>(&self, f: F) {
        *self.on_restart.lock().unwrap() = Some(Box::new(f));
    }

    pub fn shutdown_count(&self) -> usize {
        *self.shutdown_calls.lock().unwrap()
    }

    pub fn restart_count(&self) -> usize {
        *self.restart_calls.lock().unwrap()
    }
}

impl AppLifecycleApi for MockAppLifecycleApi {
    fn find_running_exe_paths(&self, exe_names: &[&str]) -> Vec<PathBuf> {
        let targets: HashSet<String> = exe_names.iter().map(|s| s.to_ascii_lowercase()).collect();
        self.running_exes
            .lock()
            .unwrap()
            .iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| targets.contains(&n.to_ascii_lowercase()))
            })
            .cloned()
            .collect()
    }

    fn has_visible_window(&self, exe_names: &[&str]) -> bool {
        let vis = self.visible_window_exes.lock().unwrap();
        exe_names
            .iter()
            .any(|n| vis.contains(&n.to_ascii_lowercase()))
    }

    fn graceful_shutdown(
        &self,
        resource_paths: &[PathBuf],
        process_names: &[&str],
        _timeout: Duration,
    ) -> Result<AppShutdownSession> {
        *self.shutdown_calls.lock().unwrap() += 1;
        let running = self.find_running_exe_paths(process_names);
        if running.is_empty() {
            return Ok(AppShutdownSession::no_op());
        }
        let behavior = self.shutdown_behavior.lock().unwrap().clone();
        match behavior {
            MockShutdownBehavior::Succeed => {
                // Simulate clean exit of all matching processes
                let targets: HashSet<String> = process_names
                    .iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect();
                self.running_exes.lock().unwrap().retain(|p| {
                    !p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| targets.contains(&n.to_ascii_lowercase()))
                });
                Ok(AppShutdownSession::with_captured_exes(running, true))
            }
            MockShutdownBehavior::Timeout => Err(GreenCError::FileLocked {
                path: resource_paths.first().cloned().unwrap_or_default(),
                reason: ZALO_MANUAL_QUIT_MESSAGE.to_string(),
            }),
            MockShutdownBehavior::Fail(msg) => Err(GreenCError::FileLocked {
                path: resource_paths.first().cloned().unwrap_or_default(),
                reason: msg,
            }),
        }
    }

    fn restart_app(&self, session: &mut AppShutdownSession) -> Result<bool> {
        *self.restart_calls.lock().unwrap() += 1;
        if session.captured_exes.is_empty() {
            return Ok(false);
        }
        let exes = session.captured_exes.clone();
        self.restarted_exes.lock().unwrap().extend(exes.clone());
        self.running_exes.lock().unwrap().extend(exes);
        if let Some(ref hook) = *self.on_restart.lock().unwrap() {
            hook();
        }
        Ok(true)
    }
}
