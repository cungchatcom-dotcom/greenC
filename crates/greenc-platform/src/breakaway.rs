//! Microsoft Store (MSIX) package-identity detection and breakaway helper execution.
//!
//! When GreenC runs inside a Windows MSIX package, newly created files under `%LOCALAPPDATA%` /
//! `%APPDATA%` are silently redirected by Windows into
//! `%LOCALAPPDATA%\Packages\<PackageFamilyName>\LocalCache\...` so other apps and users never see
//! restored files in the real AppData folders.
//!
//! This module provides:
//! - [`current_package_full_name`] / [`is_running_in_msix_package`] via `GetCurrentPackageFullName`
//!   so a packaged parent can detect MSIX containment and a breakaway child can self-verify that it
//!   genuinely escaped the package before touching real AppData.
//! - [`PrivateBreakawayJobDir`], a user-only (`0700` on Unix; protected current-user + SYSTEM DACL
//!   on Windows) temporary workspace strictly **outside** AppData (`%USERPROFILE%\.greenc-store-restore`)
//!   that both the packaged parent and the unpackaged child see as the exact same real directory.
//! - [`spawn_restore_helper_breakaway`], which spawns `greenc-cli __restore-helper` (with ZERO path
//!   arguments on the command line) using `STARTUPINFOEXW`, `InitializeProcThreadAttributeList`, and
//!   `PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY = PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_ENABLE_PROCESS_TREE`.
//! - Shared serializable request/response contracts and the injectable [`BreakawayHelperClient`]
//!   trait so Vault restore, staging recovery, and relocation undo can all be unit- and
//!   integration-tested deterministically.

use greenc_core::error::{GreenCError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Win32 `PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY`:
/// `ProcThreadAttributeValue(ProcThreadAttributeDesktopAppPolicy = 18, FALSE, TRUE, FALSE)`.
pub const PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY: usize = 0x0002_0012;

/// Win32 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`:
/// `ProcThreadAttributeValue(ProcThreadAttributeHandleList = 2, FALSE, TRUE, FALSE)`.
pub const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;

/// Win32 `PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_ENABLE_PROCESS_TREE` (`0x01`).
/// Requests that the created process (and its descendants) run outside the MSIX desktop app runtime.
pub const PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_ENABLE_PROCESS_TREE: u32 = 0x0000_0001;

/// Dedicated exit code emitted by `greenc-cli __restore-helper` when it detects that it is still
/// running inside an MSIX package (breakaway refused by OS version or policy).
pub const EXIT_HELPER_STILL_IN_PACKAGE: i32 = 78;

/// Maximum JSON request size accepted on stdin by `greenc-cli __restore-helper` (`16 MiB`).
pub const MAX_RESTORE_HELPER_STDIN_BYTES: u64 = 16 * 1024 * 1024;

static JOB_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// Conflict resolution strategy serialized across the helper IPC boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperConflictStrategy {
    RenameWithTimestamp,
    OverwriteIfZeroLength,
    Overwrite,
}

/// One Vault file to copy from the private staged payload directory into its real destination path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreFileJob {
    /// Original recorded path (used for reporting and matching).
    pub original_path: PathBuf,
    /// Staged regular file inside `payload_root`, extracted and pre-verified by the parent.
    pub staged_source: PathBuf,
    /// Real destination path to write (validated again inside the helper with `validate_restore_file_path`).
    pub dest_path: PathBuf,
    /// Expected lowercase hex SHA-256 digest of the file contents.
    pub expected_sha256: String,
    /// Expected byte length.
    pub expected_size: u64,
    /// Original access timestamp (Unix seconds).
    pub atime: f64,
    /// Original modification timestamp (Unix seconds).
    pub mtime: f64,
    /// Additional hardlink sibling destinations to reconstruct after primary restore.
    #[serde(default)]
    pub hardlink_siblings: Vec<PathBuf>,
}

