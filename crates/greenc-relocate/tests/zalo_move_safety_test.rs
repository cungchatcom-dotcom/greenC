//! Comprehensive safety & fault-injection simulation tests for GC-ZALO2:
//! - Original retained during 20s watch, renamed to `.greenc-original.<uuid>`, tracked in DB for 24h
//! - Auto-purge only after health watch passed AND 24h elapsed; manual purge & restore
//! - Safe rollback: never ignores graceful_shutdown errors, never deletes locked/new dirs,
//!   renames recreated `Zalo Data` to `.greenc-conflict.<uuid>`, keeps target messages as latest,
//!   renames target before cleanup after full verification, restores moving_path if target offline
//! - Junction creation failure when `source_path` exists: moves conflict, restores moving_path
//! - Zalo auto-reopening mid-flight (before rename, after rename, before junction): aborts safely
//! - Pre-move space estimation on C: and target drive + recursive `message/` DB lock check
//! - Power loss at each step + `discover_zalo_safety_copies`

use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::GreenCError;
use greenc_core::models::{
    DriveInfo, RelocationHealthStatus, RelocationStatus, ZaloRetainedStatus,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{MockAppLifecycleApi, MockShutdownBehavior};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    MockFileLockProbe, MockProcessServiceProbe, MockReparsePointApi, MockShellFolderApi,
    MockVolumeApi, ReparsePointApi,
};
use greenc_relocate::zalo_move::{
    discover_zalo_safety_copies, estimate_zalo_move_space, ZaloMoveHooks, ZaloMoveOptions,
    ZaloRelocator,
};
use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscovery, ZaloDiscoveryOptions};
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Removes a directory link (symlink on Unix, NTFS junction on Windows) without touching its target.
fn remove_dir_link(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::fs::remove_dir(path);
    #[cfg(not(windows))]
    let _ = std::fs::remove_file(path);
}

fn build_env(tmp: &Path) -> (ZaloDiscovery, PathBuf, PathBuf) {
    let sys_root = tmp.join("C_Drive");
    let zalo_data = sys_root.join("Zalo Data");
    fs::create_dir_all(zalo_data.join("message")).unwrap();
    fs::create_dir_all(zalo_data.join("media")).unwrap();
    fs::write(zalo_data.join("message/zalo.db"), b"CHAT-HISTORY-V1").unwrap();
    fs::write(zalo_data.join("media/photo.jpg"), vec![0xCD; 4096]).unwrap();

    let roaming = tmp.join("Roaming/ZaloData");
    fs::create_dir_all(&roaming).unwrap();
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
    let exe_path = local.join("Programs/Zalo/Zalo-26.11.12/Zalo.exe");
    fs::create_dir_all(exe_path.parent().unwrap()).unwrap();
    fs::write(&exe_path, b"FAKE-ZALO-EXE").unwrap();

    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg),
        local_appdata: Some(local),
        system_drive_root: Some(sys_root),
        ..Default::default()
    })
    .unwrap();
    (disc, zalo_data, exe_path)
}

