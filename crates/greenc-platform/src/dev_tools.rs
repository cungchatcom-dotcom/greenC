//! Developer build-tool process detection (`GC-GUARD` Item 1).
//!
//! Pure classification (`detect_busy_dev_tools`) operates on a slice of `ProcessSnapshot`
//! records so all matching rules are deterministically unit-testable without reading `/proc`.
//!
//! Platform collectors:
//! - **Linux**: reads `/proc` strictly for processes owned by the current effective UID.
//! - **macOS**: queries `ps -u <uid> -o pid=,comm=,args=` and runs the same pure classifier.
//! - **Windows**: queries ToolHelp32 via `NativeProcessServiceProbe` (conservatively mapping
//!   `java.exe`/`javaw.exe` -> `Gradle` and `node.exe` -> `Npm` since ToolHelp32 lacks cmdlines).
//! - **Other / probe failure**: returns `BusyToolsStatus::Unknown`, which causes automatic
//!   cleanup to skip developer cache profiles fail-safely.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// Developer build / package-manager tool families whose caches must not be touched while busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevTool {
    Gradle,
    Cargo,
    Npm,
    Pip,
}

impl DevTool {
    pub const ALL: [DevTool; 4] = [DevTool::Gradle, DevTool::Cargo, DevTool::Npm, DevTool::Pip];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Gradle => "gradle",
            Self::Cargo => "cargo",
            Self::Npm => "npm",
            Self::Pip => "pip",
        }
    }

    /// Short Vietnamese human-readable name for user notifications.
    pub const fn display_name_vi(&self) -> &'static str {
        match self {
            Self::Gradle => "Gradle",
            Self::Cargo => "Cargo/Rust",
            Self::Npm => "npm/pnpm/yarn",
            Self::Pip => "pip/uv",
        }
    }

    /// Parses tool names or binary names (e.g. `"gradle"`, `"rustc"`, `"pnpm"`, `"uv"`).
    pub fn from_str_loose(raw: &str) -> Option<Self> {
        let low = raw.trim().to_ascii_lowercase();
        let stripped = low.strip_suffix(".exe").unwrap_or(&low);
        match stripped {
            "gradle" | "gradlew" | "gradledaemon" | "kotlincompiledaemon" | "java" => {
                Some(Self::Gradle)
            }
            "cargo" | "rustc" | "rustdoc" | "clippy-driver" | "rust" => Some(Self::Cargo),
            "npm" | "pnpm" | "yarn" | "node" | "nodejs" => Some(Self::Npm),
            "pip" | "pip3" | "uv" => Some(Self::Pip),
            _ => None,
        }
    }
}

/// Snapshot of a single OS process used by the pure detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub pid: u32,
    /// Short process name (e.g., `/proc/<pid>/comm` on Linux).
    pub comm: String,
    /// Full command line (NUL bytes normalized to spaces).
    pub cmdline: String,
}

impl ProcessSnapshot {
    pub fn new(pid: u32, comm: impl Into<String>, cmdline: impl Into<String>) -> Self {
        Self {
            pid,
            comm: comm.into(),
            cmdline: cmdline.into(),
        }
    }
}

/// Result of probing currently running developer tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum BusyToolsStatus {
    /// Probe succeeded; contains the set of currently busy tools (empty = all idle).
    Known(BTreeSet<DevTool>),
    /// Probe could not inspect processes; automatic developer-cache cleanup MUST be skipped.
    Unknown { reason: String },
}

impl BusyToolsStatus {
    pub fn known(tools: BTreeSet<DevTool>) -> Self {
        Self::Known(tools)
    }

    pub fn idle() -> Self {
        Self::Known(BTreeSet::new())
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown {
            reason: reason.into(),
        }
    }

    /// Fail-safe: returns `true` if `tool` is detected OR if process status is `Unknown`.
    pub fn is_tool_busy(&self, tool: DevTool) -> bool {
        match self {
            Self::Known(set) => set.contains(&tool),
            Self::Unknown { .. } => true,
        }
    }

