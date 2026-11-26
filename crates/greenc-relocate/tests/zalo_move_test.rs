//! Comprehensive tests for Zalo PC chat-data transactional relocation:
//! - Happy path: graceful close, verified copy (message SHA-256 + media count), real symlink/junction
//!   write probe, reopen, 20s health watch (fast in test), config.json UNMODIFIED, 1-click Undo
//! - `--no-reopen` flag skips reopening Zalo
//! - Active call (`ZaloCall.exe` visible window) -> refused with PreconditionBlocked
//! - Graceful close timeout -> clear tray exit guidance, never force kills
//! - Cancel mid-move -> target cleaned, original untouched
//! - Hash mismatch in `message/` -> rolled back, original untouched
//! - Target drive full -> blocked before touching source
//! - Junction create failure -> atomic rollback to original
//! - Power loss (`Moving` state) -> `RecoveryService::recover_interrupted` restores original
//! - Junction breaks during post-reopen 20s watch -> automatic undo back to original
//! - Already on another drive (F:) -> reports already on F:, does not move
//! - Platform/Store guard (`enforce_platform_support = true`) -> refuses on Linux/Store

use chrono::Utc;
use greenc_core::error::GreenCError;
use greenc_core::models::{
    DriveInfo, RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{
    MockAppLifecycleApi, MockShutdownBehavior, ZALO_ACTIVE_CALL_MESSAGE, ZALO_MANUAL_QUIT_MESSAGE,
};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    MockFileLockProbe, MockProcessServiceProbe, MockReparsePointApi, MockShellFolderApi,
    MockVolumeApi, ReparsePointApi,
};
use greenc_relocate::mover::TransactionalMover;
use greenc_relocate::recovery::RecoveryService;
use greenc_relocate::zalo_move::{ZaloMoveOptions, ZaloMovePhase, ZaloRelocator};
use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscovery, ZaloDiscoveryOptions};
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// Removes a directory link (symlink on Unix, NTFS junction on Windows) without touching its target.
fn remove_dir_link(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::fs::remove_dir(path);
    #[cfg(not(windows))]
    let _ = std::fs::remove_file(path);
}

fn build_fake_zalo_env(tmp: &Path) -> (ZaloDiscovery, PathBuf, PathBuf, PathBuf, String) {
    let sys_root = tmp.join("C_Drive");
    let zalo_data = sys_root.join("Zalo Data");
    fs::create_dir_all(zalo_data.join("message")).unwrap();
    fs::create_dir_all(zalo_data.join("media")).unwrap();
    fs::write(zalo_data.join("message/zalo.db"), b"SQLITE-CHAT-DATA-001").unwrap();
    fs::write(
        zalo_data.join("message/zalo.db-wal"),
        b"WAL-ENTRIES-IMPORTANT",
    )
    .unwrap();
    fs::write(zalo_data.join("media/img1.jpg"), vec![0xAA; 8192]).unwrap();
    fs::write(zalo_data.join("media/img2.png"), vec![0xBB; 4096]).unwrap();

    let roaming = tmp.join("Roaming/ZaloData");
    fs::create_dir_all(&roaming).unwrap();
    let cfg = roaming.join("config.json");
    let db_loc = zalo_data
        .join("message")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let cfg_body = format!(r#"{{"db_path": {{"location": "{db_loc}"}}}}"#);
    fs::write(&cfg, &cfg_body).unwrap();

    let local = tmp.path_buf_join("Local");
    let exe_path = local.join("Programs/Zalo/Zalo-26.11.12/Zalo.exe");
    fs::create_dir_all(exe_path.parent().unwrap()).unwrap();
    fs::write(&exe_path, b"FAKE-ZALO-EXE").unwrap();

    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg.clone()),
        local_appdata: Some(local),
        system_drive_root: Some(sys_root),
        ..Default::default()
    })
    .unwrap();
    assert!(disc.data_on_system_drive);
    (disc, cfg, zalo_data, exe_path, cfg_body)
}

trait PathExt {
    fn path_buf_join(&self, s: &str) -> PathBuf;
}
impl PathExt for Path {
    fn path_buf_join(&self, s: &str) -> PathBuf {
        self.join(s)
    }
}