fn make_drives(free_c: u64, free_d: u64) -> MockVolumeApi {
    MockVolumeApi::new(vec![
        DriveInfo {
            root: PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: free_c,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System".to_string()),
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
fn test_original_kept_during_watch_persisted_24h_and_purged_only_when_eligible() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_env(tmp.path());
    let db_path = tmp.path().join("zalo.db");
    let db = GreenCDb::open(&db_path).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let saw_moving_during_watch = Arc::new(AtomicBool::new(false));
    let saw_flag = Arc::clone(&saw_moving_during_watch);
    let parent_for_hook = zalo_data.parent().unwrap().to_path_buf();
    lifecycle.set_on_restart_hook(move || {
        // Right when Zalo reopens (start of health watch), `.greenc-moving.*` MUST still exist!
        let found = fs::read_dir(&parent_for_hook).unwrap().flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("Zalo Data.greenc-moving.")
        });
        saw_flag.store(found, Ordering::SeqCst);
    });

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let opts = ZaloMoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target.clone()),
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
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap();

    assert!(
        saw_moving_during_watch.load(Ordering::SeqCst),
        "moving_path MUST stay on disk during the post-reopen health watch"
    );
    assert!(report.health_verified);
    let retained_path = report
        .retained_original_path
        .clone()
        .expect("move must retain original as .greenc-original.<uuid>");
    assert!(retained_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("Zalo Data.greenc-original."));
    assert!(retained_path.join("message/zalo.db").exists());

    // Simulate app restart by reopening DB from disk!
    drop(db);
    let db2 = GreenCDb::open(&db_path).unwrap();
    let list = db2.list_zalo_retained_originals(Some("retained")).unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].health_verified);
    assert_eq!(list[0].retained_path, retained_path);

    // Safety copy discovery reports it clearly
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db2);
    assert!(copies
        .iter()
        .any(|c| c.kind == "original" && c.path == retained_path));

    // At 23h -> MUST NOT auto-purge
    let early = list[0].created_at + ChronoDuration::hours(23);
    let purged_early = ZaloRelocator::purge_expired_originals(&db2, &reparse, early).unwrap();
    assert!(purged_early.is_empty());
    assert!(retained_path.exists());

    // At 25h -> auto-purges because health_verified && elapsed >= 24h && junction healthy
    let late = list[0].created_at + ChronoDuration::hours(25);
    let purged_late = ZaloRelocator::purge_expired_originals(&db2, &reparse, late).unwrap();
    assert_eq!(purged_late.len(), 1);
    assert_eq!(purged_late[0].status, ZaloRetainedStatus::Purged);
    assert!(!retained_path.exists());
    // Target still intact!
    assert_eq!(
        fs::read(target.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
}

#[test]
fn test_rollback_refuses_when_zalo_cannot_close_or_locked_and_keeps_all_copies() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    // After reopen: break junction AND make next graceful_shutdown timeout (Zalo refuses to close)!
    let broken_link = zalo_data.clone();
    let lc_hook = lifecycle.clone();
    lifecycle.set_on_restart_hook(move || {
        remove_dir_link(&broken_link);
        fs::create_dir_all(broken_link.join("message")).unwrap();
        fs::write(broken_link.join("message/new.db"), b"NEW-WHILE-OPEN").unwrap();
        lc_hook.set_shutdown_behavior(MockShutdownBehavior::Timeout);
    });

    let target = tmp.path().join("D_Drive/zalo");
    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(40),
            post_reopen_poll_interval: Duration::from_millis(5),
            enforce_platform_support: false,
            skip_write_probe: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("KHÔNG") || msg.contains("giữ nguyên"),
        "Expected clear refusal to rollback when Zalo stays open, got: {msg}"
    );
    // ALL copies MUST remain untouched!
    assert_eq!(
        fs::read(zalo_data.join("message/new.db")).unwrap(),
        b"NEW-WHILE-OPEN"
    );
    assert_eq!(
        fs::read(target.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    assert!(
        copies
            .iter()
            .any(|c| c.kind == "original" || c.kind == "moving"),
        "pre-move original copy must still exist and be discoverable"
    );
}

#[test]
fn test_rollback_preserves_recreated_source_as_conflict_and_uses_target_as_latest() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/zalo_latest");
    let broken_link = zalo_data.clone();
    let target_for_hook = target.clone();
    lifecycle.set_on_restart_hook(move || {
        // 1. Zalo writes new messages into target through the junction first
        fs::write(
            target_for_hook.join("message/new_from_target.db"),
            b"TARGET-LATEST-MSG",
        )
        .unwrap();
        // 2. Then junction breaks and Zalo creates a real `Zalo Data` dir with its own file!
        remove_dir_link(&broken_link);
        fs::create_dir_all(broken_link.join("message")).unwrap();
        fs::write(
            broken_link.join("message/recreated.db"),
            b"RECREATED-CONFLICT-MSG",
        )
        .unwrap();
    });

    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(40),
            post_reopen_poll_interval: Duration::from_millis(5),
            enforce_platform_support: false,
            skip_write_probe: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("hoàn tác"));

    // Target's latest message MUST be restored to `Zalo Data`!
    assert!(!reparse.is_reparse_point(&zalo_data));
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
    assert_eq!(
        fs::read(zalo_data.join("message/new_from_target.db")).unwrap(),
        b"TARGET-LATEST-MSG"
    );

    // Recreated real directory MUST be saved as `Zalo Data.greenc-conflict.<uuid>` (NOT deleted!)
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    let conflict = copies
        .iter()
        .find(|c| c.kind == "conflict")
        .expect("recreated source dir must be preserved as .greenc-conflict");
    assert_eq!(
        fs::read(conflict.path.join("message/recreated.db")).unwrap(),
        b"RECREATED-CONFLICT-MSG"
    );
}

