//! Comprehensive tests for safe Zalo PC cleanup:
//! - Strict allowlist only (`Cache`, `Code Cache`, `GPUCache`, `DawnCache`, `resp_cache`,
//!   and `Partitions\*\{Cache, Code Cache, GPUCache, DawnCache}`)
//! - NEVER touches `message`, `media`, `Local Storage`, `Session Storage`, `IndexedDB`,
//!   `WebStorage`, `Network`, `databases`, `shared_proto_db`, `Service Worker`, `blob_storage`,
//!   `tf-models`, `config.json`, or `*.db*`
//! - Keeps running + newest version (or 2 newest when not running) & >= 3 days old
//! - Skips locked files while Zalo is running and reports `"Còn X MB sẽ dọn được khi đóng Zalo"`
//! - Respects `vault` vs `direct` regenerable cache modes and reports real net gain
//! - Guard integration: OFF by default (`include_zalo_auto_clean = false`), works when ON

use chrono::Utc;
use greenc_core::models::{DriveInfo, DriveQuota, RegenerableCacheMode};
use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::MockDevToolProbe;
use greenc_platform::traits::{MockFileLockProbe, MockVolumeApi};
use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscoveryOptions};
use greenc_storage::db::GreenCDb;
use greenc_vault::guard::{enforce_drive_quota_units, GuardRunOptions};
use greenc_vault::zalo_cleanup::{
    execute_zalo_cleanup, scan_zalo_cleanup_units, ZaloCleanupOptions,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
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

fn age_file(path: &Path, secs_ago: u64) {
    let ts = SystemTime::now() - Duration::from_secs(secs_ago);
    let times = fs::FileTimes::new().set_accessed(ts).set_modified(ts);
    let _ = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .or_else(|_| open_for_times(path))
        .and_then(|f| f.set_times(times));
}

fn setup_fake_zalo_tree(tmp: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let zalo_data = tmp.join("Zalo Data");
    fs::create_dir_all(zalo_data.join("media")).unwrap();
    fs::create_dir_all(zalo_data.join("message")).unwrap();
    let media_file = zalo_data.join("media").join("pic.jpg");
    let msg_db = zalo_data.join("message").join("zalo.db");
    fs::write(&media_file, vec![1u8; 64 * 1024]).unwrap();
    fs::write(&msg_db, vec![2u8; 128 * 1024]).unwrap();
    age_file(&media_file, 10 * 86_400);
    age_file(&msg_db, 10 * 86_400);

    let roaming = tmp.join("Roaming").join("ZaloData");
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
        let f = dir.join("blob_001.bin");
        fs::write(&f, vec![b'A'; 32 * 1024]).unwrap();
        age_file(&f, 3600);
        age_file(&dir, 3600);
    }

    // Forbidden directories + files
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
        let f = dir.join("important.dat");
        fs::write(&f, vec![9u8; 16 * 1024]).unwrap();
        age_file(&f, 10 * 86_400);
    }

    // Put a stray *.db inside Cache to ensure *.db* is NEVER selected even there
    let stray_db = roaming.join("Cache").join("index.db");
    fs::write(&stray_db, vec![8u8; 4096]).unwrap();
    age_file(&stray_db, 10 * 86_400);

    let cfg = roaming.join("config.json");
    let db_loc = zalo_data
        .join("message")
        .to_string_lossy()
        .replace('\\', "\\\\");
    fs::write(
        &cfg,
        format!(r#"{{"db_path": {{"location": "{db_loc}"}}}}"#),
    )
    .unwrap();

    let local = tmp.join("Local");
    let v_run = local.join("Programs/Zalo/Zalo-26.11.12");
    let v_old = local.join("Programs/Zalo/Zalo-26.9.10");
    for (v, days) in [(&v_run, 1), (&v_old, 10)] {
        fs::create_dir_all(v).unwrap();
        let exe = v.join("Zalo.exe");
        let dll = v.join("core.dll");
        fs::write(&exe, vec![b'B'; 48 * 1024]).unwrap();
        fs::write(&dll, vec![b'C'; 48 * 1024]).unwrap();
        age_file(&exe, days * 86_400);
        age_file(&dll, days * 86_400);
        age_file(v, days * 86_400);
    }

    (cfg, local, roaming, zalo_data, v_run, v_old)
}