fn make_ntfs_drives(free_d: u64) -> MockVolumeApi {
    MockVolumeApi::new(vec![
        DriveInfo {
            root: PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System drive".to_string()),
        },
        DriveInfo {
            root: PathBuf::from(r"D:\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 1024 * 1024 * 1024 * 1024,
            free_bytes: free_d,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ])
}

#[test]
fn test_zalo_move_happy_path_write_probe_reopen_config_untouched_and_undo() {
    let tmp = TempDir::new().unwrap();
    let (disc, cfg, zalo_data, exe_path, cfg_before) = build_fake_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("move.db")).unwrap();
    let vol_api = make_ntfs_drives(100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi; // Real Unix symlink so write probe actually runs!
    let shell_api = MockShellFolderApi::new();
    let lock_probe = MockFileLockProbe::new();
    let proc_probe = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path.clone());

    let target_dir = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let ctrl = TaskControl::new();
    let phases: Arc<Mutex<Vec<ZaloMovePhase>>> = Arc::new(Mutex::new(Vec::new()));
    let phases_cb = Arc::clone(&phases);

    let opts = ZaloMoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target_dir.clone()),
        dry_run: false,
        reopen_zalo: true,
        post_reopen_watch_duration: Duration::from_millis(20),
        post_reopen_poll_interval: Duration::from_millis(5),
        enforce_platform_support: false,
        skip_write_probe: false,
        ..Default::default()
    };

    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &opts,
        &db,
        &vol_api,
        &reparse,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &ctrl,
        move |p| phases_cb.lock().unwrap().push(p.phase),
    )
    .expect("Zalo move must succeed");

    assert_eq!(report.status, "done");
    assert!(report.reopened_zalo);
    assert!(report.health_verified);
    assert_eq!(report.message_files_verified, 2);
    assert_eq!(report.media_files_verified, 2);
    assert_eq!(lifecycle.shutdown_count(), 1);
    assert_eq!(lifecycle.restart_count(), 1);

    // Source is now a live link pointing to target_dir, and files are readable through it!
    assert!(reparse.is_reparse_point(&zalo_data));
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"SQLITE-CHAT-DATA-001"
    );
    assert_eq!(
        fs::read(target_dir.join("media/img1.jpg")).unwrap().len(),
        8192
    );

    // config.json MUST NOT be modified!
    assert_eq!(fs::read_to_string(&cfg).unwrap(), cfg_before);

    // Verify all expected phases occurred
    let seen = phases.lock().unwrap();
    assert!(seen.contains(&ZaloMovePhase::Preparing));
    assert!(seen.contains(&ZaloMovePhase::ClosingZalo));
    assert!(seen.contains(&ZaloMovePhase::Moving));
    assert!(seen.contains(&ZaloMovePhase::Verifying));
    assert!(seen.contains(&ZaloMovePhase::Reopening));
    assert!(seen.contains(&ZaloMovePhase::HealthCheck));
    assert!(seen.contains(&ZaloMovePhase::Done));

    // 1-Click Undo restores data cleanly back to original path!
    let rec_id = report.record_id.unwrap();
    let undone = TransactionalMover::undo_relocation(
        rec_id,
        &db,
        &vol_api,
        &reparse,
        &shell_api,
        &proc_probe,
        &ctrl,
    )
    .expect("Undo must succeed");
    assert_eq!(undone.status, RelocationStatus::Undone);
    assert!(!reparse.is_reparse_point(&zalo_data));
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"SQLITE-CHAT-DATA-001"
    );
    assert!(!target_dir.exists());
}

#[test]
fn test_zalo_move_no_reopen_skips_restart() {
    let tmp = TempDir::new().unwrap();
    let (disc, _cfg, _zalo_data, exe_path, _) = build_fake_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("move.db")).unwrap();
    let vol_api = make_ntfs_drives(100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell_api = MockShellFolderApi::new();
    let lock_probe = MockFileLockProbe::new();
    let proc_probe = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let opts = ZaloMoveOptions {
        target_drive: 'D',
        custom_target_path: Some(tmp.path().join("D_Drive/zalo")),
        reopen_zalo: false,
        enforce_platform_support: false,
        ..Default::default()
    };

    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &opts,
        &db,
        &vol_api,
        &reparse,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap();
    assert!(!report.reopened_zalo);
    assert_eq!(lifecycle.restart_count(), 0);
}

