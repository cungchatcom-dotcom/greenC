//! Comprehensive TDD safety tests for (Items 1-10).

use greenc_core::error::GreenCError;
use greenc_core::models::{
    DriveInfo, RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::MockAppLifecycleApi;
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi,
};
use greenc_platform::RelocationLock;
use greenc_relocate::recovery::RecoveryService;
use greenc_relocate::zalo_move::{ZaloMoveHooks, ZaloMoveOptions, ZaloRelocator};
use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscovery, ZaloDiscoveryOptions};
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn build_zalo_env(tmp: &Path) -> (ZaloDiscovery, PathBuf, PathBuf) {
    let sys_root = tmp.join("C_Drive");
    let zalo_data = sys_root.join("Zalo Data");
    fs::create_dir_all(zalo_data.join("message")).unwrap();
    fs::create_dir_all(zalo_data.join("media")).unwrap();
    fs::write(zalo_data.join("message/zalo.db"), b"CHAT-HISTORY-V1").unwrap();
    fs::write(zalo_data.join("media/photo.jpg"), vec![0xAB; 4096]).unwrap();

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
fn item1_recover_interrupted_skips_live_move_in_flight() {
    let tmp = TempDir::new().unwrap();
    let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
    let db_path = tmp.path().join("zalo.db");
    let db = GreenCDb::open(&db_path).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let recovered_during_live = Arc::new(AtomicUsize::new(usize::MAX));
    let rec_flag = Arc::clone(&recovered_during_live);
    let db_path_clone = db_path.clone();

    let after_junction = move |_src: &Path, moving_path: &Path, _tgt: &Path| {
        // Simulate another caller/process running `recover_interrupted` while this move is LIVE!
        let db2 = GreenCDb::open(&db_path_clone).unwrap();
        let reparse2 = NativeReparsePointApi;
        let recs = RecoveryService::recover_interrupted(&db2, &reparse2).unwrap();
        rec_flag.store(recs.len(), Ordering::SeqCst);
        // `moving_path` MUST still be intact (not renamed or removed by recovery!)
        assert!(
            moving_path.exists(),
            "recover_interrupted MUST NOT touch moving_path of a live move"
        );
    };

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let report = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(15),
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
        &ZaloMoveHooks {
            after_junction: Some(&after_junction),
            ..Default::default()
        },
    )
    .expect("live move must finish cleanly even when recover_interrupted ran concurrently");

    assert_eq!(
        recovered_during_live.load(Ordering::SeqCst),
        0,
        "recover_interrupted MUST skip Moving record belonging to a live operation"
    );
    assert_eq!(report.status, "done");
}

#[test]
fn item1_concurrent_zalo_move_blocked_by_machine_wide_lock() {
    let tmp = TempDir::new().unwrap();
    let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
    let db_path = tmp.path().join("zalo.db");
    let db = GreenCDb::open(&db_path).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    // Hold the machine-wide lock (simulating another CLI or GUI process running a relocation)
    let _external_lock = RelocationLock::acquire_for_db(&db_path).unwrap();

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            post_reopen_watch_duration: Duration::from_millis(15),
            post_reopen_poll_interval: Duration::from_millis(5),
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
    .unwrap_err();

    assert!(
        matches!(err, GreenCError::PreconditionBlocked { .. }),
        "expected PreconditionBlocked while lock held, got {err:?}"
    );
    assert_eq!(
        lifecycle.shutdown_count(),
        0,
        "must NOT close Zalo when machine-wide lock is held"
    );
    assert!(!target.exists(), "must NOT copy files while lock is held");
}

#[test]
fn item1_recover_interrupted_blocks_when_lock_held_and_recovers_after_release() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("zalo.db");
    let db = GreenCDb::open(&db_path).unwrap();
    let reparse = NativeReparsePointApi;

    let src = tmp.path().join("C_Drive/AppData");
    let moving = tmp.path().join("C_Drive/AppData.greenc-moving.abc");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/app");
    fs::create_dir_all(&moving).unwrap();
    fs::write(moving.join("a.txt"), b"ok").unwrap();
    fs::create_dir_all(&target).unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "demo_app".to_string(),
        app_name: "Demo".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 2,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec![],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    };
    db.save_relocation(&rec).unwrap();

    // While lock is held -> live operation -> recover_interrupted MUST NOT touch it!
    let live_lock = RelocationLock::acquire_for_db(&db_path).unwrap();
    let while_locked = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert!(while_locked.is_empty());
    assert!(moving.exists());
    assert!(!src.exists());

    // Release lock (simulating process crash/exit) -> now recover_interrupted recovers it!
    drop(live_lock);
    let after_release = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(after_release.len(), 1);
    assert!(src.join("a.txt").exists());
}

// =========================================================================
// Item 2: Dedicated safe Zalo undo path + 24h target retention + no swallowed delete errors
// =========================================================================

#[test]
fn item2_zalo_undo_strict_copy_verifies_and_retains_target_24h() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();

    let src = tmp.path().join("C_Drive/Zalo Data");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    fs::create_dir_all(target.join("media")).unwrap();
    fs::write(
        target.join("message/zalo.db"),
        b"LATEST-MESSAGES-AFTER-MOVE",
    )
    .unwrap();
    fs::write(target.join("media/pic.jpg"), vec![0x55; 2048]).unwrap();
    use greenc_platform::traits::ReparsePointApi;
    reparse.create_junction(&src, &target).unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 2074,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    };
    let rec_id = db.save_relocation(&rec).unwrap();

    let undone = greenc_relocate::mover::TransactionalMover::undo_relocation(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .expect("Zalo undo must succeed");

    assert_eq!(undone.status, RelocationStatus::Undone);
    assert!(
        !reparse.is_reparse_point(&src),
        "source must be a real dir now"
    );
    assert_eq!(
        fs::read(src.join("message/zalo.db")).unwrap(),
        b"LATEST-MESSAGES-AFTER-MOVE"
    );

    // Target MUST NOT be deleted immediately — it must be renamed & retained for 24h in DB!
    assert!(!target.exists(), "original target path should be renamed");
    let retained = db.list_zalo_retained_originals(Some("retained")).unwrap();
    assert_eq!(
        retained.len(),
        1,
        "Zalo undo must keep renamed target in zalo_retained_originals for 24h"
    );
    assert!(
        retained[0].retained_path.join("message/zalo.db").exists(),
        "retained target backup must still exist on disk with intact chat DB"
    );
    let hours = (retained[0].purge_after - retained[0].created_at).num_hours();
    assert!(
        hours >= 24,
        "retained target backup must be kept for at least 24 hours, got {hours}h"
    );
}

#[cfg(unix)]
#[test]
fn item2_zalo_undo_rejects_nested_symlink_via_strict_copy() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();

    let src = tmp.path().join("C_Drive/Zalo Data");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    fs::write(target.join("message/zalo.db"), b"CHAT").unwrap();
    // Inject nested symlink inside target — strict copy MUST refuse instead of silently skipping!
    let outside = tmp.path().join("outside_secret.txt");
    fs::write(&outside, b"secret").unwrap();
    std::os::unix::fs::symlink(&outside, target.join("message/evil_link")).unwrap();

    use greenc_platform::traits::ReparsePointApi;
    reparse.create_junction(&src, &target).unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 100,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    };
    let rec_id = db.save_relocation(&rec).unwrap();

    let err = greenc_relocate::mover::TransactionalMover::undo_relocation(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .unwrap_err();

    assert!(
        matches!(err, GreenCError::PreconditionBlocked { .. }),
        "strict copy must reject nested symlink in target, got {err:?}"
    );
    assert!(reparse.is_reparse_point(&src), "junction must stay intact");
    assert!(
        target.join("message/zalo.db").exists(),
        "target must stay intact"
    );
}

