//! Comprehensive tests for safe Zalo PC discovery:
//! - Valid `config.json` (`db_path.location` + `resource.location.path`)
//! - Strict cache allowlist (root + `Partitions\*`) & protected dirs (`Local Storage`, `IndexedDB`, `media`, `message`, etc.)
//! - Old version retention (running + newest; or 2 newest when none running; >= 3 days age)
//! - Missing `config.json` (skip, never guess)
//! - Malicious `config.json` (`..`, UNC, relative, oversized, symlink/junction)
//! - Already on non-system drive detection

use greenc_scanner::zalo::{
    discover_zalo_installation, parse_zalo_version, validate_zalo_config_raw_path,
    ZaloConfigStatus, ZaloDiscoveryOptions, MAX_ZALO_CONFIG_BYTES,
};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

/// Sets times on a file or directory; plain `File::open` cannot touch directories on Windows.
fn open_for_times(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES + FILE_FLAG_BACKUP_SEMANTICS
        std::fs::OpenOptions::new()
            .access_mode(0x0100)
            .share_mode(7)
            .custom_flags(0x0200_0000)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)
    }
}

fn set_mtime_days_ago(path: &std::path::Path, days: u64) {
    let ts = SystemTime::now() - Duration::from_secs(days * 86_400);
    let _ = open_for_times(path).and_then(|f| f.set_modified(ts));
}

#[test]
fn test_zalo_discovery_full_tree_and_strict_allowlist() {
    let tmp = TempDir::new().unwrap();
    let sys_root = tmp.path().join("C_Drive");
    let zalo_data = sys_root.join("Zalo Data");
    fs::create_dir_all(zalo_data.join("media")).unwrap();
    fs::create_dir_all(zalo_data.join("message")).unwrap();
    fs::write(zalo_data.join("media").join("photo.jpg"), vec![1u8; 10_000]).unwrap();
    fs::write(
        zalo_data.join("media").join("thumb.rescache"),
        vec![2u8; 5_000],
    )
    .unwrap();
    fs::write(zalo_data.join("message").join("zalo.db"), vec![3u8; 20_000]).unwrap();

    let roaming = tmp.path().join("AppData").join("Roaming").join("ZaloData");
    // Allowlisted caches
    for rel in [
        "Cache",
        "Code Cache",
        "GPUCache",
        "DawnCache",
        "resp_cache",
        "Partitions/zalo/Cache",
        "Partitions/zalo/Code Cache",
        "Partitions/zalo/GPUCache",
        "Partitions/zalo/DawnCache",
    ] {
        let dir = roaming.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("data_0"), vec![4u8; 1_024]).unwrap();
    }
    // Protected dirs that MUST NEVER appear in cache_dirs
    for prot in [
        "Local Storage",
        "Session Storage",
        "IndexedDB",
        "WebStorage",
        "Network",
        "databases",
        "shared_proto_db",
        "Service Worker",
        "blob_storage",
        "tf-models",
        "Partitions/zalo/Local Storage",
        "Partitions/zalo/IndexedDB",
    ] {
        let dir = roaming.join(prot);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("secret.ldb"), vec![9u8; 2_048]).unwrap();
    }

    let cfg_file = roaming.join("config.json");
    let db_loc = zalo_data
        .join("message")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let res_loc = sys_root.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &cfg_file,
        format!(
            r#"{{"db_path": {{"location": "{db_loc}"}}, "resource": {{"location": {{"path": "{res_loc}"}}}}}}"#
        ),
    )
    .unwrap();

    let local = tmp.path().join("AppData").join("Local");
    let v_new = local.join("Programs/Zalo/Zalo-26.11.12");
    let v_mid = local.join("Programs/Zalo/Zalo-26.11.10");
    let v_old = local.join("Programs/Zalo/Zalo-26.9.10");
    for (dir, days) in [(&v_new, 1), (&v_mid, 5), (&v_old, 20)] {
        fs::create_dir_all(dir).unwrap();
        let exe = dir.join("Zalo.exe");
        fs::write(&exe, vec![7u8; 4_096]).unwrap();
        set_mtime_days_ago(&exe, days);
        set_mtime_days_ago(dir, days);
    }

    let mut running = HashSet::new();
    running.insert(v_new.clone());

    let opts = ZaloDiscoveryOptions {
        config_file: Some(cfg_file),
        local_appdata: Some(local),
        system_drive_root: Some(sys_root),
        running_exe_dirs: running,
        ..Default::default()
    };

    let disc = discover_zalo_installation(&opts).expect("Zalo should be discovered");
    assert!(disc.installed);
    assert_eq!(disc.config_status, ZaloConfigStatus::Found);
    assert_eq!(disc.data_dir.as_deref(), Some(zalo_data.as_path()));
    assert!(disc.data_on_system_drive);
    assert_eq!(disc.media_bytes, 15_000);
    assert_eq!(disc.media_file_count, 2);
    assert_eq!(disc.message_bytes, 20_000);
    assert_eq!(disc.message_file_count, 1);
    assert_eq!(disc.chat_data_bytes, 35_000);
    assert_eq!(disc.cache_dirs.len(), 9);
    assert_eq!(disc.cache_bytes, 9 * 1_024);

    // Protected dirs are NEVER in cache_dirs
    for c in &disc.cache_dirs {
        let s = c.path.to_string_lossy().to_lowercase();
        assert!(!s.contains("local storage"));
        assert!(!s.contains("indexeddb"));
        assert!(!s.contains("service worker"));
        assert!(!s.contains("tf-models"));
        assert!(!s.contains("media"));
        assert!(!s.contains("message"));
    }

    // Running is newest -> both older versions (>= 3 days) are eligible!
    assert_eq!(disc.old_version_dirs.len(), 2);
    assert!(disc.old_version_dirs.contains(&v_mid));
    assert!(disc.old_version_dirs.contains(&v_old));
    assert!(disc.kept_version_dirs.contains(&v_new));
    assert_eq!(disc.old_versions_bytes, 2 * 4_096);
}