#[test]
fn test_zalo_move_refuses_active_call_and_graceful_timeout() {
    let tmp = TempDir::new().unwrap();
    let (disc, _cfg, zalo_data, exe_path, _) = build_fake_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("move.db")).unwrap();
    let vol_api = make_ntfs_drives(100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell_api = MockShellFolderApi::new();
    let lock_probe = MockFileLockProbe::new();
    let proc_probe = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);
    lifecycle.set_visible_window("ZaloCall.exe", true);

    let opts = ZaloMoveOptions {
        target_drive: 'D',
        custom_target_path: Some(tmp.path().join("D_Drive/zalo")),
        enforce_platform_support: false,
        ..Default::default()
    };

    // 1. Active call -> PreconditionBlocked
    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &opts,
        &db,
        &vol_api,
        &reparse,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    match err {
        GreenCError::PreconditionBlocked { reason, .. } => {
            assert_eq!(reason, ZALO_ACTIVE_CALL_MESSAGE);
        }
        other => panic!("Expected PreconditionBlocked for active call, got {other:?}"),
    }
    assert_eq!(lifecycle.shutdown_count(), 0);

    // 2. Call ends, but Zalo refuses graceful close -> clear manual quit guidance
    lifecycle.set_visible_window("ZaloCall.exe", false);
    lifecycle.set_shutdown_behavior(MockShutdownBehavior::Timeout);
    let err2 = ZaloRelocator::move_zalo_data(
        &disc,
        &opts,
        &db,
        &vol_api,
        &reparse,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    match err2 {
        GreenCError::FileLocked { reason, .. } => {
            assert_eq!(reason, ZALO_MANUAL_QUIT_MESSAGE);
        }
        other => panic!("Expected FileLocked with manual quit message, got {other:?}"),
    }
    assert!(zalo_data.join("message/zalo.db").exists());
}

#[test]
fn test_zalo_move_rolls_back_on_hash_mismatch_cancel_full_disk_and_junction_failure() {
    let tmp = TempDir::new().unwrap();
    let (disc, _cfg, zalo_data, _exe_path, _) = build_fake_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("move.db")).unwrap();
    let shell_api = MockShellFolderApi::new();
    let lock_probe = MockFileLockProbe::new();
    let proc_probe = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    // 1. Hash mismatch in message/ -> rolls back, target removed, original intact!
    let target_hash = tmp.path().join("D_Drive/zalo_hash");
    let vol_ok = make_ntfs_drives(100 * 1024 * 1024 * 1024);
    let err_hash = ZaloRelocator::move_zalo_data_with_hook(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target_hash.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol_ok,
        &NativeReparsePointApi,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        Some(&|dst: &Path| {
            // Corrupt copied DB file with same byte length to test SHA-256 verification!
            fs::write(dst.join("message/zalo.db"), b"CORRUPTED-DATA-XX001").unwrap();
        }),
    )
    .unwrap_err();
    assert!(matches!(err_hash, GreenCError::IntegrityMismatch { .. }));
    assert!(!target_hash.exists());
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"SQLITE-CHAT-DATA-001"
    );

    // 2. Cancelled mid-copy -> original untouched, partial target cleaned
    let target_cancel = tmp.path().join("D_Drive/zalo_cancel");
    let ctrl_cancel = Arc::new(TaskControl::new());
    let ctrl_for_cb = Arc::clone(&ctrl_cancel);
    let err_cancel = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target_cancel.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol_ok,
        &NativeReparsePointApi,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &ctrl_cancel,
        move |p| {
            if p.phase == ZaloMovePhase::Moving {
                ctrl_for_cb.cancel();
            }
        },
    )
    .unwrap_err();
    assert!(matches!(err_cancel, GreenCError::Cancelled));
    assert!(!target_cancel.exists());
    assert!(zalo_data.join("message/zalo.db").exists());

    // 2b. Media file count mismatch -> rolled back, target removed, original intact!
    let target_media_err = tmp.path().join("D_Drive/zalo_media_mismatch");
    let err_media = ZaloRelocator::move_zalo_data_with_hook(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target_media_err.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol_ok,
        &NativeReparsePointApi,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        Some(&|dst: &Path| {
            let _ = fs::remove_file(dst.join("media/img2.png"));
        }),
    )
    .unwrap_err();
    assert!(matches!(err_media, GreenCError::IntegrityMismatch { .. }));
    assert!(!target_media_err.exists());
    assert!(zalo_data.join("media/img2.png").exists());

    // 3. Target drive full (< 512 MiB reserve) -> InsufficientVaultSpace
    let vol_full = make_ntfs_drives(100 * 1024 * 1024); // only 100 MiB free!
    let err_full = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(tmp.path().join("D_Drive/zalo_full")),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol_full,
        &NativeReparsePointApi,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err_full, GreenCError::InsufficientVaultSpace(_)));

    // 4. Junction creation failure -> atomic rollback to original, target cleaned!
    let mock_reparse = MockReparsePointApi::new();
    mock_reparse.set_fail_create(true);
    let target_jfail = tmp.path().join("D_Drive/zalo_jfail");
    let err_jfail = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target_jfail.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol_ok,
        &mock_reparse,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err_jfail, GreenCError::Win32 { .. }));
    assert!(!target_jfail.exists());
    assert!(zalo_data.is_dir());
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"SQLITE-CHAT-DATA-001"
    );
}