#[test]
fn item2_zalo_undo_aborts_if_target_changes_or_zalo_reopens_before_swap() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let src = tmp.path().join("C_Drive/Zalo Data");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    fs::write(target.join("message/zalo.db"), b"CHAT-V1").unwrap();
    use greenc_platform::traits::ReparsePointApi;
    reparse.create_junction(&src, &target).unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 100,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    };
    let rec_id = db.save_relocation(&rec).unwrap();

    // Case A: target file mutated right before swap -> must abort!
    let target_for_hook = target.clone();
    let mutate_hook = move || {
        fs::write(target_for_hook.join("message/zalo.db"), b"CHAT-V2-NEW-MSG").unwrap();
    };
    let err = greenc_relocate::mover::TransactionalMover::undo_relocation_with_deps(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &lock,
        Some(&lifecycle),
        &TaskControl::new(),
        Some(&mutate_hook),
    )
    .unwrap_err();
    assert!(
        matches!(err, GreenCError::IntegrityMismatch { .. }),
        "must abort with IntegrityMismatch if target changed before swap, got {err:?}"
    );
    assert!(
        reparse.is_reparse_point(&src),
        "junction must remain intact"
    );
    assert_eq!(
        fs::read(target.join("message/zalo.db")).unwrap(),
        b"CHAT-V2-NEW-MSG"
    );

    // Case B: Zalo reopens right before swap -> must abort with FileLocked!
    let proc_for_hook = proc.clone();
    let reopen_hook = move || {
        proc_for_hook.set_process_running("Zalo.exe", true);
    };
    let err2 = greenc_relocate::mover::TransactionalMover::undo_relocation_with_deps(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &lock,
        Some(&lifecycle),
        &TaskControl::new(),
        Some(&reopen_hook),
    )
    .unwrap_err();
    assert!(
        matches!(err2, GreenCError::FileLocked { .. }),
        "must abort with FileLocked if Zalo reopened before swap, got {err2:?}"
    );
    assert!(
        reparse.is_reparse_point(&src),
        "junction must remain intact"
    );
}

#[cfg(unix)]
#[test]
fn item2_undo_does_not_swallow_delete_error() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("app.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();

    let src = tmp.path().join("C_Drive/DemoApp");
    let target_parent = tmp.path().join("D_Drive/GreenC/Relocated");
    let target = target_parent.join("demo_app");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("data.bin"), b"HELLO").unwrap();
    use greenc_platform::traits::ReparsePointApi;
    reparse.create_junction(&src, &target).unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "demo_app".to_string(),
        app_name: "Demo App".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 5,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec![],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    };
    let rec_id = db.save_relocation(&rec).unwrap();

    // Make `target` and `target_parent` read-only so deleting `target` fails with PermissionDenied
    // (skip assertion if running as root uid 0, where chmod 0555 does not block unlink).
    if unsafe_free_is_non_root() {
        fs::set_permissions(&target, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&target_parent, fs::Permissions::from_mode(0o555)).unwrap();

        let res = greenc_relocate::mover::TransactionalMover::undo_relocation(
            rec_id,
            &db,
            &vol,
            &reparse,
            &shell,
            &proc,
            &TaskControl::new(),
        );

        // Restore permissions so TempDir cleanup succeeds
        let _ = fs::set_permissions(&target_parent, fs::Permissions::from_mode(0o755));
        let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));

        assert!(
            res.is_err(),
            "undo_relocation MUST NOT swallow target deletion errors; got {res:?}"
        );
    }
}

#[cfg(unix)]
fn unsafe_free_is_non_root() -> bool {
    greenc_platform::current_user_uid() != 0
}

// =========================================================================
// Item 3: Zalo recovery never deletes target; offline target -> NeedsReview;
//         detect zalo-restoring.* & restoring.<uuid>; select exact moving dir
// =========================================================================

#[test]
fn item3_zalo_recovery_never_deletes_target_renames_to_conflict() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let moving = c_dir.join("Zalo Data.greenc-moving.11111111-1111-1111-1111-111111111111");
    let target = d_dir.join("zalo");
    fs::create_dir_all(moving.join("message")).unwrap();
    fs::write(moving.join("message/zalo.db"), b"ORIGINAL-ZALO").unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    fs::write(target.join("message/zalo.db"), b"TARGET-COPY-ZALO").unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 16,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: Some(moving.clone()),
        target_user_repaired: false,
    };
    db.save_relocation(&rec).unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        fs::read(src.join("message/zalo.db")).unwrap(),
        b"ORIGINAL-ZALO"
    );

    // Target MUST NOT be deleted — it must be renamed to `.greenc-conflict.<uuid>`!
    assert!(!target.exists(), "original target path should be renamed");
    let conflict_entry = fs::read_dir(&d_dir)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("zalo.greenc-conflict.")
        })
        .expect("Zalo recovery must rename target to .greenc-conflict.<uuid>, NEVER delete it");
    assert_eq!(
        fs::read(conflict_entry.path().join("message/zalo.db")).unwrap(),
        b"TARGET-COPY-ZALO"
    );
}

#[test]
fn item3_recovery_target_offline_sets_needs_review_without_silent_restore() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    let moving = c_dir.join("Zalo Data.greenc-moving.22222222-2222-2222-2222-222222222222");
    let offline_target = tmp.path().join("D_Offline_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(moving.join("message")).unwrap();
    fs::write(moving.join("message/zalo.db"), b"OLD-BEFORE-MOVE").unwrap();
    // Create target + valid junction, then remove target to simulate drive D: going offline!
    fs::create_dir_all(offline_target.join("message")).unwrap();
    use greenc_platform::traits::ReparsePointApi;
    reparse.create_junction(&src, &offline_target).unwrap();
    fs::remove_dir_all(tmp.path().join("D_Offline_Drive")).unwrap();

    let rec = RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: offline_target.clone(),
        size_bytes: 16,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: Some(moving.clone()),
        target_user_repaired: false,
    };
    db.save_relocation(&rec).unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].status,
        RelocationStatus::NeedsReview,
        "offline target must set NeedsReview, never silently restore old copy"
    );
    assert_eq!(
        recovered[0].health_status,
        RelocationHealthStatus::TargetOffline
    );
    assert!(
        reparse.is_reparse_point(&src),
        "junction must NOT be deleted when target is merely offline"
    );
    assert!(
        moving.join("message/zalo.db").exists(),
        "moving dir must NOT be silently renamed over source when target is offline"
    );
    let detail = recovered[0].detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("ngoại tuyến") || detail.contains("offline"),
        "detail must clearly report offline target, got: {detail}"
    );
}