/// Request payload passed to `greenc-cli __restore-helper` over stdin (never on the command line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreHelperRequest {
    /// Restores one or more Vault files from a private staged payload directory into real paths
    /// and verifies every destination file's existence and SHA-256 digest from outside the package.
    RestoreFiles {
        payload_root: PathBuf,
        conflict_strategy: HelperConflictStrategy,
        verify_sha256: bool,
        restore_hardlinks: bool,
        restore_timestamps: bool,
        items: Vec<RestoreFileJob>,
    },
    /// Atomically moves a staged directory (from the non-AppData staging area) back to its real
    /// AppData `original_path` and verifies existence from outside the package.
    RestoreStagedDir {
        staged_dir: PathBuf,
        original_path: PathBuf,
        expected_dev: Option<u64>,
        expected_ino: Option<u64>,
    },
    /// Undoes an AppData junction relocation from outside the package: strictly copies
    /// `target_path` to a temporary sibling, removes the junction at `source_path`, renames the
    /// verified copy into `source_path`, and verifies every file's existence and SHA-256 digest
    /// before reporting success.
    UndoRelocationDir {
        source_path: PathBuf,
        target_path: PathBuf,
    },
}

/// Per-file verified outcome reported by `greenc-cli __restore-helper`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreFileOutcome {
    pub original_path: PathBuf,
    pub restored_path: PathBuf,
    pub bytes_written: u64,
    pub verified_exists: bool,
    pub verified_sha256: String,
    #[serde(default)]
    pub restored_siblings: Vec<PathBuf>,
}

/// Structured JSON response emitted on stdout by `greenc-cli __restore-helper`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreHelperResponse {
    pub ok: bool,
    pub running_outside_package: bool,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub files: Vec<RestoreFileOutcome>,
    #[serde(default)]
    pub verified_dir: Option<PathBuf>,
    #[serde(default)]
    pub verified_file_count: u64,
    #[serde(default)]
    pub verified_bytes: u64,
}

impl RestoreHelperResponse {
    pub fn still_in_package(package_name: Option<&str>) -> Self {
        let pkg = package_name.unwrap_or("MSIX");
        Self {
            ok: false,
            running_outside_package: false,
            error_code: Some("still_in_package".to_string()),
            error: Some(format!(
                "Tiến trình trợ giúp khôi phục vẫn đang chạy trong gói Microsoft Store ({pkg}) và không thể thoát khỏi cơ chế ảo hoá AppData."
            )),
            files: Vec::new(),
            verified_dir: None,
            verified_file_count: 0,
            verified_bytes: 0,
        }
    }

    pub fn failed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            running_outside_package: true,
            error_code: Some(code.into()),
            error: Some(message.into()),
            files: Vec::new(),
            verified_dir: None,
            verified_file_count: 0,
            verified_bytes: 0,
        }
    }
}

/// Raw stdout/stderr/exit-code output from invoking `greenc-cli __restore-helper`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakawayExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Injectable execution boundary for spawning `greenc-cli __restore-helper` outside MSIX.
pub trait BreakawayHelperClient: Send + Sync {
    /// Returns `true` when the current process is executing inside a Windows MSIX package.
    fn is_packaged(&self) -> bool;

    /// Runs `greenc-cli __restore-helper` with `stdin_json` (no path arguments on CLI) and
    /// returns the helper's exit code and captured stdout/stderr.
    fn execute(
        &self,
        job_dir: &PrivateBreakawayJobDir,
        stdin_json: &[u8],
    ) -> Result<BreakawayExecOutput>;
}

/// Production [`BreakawayHelperClient`] backed by `GetCurrentPackageFullName` and Win32
/// `CreateProcessW` with `PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeBreakawayHelperClient;

impl BreakawayHelperClient for NativeBreakawayHelperClient {
    fn is_packaged(&self) -> bool {
        is_running_in_msix_package()
    }

    fn execute(
        &self,
        job_dir: &PrivateBreakawayJobDir,
        stdin_json: &[u8],
    ) -> Result<BreakawayExecOutput> {
        let exe = locate_restore_helper_exe()?;
        spawn_restore_helper_breakaway(&exe, job_dir, stdin_json)
    }
}