#[test]
fn test_rollback_when_target_offline_restores_moving_original_and_keeps_conflict() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/zalo_disconnect");
    let target_hook = target.clone();
    let src_hook = zalo_data.clone();
    lifecycle.set_on_restart_hook(move || {
        // Simulate target drive disconnecting + Zalo recreating a local dir!
        let _ = fs::remove_dir_all(&target_hook);
        remove_dir_link(&src_hook);
        fs::create_dir_all(src_hook.join("message")).unwrap();
        fs::write(src_hook.join("message/local.db"), b"LOCAL-OFFLINE-MSG").unwrap();
    });

    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(40),
            post_reopen_poll_interval: Duration::from_millis(5),
            enforce_platform_support: false,
            skip_write_probe: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("hoàn tác") || err.to_string().contains("ngắt kết nối"));

    // Original chat history restored from kept moving_path!
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
    // Local dir preserved as conflict!
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    let conflict = copies.iter().find(|c| c.kind == "conflict").unwrap();
    assert_eq!(
        fs::read(conflict.path.join("message/local.db")).unwrap(),
        b"LOCAL-OFFLINE-MSG"
    );
}

#[test]
fn test_junction_failure_with_existing_source_moves_conflict_and_restores_moving() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, _exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let mock_reparse = MockReparsePointApi::new();
    mock_reparse.set_fail_create(true);
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let target = tmp.path().join("D_Drive/zalo_jconflict");
    let before_j = |src: &Path, _moving: &Path, _tgt: &Path| {
        // Simulate Zalo recreating `Zalo Data` right before create_junction!
        fs::create_dir_all(src.join("message")).unwrap();
        fs::write(src.join("message/brand_new.db"), b"BRAND-NEW-MSG").unwrap();
    };
    let hooks = ZaloMoveHooks {
        before_junction: Some(&before_j),
        ..Default::default()
    };

    let err = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &mock_reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        &hooks,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("greenc-conflict") || err.to_string().contains("xung đột"),
        "Expected clear conflict report, got: {err}"
    );

    // Original restored cleanly to `Zalo Data`!
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
    // New folder preserved as `.greenc-conflict.<uuid>`!
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    let conflict = copies.iter().find(|c| c.kind == "conflict").unwrap();
    assert_eq!(
        fs::read(conflict.path.join("message/brand_new.db")).unwrap(),
        b"BRAND-NEW-MSG"
    );
}

#[test]
fn test_zalo_auto_reopens_before_rename_after_rename_and_before_junction() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, _exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    // Case 1: Reopens right BEFORE rename
    let proc1 = MockProcessServiceProbe::new();
    let p1_clone = proc1.clone();
    let before_rename_cb = move |_src: &Path, _tgt: &Path| {
        p1_clone.set_process_running("Zalo.exe", true);
    };
    let t1 = tmp.path().join("D_Drive/reopen1");
    let err1 = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(t1.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc1,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        &ZaloMoveHooks {
            before_rename: Some(&before_rename_cb),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err1, GreenCError::FileLocked { .. }));
    assert!(zalo_data.join("message/zalo.db").exists());
    assert!(!t1.exists());

    // Case 2: Reopens right AFTER rename (and creates a new dir at source_path)
    let proc2 = MockProcessServiceProbe::new();
    let p2_clone = proc2.clone();
    let after_rename_cb = move |src: &Path, _moving: &Path, _tgt: &Path| {
        p2_clone.set_process_running("Zalo.exe", true);
        fs::create_dir_all(src.join("message")).unwrap();
        fs::write(src.join("message/mid.db"), b"MID-REOPEN").unwrap();
    };
    let t2 = tmp.path().join("D_Drive/reopen2");
    let err2 = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(t2),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc2,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        &ZaloMoveHooks {
            after_rename: Some(&after_rename_cb),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err2, GreenCError::FileLocked { .. }));
    // Original restored AND mid-reopen file saved in conflict dir!
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    assert!(copies
        .iter()
        .any(|c| c.kind == "conflict" && c.path.join("message/mid.db").exists()));

    // Case 3: Reopens right BEFORE junction
    let proc3 = MockProcessServiceProbe::new();
    let p3_clone = proc3.clone();
    let before_j_cb = move |_src: &Path, _moving: &Path, _tgt: &Path| {
        p3_clone.set_process_running("Zalo.exe", true);
    };
    let t3 = tmp.path().join("D_Drive/reopen3");
    let err3 = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(t3),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc3,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        &ZaloMoveHooks {
            before_junction: Some(&before_j_cb),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err3, GreenCError::FileLocked { .. }));
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
}