#[test]
fn item3_detects_zalo_restoring_and_restoring_uuid_dirs() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(&c_dir).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    fs::write(target.join("message/zalo.db"), b"TGT").unwrap();

    let zalo_restoring = c_dir.join("zalo-restoring.aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
    let generic_restoring = c_dir.join("restoring.bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
    fs::create_dir_all(zalo_restoring.join("message")).unwrap();
    fs::write(zalo_restoring.join("message/zalo.db"), b"ZR").unwrap();
    fs::create_dir_all(&generic_restoring).unwrap();
    fs::write(generic_restoring.join("file.bin"), b"GR").unwrap();

    // 1. `discover_zalo_safety_copies` must detect BOTH `zalo-restoring.*` and `restoring.<uuid>`
    let copies = greenc_relocate::zalo_move::discover_zalo_safety_copies(Some(&src), &db);
    assert!(
        copies.iter().any(|c| c.path == zalo_restoring),
        "discover_zalo_safety_copies must detect zalo-restoring.*"
    );
    assert!(
        copies.iter().any(|c| c.path == generic_restoring),
        "discover_zalo_safety_copies must detect restoring.<uuid>"
    );

    // 2. `recover_interrupted` must also detect restoring dirs and preserve them with NeedsReview
    let rec = RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src.clone(),
        target_path: target.clone(),
        size_bytes: 16,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    };
    db.save_relocation(&rec).unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, RelocationStatus::NeedsReview);
    let detail = recovered[0].detail.as_deref().unwrap_or("");
    assert!(
        detail.contains("zalo-restoring.") && detail.contains("restoring."),
        "recovery detail must report both zalo-restoring.* and restoring.<uuid>, got: {detail}"
    );
    assert!(zalo_restoring.exists());
    assert!(generic_restoring.exists());
}

#[test]
fn item3_recovery_selects_exact_moving_dir_for_record_or_marks_needs_review_when_ambiguous() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let reparse = NativeReparsePointApi;

    // Subcase A: record specifies exact `moving_path` while another `.greenc-moving.*` exists
    let c_dir_a = tmp.path().join("CaseA/C_Drive");
    let src_a = c_dir_a.join("Zalo Data");
    let old_moving = c_dir_a.join("Zalo Data.greenc-moving.00000000-0000-0000-0000-000000000001");
    let cur_moving = c_dir_a.join("Zalo Data.greenc-moving.ffffffff-ffff-ffff-ffff-ffffffffffff");
    let tgt_a = tmp.path().join("CaseA/D_Drive/zalo");
    fs::create_dir_all(old_moving.join("message")).unwrap();
    fs::write(old_moving.join("message/zalo.db"), b"OLD-STALE").unwrap();
    fs::create_dir_all(cur_moving.join("message")).unwrap();
    fs::write(cur_moving.join("message/zalo.db"), b"CURRENT-EXACT").unwrap();
    fs::create_dir_all(&tgt_a).unwrap();

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_a.clone(),
        target_path: tgt_a.clone(),
        size_bytes: 13,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: Some(cur_moving.clone()),
        target_user_repaired: false,
    })
    .unwrap();

    let rec_a = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(rec_a.len(), 1);
    assert_eq!(
        fs::read(src_a.join("message/zalo.db")).unwrap(),
        b"CURRENT-EXACT",
        "must restore the exact moving_path belonging to this record, NOT an unrelated older dir"
    );
    assert!(
        old_moving.join("message/zalo.db").exists(),
        "unrelated older moving dir must stay untouched"
    );

    // Subcase B: record has NO `moving_path` AND two `.greenc-moving.*` exist -> MUST NOT pick first!
    let c_dir_b = tmp.path().join("CaseB/C_Drive");
    let src_b = c_dir_b.join("Zalo Data");
    let m1 = c_dir_b.join("Zalo Data.greenc-moving.1111");
    let m2 = c_dir_b.join("Zalo Data.greenc-moving.2222");
    let tgt_b = tmp.path().join("CaseB/D_Drive/zalo");
    fs::create_dir_all(&m1).unwrap();
    fs::write(m1.join("a.db"), b"ONE").unwrap();
    fs::create_dir_all(&m2).unwrap();
    fs::write(m2.join("a.db"), b"TWO").unwrap();
    fs::create_dir_all(&tgt_b).unwrap();

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_b.clone(),
        target_path: tgt_b.clone(),
        size_bytes: 3,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    })
    .unwrap();

    let rec_b = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(rec_b.len(), 1);
    assert_eq!(
        rec_b[0].status,
        RelocationStatus::NeedsReview,
        "multiple unassociated .greenc-moving.* dirs must mark NeedsReview instead of picking the first"
    );
    assert!(
        m1.exists() && m2.exists(),
        "both moving dirs must be preserved"
    );
    assert!(
        !src_b.exists(),
        "must not blindly rename either moving dir to source"
    );
}

// =========================================================================
// Item 4: Durable intermediate state (synchronous=FULL) before renames +
//         restore_retained_original rebuilds junction & reopens Zalo on rename failure
// =========================================================================

#[test]
fn item4_durable_tx_enforces_synchronous_full_then_restores_normal() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("durable.db")).unwrap();

    // Default connection pragma is NORMAL (1)
    let before: i64 = db
        .with_conn(|c| {
            c.query_row("PRAGMA synchronous;", [], |r| r.get(0))
                .map_err(|e| GreenCError::Sqlite(e.to_string()))
        })
        .unwrap();
    assert_eq!(before, 1, "default PRAGMA synchronous should be NORMAL (1)");

    // Inside durable helper, PRAGMA synchronous MUST be FULL (2)
    let inside: i64 = db
        .with_durable_sync(|c| {
            c.query_row("PRAGMA synchronous;", [], |r| r.get(0))
                .map_err(|e| GreenCError::Sqlite(e.to_string()))
        })
        .unwrap();
    assert_eq!(
        inside, 2,
        "durable transaction MUST use PRAGMA synchronous = FULL (2)"
    );

    // Restored to NORMAL (1) immediately after durable transaction finishes
    let after: i64 = db
        .with_conn(|c| {
            c.query_row("PRAGMA synchronous;", [], |r| r.get(0))
                .map_err(|e| GreenCError::Sqlite(e.to_string()))
        })
        .unwrap();
    assert_eq!(
        after, 1,
        "PRAGMA synchronous must be restored to NORMAL (1)"
    );
}

