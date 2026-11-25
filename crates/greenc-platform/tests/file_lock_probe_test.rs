#![cfg_attr(unix, allow(unsafe_code))]

use greenc_platform::handle::is_file_locked;
use std::fs::OpenOptions;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_exclusive_lock_detection() {
    let mut temp = NamedTempFile::new().unwrap();
    writeln!(temp, "test lock content").unwrap();
    let _path = temp.path().to_path_buf();

    // Initially not locked by another exclusive handle
    // Note: NamedTempFile handle on Windows may hold sharing modes, let's test with a fresh file path
    drop(temp);
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("probe_test.dat");
    std::fs::write(&file_path, b"hello").unwrap();

    assert!(!is_file_locked(&file_path));

    // Open file and hold OS-appropriate exclusive lock
    let open_file = OpenOptions::new().write(true).open(&file_path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: `open_file` owns a valid open file descriptor.
        let rc = unsafe { libc::flock(open_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(rc, 0, "Failed to acquire flock for Unix lock probe test");
    }

    // Now is_file_locked must report true
    assert!(is_file_locked(&file_path));

    // Close handle by dropping it
    drop(open_file);

    // Now unlocked
    assert!(!is_file_locked(&file_path));
}

#[cfg(unix)]
#[test]
fn plain_open_fd_without_advisory_lock_is_not_reported_locked_on_unix() {
    let temp_dir = tempfile::tempdir().unwrap();
    let file_path = temp_dir.path().join("plain_open.dat");
    std::fs::write(&file_path, b"gradle cache artifact").unwrap();

    // Opening a file on Unix without flock/fcntl does NOT acquire an advisory lock.
    // This verifies why process-level busy-tool detection is required on Linux/macOS.
    let _plain_open = OpenOptions::new().read(true).open(&file_path).unwrap();
    assert!(!is_file_locked(&file_path));
}
