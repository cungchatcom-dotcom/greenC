//! Unix process running check for `NativeProcessServiceProbe`.

/// Returns true if a process matching `exe_name` (case-insensitive, ignoring `.exe` suffix) is running.
pub fn is_process_running_unix(exe_name: &str) -> bool {
    let trimmed = exe_name.trim();
    if trimmed.is_empty() {
        return false;
    }
    let normalized = trimmed
        .strip_suffix(".exe")
        .or_else(|| trimmed.strip_suffix(".EXE"))
        .unwrap_or(trimmed);

    #[cfg(target_os = "linux")]
    {
        let target_low = normalized.to_ascii_lowercase();
        // SAFETY: `geteuid()` is a standard POSIX syscall with no pointer arguments.
        let my_uid = unsafe { libc::geteuid() };
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let s = fname.to_string_lossy();
                if !s.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
                    if comm.trim().to_ascii_lowercase() == target_low {
                        // F10: Filter by current user session (UID) so another user's process
                        // does not count as alive in our session.
                        if let Ok(status) = std::fs::read_to_string(entry.path().join("status")) {
                            let same_uid = status.lines().any(|line| {
                                line.strip_prefix("Uid:")
                                    .and_then(|rest| rest.split_whitespace().next())
                                    .and_then(|uid_s| uid_s.parse::<u32>().ok())
                                    == Some(my_uid)
                            });
                            if !same_uid {
                                continue;
                            }
                        }
                        return true;
                    }
                }
            }
        }
        false
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pgrep")
            .args(["-ix", normalized])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = normalized;
        false
    }
}