#[cfg(unix)]
#[test]
fn item4_restore_retained_original_rebuilds_junction_and_reopens_zalo_on_rename_failure() {
    use greenc_core::models::{ZaloRetainedOriginal, ZaloRetainedStatus};
    use greenc_platform::traits::ReparsePointApi;
    use std::os::unix::fs::PermissionsExt;

    if !unsafe_free_is_non_root() {
        return;
    }

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    let fake_exe = tmp.path().join("Zalo.exe");
    fs::write(&fake_exe, b"EXE").unwrap();
    lifecycle.add_running_exe(fake_exe);

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let retained = c_dir.join("Zalo Data.greenc-original.fail-rename");
    fs::create_dir_all(&c_dir).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    fs::write(target.join("message/zalo.db"), b"TARGET-LIVE").unwrap();
    fs::create_dir_all(retained.join("message")).unwrap();
    fs::write(retained.join("message/zalo.db"), b"RETAINED-ORIG").unwrap();

    // Wrapper that makes `c_dir` read-only immediately after `delete_junction` so
    // `fs::rename(retained, src)` fails with EACCES, then restores writeability when rollback
    // invokes `create_junction`.
    struct FailRenameAfterDeleteReparse {
        inner: NativeReparsePointApi,
        c_dir: PathBuf,
    }
    impl ReparsePointApi for FailRenameAfterDeleteReparse {
        fn is_reparse_point(&self, path: &Path) -> bool {
            self.inner.is_reparse_point(path)
        }
        fn get_junction_target(&self, path: &Path) -> greenc_core::error::Result<PathBuf> {
            self.inner.get_junction_target(path)
        }
        fn create_junction(&self, link: &Path, target: &Path) -> greenc_core::error::Result<()> {
            let _ = fs::set_permissions(&self.c_dir, fs::Permissions::from_mode(0o755));
            self.inner.create_junction(link, target)
        }
        fn delete_junction(&self, link: &Path) -> greenc_core::error::Result<()> {
            self.inner.delete_junction(link)?;
            fs::set_permissions(&self.c_dir, fs::Permissions::from_mode(0o555))?;
            Ok(())
        }
    }
    let reparse = FailRenameAfterDeleteReparse {
        inner: NativeReparsePointApi,
        c_dir: c_dir.clone(),
    };

    // Active valid junction at `src -> target`
    reparse.create_junction(&src, &target).unwrap();

    let now = chrono::Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 13,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now,
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let retained_id = db
        .save_zalo_retained_original(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: retained.clone(),
            size_bytes: 13,
            file_count: 1,
            message_file_count: 1,
            health_verified: false,
            created_at: now,
            purge_after: now + chrono::Duration::hours(24),
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    let res = ZaloRelocator::restore_retained_original(
        retained_id,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(5),
    );

    // Restore permissions so TempDir cleanup succeeds
    let _ = fs::set_permissions(&c_dir, fs::Permissions::from_mode(0o755));

    assert!(res.is_err(), "restore must fail when rename fails");
    // Junction MUST have been rebuilt so Zalo still points to live `target`!
    assert!(
        reparse.is_reparse_point(&src),
        "restore_retained_original MUST rebuild junction when rename fails"
    );
    assert_eq!(
        reparse.get_junction_target(&src).unwrap(),
        target,
        "rebuilt junction must point back to target"
    );
    // Zalo MUST have been reopened!
    assert_eq!(
        lifecycle.restart_count(),
        1,
        "restore_retained_original MUST reopen Zalo when rename fails"
    );
    // Retained copy must remain intact and NOT marked restored!
    assert!(retained.join("message/zalo.db").exists());
    let row = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
    assert_ne!(row.status, ZaloRetainedStatus::Restored);
}

// =========================================================================
// Item 5: health_verified requires live Zalo throughout watch + write through target;
//         pre-purge message/ & file-count check; unreadable purge_after = never expired;
//         clock jump cannot expire immediately (requires real accumulated observation time)
// =========================================================================

#[test]
fn item5_health_verified_requires_live_zalo_process_and_write_probe() {
    // Subcase A: Zalo exits immediately upon reopen (or RmRestart returned 0 without launching)
    {
        let tmp = TempDir::new().unwrap();
        let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
        let db = GreenCDb::open(tmp.path().join("zalo_a.db")).unwrap();
        let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        let reparse = NativeReparsePointApi;
        let shell = MockShellFolderApi::new();
        let lock = MockFileLockProbe::new();
        let proc = MockProcessServiceProbe::new();
        let lifecycle = MockAppLifecycleApi::new();
        lifecycle.add_running_exe(exe_path);
        let lc_hook = lifecycle.clone();
        lifecycle.set_on_restart_hook(move || {
            // Simulate Zalo NOT staying alive after restart_app returned true!
            lc_hook.clear_running_exes();
        });

        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_a");
        let report = ZaloRelocator::move_zalo_data(
            &disc,
            &ZaloMoveOptions {
                target_drive: 'D',
                custom_target_path: Some(target),
                reopen_zalo: true,
                post_reopen_watch_duration: Duration::from_millis(20),
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
        .unwrap();

        assert!(
            !report.health_verified,
            "health_verified MUST be false when Zalo process is NOT alive during watch"
        );
    }

    // Subcase B: write probe skipped (no verified write through target)
    {
        let tmp = TempDir::new().unwrap();
        let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
        let db = GreenCDb::open(tmp.path().join("zalo_b.db")).unwrap();
        let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        let reparse = NativeReparsePointApi;
        let shell = MockShellFolderApi::new();
        let lock = MockFileLockProbe::new();
        let proc = MockProcessServiceProbe::new();
        let lifecycle = MockAppLifecycleApi::new();
        lifecycle.add_running_exe(exe_path);

        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_b");
        let report = ZaloRelocator::move_zalo_data(
            &disc,
            &ZaloMoveOptions {
                target_drive: 'D',
                custom_target_path: Some(target),
                reopen_zalo: true,
                post_reopen_watch_duration: Duration::from_millis(20),
                post_reopen_poll_interval: Duration::from_millis(5),
                enforce_platform_support: false,
                skip_write_probe: true,
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

        assert!(
            !report.health_verified,
            "health_verified MUST be false when write through target was not verified"
        );
    }
}

fn write_valid_sqlite_db(path: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL);",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (body) VALUES ('CHAT-HISTORY-V1');",
        [],
    )
    .unwrap();
}

#[test]
fn item5_purge_refuses_when_target_message_or_file_count_missing() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env(tmp.path());
    // Replace fake bytes with real SQLite DBs (Item 2 enforces real SQLite openability)
    let zalo_data_dir = disc.data_dir.as_ref().unwrap();
    fs::remove_file(zalo_data_dir.join("message/zalo.db")).unwrap();
    write_valid_sqlite_db(&zalo_data_dir.join("message/zalo.db"));
    write_valid_sqlite_db(&zalo_data_dir.join("message/zalo2.db"));

    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);
    let zd_hook = zalo_data.clone();
    lifecycle.set_on_restart_hook(move || {
        let _ = fs::write(zd_hook.join("message/zalo.db-wal"), b"ZALO-LIVE-WAL");
    });

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(20),
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
    .unwrap();
    assert!(report.health_verified);
    let retained_id = report.retained_original_id.unwrap();
    let retained_path = report.retained_original_path.unwrap();

    // Delete one message DB file from target -> target now has FEWER files than recorded!
    fs::remove_file(target.join("message/zalo2.db")).unwrap();

    // Manual purge MUST refuse!
    let manual_err =
        ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap_err();
    assert!(
        matches!(manual_err, GreenCError::PreconditionBlocked { .. }),
        "manual purge must refuse when target lost message/ files, got {manual_err:?}"
    );
    assert!(
        retained_path.exists(),
        "retained original must NOT be deleted"
    );

    // Auto-purge MUST ALSO refuse even after accumulating 25h of time!
    let mut t = chrono::Utc::now();
    for _ in 0..300 {
        t += chrono::Duration::minutes(5);
        let purged = ZaloRelocator::purge_expired_originals(&db, &reparse, t).unwrap();
        assert!(
            purged.is_empty(),
            "auto-purge must refuse when target files are missing"
        );
    }
    assert!(retained_path.exists());
}

#[test]
fn item5_unreadable_purge_after_is_never_expired() {
    let tmp = TempDir::new().unwrap();
    let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(20),
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
    .unwrap();
    let retained_id = report.retained_original_id.unwrap();
    let retained_path = report.retained_original_path.unwrap();

    // Corrupt `purge_after` in SQLite directly!
    db.with_conn(|c| {
        c.execute(
            "UPDATE zalo_retained_originals SET purge_after = 'corrupt-timestamp' WHERE id = ?1;",
            [retained_id],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        Ok(())
    })
    .unwrap();

    let row = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
    assert!(
        !row.purge_after_valid,
        "unparseable purge_after must set purge_after_valid = false"
    );

    // Even far in the future, it MUST NEVER expire!
    let far_future = chrono::Utc::now() + chrono::Duration::days(365);
    let purged = ZaloRelocator::purge_expired_originals(&db, &reparse, far_future).unwrap();
    assert!(
        purged.is_empty(),
        "unreadable purge_after must NEVER expire"
    );
    assert!(retained_path.exists());
}

#[test]
fn item5_clock_jump_forward_does_not_expire_immediately_requires_real_observation() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env(tmp.path());
    fs::remove_file(zalo_data.join("message/zalo.db")).unwrap();
    write_valid_sqlite_db(&zalo_data.join("message/zalo.db"));
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);
    let zd_hook = zalo_data.clone();
    lifecycle.set_on_restart_hook(move || {
        let _ = fs::write(zd_hook.join("message/zalo.db-wal"), b"ZALO-LIVE-WAL");
    });

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(20),
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
    .unwrap();
    let retained_id = report.retained_original_id.unwrap();
    let retained_path = report.retained_original_path.unwrap();
    let created = db
        .get_zalo_retained_original(retained_id)
        .unwrap()
        .unwrap()
        .created_at;

    // Sudden clock jump +72 hours in a single tick MUST NOT expire immediately!
    let jumped = created + chrono::Duration::hours(72);
    let purged_jump = ZaloRelocator::purge_expired_originals(&db, &reparse, jumped).unwrap();
    assert!(
        purged_jump.is_empty(),
        "a single forward clock jump MUST NOT expire the retained original immediately"
    );
    assert!(retained_path.exists());

    // Accumulating real bounded healthy observations over 24h (288 x 5-min steps) DOES expire it!
    let mut t = jumped;
    let mut eventually_purged = false;
    for _ in 0..290 {
        t += chrono::Duration::minutes(5);
        let purged = ZaloRelocator::purge_expired_originals(&db, &reparse, t).unwrap();
        if !purged.is_empty() {
            eventually_purged = true;
            break;
        }
    }
    assert!(
        eventually_purged,
        "retained original should expire once full 24h of real healthy observation accumulates"
    );
    assert!(!retained_path.exists());
}