#[test]
fn test_zalo_move_auto_rolls_back_if_junction_breaks_after_reopen() {
    let tmp = TempDir::new().unwrap();
    let (disc, _cfg, zalo_data, exe_path, _) = build_fake_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("watch.db")).unwrap();
    let vol_api = make_ntfs_drives(100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell_api = MockShellFolderApi::new();
    let lock_probe = MockFileLockProbe::new();
    let proc_probe = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    // Simulate Zalo breaking/replacing the junction right after reopen!
    let broken_link = zalo_data.clone();
    lifecycle.set_on_restart_hook(move || {
        remove_dir_link(&broken_link);
        let _ = fs::create_dir_all(&broken_link); // empty regular dir replaces junction
    });

    let target_dir = tmp.path().join("D_Drive/zalo_watch");
    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target_dir.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(50),
            post_reopen_poll_interval: Duration::from_millis(5),
            enforce_platform_support: false,
            skip_write_probe: false,
            ..Default::default()
        },
        &db,
        &vol_api,
        &reparse,
        &shell_api,
        &lock_probe,
        &proc_probe,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("tự hoàn tác"),
        "Expected auto-rollback error message, got: {err}"
    );
    // Original data must be fully restored and target cleaned!
    assert!(!reparse.is_reparse_point(&zalo_data));
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"SQLITE-CHAT-DATA-001"
    );
    assert!(!target_dir.exists());
}

#[test]
fn test_zalo_power_loss_recovery_from_moving_state() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("recover.db")).unwrap();
    let source = tmp.path().join("C_Drive/Zalo Data");
    let moving = tmp.path().join("C_Drive/Zalo Data.greenc-moving.crash-123");
    let partial_target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(moving.join("message")).unwrap();
    fs::write(moving.join("message/zalo.db"), b"ORIGINAL-SAFE-DB").unwrap();
    fs::create_dir_all(partial_target.join("message")).unwrap();
    fs::write(partial_target.join("message/zalo.db"), b"ORIGINAL-SAFE-DB").unwrap();

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: source.clone(),
        target_path: partial_target.clone(),
        size_bytes: 16,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: Utc::now(),
        detail: None,
    })
    .unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &NativeReparsePointApi).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, RelocationStatus::Failed);
    assert!(source.join("message/zalo.db").exists());
    assert!(!moving.exists());
    assert!(!partial_target.exists());
}

#[test]
fn test_zalo_already_on_other_drive_and_platform_store_guard() {
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
    assert!(!disc.data_on_system_drive);

    let db = GreenCDb::open(tmp.path().join("other.db")).unwrap();
    let vol_api = make_ntfs_drives(100 * 1024 * 1024 * 1024);

    // Already on F: -> succeeds as no-op with clear message
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol_api,
        &NativeReparsePointApi,
        &MockShellFolderApi::new(),
        &MockFileLockProbe::new(),
        &MockProcessServiceProbe::new(),
        &MockAppLifecycleApi::new(),
        &TaskControl::new(),
        |_| {},
    )
    .unwrap();
    assert!(report.already_on_other_drive);
    assert!(report.message.contains("F:"));

    // Enforce platform/store support on Linux (or --features store) -> PreconditionBlocked
    #[cfg(any(not(windows), feature = "store"))]
    {
        let err = ZaloRelocator::move_zalo_data(
            &disc,
            &ZaloMoveOptions {
                target_drive: 'D',
                enforce_platform_support: true,
                ..Default::default()
            },
            &db,
            &vol_api,
            &NativeReparsePointApi,
            &MockShellFolderApi::new(),
            &MockFileLockProbe::new(),
            &MockProcessServiceProbe::new(),
            &MockAppLifecycleApi::new(),
            &TaskControl::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, GreenCError::PreconditionBlocked { .. }));
    }
}
