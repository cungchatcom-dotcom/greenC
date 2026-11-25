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

/// Snapshot of a candidate running process for targeted Windows Restart Manager registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmProcessCandidate {
    pub pid: u32,
    pub exe_path: PathBuf,
    pub start_time_low: u32,
    pub start_time_high: u32,
}

fn exe_basename_lower(path: &Path) -> Option<String> {
    let raw = path.to_string_lossy();
    raw.rsplit(['\\', '/'])
        .find(|seg| !seg.is_empty())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

/// Filters running process candidates so Restart Manager (`RmShutdown`) ONLY targets processes
/// whose executable filename matches `allowed_process_names` (case-insensitive).
///
/// Restart Manager MUST register explicit `RM_UNIQUE_PROCESS` entries returned by this filter
/// (`rgsFileNames = None`) rather than registering data directory files, so unrelated apps
/// (Explorer, editors, backup tools, antivirus, GreenC itself) are NEVER shut down.
pub fn select_rm_target_processes(
    candidates: &[RmProcessCandidate],
    allowed_process_names: &[&str],
) -> Vec<RmProcessCandidate> {
    let targets: HashSet<String> = allowed_process_names
        .iter()
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }

    let mut seen: HashSet<(u32, u32, u32)> = HashSet::new();
    let mut selected = Vec::new();
    for cand in candidates {
        if cand.pid == 0 {
            continue;
        }
        let Some(base) = exe_basename_lower(&cand.exe_path) else {
            continue;
        };
        if !targets.contains(&base) {
            continue;
        }
        let key = (cand.pid, cand.start_time_low, cand.start_time_high);
        if seen.insert(key) {
            selected.push(cand.clone());
        }
    }
    selected
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

/// Polls `all_exited` until it returns `true` or `timeout` elapses.
/// Used after `RmShutdown` (even when `RmShutdown` returns a non-zero error code) so GreenC
/// waits for target processes to finish exiting before concluding shutdown failed (Item 11).
pub fn wait_for_processes_to_exit<F>(
    timeout: Duration,
    poll_interval: Duration,
    mut all_exited: F,
) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if all_exited() {
            return true;
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(now);
        std::thread::sleep(poll_interval.min(remaining));
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
    use crate::handle::OwnedHandle;
    use crate::win32::restart_mgr::RmSessionGuard;
    use std::time::Instant;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{BOOL, FILETIME, HWND, LPARAM};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::RestartManager::{
        RmRegisterResources, RmRestart, RmShutdown, RM_UNIQUE_PROCESS,
    };
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
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

    fn query_process_candidate(pid: u32) -> Option<RmProcessCandidate> {
        if pid == 0 {
            return None;
        }
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
            return None;
        }

        let mut creation_time = FILETIME::default();
        let mut exit_time = FILETIME::default();
        let mut kernel_time = FILETIME::default();
        let mut user_time = FILETIME::default();
        // SAFETY: `guard.raw()` is open with `PROCESS_QUERY_LIMITED_INFORMATION` and all output
        // `FILETIME` pointers refer to valid stack variables.
        unsafe {
            GetProcessTimes(
                guard.raw(),
                &mut creation_time,
                &mut exit_time,
                &mut kernel_time,
                &mut user_time,
            )
        }
        .ok()?;

        Some(RmProcessCandidate {
            pid,
            exe_path: PathBuf::from(trimmed),
            start_time_low: creation_time.dwLowDateTime,
            start_time_high: creation_time.dwHighDateTime,
        })
    }

    fn find_running_process_candidates_native(exe_names: &[&str]) -> Vec<RmProcessCandidate> {
        let targets: HashSet<String> = exe_names
            .iter()
            .map(|n| n.trim().to_ascii_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        if targets.is_empty() {
            return Vec::new();
        }

        let mut raw_candidates = Vec::new();
        // SAFETY: Snapshot handle wrapped immediately in `OwnedHandle`; `dwSize` initialized.
        unsafe {
            let Ok(raw_snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return raw_candidates;
            };
            let Some(snap) = OwnedHandle::new(raw_snap) else {
                return raw_candidates;
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
                        if let Some(cand) = query_process_candidate(entry.th32ProcessID) {
                            raw_candidates.push(cand);
                        }
                    }
                    if Process32NextW(snap.raw(), &mut entry).is_err() {
                        break;
                    }
                }
            }
        }
        select_rm_target_processes(&raw_candidates, exe_names)
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

    pub fn graceful_shutdown_native(
        resource_paths: &[PathBuf],
        process_names: &[&str],
        timeout: Duration,
    ) -> Result<AppShutdownSession> {
        // Item 10: Register ONLY explicit `RM_UNIQUE_PROCESS` handles for running processes
        // matching `process_names` (e.g. Zalo.exe / ZaloCall.exe / ZaloCap.exe) — NEVER pass
        // `resource_paths` as `rgsFileNames` to `RmRegisterResources`, so `RmShutdown` can NEVER
        // close unrelated applications (Explorer, editors, backup tools, antivirus, GreenC).
        let target_procs = find_running_process_candidates_native(process_names);
        if target_procs.is_empty() {
            return Ok(AppShutdownSession::no_op());
        }

        let mut captured_exes: Vec<PathBuf> = Vec::new();
        let mut rm_apps: Vec<RM_UNIQUE_PROCESS> = Vec::with_capacity(target_procs.len());
        for cand in &target_procs {
            if !captured_exes.contains(&cand.exe_path) {
                captured_exes.push(cand.exe_path.clone());
            }
            rm_apps.push(RM_UNIQUE_PROCESS {
                dwProcessId: cand.pid,
                ProcessStartTime: FILETIME {
                    dwLowDateTime: cand.start_time_low,
                    dwHighDateTime: cand.start_time_high,
                },
            });
        }

        let session = RmSessionGuard::start()?;
        // SAFETY: `rm_apps` contains ONLY verified `(PID, ProcessStartTime)` tuples matching
        // `process_names`. `rgsFileNames` and `rgsServiceNames` are strictly `None`.
        let reg_err = unsafe { RmRegisterResources(session.raw(), None, Some(&rm_apps), None) };
        if reg_err.0 != 0 {
            return Err(GreenCError::FileLocked {
                path: resource_paths.first().cloned().unwrap_or_default(),
                reason: ZALO_MANUAL_QUIT_MESSAGE.to_string(),
            });
        }

        // SAFETY: `lActionFlags = 0` requests graceful shutdown ONLY (NEVER `RmForceShutdown`).
        // Item 11: Even if `RmShutdown` returns a non-zero status code, wait up to
        // `timeout` for the target processes to exit before concluding shutdown failed!
        let shut_err = unsafe { RmShutdown(session.raw(), 0, None) };
        let rm_succeeded = shut_err.0 == 0;

        if super::wait_for_processes_to_exit(timeout, Duration::from_millis(250), || {
            find_running_exe_paths_native(process_names).is_empty()
        }) {
            return Ok(AppShutdownSession {
                captured_exes,
                used_restart_manager: rm_succeeded,
                rm_guard: if rm_succeeded { Some(session) } else { None },
            });
        }

        Err(GreenCError::FileLocked {
            path: resource_paths.first().cloned().unwrap_or_default(),
            reason: ZALO_MANUAL_QUIT_MESSAGE.to_string(),
        })
    }

    pub fn restart_app_native(session: &mut AppShutdownSession) -> Result<bool> {
        let exe_names_owned: Vec<String> = session
            .captured_exes
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        let exe_names: Vec<&str> = exe_names_owned.iter().map(String::as_str).collect();

        if let Some(ref rm_guard) = session.rm_guard {
            // SAFETY: `rm_guard.raw()` is a live Restart Manager session that performed `RmShutdown`.
            let _ = unsafe { RmRestart(rm_guard.raw(), 0, None) };
        }
        // Drop RM session cleanly after attempting RmRestart.
        session.rm_guard = None;

        // `RmRestart` returning 0 is NOT sufficient: apps that did not register with
        // `RegisterApplicationRestart` will NOT be relaunched by Restart Manager!
        // Poll briefly for a real running process before deciding whether to spawn the saved exe.
        let wait_for_running = |names: &[&str], max_wait: Duration| -> bool {
            if names.is_empty() {
                return false;
            }
            let deadline = Instant::now() + max_wait;
            loop {
                if !find_running_exe_paths_native(names).is_empty() {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        };

        if wait_for_running(&exe_names, Duration::from_secs(3)) {
            return Ok(true);
        }

        // Fallback: explicitly launch the exact primary executable saved before shutdown.
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
                return Ok(wait_for_running(&exe_names, Duration::from_secs(3)));
            }
        }
        Ok(false)
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
    pub shutdown_targeted_exes: Arc<Mutex<Vec<PathBuf>>>,
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
        self.shutdown_targeted_exes
            .lock()
            .unwrap()
            .extend(running.iter().cloned());
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