// =========================================================================
// Item 6: Purge renames to `.greenc-purging.*` + sets Purging status before delete;
//         half-purged copy is never restorable and never shows as restorable
// =========================================================================

#[cfg(unix)]
#[test]
fn item6_purge_renames_to_greenc_purging_and_blocks_restore_on_partial_purge() {
    use greenc_core::models::ZaloRetainedStatus;
    use std::os::unix::fs::PermissionsExt;

    if !unsafe_free_is_non_root() {
        return;
    }

    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env(tmp.path());
    fs::remove_file(zalo_data.join("message/zalo.db")).unwrap();
    write_valid_sqlite_db(&zalo_data.join("message/zalo.db"));
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(20),
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
    .unwrap();
    let retained_id = report.retained_original_id.unwrap();
    let orig_path = report.retained_original_path.unwrap();

    // Make `orig_path/message` read-only so `fs::rename(orig_path, purging_path)` SUCCEEDS
    // (parent is writable!) while `fs::remove_dir_all(purging_path)` FAILS mid-delete!
    let msg_sub = orig_path.join("message");
    fs::set_permissions(&msg_sub, fs::Permissions::from_mode(0o555)).unwrap();

    let purge_err = ZaloRelocator::purge_retained_original(retained_id, &db, &reparse);
    assert!(
        purge_err.is_err(),
        "purge must fail when nested read-only dir cannot be removed"
    );

    // 1. Original path was renamed to `.greenc-purging.<uuid>` BEFORE deletion started!
    assert!(
        !orig_path.exists(),
        "original path must have been renamed to .greenc-purging.*"
    );
    let row = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
    assert_eq!(
        row.status,
        ZaloRetainedStatus::Purging,
        "DB status must remain Purging after partial delete"
    );
    let purging_name = row
        .retained_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        purging_name.contains(".greenc-purging."),
        "retained_path must point to .greenc-purging.*, got: {purging_name}"
    );
    assert!(row.retained_path.exists());

    // 2. `discover_zalo_safety_copies` reports it as `kind = "purging"`, `status = "purging"`
    let copies = greenc_relocate::zalo_move::discover_zalo_safety_copies(Some(&zalo_data), &db);
    let found = copies
        .iter()
        .find(|c| c.id == Some(retained_id))
        .expect("half-purged copy must be listed with purging kind/status");
    assert_eq!(found.kind, "purging");
    assert_eq!(found.status, "purging");

    // 3. Restoring a half-purged copy MUST BE REFUSED!
    let restore_err = ZaloRelocator::restore_retained_original(
        retained_id,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(
        matches!(restore_err, GreenCError::PreconditionBlocked { .. }),
        "restoring a Purging copy MUST be blocked, got {restore_err:?}"
    );

    // 4. Restore permissions -> re-running purge finishes deleting cleanly -> Purged!
    let _ = fs::set_permissions(
        row.retained_path.join("message"),
        fs::Permissions::from_mode(0o755),
    );
    let done = ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap();
    assert_eq!(done.status, ZaloRetainedStatus::Purged);
    assert!(!row.retained_path.exists());
}

// =========================================================================
// Item 7: Re-verify source size/mtime/file-count right before rename;
//         full SHA-256 on EVERY copied file (even > 64 MiB outside message/)
// =========================================================================

#[test]
fn item7_source_changed_after_verify_before_rename_aborts_and_keeps_original() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env(tmp.path());
    fs::remove_file(zalo_data.join("message/zalo.db")).unwrap();
    write_valid_sqlite_db(&zalo_data.join("message/zalo.db"));
    let db = GreenCDb::open(tmp.path().join("zalo.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    // Mutate `source` AFTER `verify_zalo_copy_integrity` passed and RIGHT BEFORE rename!
    let before_rename_cb = |src: &Path, _tgt: &Path| {
        fs::write(
            src.join("message/late_write.db"),
            b"LATE-WRITE-BEFORE-RENAME",
        )
        .unwrap();
    };

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let err = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(15),
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
        &ZaloMoveHooks {
            before_rename: Some(&before_rename_cb),
            ..Default::default()
        },
    )
    .unwrap_err();

    assert!(
        matches!(err, GreenCError::IntegrityMismatch { .. }),
        "must abort with IntegrityMismatch when source changed right before rename, got {err:?}"
    );
    use greenc_platform::traits::ReparsePointApi;
    assert!(
        !reparse.is_reparse_point(&zalo_data),
        "source must remain a real directory"
    );
    assert_eq!(
        fs::read(zalo_data.join("message/late_write.db")).unwrap(),
        b"LATE-WRITE-BEFORE-RENAME"
    );
    assert!(!target.exists(), "target copy must be cleaned on abort");
}

#[test]
fn item7_full_sha256_catches_mid_file_corruption_on_large_non_message_file() {
    use std::io::{Seek, SeekFrom, Write};

    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src_zalo");
    let dst = tmp.path().join("dst_zalo");
    fs::create_dir_all(src.join("message")).unwrap();
    fs::create_dir_all(src.join("media")).unwrap();
    fs::create_dir_all(dst.join("message")).unwrap();
    fs::create_dir_all(dst.join("media")).unwrap();
    fs::write(src.join("message/zalo.db"), b"OK").unwrap();
    fs::write(dst.join("message/zalo.db"), b"OK").unwrap();

    // Create a 65 MiB file (> old 64 MiB head/tail threshold) in `media/` in both src and dst
    let big_len: u64 = 65 * 1024 * 1024;
    let src_big = src.join("media/video.mp4");
    let dst_big = dst.join("media/video.mp4");
    {
        let mut f = fs::File::create(&src_big).unwrap();
        f.set_len(big_len).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(b"HEAD-OK").unwrap();
        f.seek(SeekFrom::Start(big_len - 8)).unwrap();
        f.write_all(b"TAIL-OK!").unwrap();
    }
    fs::copy(&src_big, &dst_big).unwrap();

    // Flip ONE byte right in the middle (offset 32 MiB) of `dst/media/video.mp4`!
    {
        let mut f = fs::OpenOptions::new().write(true).open(&dst_big).unwrap();
        f.seek(SeekFrom::Start(32 * 1024 * 1024)).unwrap();
        f.write_all(&[0xFF]).unwrap();
    }

    let err = greenc_relocate::zalo_move::verify_zalo_copy_integrity(&src, &dst).unwrap_err();
    assert!(
        matches!(err, GreenCError::IntegrityMismatch { .. }),
        "full SHA-256 MUST catch mid-file corruption even on files > 64 MiB outside message/, got {err:?}"
    );
}

