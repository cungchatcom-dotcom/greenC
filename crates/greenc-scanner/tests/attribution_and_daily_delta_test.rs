use greenc_core::models::{DriveInfo, WriteReasonCode};
use greenc_platform::restart_mgr::{LockingProcessInfo, MockRestartManagerApi};
use greenc_platform::traits::MockVolumeApi;
use greenc_scanner::attribution::{classify_write_reason, AttributionEngine};
use greenc_scanner::tracker::DiskTracker;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn test_classify_all_11_write_reason_codes() {
    let cases: &[(&str, WriteReasonCode)] = &[
        (
            r"C:\Users\me\AppData\Roaming\DingTalk\123_v2\EAppFiles\download\bundle.amr",
            WriteReasonCode::EappBundle,
        ),
        (
            r"F:\Zalo Data\media\ZaloDownloads\resource\chat1\img.jpg",
            WriteReasonCode::MediaChat,
        ),
        (
            r"C:\Users\me\AppData\Local\Programs\Zalo\Zalo-24.8.1\Zalo.exe",
            WriteReasonCode::OldAppVersion,
        ),
        (
            r"C:\Users\me\AppData\Local\CrashDumps\app.1234.dmp",
            WriteReasonCode::CrashDump,
        ),
        (
            r"C:\Users\me\AppData\Local\MyApp\logs\runtime.log",
            WriteReasonCode::LogGrowth,
        ),
        (
            r"C:\Users\me\AppData\Roaming\Orphan\old.dat",
            WriteReasonCode::OrphanLeftover,
        ),
        (
            r"C:\Users\me\AppData\Local\Google\Chrome\User Data\Default\Cache\data_1",
            WriteReasonCode::CacheTemp,
        ),
        (
            r"C:\Users\me\AppData\Roaming\MyApp\Database\index.sqlite3",
            WriteReasonCode::DbStorage,
        ),
        (
            r"C:\Users\me\AppData\Local\Docker\wsl\data\ext4.vhdx",
            WriteReasonCode::ContainerVm,
        ),
        (
            r"C:\Users\me\Downloads\ubuntu-24.04.iso",
            WriteReasonCode::MediaDownload,
        ),
        (
            r"C:\Users\me\AppData\Roaming\CustomStudio\workspace\state.bin",
            WriteReasonCode::AppData,
        ),
    ];

    for (raw_path, expected) in cases {
        let (actual, label) = classify_write_reason(Path::new(raw_path));
        assert_eq!(actual, *expected, "Mismatch for path {raw_path}");
        assert!(!label.is_empty());
    }
}

#[test]
fn test_attribution_prefers_restart_manager_lock_and_scm_service() {
    let tmp = TempDir::new().unwrap();
    let locked_file = tmp.path().join("active.log");
    fs::write(&locked_file, vec![b'x'; 1024]).unwrap();

    let mock_rm = Arc::new(MockRestartManagerApi::new());
    mock_rm.register_pid_service(4242, "Spooler", "Print Spooler");
    mock_rm.register_lock(
        &locked_file,
        LockingProcessInfo {
            pid: 4242,
            app_name: "spoolsv.exe".to_string(),
            process_name: "spoolsv.exe".to_string(),
            service_short_name: "Spooler".to_string(),
        },
    );

    let engine = AttributionEngine::new(mock_rm);
    let attr = engine.attribute_path(&locked_file, true);
    assert_eq!(attr.pid, 4242);
    assert_eq!(attr.process_name, "spoolsv.exe");
    assert_eq!(attr.service_name, "Spooler");
    assert_eq!(attr.service_display_name, "Print Spooler");
    assert_eq!(attr.reason_code, WriteReasonCode::LogGrowth);
}

#[test]
fn test_disk_tracker_daily_snapshots_and_drilldown() {
    let tmp = TempDir::new().unwrap();
    let db = Arc::new(GreenCDb::open(tmp.path().join("tracker.db")).unwrap());

    let scan_root = tmp.path().join("ZaloData").join("Cache");
    fs::create_dir_all(&scan_root).unwrap();
    // Write >= 64 KiB so the bucket exceeds MIN_BUCKET_DELTA_BYTES.
    fs::write(scan_root.join("chunk.cache"), vec![7u8; 128 * 1024]).unwrap();

    let mock_vol = Arc::new(MockVolumeApi::new(vec![DriveInfo {
        root: PathBuf::from("C:\\"),
        drive_letter: 'C',
        label: "Windows".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 100 * 1024 * 1024 * 1024,
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    }]));

    let tracker = DiskTracker::with_dependencies(
        db.clone(),
        AttributionEngine::new(Arc::new(MockRestartManagerApi::new())),
        mock_vol,
    );

    let snapshots = tracker
        .scan_drive_history_with_roots("C:\\", 7, std::slice::from_ref(&scan_root), None)
        .unwrap();
    assert_eq!(snapshots.len(), 7);
    let today_snap = snapshots.last().unwrap();
    assert!(today_snap.net_change_bytes >= 128 * 1024);

    let today_str = today_snap.snapshot_date.format("%Y-%m-%d").to_string();
    let drill = tracker.get_daily_drilldown("C:\\", &today_str).unwrap();
    assert!(!drill.folders.is_empty());
    assert!(!drill.actors.is_empty());
    assert_eq!(drill.actors[0].process_name, "Zalo.exe");

    // Verify signed deletion accounting via record_live_delta
    tracker
        .record_live_delta("C:\\", &scan_root.join("chunk.cache"), -(64 * 1024))
        .unwrap();
    let drill_after = tracker.get_daily_drilldown("C:\\", &today_str).unwrap();
    assert_eq!(drill_after.folders[0].total_delta_bytes, 64 * 1024);
}

#[test]
fn test_disk_tracker_unix_mount_normalization_and_scan() {
    assert_eq!(DiskTracker::normalize_drive("/"), "/");
    assert_eq!(DiskTracker::normalize_drive("/home/"), "/home");
    assert_eq!(DiskTracker::normalize_drive("/mnt/data/"), "/mnt/data");
    assert_eq!(DiskTracker::normalize_drive("c"), "C:\\");

    let tmp = TempDir::new().unwrap();
    let db = Arc::new(GreenCDb::open(tmp.path().join("unix_tracker.db")).unwrap());
    let scan_root = tmp.path().join("cache");
    fs::create_dir_all(&scan_root).unwrap();
    fs::write(scan_root.join("blob.cache"), vec![3u8; 128 * 1024]).unwrap();

    let mock_vol = Arc::new(MockVolumeApi::new(vec![DriveInfo {
        root: PathBuf::from("/home"),
        drive_letter: 'H',
        label: "home".to_string(),
        filesystem: "EXT4".to_string(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        free_bytes: 40 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    }]));

    let tracker = DiskTracker::with_dependencies(
        db,
        AttributionEngine::new(Arc::new(MockRestartManagerApi::new())),
        mock_vol,
    );
    let snapshots = tracker
        .scan_drive_history_with_roots("/home/", 3, std::slice::from_ref(&scan_root), None)
        .unwrap();
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots.last().unwrap().drive_letter, "/home");
    assert!(snapshots.last().unwrap().net_change_bytes >= 128 * 1024);
}