/// Returns the current MSIX package full name if running inside a Windows Store / MSIX package,
/// or `None` when running unpackaged (standard EXE, broken-away helper, or non-Windows OS).
pub fn current_package_full_name() -> Option<String> {
    #[cfg(windows)]
    {
        win_impl::current_package_full_name()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Returns `true` if the current process has a Windows MSIX package identity.
pub fn is_running_in_msix_package() -> bool {
    current_package_full_name().is_some()
}

/// Suggests a user-friendly directory strictly outside AppData for manual fallback restoration
/// when MSIX breakaway is unavailable (e.g., `C:\Users\<User>\Documents\GreenC Restored`).
pub fn suggested_non_appdata_restore_dir() -> PathBuf {
    #[cfg(windows)]
    {
        use crate::known_folders::{NativeShellFolderApi, FOLDERID_DOCUMENTS};
        use crate::traits::ShellFolderApi;

        if let Ok(docs) = NativeShellFolderApi.get_known_folder(FOLDERID_DOCUMENTS) {
            if !docs.as_os_str().is_empty()
                && !greenc_core::config::is_windows_appdata_app_dir(&docs)
            {
                return docs.join("GreenC Restored");
            }
        }
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            return PathBuf::from(profile)
                .join("Documents")
                .join("GreenC Restored");
        }
        PathBuf::from(r"C:\GreenC Restored")
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(home)
                .join("Documents")
                .join("GreenC Restored");
        }
        PathBuf::from("/tmp/GreenC Restored")
    }
}

/// Formats clear Vietnamese guidance when MSIX breakaway is unavailable (older Windows build or
/// system policy), confirming that original Vault/data packages are preserved and directing the
/// user to restore into a directory outside AppData (such as `Documents\GreenC Restored`).
pub fn format_breakaway_unavailable_guidance(operation_label: &str, detail: &str) -> String {
    let suggested = suggested_non_appdata_restore_dir();
    format!(
        "Không thể {operation_label} trực tiếp vào thư mục AppData thật trong bản Microsoft Store vì hệ thống không cho tiến trình trợ giúp thoát khỏi hộp ảo hoá MSIX ({detail}). \
         GreenC đã dừng an toàn và GIỮ NGUYÊN gói Vault/dữ liệu gốc (KHÔNG đánh dấu đã khôi phục). \
         Hãy chọn một thư mục ngoài AppData (ví dụ: {}) rồi khôi phục vào đó (CLI: `greenc-cli vault restore <ID> --to \"{}\"`).",
        suggested.display(),
        suggested.display()
    )
}

/// Locates the `greenc-cli` helper binary installed alongside the current executable, refusing
/// symlinks and Windows `AppExecutionAlias` stubs under `%LOCALAPPDATA%\Microsoft\WindowsApps`.
pub fn locate_restore_helper_exe() -> Result<PathBuf> {
    let current = std::env::current_exe().map_err(|e| {
        GreenCError::Vault(format!(
            "Không xác định được đường dẫn tệp thực thi hiện tại: {e}"
        ))
    })?;
    let exe_name = if cfg!(windows) {
        "greenc-cli.exe"
    } else {
        "greenc-cli"
    };

    let candidate = if current
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(exe_name))
    {
        current
    } else {
        let dir = current.parent().ok_or_else(|| {
            GreenCError::Vault("Không tìm thấy thư mục cài đặt của GreenC".to_string())
        })?;
        dir.join(exe_name)
    };

    let meta = std::fs::symlink_metadata(&candidate).map_err(|e| {
        GreenCError::Vault(format!(
            "Không tìm thấy tệp trợ giúp khôi phục '{}': {e}",
            candidate.display()
        ))
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "locate_restore_helper".to_string(),
            reason: format!(
                "Từ chối chạy tệp trợ giúp '{}' vì là symlink/reparse hoặc không phải tệp thường",
                candidate.display()
            ),
        });
    }

    // Refuse `%LOCALAPPDATA%\Microsoft\WindowsApps\greenc-cli.exe` execution aliases because
    // launching an execution alias always re-enters MSIX virtualization.
    let norm = candidate
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if norm.contains("/appdata/local/microsoft/windowsapps/") {
        return Err(GreenCError::PreconditionBlocked {
            operation: "locate_restore_helper".to_string(),
            reason: format!(
                "Từ chối chạy qua AppExecutionAlias '{}' (luôn bị ảo hoá lại vào MSIX)",
                candidate.display()
            ),
        });
    }

    Ok(candidate)
}

/// RAII private job directory strictly outside AppData with zero permissions for other users
/// (`0700` on Unix; protected current-user + SYSTEM DACL on Windows). Automatically removed on drop.
#[derive(Debug)]
pub struct PrivateBreakawayJobDir {
    path: PathBuf,
    payload_dir: PathBuf,
}