// ============================================================================
// ITEM 8: repair_junction_target hardening (all apps) & undo preserves user-typed target
// ============================================================================

#[test]
fn item8_repair_junction_rejects_root_overlap_unmanaged_and_bad_status() {
    use chrono::Utc;
    use greenc_platform::traits::ReparsePointApi;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item8_a.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let managed_root = tmp.path().join("D_Drive").join("GreenC").join("Relocated");
    let app_a_src = tmp.path().join("C_Drive").join("AppA");
    let app_a_old_tgt = managed_root.join("app_a_old");
    let app_a_new_tgt = managed_root.join("app_a_new");
    let app_b_src = tmp.path().join("C_Drive").join("AppB");
    let app_b_tgt = managed_root.join("app_b");
    let app_b_sub = app_b_tgt.join("sub");
    let unmanaged_dir = tmp.path().join("D_Drive").join("UserPhotos");

    fs::create_dir_all(app_a_src.parent().unwrap()).unwrap();
    fs::create_dir_all(&app_a_old_tgt).unwrap();
    fs::create_dir_all(&app_a_new_tgt).unwrap();
    fs::create_dir_all(&app_b_sub).unwrap();
    fs::create_dir_all(&unmanaged_dir).unwrap();
    reparse.create_junction(&app_a_src, &app_a_old_tgt).unwrap();
    reparse.create_junction(&app_b_src, &app_b_tgt).unwrap();

    let rec_a_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "app_a".to_string(),
            app_name: "App A".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_a_src.clone(),
            target_path: app_a_old_tgt.clone(),
            size_bytes: 128,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::TargetOffline,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let _rec_b_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "app_b".to_string(),
            app_name: "App B".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_b_src.clone(),
            target_path: app_b_tgt.clone(),
            size_bytes: 128,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // 1. Drive / filesystem root MUST be rejected
    let root_err = RecoveryService::repair_junction_target(rec_a_id, Path::new("/"), &db, &reparse)
        .unwrap_err();
    assert!(
        matches!(root_err, GreenCError::PreconditionBlocked { .. }),
        "drive/fs root must be rejected, got {root_err:?}"
    );

    // 2. Unmanaged folder outside GreenC/Relocated MUST be rejected
    let unmanaged_err =
        RecoveryService::repair_junction_target(rec_a_id, &unmanaged_dir, &db, &reparse)
            .unwrap_err();
    assert!(
        matches!(unmanaged_err, GreenCError::PreconditionBlocked { .. }),
        "unmanaged dir must be rejected, got {unmanaged_err:?}"
    );

    // 3. Parent of another record's target (`GreenC/Relocated`) MUST be rejected
    let parent_err =
        RecoveryService::repair_junction_target(rec_a_id, &managed_root, &db, &reparse)
            .unwrap_err();
    assert!(
        matches!(parent_err, GreenCError::PreconditionBlocked { .. }),
        "parent of another record's target must be rejected, got {parent_err:?}"
    );

    // 4. Duplicate of another record's target MUST be rejected
    let dup_err =
        RecoveryService::repair_junction_target(rec_a_id, &app_b_tgt, &db, &reparse).unwrap_err();
    assert!(
        matches!(dup_err, GreenCError::PreconditionBlocked { .. }),
        "duplicate of another record's target must be rejected, got {dup_err:?}"
    );

    // 5. Child of another record's target MUST be rejected
    let child_err =
        RecoveryService::repair_junction_target(rec_a_id, &app_b_sub, &db, &reparse).unwrap_err();
    assert!(
        matches!(child_err, GreenCError::PreconditionBlocked { .. }),
        "child of another record's target must be rejected, got {child_err:?}"
    );

    // 6. Wrong record status (Moving, Failed, Undone) MUST be rejected
    for bad_status in [
        RelocationStatus::Moving,
        RelocationStatus::Failed,
        RelocationStatus::Undone,
    ] {
        let bad_id = db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: format!("bad_{}", bad_status.as_str()),
                app_name: "Bad".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: tmp
                    .path()
                    .join(format!("C_Drive/Bad_{}", bad_status.as_str())),
                target_path: managed_root.join(format!("bad_old_{}", bad_status.as_str())),
                size_bytes: 128,
                status: bad_status,
                health_status: RelocationHealthStatus::LinkBroken,
                process_names: vec![],
                created_at: Utc::now(),
                detail: None,
                moving_path: None,
                target_user_repaired: false,
            })
            .unwrap();
        let st_err = RecoveryService::repair_junction_target(bad_id, &app_a_new_tgt, &db, &reparse)
            .unwrap_err();
        assert!(
            matches!(st_err, GreenCError::PreconditionBlocked { .. }),
            "status {:?} must be rejected, got {st_err:?}",
            bad_status
        );
    }

    // 7. Valid managed target on Done record MUST succeed
    let repaired =
        RecoveryService::repair_junction_target(rec_a_id, &app_a_new_tgt, &db, &reparse).unwrap();
    assert_eq!(repaired.status, RelocationStatus::Done);
    assert_eq!(repaired.health_status, RelocationHealthStatus::Healthy);
    assert_eq!(repaired.target_path, app_a_new_tgt);
    assert!(greenc_core::models::same_windows_path(
        &reparse.get_junction_target(&app_a_src).unwrap(),
        &app_a_new_tgt
    ));
}

#[test]
fn item8_repair_junction_creates_new_before_deleting_old() {
    use chrono::Utc;
    use greenc_platform::traits::ReparsePointApi;

    struct FailCreateKeepDeleteReparse {
        inner: NativeReparsePointApi,
        deleted_paths: std::sync::Mutex<Vec<PathBuf>>,
    }
    impl ReparsePointApi for FailCreateKeepDeleteReparse {
        fn is_reparse_point(&self, path: &Path) -> bool {
            self.inner.is_reparse_point(path)
        }
        fn get_junction_target(&self, path: &Path) -> greenc_core::error::Result<PathBuf> {
            self.inner.get_junction_target(path)
        }
        fn create_junction(&self, _link: &Path, _target: &Path) -> greenc_core::error::Result<()> {
            Err(GreenCError::Relocation(
                "simulated create_junction failure".to_string(),
            ))
        }
        fn delete_junction(&self, link: &Path) -> greenc_core::error::Result<()> {
            self.deleted_paths.lock().unwrap().push(link.to_path_buf());
            self.inner.delete_junction(link)
        }
    }

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item8_b.db")).unwrap();
    let real_reparse = NativeReparsePointApi;

    let managed_root = tmp.path().join("D_Drive").join("GreenC").join("Relocated");
    let src = tmp.path().join("C_Drive").join("Discord");
    let old_tgt = managed_root.join("discord_old");
    let new_tgt = managed_root.join("discord_new");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(&old_tgt).unwrap();
    fs::create_dir_all(&new_tgt).unwrap();
    real_reparse.create_junction(&src, &old_tgt).unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "discord".to_string(),
            app_name: "Discord".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: old_tgt.clone(),
            size_bytes: 256,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let failing_reparse = FailCreateKeepDeleteReparse {
        inner: NativeReparsePointApi,
        deleted_paths: std::sync::Mutex::new(Vec::new()),
    };

    let err = RecoveryService::repair_junction_target(rec_id, &new_tgt, &db, &failing_reparse)
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("simulated create_junction failure"));

    // CRITICAL: Old junction MUST NOT have been deleted before new junction succeeded!
    assert!(
        failing_reparse.deleted_paths.lock().unwrap().is_empty(),
        "delete_junction must NOT be called before new junction creation succeeds"
    );
    assert!(
        real_reparse.is_reparse_point(&src),
        "old junction must still exist untouched"
    );
    assert!(greenc_core::models::same_windows_path(
        &real_reparse.get_junction_target(&src).unwrap(),
        &old_tgt
    ));
}

