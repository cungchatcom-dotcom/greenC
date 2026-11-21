use greenc_core::config::*;
use std::path::Path;

#[test]
fn test_default_patterns_matching() {
    // Should match protected patterns
    assert!(is_protected_filename("app.exe", None));
    assert!(is_protected_filename("kernel.sys", None));
    assert!(is_protected_filename("library.dll", None));
    assert!(is_protected_filename("greenc.db", None));
    assert!(is_protected_filename("greenc.db-wal", None));
    assert!(is_protected_filename("greenc.db-shm", None));
    assert!(is_protected_filename("greenc.sqlite", None));
    assert!(is_protected_filename("greenc.sqlite3", None));
    assert!(is_protected_filename("settings.json", None));
    assert!(is_protected_filename("config.toml", None));
    assert!(is_protected_filename("config.yaml", None));
    assert!(is_protected_filename("config.yml", None));
    assert!(is_protected_filename("config.ini", None));
    assert!(is_protected_filename("config.cfg", None));
    assert!(is_protected_filename("app.config", None));
    assert!(is_protected_filename("db.mdf", None));
    assert!(is_protected_filename("db.ldf", None));
    assert!(is_protected_filename("cache.rescache", None));
    assert!(is_protected_filename("lockfile", None));
    assert!(is_protected_filename("CURRENT", None));
    assert!(is_protected_filename("MANIFEST-0001", None));

    // Custom pattern
    let custom = vec!["*.custom".to_string()];
    assert!(is_protected_filename("test.custom", Some(&custom)));

    // Non-protected candidates
    assert!(!is_protected_filename("app.log", None));
    assert!(!is_protected_filename("photo.png", None));
    assert!(!is_protected_filename("document.docx", None));
    assert!(!is_protected_filename("temp_data.tmp", None));
}

#[test]
fn test_retention_bounds_and_paths() {
    assert_eq!(MIN_RETENTION_DAYS, 1);
    assert_eq!(MAX_RETENTION_DAYS, 90);
    assert_eq!(DEFAULT_RETENTION_DAYS, 7);

    assert_eq!(MIN_VAULT_RESERVE_BYTES, 512 * 1024 * 1024);

    #[cfg(windows)]
    {
        let drive = Path::new("D:\\");
        let root = data_root_for(drive);
        assert_eq!(root, Path::new("D:\\GreenC"));
    }
    #[cfg(unix)]
    {
        let drive = Path::new("/Volumes/Data");
        let root = data_root_for(drive);
        assert_eq!(root, Path::new("/Volumes/Data/GreenC"));
    }

    let app_dir = app_home();
    assert!(app_dir
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("greenc"));

    let db_path = default_db_path();
    assert!(db_path.to_string_lossy().ends_with("greenc.db"));
}