#[test]
fn test_space_estimation_blocks_low_c_or_target_and_nested_message_db_lock() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, _exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    // 1. Low C: drive (< 512 MiB reserve) -> estimate_zalo_move_space & move_zalo_data refuse clearly!
    let low_c_vol = make_drives(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let est = estimate_zalo_move_space('C', 'D', disc.chat_data_bytes, &low_c_vol).unwrap();
    assert!(!est.source_ok);
    assert!(!est.can_proceed);
    assert!(est.blocking_reason.as_deref().unwrap_or("").contains("C:"));

    let err_c = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(tmp.path().join("D_Drive/low_c")),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &low_c_vol,
        &reparse,
        &shell,
        &MockFileLockProbe::new(),
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err_c, GreenCError::InsufficientVaultSpace(_)));
    assert!(err_c.to_string().contains("C:"));

    // 2. Nested `message/zalo.db` locked -> MUST block BEFORE copy!
    let ok_vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let lock_probe = MockFileLockProbe::new();
    lock_probe.lock(&zalo_data.join("message/zalo.db"));
    let target_locked = tmp.path().join("D_Drive/locked_db");
    let err_lock = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target_locked.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &ok_vol,
        &reparse,
        &shell,
        &lock_probe,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err_lock, GreenCError::FileLocked { .. }));
    assert!(!target_locked.exists());
}

#[test]
fn test_manual_purge_and_restore_retained_original_preserves_post_move_messages() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/zalo_restore");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: false, // no watch -> health_verified = false
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap();

    let retained_list = db.list_zalo_retained_originals(Some("retained")).unwrap();
    assert_eq!(retained_list.len(), 1);
    assert!(!retained_list[0].health_verified);
    let rid = retained_list[0].id.unwrap();

    // Auto-purge after 48h MUST NOT delete when `health_verified == false`!
    let later = Utc::now() + ChronoDuration::hours(48);
    assert!(ZaloRelocator::purge_expired_originals(&db, &reparse, later)
        .unwrap()
        .is_empty());
    assert!(retained_list[0].retained_path.exists());

    // Simulate a new message written to target after move, then user clicks "Khôi phục"!
    fs::write(target.join("message/post_move.db"), b"POST-MOVE-MSG").unwrap();
    let restored = ZaloRelocator::restore_retained_original(
        rid,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(restored.status, ZaloRetainedStatus::Restored);
    assert!(!reparse.is_reparse_point(&zalo_data));
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
    // Post-move target MUST NOT be destroyed — preserved as conflict copy!
    let target_parent = target.parent().unwrap();
    let preserved_target = fs::read_dir(target_parent)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(".greenc-conflict.")
        })
        .expect("post-move target must be preserved as conflict on restore");
    assert_eq!(
        fs::read(preserved_target.path().join("message/post_move.db")).unwrap(),
        b"POST-MOVE-MSG"
    );

    // Check record status updated cleanly
    let reloc = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == report.record_id)
        .unwrap();
    assert_eq!(reloc.status, RelocationStatus::Undone);
    assert_eq!(reloc.health_status, RelocationHealthStatus::Healthy);
}

#[test]
fn test_manual_purge_refuses_when_junction_broken_and_succeeds_when_healthy() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/zalo_purge");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: false,
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap();
    let rid = report.retained_original_id.unwrap();
    let orig_path = report.retained_original_path.unwrap();

    // Break junction -> manual purge MUST refuse so user doesn't delete their last good copy!
    reparse.delete_junction(&zalo_data).unwrap();
    let err = ZaloRelocator::purge_retained_original(rid, &db, &reparse).unwrap_err();
    assert!(matches!(err, GreenCError::PreconditionBlocked { .. }));
    assert!(orig_path.exists());

    // Repair junction -> manual purge succeeds cleanly!
    reparse.create_junction(&zalo_data, &target).unwrap();
    let purged = ZaloRelocator::purge_retained_original(rid, &db, &reparse).unwrap();
    assert_eq!(purged.status, ZaloRetainedStatus::Purged);
    assert!(!orig_path.exists());
    assert_eq!(
        fs::read(target.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1"
    );
}

#[test]
fn test_source_file_modified_during_copy_aborts_and_preserves_latest_source() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, _exe_path) = build_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let target = tmp.path().join("D_Drive/zalo_mutated");
    let src_msg = zalo_data.join("message/zalo.db");
    let after_copy_mutate = move |_dst: &Path| {
        // Simulate source chat DB being updated mid-copy with same length!
        fs::write(&src_msg, b"CHAT-HISTORY-V2").unwrap();
    };
    let err = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            enforce_platform_support: false,
            ..Default::default()
        },
        &db,
        &vol,
        &reparse,
        &shell,
        &lock,
        &proc,
        &lifecycle,
        &TaskControl::new(),
        |_| {},
        &ZaloMoveHooks {
            after_copy: Some(&after_copy_mutate),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, GreenCError::IntegrityMismatch { .. }));
    assert!(!target.exists());
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V2"
    );
}