#[test]
fn item8_undo_never_deletes_user_repaired_target() {
    use chrono::Utc;
    use greenc_platform::traits::ReparsePointApi;
    use greenc_relocate::mover::TransactionalMover;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item8_c.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let vol = make_drives(20 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024);
    let shell = MockShellFolderApi::default();
    let proc = MockProcessServiceProbe::new();

    let managed_root = tmp.path().join("D_Drive").join("GreenC").join("Relocated");
    let src = tmp.path().join("C_Drive").join("Telegram");
    let old_tgt = managed_root.join("telegram_old");
    let user_tgt = managed_root.join("telegram_user_picked");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(&old_tgt).unwrap();
    fs::create_dir_all(&user_tgt).unwrap();
    fs::write(user_tgt.join("chat.dat"), b"USER-PICKED-DATA").unwrap();
    reparse.create_junction(&src, &old_tgt).unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "telegram".to_string(),
            app_name: "Telegram".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: old_tgt.clone(),
            size_bytes: 16,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::LinkBroken,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // User repairs junction to point at `user_tgt`
    RecoveryService::repair_junction_target(rec_id, &user_tgt, &db, &reparse).unwrap();

    // Now undo the relocation
    let undone = TransactionalMover::undo_relocation(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .unwrap();
    assert_eq!(undone.status, RelocationStatus::Undone);

    // Source must have restored files
    assert!(!reparse.is_reparse_point(&src));
    assert_eq!(fs::read(src.join("chat.dat")).unwrap(), b"USER-PICKED-DATA");

    // CRITICAL: Undo MUST NEVER delete the user-typed target directory!
    assert!(
        user_tgt.join("chat.dat").exists(),
        "undo MUST NOT delete a user-repaired target directory"
    );

    // Also verify Zalo undo leaves a user-repaired target intact in place (neither deleted nor renamed)
    let zalo_src = tmp.path().join("C_Drive").join("Zalo Data");
    let zalo_old_tgt = managed_root.join("zalo_old");
    let zalo_user_tgt = managed_root.join("zalo_user_picked");
    fs::create_dir_all(&zalo_old_tgt).unwrap();
    fs::create_dir_all(zalo_user_tgt.join("message")).unwrap();
    fs::write(zalo_user_tgt.join("message/zalo.db"), b"ZALO-USER-PICKED").unwrap();
    reparse.create_junction(&zalo_src, &zalo_old_tgt).unwrap();

    let zalo_rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: zalo_src.clone(),
            target_path: zalo_old_tgt.clone(),
            size_bytes: 16,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::LinkBroken,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    RecoveryService::repair_junction_target(zalo_rec_id, &zalo_user_tgt, &db, &reparse).unwrap();
    let zalo_undone = TransactionalMover::undo_relocation(
        zalo_rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .unwrap();
    assert_eq!(zalo_undone.status, RelocationStatus::Undone);
    assert_eq!(
        fs::read(zalo_src.join("message/zalo.db")).unwrap(),
        b"ZALO-USER-PICKED"
    );
    assert_eq!(
        fs::read(zalo_user_tgt.join("message/zalo.db")).unwrap(),
        b"ZALO-USER-PICKED",
        "Zalo undo must preserve user-repaired target intact at its exact path"
    );
}

// ============================================================================
// ITEM 9: Restore conflict/moving copies, real 24h promise, and crash-during-copy cleanup
// ============================================================================

#[test]
fn item9_restore_safety_copy_by_path_supports_conflict_and_moving_and_blocks_purging() {
    use greenc_platform::traits::ReparsePointApi;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item9_restore.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::default();

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    let conflict_copy =
        c_dir.join("Zalo Data.greenc-conflict.11111111-1111-1111-1111-111111111111");
    let moving_copy = c_dir.join("Zalo Data.greenc-moving.22222222-2222-2222-2222-222222222222");
    let purging_copy = c_dir.join("Zalo Data.greenc-purging.33333333-3333-3333-3333-333333333333");

    fs::create_dir_all(src.join("message")).unwrap();
    fs::write(src.join("message/zalo.db"), b"CURRENT-SRC").unwrap();
    fs::create_dir_all(conflict_copy.join("message")).unwrap();
    fs::write(conflict_copy.join("message/zalo.db"), b"FROM-CONFLICT-COPY").unwrap();
    fs::create_dir_all(moving_copy.join("message")).unwrap();
    fs::write(moving_copy.join("message/zalo.db"), b"FROM-MOVING-COPY").unwrap();
    fs::create_dir_all(purging_copy.join("message")).unwrap();
    fs::write(purging_copy.join("message/zalo.db"), b"HALF-PURGED").unwrap();

    // 1. Purging copy MUST be refused!
    let purge_err = ZaloRelocator::restore_safety_copy_by_path(
        &purging_copy,
        &src,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(
        matches!(purge_err, GreenCError::PreconditionBlocked { .. }),
        "purging copy must be refused, got {purge_err:?}"
    );

    // 2. Restoring conflict copy succeeds and preserves old src as a conflict sibling
    let msg1 = ZaloRelocator::restore_safety_copy_by_path(
        &conflict_copy,
        &src,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(!msg1.is_empty());
    assert!(!reparse.is_reparse_point(&src));
    assert_eq!(
        fs::read(src.join("message/zalo.db")).unwrap(),
        b"FROM-CONFLICT-COPY"
    );

    // 3. Restoring moving copy also succeeds
    let msg2 = ZaloRelocator::restore_safety_copy_by_path(
        &moving_copy,
        &src,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(!msg2.is_empty());
    assert_eq!(
        fs::read(src.join("message/zalo.db")).unwrap(),
        b"FROM-MOVING-COPY"
    );
}

#[test]
fn item9_crash_during_copy_cleans_partial_staging_target_and_allows_retry() {
    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item9_copy_crash.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let vol = make_drives(20 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024);
    let shell = MockShellFolderApi::default();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::default();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let partial_staging = tmp
        .path()
        .join("D_Drive/GreenC/Relocated/zalo.greenc-copying.crash-during-copy-1");
    fs::create_dir_all(partial_staging.join("message")).unwrap();
    fs::write(partial_staging.join("message/partial.tmp"), b"HALF-COPIED").unwrap();

    // Simulate a Moving DB record left behind when process crashed mid-copy (before rename)
    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: zalo_data.clone(),
        target_path: target.clone(),
        size_bytes: 11,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: Some("Đang sao chép dữ liệu sang ổ đích...".to_string()),
        moving_path: None,
        target_user_repaired: false,
    })
    .unwrap();

    // 1. Recovery MUST clean the abandoned `.greenc-copying.*` dir and preserve intact source
    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, RelocationStatus::Failed);
    assert!(
        !partial_staging.exists(),
        "partial .greenc-copying.* target MUST be cleaned on crash-during-copy recovery"
    );
    assert_eq!(
        fs::read(zalo_data.join("message/zalo.db")).unwrap(),
        b"CHAT-HISTORY-V1",
        "original source must stay untouched"
    );

    // 2. Even an orphaned `.greenc-copying.*` without a DB record is cleaned on retry move
    let orphan_staging = tmp
        .path()
        .join("D_Drive/GreenC/Relocated/zalo.greenc-copying.orphan-2");
    fs::create_dir_all(&orphan_staging).unwrap();
    fs::write(orphan_staging.join("junk.bin"), b"ORPHAN").unwrap();

    let mut opts = ZaloMoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target.clone()),
        reopen_zalo: true,
        dry_run: false,
        enforce_platform_support: false,
        post_reopen_watch_duration: Duration::from_millis(15),
        ..Default::default()
    };
    opts.shutdown_timeout = Duration::from_secs(2);

    let report = ZaloRelocator::move_zalo_data_with_hooks(
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
        &ZaloMoveHooks::default(),
    )
    .unwrap();
    assert_eq!(report.status, "done");
    assert!(
        !orphan_staging.exists(),
        "orphaned .greenc-copying.* staging dir must be cleaned before/during retry move"
    );
}