#[test]
fn test_zalo_old_versions_keep_two_newest_when_not_running_and_respect_3_day_age() {
    let tmp = TempDir::new().unwrap();
    let local = tmp.path().join("Local");
    let v1 = local.join("Programs/Zalo/Zalo-26.11.12");
    let v2 = local.join("Programs/Zalo/Zalo-26.11.10");
    let v3 = local.join("Programs/Zalo/Zalo-26.9.10");
    let v4_recent = local.join("Programs/Zalo/Zalo-26.8.1");
    for (dir, days) in [(&v1, 10), (&v2, 10), (&v3, 10), (&v4_recent, 1)] {
        fs::create_dir_all(dir).unwrap();
        let exe = dir.join("Zalo.exe");
        fs::write(&exe, vec![1u8; 1_000]).unwrap();
        set_mtime_days_ago(&exe, days);
        set_mtime_days_ago(dir, days);
    }

    let opts = ZaloDiscoveryOptions {
        config_file: Some(tmp.path().join("Roaming/ZaloData/config.json")),
        local_appdata: Some(local),
        running_exe_dirs: HashSet::new(), // NOT running!
        ..Default::default()
    };

    let disc = discover_zalo_installation(&opts).unwrap();
    assert_eq!(disc.config_status, ZaloConfigStatus::Missing);
    assert!(disc.data_dir.is_none()); // missing config -> skip, do NOT guess!
                                      // Keeps v1 (newest), v2 (2nd newest because not running), and v4_recent (< 3 days)
    assert!(disc.kept_version_dirs.contains(&v1));
    assert!(disc.kept_version_dirs.contains(&v2));
    assert!(disc.kept_version_dirs.contains(&v4_recent));
    assert_eq!(disc.old_version_dirs, vec![v3]);
}