#[test]
fn test_zalo_clean_vault_mode_protects_all_forbidden_and_reports_locked_while_running() {
    let tmp = TempDir::new().unwrap();
    let (cfg, local, roaming, zalo_data, v_run, v_old) = setup_fake_zalo_tree(tmp.path());

    let mut running = HashSet::new();
    running.insert(v_run.clone());

    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg.clone()),
        local_appdata: Some(local),
        running_exe_dirs: running,
        ..Default::default()
    })
    .unwrap();

    // Simulate Zalo running and locking Partitions/zalo/Cache/blob_001.bin (make it 2 MiB)
    let locked_cache = roaming.join("Partitions/zalo/Cache/locked_big.bin");
    fs::write(&locked_cache, vec![b'Z'; 2 * 1024 * 1024]).unwrap();
    age_file(&locked_cache, 3600);
    let lock_probe = MockFileLockProbe::new();
    lock_probe.lock(&locked_cache);

    let db = GreenCDb::open(tmp.path().join("test.db")).unwrap();
    let vault_dir = tmp.path().join("Vault");
    fs::create_dir_all(&vault_dir).unwrap();
    let vol_api = MockVolumeApi::new(vec![DriveInfo {
        root: tmp.path().to_path_buf(),
        drive_letter: 'C',
        label: "System".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        free_bytes: 20 * 1024 * 1024 * 1024,
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    }]);

    let ctrl = TaskControl::new();
    let report = execute_zalo_cleanup(
        &disc,
        &vault_dir,
        &db,
        &vol_api,
        &lock_probe,
        true, // Zalo is running
        &ZaloCleanupOptions {
            mode: RegenerableCacheMode::Vault,
            ..Default::default()
        },
        &ctrl,
    )
    .unwrap();

    assert!(!report.dry_run);
    assert_eq!(report.mode, RegenerableCacheMode::Vault);
    assert!(report.vault_archive_id.is_some());
    assert!(report.cleaned_files >= 11); // 9 cache blobs + 2 old version files
    assert!(report.logical_freed_bytes > 0);
    assert!(report.locked_skipped_bytes >= 2 * 1024 * 1024);
    let notice = report
        .locked_notice
        .expect("must report remaining MB when Zalo is running");
    assert!(
        notice.contains("2 MB") && notice.contains("khi đóng Zalo"),
        "unexpected locked notice: {notice}"
    );

    // Locked cache file still intact
    assert!(locked_cache.exists());
    // Old version directory removed cleanly; running version intact
    assert!(!v_old.exists());
    assert!(v_run.join("Zalo.exe").exists());
    // ALL forbidden paths & files MUST be 100% untouched!
    assert!(zalo_data.join("media/pic.jpg").exists());
    assert!(zalo_data.join("message/zalo.db").exists());
    assert!(cfg.exists());
    assert!(roaming.join("Cache/index.db").exists());
    for prot in [
        "Local Storage/important.dat",
        "Session Storage/important.dat",
        "IndexedDB/important.dat",
        "WebStorage/important.dat",
        "Network/important.dat",
        "databases/important.dat",
        "shared_proto_db/important.dat",
        "Service Worker/important.dat",
        "blob_storage/important.dat",
        "tf-models/important.dat",
        "Partitions/zalo/Local Storage/important.dat",
        "Partitions/zalo/IndexedDB/important.dat",
    ] {
        assert!(
            roaming.join(prot).exists(),
            "protected file missing: {prot}"
        );
    }
}