#[test]
fn item9_confirmation_text_reflects_real_24h_conditions() {
    let text = greenc_relocate::zalo_move::format_zalo_move_confirmation(1024 * 1024 * 500, 'D');
    assert!(
        text.contains("tối thiểu 24 giờ") && text.contains("xác minh"),
        "24h promise must reflect real health-verification + observation conditions, got: {text}"
    );
}

// =========================================================================
// Item 10: RmShutdown targets ONLY Zalo processes; accept data dir only with
//          Zalo `message/` structure; set current-user-only ACL on target dir
// =========================================================================

#[test]
fn item10_rm_shutdown_targets_only_zalo_processes() {
    use greenc_platform::app_lifecycle::{
        select_rm_target_processes, AppLifecycleApi, RmProcessCandidate,
    };
    use greenc_scanner::zalo::ZALO_PROCESS_NAMES;

    let candidates = vec![
        RmProcessCandidate {
            pid: 0,
            exe_path: PathBuf::from(r"C:\Users\u\AppData\Local\Programs\Zalo\Zalo.exe"),
            start_time_low: 1,
            start_time_high: 2,
        },
        RmProcessCandidate {
            pid: 1001,
            exe_path: PathBuf::from(r"C:\Windows\explorer.exe"),
            start_time_low: 10,
            start_time_high: 20,
        },
        RmProcessCandidate {
            pid: 1002,
            exe_path: PathBuf::from(r"C:\Windows\System32\notepad.exe"),
            start_time_low: 11,
            start_time_high: 20,
        },
        RmProcessCandidate {
            pid: 2001,
            exe_path: PathBuf::from(
                r"C:\Users\u\AppData\Local\Programs\Zalo\Zalo-26.11.12\Zalo.exe",
            ),
            start_time_low: 30,
            start_time_high: 40,
        },
        // Duplicate PID + start time should be deduplicated
        RmProcessCandidate {
            pid: 2001,
            exe_path: PathBuf::from(
                r"C:\Users\u\AppData\Local\Programs\Zalo\Zalo-26.11.12\Zalo.exe",
            ),
            start_time_low: 30,
            start_time_high: 40,
        },
        RmProcessCandidate {
            pid: 2002,
            exe_path: PathBuf::from(
                r"C:\Users\u\AppData\Local\Programs\Zalo\Zalo-26.11.12\zaloCap.exe",
            ),
            start_time_low: 31,
            start_time_high: 40,
        },
        RmProcessCandidate {
            pid: 3001,
            exe_path: PathBuf::from(r"C:\Program Files\GreenC\greenc.exe"),
            start_time_low: 50,
            start_time_high: 60,
        },
    ];

    let selected = select_rm_target_processes(&candidates, ZALO_PROCESS_NAMES);
    assert_eq!(
        selected.len(),
        2,
        "only non-zero-PID Zalo processes must be selected, got {selected:?}"
    );
    assert_eq!(selected[0].pid, 2001);
    assert_eq!(selected[1].pid, 2002);

    // Empty allowlist must NEVER select any process
    assert!(select_rm_target_processes(&candidates, &[]).is_empty());

    // End-to-end move: non-Zalo processes (explorer.exe, notepad.exe) stay running & untouched!
    let tmp = TempDir::new().unwrap();
    let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item10_rm.db")).unwrap();
    let vol = make_drives(20 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let explorer = PathBuf::from("/fake/explorer.exe");
    let notepad = PathBuf::from("/fake/notepad.exe");
    lifecycle.add_running_exe(explorer.clone());
    lifecycle.add_running_exe(notepad.clone());
    lifecycle.add_running_exe(exe_path.clone());

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(15),
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
    .unwrap();
    assert_eq!(report.status, "done");

    let targeted = lifecycle.shutdown_targeted_exes.lock().unwrap().clone();
    assert_eq!(
        targeted,
        vec![exe_path],
        "graceful_shutdown must ONLY target Zalo processes"
    );
    assert_eq!(
        lifecycle.find_running_exe_paths(&["explorer.exe", "notepad.exe"]),
        vec![explorer, notepad],
        "unrelated processes must remain running and untouched"
    );
}

#[test]
fn item10_move_rejects_source_without_zalo_message_structure() {
    let tmp = TempDir::new().unwrap();
    let (mut disc, zalo_data, exe_path) = build_zalo_env(tmp.path());

    // Point discovery at a non-Zalo folder that has NO `message/` directory
    let non_zalo_dir = tmp.path().join("C_Drive/RandomUserFolder");
    fs::create_dir_all(&non_zalo_dir).unwrap();
    fs::write(non_zalo_dir.join("notes.txt"), b"PERSONAL-NOTES").unwrap();
    disc.data_dir = Some(non_zalo_dir.clone());

    let db = GreenCDb::open(tmp.path().join("item10_struct.db")).unwrap();
    let vol = make_drives(20 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
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

    assert!(
        matches!(err, GreenCError::PreconditionBlocked { .. }),
        "moving a folder without Zalo message/ structure MUST be blocked, got {err:?}"
    );
    assert!(non_zalo_dir.join("notes.txt").exists());
    assert!(!target.exists());
    assert!(zalo_data.join("message/zalo.db").exists());
}

#[test]
fn item10_target_dir_sets_current_user_only_acl() {
    let tmp = TempDir::new().unwrap();
    let (disc, _zalo_data, exe_path) = build_zalo_env(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item10_acl.db")).unwrap();
    let vol = make_drives(20 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let parent_d = tmp.path().join("D_Drive/GreenC/Relocated");
    fs::create_dir_all(&parent_d).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Simulate permissive D:\ root permissions
        fs::set_permissions(&parent_d, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let target = parent_d.join("zalo");
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(15),
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
    .unwrap();
    assert_eq!(report.status, "done");

    #[cfg(unix)]
    {
        use greenc_platform::restrict_dir_to_current_user;
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "target dir on D: MUST have current-user-only permissions (0700), got {mode:04o}"
        );

        // `restrict_dir_to_current_user` must reject symlinks and plain files
        let sym = tmp.path().join("sym_dir");
        std::os::unix::fs::symlink(&target, &sym).unwrap();
        assert!(
            restrict_dir_to_current_user(&sym).is_err(),
            "restrict_dir_to_current_user must refuse symlinks"
        );
        let plain_file = tmp.path().join("plain.txt");
        fs::write(&plain_file, b"hi").unwrap();
        assert!(
            restrict_dir_to_current_user(&plain_file).is_err(),
            "restrict_dir_to_current_user must refuse non-directories"
        );
    }
}