impl PrivateBreakawayJobDir {
    /// Creates a new unique private job directory outside AppData.
    pub fn create() -> Result<Self> {
        let base = resolve_private_restore_base_dir()?;
        Self::create_in(&base)
    }

    /// Creates a new unique private job directory inside `base_dir` (must not be inside AppData).
    pub fn create_in(base_dir: &Path) -> Result<Self> {
        if greenc_core::config::is_windows_appdata_app_dir(base_dir) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "breakaway_job_dir".to_string(),
                reason: format!(
                    "Thư mục tạm trung gian '{}' không được nằm trong AppData",
                    base_dir.display()
                ),
            });
        }

        ensure_private_dir(base_dir)?;

        let pid = std::process::id();
        let seq = JOB_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let job_name = format!("job_{pid}_{nanos}_{seq}");
        let job_path = base_dir.join(job_name);
        let payload_dir = job_path.join("payload");

        create_private_leaf_dir(&job_path)?;
        create_private_leaf_dir(&payload_dir)?;

        Ok(Self {
            path: job_path,
            payload_dir,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn payload_dir(&self) -> &Path {
        &self.payload_dir
    }
}

impl Drop for PrivateBreakawayJobDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn resolve_private_restore_base_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            let candidate = PathBuf::from(profile).join(".greenc-store-restore");
            if !greenc_core::config::is_windows_appdata_app_dir(&candidate) {
                return Ok(candidate);
            }
        }
        Err(GreenCError::PreconditionBlocked {
            operation: "breakaway_job_dir".to_string(),
            reason:
                "Không xác định được thư mục USERPROFILE ngoài AppData để tạo khu tạm khôi phục"
                    .to_string(),
        })
    }
    #[cfg(not(windows))]
    {
        Ok(std::env::temp_dir().join(format!("greenc-store-restore-{}", std::process::id())))
    }
}

fn ensure_private_dir(dir: &Path) -> Result<()> {
    if std::fs::symlink_metadata(dir).is_err() {
        match create_private_leaf_dir(dir) {
            Ok(()) => return Ok(()),
            Err(GreenCError::Io(ref e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(GreenCError::Win32 { code, .. }) if (code & 0xFFFF) == 183 => {}
            Err(err) => return Err(err),
        }
    }

    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "breakaway_job_dir".to_string(),
            reason: format!(
                "Thư mục tạm '{}' là symlink hoặc không phải thư mục thật",
                dir.display()
            ),
        });
    }
    crate::handle::is_trusted_user_dir(dir, None).map_err(|reason| {
        GreenCError::PreconditionBlocked {
            operation: "breakaway_job_dir".to_string(),
            reason,
        }
    })?;
    Ok(())
}

fn create_private_leaf_dir(dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        win_impl::create_private_dir_current_user_only(dir)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            if std::fs::symlink_metadata(parent).is_err() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(())
    }
}

