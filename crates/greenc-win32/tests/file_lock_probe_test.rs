use greenc_win32::handle::is_file_locked;
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

    // Open file with exclusive handle
    let _open_file = OpenOptions::new().write(true).open(&file_path).unwrap();

    // Now is_file_locked must report true
    assert!(is_file_locked(&file_path));

    // Close handle by dropping it
    drop(_open_file);

    // Now unlocked
    assert!(!is_file_locked(&file_path));
}