#[test]
fn test_zalo_config_rejects_malicious_paths_and_oversized_and_symlinks() {
    assert!(validate_zalo_config_raw_path(r"C:\Users\u\..\Windows\System32").is_err());
    assert!(validate_zalo_config_raw_path(r"\\evil-server\share\Zalo Data\message").is_err());
    assert!(validate_zalo_config_raw_path("//evil-server/share/Zalo Data/message").is_err());
    assert!(validate_zalo_config_raw_path("relative/Zalo Data/message").is_err());
    assert!(validate_zalo_config_raw_path("C:relative\\message").is_err());
    assert!(validate_zalo_config_raw_path("C:\\Valid\\Zalo Data\\message").is_ok());

    let tmp = TempDir::new().unwrap();
    let roaming = tmp.path().join("Roaming/ZaloData");
    fs::create_dir_all(&roaming).unwrap();
    let cfg = roaming.join("config.json");

    // 1. Traversal attack in config.json
    fs::write(
        &cfg,
        r#"{"db_path": {"location": "/tmp/../etc/passwd/message"}}"#,
    )
    .unwrap();
    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg.clone()),
        local_appdata: Some(tmp.path().join("Local")),
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        disc.config_status,
        ZaloConfigStatus::RejectedUnsafe { .. }
    ));
    assert!(disc.data_dir.is_none());

    // 2. UNC path in config.json
    fs::write(
        &cfg,
        r#"{"db_path": {"location": "\\\\10.0.0.1\\share\\Zalo Data\\message"}}"#,
    )
    .unwrap();
    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg.clone()),
        local_appdata: Some(tmp.path().join("Local")),
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        disc.config_status,
        ZaloConfigStatus::RejectedUnsafe { .. }
    ));
    assert!(disc.data_dir.is_none());

    // 3. Oversized config.json (> 256 KiB)
    fs::write(&cfg, vec![b' '; (MAX_ZALO_CONFIG_BYTES + 16) as usize]).unwrap();
    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg.clone()),
        local_appdata: Some(tmp.path().join("Local")),
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        disc.config_status,
        ZaloConfigStatus::RejectedUnsafe { .. }
    ));
    assert!(disc.data_dir.is_none());

    // 4. Symlink pointing to foreign target
    #[cfg(unix)]
    {
        let target_real = tmp.path().join("RealTarget");
        fs::create_dir_all(target_real.join("message")).unwrap();
        let link_data = tmp.path().join("FakeZaloDataLink");
        std::os::unix::fs::symlink(&target_real, &link_data).unwrap();
        let db_loc = link_data.join("message");
        fs::write(
            &cfg,
            format!(r#"{{"db_path": {{"location": "{}"}}}}"#, db_loc.display()),
        )
        .unwrap();

        let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
            config_file: Some(cfg.clone()),
            local_appdata: Some(tmp.path().join("Local")),
            ..Default::default()
        })
        .unwrap();
        assert!(matches!(
            disc.config_status,
            ZaloConfigStatus::RejectedUnsafe { .. }
        ));
        assert!(disc.data_dir.is_none());

        // Same symlink IS accepted when explicitly listed in known_relocated_sources
        let mut known = HashSet::new();
        known.insert(link_data.clone());
        let disc_known = discover_zalo_installation(&ZaloDiscoveryOptions {
            config_file: Some(cfg),
            local_appdata: Some(tmp.path().join("Local")),
            known_relocated_sources: known,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(disc_known.config_status, ZaloConfigStatus::Found);
        assert!(disc_known.data_is_known_relocation);
    }
}

#[test]
fn test_zalo_already_on_other_drive_detection() {
    let tmp = TempDir::new().unwrap();
    let roaming = tmp.path().join("Roaming/ZaloData");
    fs::create_dir_all(&roaming).unwrap();
    let cfg = roaming.join("config.json");
    fs::write(
        &cfg,
        r#"{"db_path": {"location": "F:\\Zalo Data\\message"}}"#,
    )
    .unwrap();

    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg),
        local_appdata: Some(tmp.path().join("Local")),
        system_drive_root: Some(PathBuf::from(r"C:\")),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(disc.config_status, ZaloConfigStatus::Found);
    assert_eq!(disc.data_drive.as_deref(), Some("F:"));
    assert!(!disc.data_on_system_drive);
}

#[test]
fn test_parse_zalo_version_numeric_ordering() {
    let (_, a) = parse_zalo_version("Zalo-26.11.12").unwrap();
    let (_, b) = parse_zalo_version("Zalo-26.11.10").unwrap();
    let (_, c) = parse_zalo_version("Zalo-26.9.10").unwrap();
    assert!(a > b && b > c);
    assert!(parse_zalo_version("Zalo-latest").is_none());
    assert!(parse_zalo_version("Other-1.2.3").is_none());
}