#[test]
fn test_zalo_clean_direct_mode_frees_full_bytes_without_vault() {
    let tmp = TempDir::new().unwrap();
    let (cfg, local, _roaming, zalo_data, v_run, v_old) = setup_fake_zalo_tree(tmp.path());

    let mut running = HashSet::new();
    running.insert(v_run.clone());
    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg),
        local_appdata: Some(local),
        running_exe_dirs: running,
        ..Default::default()
    })
    .unwrap();

    let db = GreenCDb::open(tmp.path().join("test.db")).unwrap();
    let vault_dir = tmp.path().join("Vault");
    fs::create_dir_all(&vault_dir).unwrap();
    let vol_api = MockVolumeApi::new(vec![]);
    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();

    let report = execute_zalo_cleanup(
        &disc,
        &vault_dir,
        &db,
        &vol_api,
        &lock_probe,
        false,
        &ZaloCleanupOptions {
            mode: RegenerableCacheMode::Direct,
            ..Default::default()
        },
        &ctrl,
    )
    .unwrap();

    assert_eq!(report.mode, RegenerableCacheMode::Direct);
    assert!(report.vault_archive_id.is_none());
    assert_eq!(report.vault_bytes, 0);
    assert_eq!(report.net_freed_bytes, report.logical_freed_bytes as i64);
    assert!(!v_old.exists());
    assert!(v_run.exists());
    assert!(zalo_data.join("message/zalo.db").exists());
    assert!(zalo_data.join("media/pic.jpg").exists());
}

#[test]
fn test_guard_respects_zalo_auto_clean_opt_in_default_off() {
    let tmp = TempDir::new().unwrap();
    let (cfg, local, _roaming, zalo_data, v_run, v_old) = setup_fake_zalo_tree(tmp.path());

    let mut running = HashSet::new();
    running.insert(v_run.clone());
    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg),
        local_appdata: Some(local),
        running_exe_dirs: running,
        ..Default::default()
    })
    .unwrap();

    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();
    let scan = scan_zalo_cleanup_units(
        &disc,
        SystemTime::now(),
        Duration::from_secs(60),
        Duration::from_secs(3 * 86_400),
        &lock_probe,
        false,
        &ctrl,
    );
    assert!(!scan.units.is_empty());

    let db = GreenCDb::open(tmp.path().join("guard.db")).unwrap();
    let vault_dir = tmp.path().join("Vault");
    fs::create_dir_all(&vault_dir).unwrap();
    let dev_probe = MockDevToolProbe::default();

    // Critical drive (95% full)
    let drive_root = tmp.path().to_string_lossy().to_string();
    let quota = DriveQuota {
        id: Some(1),
        drive_root: drive_root.clone(),
        limit_bytes: 0,
        warn_ratio: 0.80,
        clean_ratio: 0.90,
        target_ratio: 0.85,
        auto_clean: true,
        auto_backup: false,
        backup_dir: None,
        enabled: true,
        regenerable_cache_mode: RegenerableCacheMode::Vault,
        created_at: Utc::now(),
    };
    let vol_api = MockVolumeApi::new(vec![DriveInfo {
        root: tmp.path().to_path_buf(),
        drive_letter: 'C',
        label: "System".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        free_bytes: 5 * 1024 * 1024 * 1024, // 95% full
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    }]);

    // 1. Default GuardRunOptions: include_zalo_auto_clean = false -> NOTHING cleaned!
    let opts_off = GuardRunOptions {
        ignore_cooldown: true,
        ..Default::default()
    };
    assert!(!opts_off.include_zalo_auto_clean);
    let out_off = enforce_drive_quota_units(
        &quota,
        scan.units.clone(),
        &vault_dir,
        &db,
        &vol_api,
        &lock_probe,
        &dev_probe,
        &opts_off,
        &ctrl,
    )
    .unwrap();
    assert_eq!(out_off.cleaned_files, 0);
    assert!(
        v_old.exists(),
        "Zalo old version must NOT be touched when opt-in is OFF"
    );

    // 2. Opt-in enabled: include_zalo_auto_clean = true -> Zalo cache & old version cleaned into Vault!
    let opts_on = GuardRunOptions {
        ignore_cooldown: true,
        include_zalo_auto_clean: true,
        ..Default::default()
    };
    let out_on = enforce_drive_quota_units(
        &quota,
        scan.units,
        &vault_dir,
        &db,
        &vol_api,
        &lock_probe,
        &dev_probe,
        &opts_on,
        &ctrl,
    )
    .unwrap();
    assert!(out_on.cleaned_files >= 11);
    assert!(out_on.vault_zip.is_some());
    assert!(!v_old.exists());
    // Chat data untouched!
    assert!(zalo_data.join("media/pic.jpg").exists());
    assert!(zalo_data.join("message/zalo.db").exists());
}