/// Spawns `helper_exe __restore-helper` (no path arguments on the command line!) outside the MSIX
/// package on Windows using `STARTUPINFOEXW` + `InitializeProcThreadAttributeList` +
/// `PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY = PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_ENABLE_PROCESS_TREE`.
pub fn spawn_restore_helper_breakaway(
    helper_exe: &Path,
    job_dir: &PrivateBreakawayJobDir,
    stdin_json: &[u8],
) -> Result<BreakawayExecOutput> {
    #[cfg(windows)]
    {
        win_impl::spawn_restore_helper_breakaway(helper_exe, job_dir, stdin_json)
    }
    #[cfg(unix)]
    {
        unix_impl::spawn_restore_helper_process(helper_exe, job_dir, stdin_json)
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    pub fn spawn_restore_helper_process(
        helper_exe: &Path,
        _job_dir: &PrivateBreakawayJobDir,
        stdin_json: &[u8],
    ) -> Result<BreakawayExecOutput> {
        let mut child = Command::new(helper_exe)
            .arg("__restore-helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                GreenCError::Vault(format!(
                    "Không khởi chạy được tiến trình trợ giúp khôi phục '{}': {e}",
                    helper_exe.display()
                ))
            })?;

        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(stdin_json)?;
            child_stdin.flush()?;
        }

        let output = child.wait_with_output()?;
        Ok(BreakawayExecOutput {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

#[cfg(windows)]
mod win_impl {
    use super::*;
    use crate::handle::{path_to_wide, OwnedHandle};
    use std::ffi::c_void;
    use std::io::Write;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        LocalFree, APPMODEL_ERROR_NO_PACKAGE, BOOL, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
        GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    use windows::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
        InitializeProcThreadAttributeList, OpenProcessToken, UpdateProcThreadAttribute,
        WaitForSingleObject, CREATE_NO_WINDOW, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    const SDDL_REVISION_1: u32 = 1;

    /// Queries the current process's Windows MSIX package full name via `GetCurrentPackageFullName`.
    pub fn current_package_full_name() -> Option<String> {
        let mut len: u32 = 0;
        // SAFETY: First sizing call with null `PWSTR` and valid `&mut len` per Win32 contract.
        let rc = unsafe { GetCurrentPackageFullName(&mut len, PWSTR::null()) };
        if rc == APPMODEL_ERROR_NO_PACKAGE || rc != ERROR_INSUFFICIENT_BUFFER || len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        // SAFETY: `buf` has `len` writable UTF-16 elements for `GetCurrentPackageFullName`.
        let rc2 = unsafe { GetCurrentPackageFullName(&mut len, PWSTR(buf.as_mut_ptr())) };
        if rc2 != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let name = String::from_utf16_lossy(&buf[..end]).trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    struct LocalMemGuard(HLOCAL);

    impl Drop for LocalMemGuard {
        fn drop(&mut self) {
            if !self.0 .0.is_null() {
                // SAFETY: `self.0` was allocated by `ConvertSidToStringSidW` or
                // `ConvertStringSecurityDescriptorToSecurityDescriptorW` and freed once on drop.
                unsafe {
                    let _ = LocalFree(self.0);
                }
            }
        }
    }

    fn current_user_sid_string() -> Result<String> {
        let mut token = HANDLE::default();
        // SAFETY: Queries current process token with read-only `TOKEN_QUERY`; closed via `OwnedHandle`.
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| {
                GreenCError::Win32 {
                    code: e.code().0 as u32,
                    message: format!("OpenProcessToken failed: {e}"),
                }
            })?;
        }
        let token_guard = OwnedHandle::new(token).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid process token handle".to_string(),
        })?;

        let mut token_len = 0u32;
        // SAFETY: First pass queries required `TokenUser` byte size.
        let _ =
            unsafe { GetTokenInformation(token_guard.raw(), TokenUser, None, 0, &mut token_len) };
        if token_len == 0 {
            return Err(GreenCError::Win32 {
                code: 122,
                message: "GetTokenInformation(TokenUser) returned zero size".to_string(),
            });
        }
        let mut token_buf = vec![0u8; token_len as usize];
        // SAFETY: `token_buf` has exact length `token_len`.
        unsafe {
            GetTokenInformation(
                token_guard.raw(),
                TokenUser,
                Some(token_buf.as_mut_ptr().cast()),
                token_len,
                &mut token_len,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("GetTokenInformation(TokenUser) failed: {e}"),
            })?;
        }
        // SAFETY: Buffer starts with `TOKEN_USER` populated by `GetTokenInformation`.
        let user_sid: PSID =
            unsafe { std::ptr::read_unaligned(token_buf.as_ptr().cast::<TOKEN_USER>()) }
                .User
                .Sid;

        let mut sid_pwstr = PWSTR::null();
        // SAFETY: `user_sid` points into `token_buf`; returned string is freed via `LocalMemGuard`.
        unsafe {
            ConvertSidToStringSidW(user_sid, &mut sid_pwstr).map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("ConvertSidToStringSidW failed: {e}"),
            })?;
        }
        let _sid_guard = LocalMemGuard(HLOCAL(sid_pwstr.0.cast()));
        if sid_pwstr.is_null() {
            return Err(GreenCError::Win32 {
                code: 87,
                message: "ConvertSidToStringSidW returned null".to_string(),
            });
        }
        // SAFETY: `sid_pwstr` is a NUL-terminated UTF-16 string allocated by Windows.
        let sid_str = unsafe { sid_pwstr.to_string() }.map_err(|e| GreenCError::Win32 {
            code: 87,
            message: format!("Invalid UTF-16 SID string: {e}"),
        })?;
        Ok(sid_str)
    }

    /// Creates `dir` with a protected DACL granting Full Access ONLY to the current user SID and
    /// LocalSystem (`D:PAI(A;OICI;FA;;;{user_sid})(A;OICI;FA;;;SY)`), blocking other local users.
    pub fn create_private_dir_current_user_only(dir: &Path) -> Result<()> {
        if let Some(parent) = dir.parent() {
            if std::fs::symlink_metadata(parent).is_err() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let user_sid = current_user_sid_string()?;
        let sddl = format!("D:PAI(A;OICI;FA;;;{user_sid})(A;OICI;FA;;;SY)");
        let sddl_wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();

        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl_wide` is NUL-terminated UTF-16; allocated descriptor is freed on drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl_wide.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!(
                    "ConvertStringSecurityDescriptorToSecurityDescriptorW failed: {e}"
                ),
            })?;
        }
        let _sd_guard = LocalMemGuard(HLOCAL(sd.0));

        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: BOOL(0),
        };
        let wide_dir = path_to_wide(dir);
        // SAFETY: `wide_dir` is NUL-terminated UTF-16 and `sa` points to a live self-relative SD.
        unsafe {
            CreateDirectoryW(PCWSTR(wide_dir.as_ptr()), Some(&sa)).map_err(|e| {
                GreenCError::Win32 {
                    code: e.code().0 as u32,
                    message: format!("CreateDirectoryW('{}') failed: {e}", dir.display()),
                }
            })?;
        }
        Ok(())
    }

    struct ProcThreadAttrListGuard {
        _storage: Vec<usize>,
        list: LPPROC_THREAD_ATTRIBUTE_LIST,
        initialized: bool,
    }

    impl ProcThreadAttrListGuard {
        fn new(attr_count: u32) -> Result<Self> {
            let mut size: usize = 0;
            // SAFETY: First call queries required byte size for `attr_count` attributes.
            let _ = unsafe {
                InitializeProcThreadAttributeList(
                    LPPROC_THREAD_ATTRIBUTE_LIST::default(),
                    attr_count,
                    0,
                    &mut size,
                )
            };
            if size == 0 {
                return Err(GreenCError::Win32 {
                    code: 87,
                    message: "InitializeProcThreadAttributeList returned size 0".to_string(),
                });
            }
            let word_count = size.div_ceil(std::mem::size_of::<usize>());
            let mut storage = vec![0usize; word_count];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast::<c_void>());
            // SAFETY: `storage` is pointer-aligned, has at least `size` bytes, and lives as long as `Self`.
            unsafe {
                InitializeProcThreadAttributeList(list, attr_count, 0, &mut size).map_err(|e| {
                    GreenCError::Win32 {
                        code: e.code().0 as u32,
                        message: format!("InitializeProcThreadAttributeList failed: {e}"),
                    }
                })?;
            }
            Ok(Self {
                _storage: storage,
                list,
                initialized: true,
            })
        }

        fn raw(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
            self.list
        }
    }

    impl Drop for ProcThreadAttrListGuard {
        fn drop(&mut self) {
            if self.initialized {
                // SAFETY: `self.list` was initialized by `InitializeProcThreadAttributeList` and
                // its backing `_storage` is still alive during `Drop::drop`.
                unsafe {
                    DeleteProcThreadAttributeList(self.list);
                }
            }
        }
    }

    pub fn spawn_restore_helper_breakaway(
        helper_exe: &Path,
        job_dir: &PrivateBreakawayJobDir,
        stdin_json: &[u8],
    ) -> Result<BreakawayExecOutput> {
        // Write request into a private file inside the 0-other-user-permissions job directory and
        // redirect the child's stdin/stdout/stderr to private files inside the same directory.
        // The command line contains ONLY `"<helper_exe>" __restore-helper` (zero path arguments).
        let stdin_path = job_dir.path().join("request.json");
        let stdout_path = job_dir.path().join("response.json");
        let stderr_path = job_dir.path().join("stderr.log");

        {
            let mut f = std::fs::File::create(&stdin_path)?;
            f.write_all(stdin_json)?;
            f.sync_all()?;
        }

        let inherit_sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: BOOL(1),
        };

        let stdin_wide = path_to_wide(&stdin_path);
        let stdout_wide = path_to_wide(&stdout_path);
        let stderr_wide = path_to_wide(&stderr_path);

        // SAFETY: All wide paths are NUL-terminated; returned handles are immediately wrapped in
        // `OwnedHandle` so they close deterministically on every return path.
        let stdin_handle = OwnedHandle::new(unsafe {
            CreateFileW(
                PCWSTR(stdin_wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ,
                Some(&inherit_sa),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("CreateFileW(stdin) failed: {e}"),
            })?
        })
        .ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid stdin handle".to_string(),
        })?;

        let stdout_handle = OwnedHandle::new(unsafe {
            CreateFileW(
                PCWSTR(stdout_wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                Some(&inherit_sa),
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("CreateFileW(stdout) failed: {e}"),
            })?
        })
        .ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid stdout handle".to_string(),
        })?;

        let stderr_handle = OwnedHandle::new(unsafe {
            CreateFileW(
                PCWSTR(stderr_wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                Some(&inherit_sa),
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!("CreateFileW(stderr) failed: {e}"),
            })?
        })
        .ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid stderr handle".to_string(),
        })?;

        let attr_list = ProcThreadAttrListGuard::new(2)?;
        let breakaway_policy: u32 = PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_ENABLE_PROCESS_TREE;
        let inherited_handles: [HANDLE; 3] =
            [stdin_handle.raw(), stdout_handle.raw(), stderr_handle.raw()];

        // SAFETY: `breakaway_policy` and `inherited_handles` remain alive on the stack until after
        // `CreateProcessW` returns and `attr_list` is dropped.
        unsafe {
            UpdateProcThreadAttribute(
                attr_list.raw(),
                0,
                PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY,
                Some((&breakaway_policy as *const u32).cast::<c_void>()),
                std::mem::size_of::<u32>(),
                None,
                None,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!(
                    "UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY) failed: {e}"
                ),
            })?;

            UpdateProcThreadAttribute(
                attr_list.raw(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                Some(inherited_handles.as_ptr().cast::<c_void>()),
                std::mem::size_of_val(&inherited_handles),
                None,
                None,
            )
            .map_err(|e| GreenCError::Win32 {
                code: e.code().0 as u32,
                message: format!(
                    "UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_HANDLE_LIST) failed: {e}"
                ),
            })?;
        }

        let clean_exe_str = helper_exe.to_string_lossy();
        let clean_exe_str = clean_exe_str
            .strip_prefix(r"\\?\")
            .unwrap_or(&clean_exe_str);
        let app_wide: Vec<u16> = clean_exe_str.encode_utf16().chain(Some(0)).collect();
        let cmd_str = format!("\"{clean_exe_str}\" __restore-helper");
        let mut cmd_wide: Vec<u16> = cmd_str.encode_utf16().chain(Some(0)).collect();

        let mut si_ex = STARTUPINFOEXW::default();
        si_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si_ex.StartupInfo.hStdInput = stdin_handle.raw();
        si_ex.StartupInfo.hStdOutput = stdout_handle.raw();
        si_ex.StartupInfo.hStdError = stderr_handle.raw();
        si_ex.lpAttributeList = attr_list.raw();

        let mut pi = PROCESS_INFORMATION::default();
        // SAFETY: `si_ex.StartupInfo.cb` is `size_of::<STARTUPINFOEXW>()`, `EXTENDED_STARTUPINFO_PRESENT`
        // is set, `attr_list` + `breakaway_policy` + `inherited_handles` remain live across the call,
        // and `cmd_wide` is a mutable NUL-terminated UTF-16 buffer.
        let create_res = unsafe {
            CreateProcessW(
                PCWSTR(app_wide.as_ptr()),
                PWSTR(cmd_wide.as_mut_ptr()),
                None,
                None,
                true,
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                None,
                PCWSTR::null(),
                &si_ex.StartupInfo,
                &mut pi,
            )
        };

        // Release attribute list and parent stdio handles before waiting / reading outputs.
        drop(attr_list);
        drop(stdin_handle);
        drop(stdout_handle);
        drop(stderr_handle);

        create_res.map_err(|e| GreenCError::Win32 {
            code: e.code().0 as u32,
            message: format!("CreateProcessW(breakaway __restore-helper) failed: {e}"),
        })?;

        let proc_guard = OwnedHandle::new(pi.hProcess).ok_or_else(|| GreenCError::Win32 {
            code: 6,
            message: "Invalid child process handle".to_string(),
        })?;
        let _thread_guard = OwnedHandle::new(pi.hThread);

        // SAFETY: `proc_guard.raw()` is a live process handle; wait until helper finishes.
        unsafe {
            let _ = WaitForSingleObject(proc_guard.raw(), INFINITE);
        }
        let mut exit_code: u32 = 1;
        // SAFETY: `proc_guard.raw()` is a live process handle and `exit_code` is writable stack memory.
        unsafe {
            GetExitCodeProcess(proc_guard.raw(), &mut exit_code).map_err(|e| {
                GreenCError::Win32 {
                    code: e.code().0 as u32,
                    message: format!("GetExitCodeProcess failed: {e}"),
                }
            })?;
        }
        drop(proc_guard);

        let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
        let stderr = std::fs::read_to_string(&stderr_path).unwrap_or_default();

        Ok(BreakawayExecOutput {
            exit_code: exit_code as i32,
            stdout,
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn breakaway_constants_match_win32_abi() {
        assert_eq!(PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY, 0x0002_0012);
        assert_eq!(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, 0x0002_0002);
        assert_eq!(
            PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_ENABLE_PROCESS_TREE,
            0x0000_0001
        );
        assert_eq!(EXIT_HELPER_STILL_IN_PACKAGE, 78);
    }

    #[test]
    fn private_job_dir_rejects_appdata_and_cleans_up_on_drop() {
        let tmp = tempdir().unwrap();
        // Not under %TEMP%: the guard deliberately exempts AppData\Local\Temp. Rejected before any
        // directory is created.
        let fake_appdata = Path::new(r"C:\Users\a\AppData\Local\GreenC");
        let err = PrivateBreakawayJobDir::create_in(fake_appdata).unwrap_err();
        assert!(matches!(err, GreenCError::PreconditionBlocked { .. }));

        let ok_base = tmp.path().join("OutsideAppData");
        let job_path;
        {
            let job = PrivateBreakawayJobDir::create_in(&ok_base).unwrap();
            job_path = job.path().to_path_buf();
            assert!(job.path().is_dir());
            assert!(job.payload_dir().is_dir());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(job.path()).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "job dir must be 0700 (zero other permissions)");
            }
        }
        assert!(
            !job_path.exists(),
            "job dir must be cleaned up automatically on drop"
        );
    }

    #[test]
    fn guidance_message_mentions_non_appdata_folder_and_preserving_vault() {
        let msg = format_breakaway_unavailable_guidance("khôi phục Vault", "policy blocked");
        assert!(msg.contains("GreenC Restored"));
        assert!(msg.contains("GIỮ NGUYÊN"));
        assert!(msg.contains("--to"));
    }

    #[test]
    fn restore_helper_protocol_roundtrips_json() {
        let req = RestoreHelperRequest::RestoreFiles {
            payload_root: PathBuf::from("/tmp/job/payload"),
            conflict_strategy: HelperConflictStrategy::OverwriteIfZeroLength,
            verify_sha256: true,
            restore_hardlinks: true,
            restore_timestamps: true,
            items: vec![RestoreFileJob {
                original_path: PathBuf::from("/tmp/orig.dat"),
                staged_source: PathBuf::from("/tmp/job/payload/0.bin"),
                dest_path: PathBuf::from("/tmp/orig.dat"),
                expected_sha256: "a".repeat(64),
                expected_size: 12,
                atime: 100.0,
                mtime: 200.0,
                hardlink_siblings: vec![],
            }],
        };
        let encoded = serde_json::to_string(&req).unwrap();
        let decoded: RestoreHelperRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(req, decoded);

        let resp = RestoreHelperResponse::still_in_package(Some("Test.Pkg_1.0_x64"));
        assert!(!resp.ok);
        assert!(!resp.running_outside_package);
        assert_eq!(resp.error_code.as_deref(), Some("still_in_package"));
    }
}