    /// Returns `true` if any developer tool is busy or status is `Unknown`.
    pub fn is_any_busy(&self) -> bool {
        match self {
            Self::Known(set) => !set.is_empty(),
            Self::Unknown { .. } => true,
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    pub fn busy_tools(&self) -> Vec<DevTool> {
        match self {
            Self::Known(set) => set.iter().copied().collect(),
            Self::Unknown { .. } => Vec::new(),
        }
    }

    /// Merges extra externally detected busy tools (e.g., from a shell hotfix guard).
    pub fn merge_extra_tools(&mut self, extra: impl IntoIterator<Item = DevTool>) {
        if let Self::Known(set) = self {
            set.extend(extra);
        }
    }
}

/// Extracts the lowercased executable basename with `.exe` stripped.
fn normalized_basename(token: &str) -> String {
    let trimmed = token.trim().trim_matches('"').trim_matches('\'');
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
}

fn is_text_inspection_tool(name: &str) -> bool {
    matches!(
        name,
        "grep" | "egrep" | "fgrep" | "rg" | "awk" | "gawk" | "mawk" | "sed" | "cat" | "less"
    )
}

fn token_matches_script_basename(token: &str, candidates: &[&str]) -> bool {
    let base = normalized_basename(token);
    candidates.contains(&base.as_str())
}

/// Pure function that classifies running developer tools from process snapshots.
///
/// Design invariants:
/// - Even an **idle** `GradleDaemon` is classified as `DevTool::Gradle` busy.
/// - Covers `GradleDaemon`, `GradleWrapperMain`, `GradleWorkerMain`, `KotlinCompileDaemon`,
///   `cargo`, `rustc`, `npm`, `pnpm`, `yarn`, `node` running `npm-cli`/`pnpm`/`yarn`,
///   `pip`, `pip3`, `uv`.
/// - Avoids false positives from plain text tools like `grep GradleDaemon`.
pub fn detect_busy_dev_tools(processes: &[ProcessSnapshot]) -> BTreeSet<DevTool> {
    let mut busy = BTreeSet::new();

    for proc in processes {
        let comm = normalized_basename(&proc.comm);
        let tokens: Vec<&str> = proc.cmdline.split_whitespace().collect();
        let argv0 = tokens
            .first()
            .map(|t| normalized_basename(t))
            .unwrap_or_default();

        if is_text_inspection_tool(&comm) || is_text_inspection_tool(&argv0) {
            continue;
        }

        // 1. Direct binary name checks (comm or argv[0]).
        for name in [comm.as_str(), argv0.as_str()] {
            match name {
                "gradle" | "gradlew" => {
                    busy.insert(DevTool::Gradle);
                }
                "cargo" | "rustc" | "rustdoc" | "clippy-driver" => {
                    busy.insert(DevTool::Cargo);
                }
                "npm" | "pnpm" | "yarn" => {
                    busy.insert(DevTool::Npm);
                }
                "pip" | "pip3" | "uv" => {
                    busy.insert(DevTool::Pip);
                }
                _ => {}
            }
        }

        // 2. Gradle / Kotlin JVM daemon, worker, and wrapper signatures (active OR idle!).
        // Only JVM processes count: shells, editors or AI agents whose arguments merely mention
        // these names must not block cleanup forever.
        let cmdline = &proc.cmdline;
        let is_jvm =
            matches!(comm.as_str(), "java" | "javaw") || matches!(argv0.as_str(), "java" | "javaw");
        if is_jvm
            && (cmdline.contains("GradleDaemon")
                || cmdline.contains("GradleWrapperMain")
                || cmdline.contains("GradleWorkerMain")
                || cmdline.contains("KotlinCompileDaemon")
                || cmdline.contains("org.gradle.launcher.daemon")
                || cmdline.contains("org.gradle.wrapper.")
                || cmdline.contains("org.gradle.process.internal.worker"))
        {
            busy.insert(DevTool::Gradle);
        }

        // 3. Node-based npm / pnpm / yarn invocations (e.g. `node /usr/lib/node_modules/npm/bin/npm-cli.js`).
        let is_node = matches!(comm.as_str(), "node" | "nodejs")
            || matches!(argv0.as_str(), "node" | "nodejs");
        if is_node
            && (cmdline.contains("npm-cli")
                || tokens.iter().skip(1).any(|tok| {
                    token_matches_script_basename(
                        tok,
                        &[
                            "npm",
                            "npm-cli.js",
                            "npm.js",
                            "pnpm",
                            "pnpm.cjs",
                            "pnpm.js",
                            "yarn",
                            "yarn.js",
                            "yarn.cjs",
                        ],
                    )
                }))
        {
            busy.insert(DevTool::Npm);
        }

        // 4. Python-based pip / uv invocations (e.g. `python3 -m pip install ...` or `python3 /usr/bin/pip3`).
        let is_python = comm.starts_with("python") || argv0.starts_with("python");
        if is_python {
            let mut prev_m = false;
            for tok in tokens.iter().skip(1) {
                if prev_m {
                    let mod_name = tok.trim().to_ascii_lowercase();
                    if matches!(mod_name.as_str(), "pip" | "pip3" | "uv") {
                        busy.insert(DevTool::Pip);
                        break;
                    }
                    prev_m = false;
                } else if *tok == "-m" {
                    prev_m = true;
                } else if token_matches_script_basename(tok, &["pip", "pip3", "uv"]) {
                    busy.insert(DevTool::Pip);
                    break;
                }
            }
        }
    }

    busy
}

/// Trait abstracting developer build-tool detection for deterministic testing.
pub trait DevToolProbe: Send + Sync {
    fn detect_busy_tools(&self) -> BusyToolsStatus;
}

/// Native OS implementation of [`DevToolProbe`].
#[derive(Default, Clone, Debug)]
pub struct NativeDevToolProbe;

impl DevToolProbe for NativeDevToolProbe {
    fn detect_busy_tools(&self) -> BusyToolsStatus {
        detect_busy_dev_tools_native()
    }
}

/// Deterministic mock implementation of [`DevToolProbe`] for unit and integration tests.
///
/// Supports both a persistent status and an optional FIFO queue of statuses (so TOCTOU tests
/// can simulate a tool becoming busy mid-cleanup).
#[derive(Clone, Debug)]
pub struct MockDevToolProbe {
    status: Arc<Mutex<BusyToolsStatus>>,
    queued: Arc<Mutex<std::collections::VecDeque<BusyToolsStatus>>>,
}

impl Default for MockDevToolProbe {
    fn default() -> Self {
        Self::idle()
    }
}

impl MockDevToolProbe {
    pub fn idle() -> Self {
        Self {
            status: Arc::new(Mutex::new(BusyToolsStatus::idle())),
            queued: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub fn with_busy(tools: impl IntoIterator<Item = DevTool>) -> Self {
        let set: BTreeSet<DevTool> = tools.into_iter().collect();
        Self {
            status: Arc::new(Mutex::new(BusyToolsStatus::known(set))),
            queued: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            status: Arc::new(Mutex::new(BusyToolsStatus::unknown(reason))),
            queued: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub fn set_status(&self, status: BusyToolsStatus) {
        *self.status.lock().unwrap() = status;
    }

    pub fn set_tool_busy(&self, tool: DevTool, busy: bool) {
        let mut guard = self.status.lock().unwrap();
        match &mut *guard {
            BusyToolsStatus::Known(set) => {
                if busy {
                    set.insert(tool);
                } else {
                    set.remove(&tool);
                }
            }
            BusyToolsStatus::Unknown { .. } => {
                let mut set = BTreeSet::new();
                if busy {
                    set.insert(tool);
                }
                *guard = BusyToolsStatus::Known(set);
            }
        }
    }

    /// Enqueues statuses returned on subsequent `detect_busy_tools()` calls in order.
    /// Once the queue drains, falls back to the persistent `status`.
    pub fn push_sequence(&self, items: impl IntoIterator<Item = BusyToolsStatus>) {
        let mut q = self.queued.lock().unwrap();
        q.extend(items);
    }
}

impl DevToolProbe for MockDevToolProbe {
    fn detect_busy_tools(&self) -> BusyToolsStatus {
        if let Some(next) = self.queued.lock().unwrap().pop_front() {
            return next;
        }
        self.status.lock().unwrap().clone()
    }
}

/// Reads `/proc` on Linux strictly for the current UID and returns `BusyToolsStatus`.
#[cfg(target_os = "linux")]
pub fn detect_busy_dev_tools_native() -> BusyToolsStatus {
    use std::os::unix::fs::MetadataExt;

    let my_uid = match std::fs::metadata("/proc/self") {
        Ok(m) => m.uid(),
        Err(e) => {
            return BusyToolsStatus::unknown(format!(
                "Không đọc được /proc/self để xác định UID: {e}"
            ));
        }
    };

    let entries = match std::fs::read_dir("/proc") {
        Ok(rd) => rd,
        Err(e) => {
            return BusyToolsStatus::unknown(format!("Không đọc được /proc: {e}"));
        }
    };

    let my_pid = std::process::id();
    let mut snapshots = Vec::new();

    for entry in entries.flatten() {
        let fname = entry.file_name();
        let s = fname.to_string_lossy();
        let Ok(pid) = s.parse::<u32>() else {
            continue;
        };
        if pid == my_pid {
            continue;
        }

        let proc_dir = entry.path();
        let Ok(meta) = std::fs::metadata(&proc_dir) else {
            continue;
        };
        // Only inspect processes belonging to the current user's UID.
        if meta.uid() != my_uid {
            continue;
        }

        let comm = std::fs::read_to_string(proc_dir.join("comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let cmdline_bytes = std::fs::read(proc_dir.join("cmdline")).unwrap_or_default();
        let cmdline = cmdline_bytes
            .split(|&b| b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(" ");

        if !comm.is_empty() || !cmdline.is_empty() {
            snapshots.push(ProcessSnapshot { pid, comm, cmdline });
        }
    }

    BusyToolsStatus::known(detect_busy_dev_tools(&snapshots))
}

/// Parses `ps -o pid=,comm=,args=` output into `ProcessSnapshot` entries (used on macOS and tested cross-platform).
pub fn parse_ps_output(stdout: &str, self_pid: u32) -> Vec<ProcessSnapshot> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(pid_str) = parts.next() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Some(comm) = parts.next() else {
            continue;
        };
        let cmdline = parts.collect::<Vec<_>>().join(" ");
        out.push(ProcessSnapshot {
            pid,
            comm: comm.to_string(),
            cmdline,
        });
    }
    out
}

#[cfg(target_os = "macos")]
pub fn detect_busy_dev_tools_native() -> BusyToolsStatus {
    // SAFETY: `geteuid` has no preconditions and always succeeds on POSIX systems.
    let uid = unsafe { libc::geteuid() };
    let Ok(output) = std::process::Command::new("ps")
        .args(["-u", &uid.to_string(), "-o", "pid=,comm=,args="])
        .output()
    else {
        return BusyToolsStatus::unknown("Không chạy được ps trên macOS");
    };
    if !output.status.success() {
        return BusyToolsStatus::unknown(format!(
            "ps trên macOS trả mã lỗi {:?}",
            output.status.code()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snaps = parse_ps_output(&stdout, std::process::id());
    BusyToolsStatus::known(detect_busy_dev_tools(&snaps))
}

#[cfg(windows)]
pub fn detect_busy_dev_tools_native() -> BusyToolsStatus {
    use crate::traits::{NativeProcessServiceProbe, ProcessServiceProbe};

    // Windows ToolHelp32 (`NativeProcessServiceProbe`) provides executable names only (not cmdlines).
    // We conservatively treat `java.exe`/`javaw.exe` as Gradle-busy and `node.exe` as npm-busy.
    // First verify that taking a ToolHelp32 snapshot actually works; if not, report Unknown.
    // SAFETY: `CreateToolhelp32Snapshot` returns a snapshot handle immediately closed via `OwnedHandle`.
    let snap_ok = unsafe {
        windows::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot(
            windows::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS,
            0,
        )
        .ok()
        .and_then(crate::handle::OwnedHandle::new)
        .is_some()
    };
    if !snap_ok {
        return BusyToolsStatus::unknown("Không lấy được danh sách tiến trình Win32");
    }

    let probe = NativeProcessServiceProbe;
    let mut busy = BTreeSet::new();
    for exe in ["gradle.exe", "java.exe", "javaw.exe"] {
        if probe.is_process_running(exe) {
            busy.insert(DevTool::Gradle);
            break;
        }
    }
    for exe in ["cargo.exe", "rustc.exe", "rustdoc.exe", "clippy-driver.exe"] {
        if probe.is_process_running(exe) {
            busy.insert(DevTool::Cargo);
            break;
        }
    }
    for exe in ["npm.exe", "pnpm.exe", "yarn.exe", "node.exe"] {
        if probe.is_process_running(exe) {
            busy.insert(DevTool::Npm);
            break;
        }
    }
    for exe in ["pip.exe", "pip3.exe", "uv.exe"] {
        if probe.is_process_running(exe) {
            busy.insert(DevTool::Pip);
            break;
        }
    }
    BusyToolsStatus::known(busy)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn detect_busy_dev_tools_native() -> BusyToolsStatus {
    BusyToolsStatus::unknown("Nền tảng này chưa hỗ trợ phát hiện tiến trình công cụ lập trình")
}
