//! Comprehensive TDD safety tests for (Items 1-12).

use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::GreenCError;
use greenc_core::models::{
    DriveInfo, RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
    ZaloRetainedOriginal, ZaloRetainedStatus,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::MockAppLifecycleApi;
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi, ReparsePointApi,
};
use greenc_relocate::zalo_move::{ZaloMoveOptions, ZaloRelocator};
use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscovery, ZaloDiscoveryOptions};
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

fn create_real_sqlite_db(path: &Path, rows: usize) {
    let mut conn = rusqlite::Connection::open(path).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, body TEXT NOT NULL);",
        [],
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    for i in 0..rows {
        tx.execute(
            "INSERT INTO messages (body) VALUES (?1);",
            [format!("chat-message-payload-{i:06}-{}", "x".repeat(120))],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

fn build_zalo_env_sqlite(tmp: &Path) -> (ZaloDiscovery, PathBuf, PathBuf) {
    let sys_root = tmp.join("C_Drive");
    let zalo_data = sys_root.join("Zalo Data");
    fs::create_dir_all(zalo_data.join("message")).unwrap();
    fs::create_dir_all(zalo_data.join("media")).unwrap();
    create_real_sqlite_db(&zalo_data.join("message/zalo.db"), 200);
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

#[allow(dead_code)]
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

// =========================================================================
// Item 1: Purging saved before rename — if DB status is Purging but path is
//         NOT `.greenc-purging.*`, revert to Retained and enforce ALL gates
//         (both auto-purge and manual purge).
// =========================================================================

#[test]
fn item1_purging_without_purging_path_reverts_to_retained_and_runs_all_gates() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item1.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let src = tmp.path().join("C_Drive/Zalo Data");
    let orig = tmp
        .path()
        .join("C_Drive/Zalo Data.greenc-original.11111111-1111-1111-1111-111111111111");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");

    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(orig.join("message")).unwrap();
    fs::write(orig.join("message/zalo.db"), b"ORIGINAL-INTACT-DATA").unwrap();
    // Target is broken/missing `message/` (or junction missing) so safety gates MUST fail!
    fs::create_dir_all(&target).unwrap();

    let now = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 20,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::LinkBroken,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now - ChronoDuration::hours(30),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // Simulate crash right after DB wrote `status = Purging` BEFORE `fs::rename` to `.greenc-purging.*`:
    // `retained_path` is STILL `.greenc-original.*`, NOT `.greenc-purging.*`!
    let retained_id = db
        .save_zalo_retained_original(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: orig.clone(),
            size_bytes: 20,
            file_count: 1,
            message_file_count: 1,
            health_verified: false, // NOT verified!
            created_at: now - ChronoDuration::hours(30),
            purge_after: now - ChronoDuration::hours(1),
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now - ChronoDuration::hours(30)),
            status: ZaloRetainedStatus::Purging,
            resolved_at: None,
            detail: Some("Crash before rename".to_string()),
        })
        .unwrap();

    // 1. Auto-purge MUST NOT bypass gates; it must revert status to Retained and keep `orig`!
    let purged = ZaloRelocator::purge_expired_originals(&db, &reparse, now).unwrap();
    assert!(
        purged.is_empty(),
        "auto-purge MUST NOT purge a Purging row whose path is not .greenc-purging.*"
    );
    assert!(
        orig.join("message/zalo.db").exists(),
        "intact .greenc-original.* copy MUST NOT be deleted"
    );
    let after_auto = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
    assert_eq!(
        after_auto.status,
        ZaloRetainedStatus::Retained,
        "status must revert to Retained when path was never renamed to .greenc-purging.*"
    );

    // 2. Re-set to Purging (simulating manual purge right after crash before rename) ->
    //    Manual purge MUST also revert to Retained and enforce all safety gates!
    db.update_zalo_retained_original_status_durable(
        retained_id,
        ZaloRetainedStatus::Purging.as_str(),
        "Crash before rename again",
    )
    .unwrap();

    let manual_err =
        ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap_err();
    assert!(
        matches!(manual_err, GreenCError::PreconditionBlocked { .. }),
        "manual purge MUST block when gates fail on Purging row without .greenc-purging.* path, got {manual_err:?}"
    );
    assert!(
        orig.join("message/zalo.db").exists(),
        "manual purge MUST NOT delete original when gates fail"
    );
    let after_manual = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
    assert_eq!(
        after_manual.status,
        ZaloRetainedStatus::Retained,
        "manual purge must revert Purging status to Retained before/when gates fail"
    );

    // 3. Even if junction IS healthy, if `health_verified == false`, auto-purge must STILL refuse
    //    after reverting Purging -> Retained!
    fs::create_dir_all(target.join("message")).unwrap();
    fs::write(target.join("message/zalo.db"), b"ORIGINAL-INTACT-DATA").unwrap();
    reparse.create_junction(&src, &target).unwrap();
    db.update_zalo_retained_original_status_durable(
        retained_id,
        ZaloRetainedStatus::Purging.as_str(),
        "Crash before rename with healthy link",
    )
    .unwrap();

    let purged2 = ZaloRelocator::purge_expired_originals(&db, &reparse, now).unwrap();
    assert!(
        purged2.is_empty(),
        "auto-purge must still respect health_verified=false after reverting Purging -> Retained"
    );
    assert!(orig.join("message/zalo.db").exists());
    assert_eq!(
        db.get_zalo_retained_original(retained_id)
            .unwrap()
            .unwrap()
            .status,
        ZaloRetainedStatus::Retained
    );
}

// =========================================================================
// Item 2: Pre-purge gate compares against on-disk original; every DB in
//         message/ (excluding -wal/-shm/-journal) must exist on target, not
//         be significantly smaller, and open as SQLite; ignore media count;
//         health_verified requires real Zalo write to target (not GreenC probe).
// =========================================================================

#[test]
fn item2_purge_gate_compares_on_disk_original_sqlite_and_ignores_media() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item2_gate.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let src = tmp.path().join("C_Drive/Zalo Data");
    let orig = tmp
        .path()
        .join("C_Drive/Zalo Data.greenc-original.22222222-2222-2222-2222-222222222222");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");

    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(orig.join("message")).unwrap();
    fs::create_dir_all(orig.join("media")).unwrap();
    create_real_sqlite_db(&orig.join("message/zalo.db"), 250);
    create_real_sqlite_db(&orig.join("message/extra.db"), 50);
    // Sidecar files in original that SQLite may checkpoint/remove on target
    fs::write(orig.join("message/zalo.db-wal"), b"WAL-SIDECAR").unwrap();
    fs::write(orig.join("message/zalo.db-shm"), b"SHM-SIDECAR").unwrap();
    fs::write(orig.join("message/zalo.db-journal"), b"JRN-SIDECAR").unwrap();
    fs::write(orig.join("media/img1.jpg"), vec![0xAA; 2048]).unwrap();
    fs::write(orig.join("media/img2.jpg"), vec![0xBB; 2048]).unwrap();

    fs::create_dir_all(target.join("message")).unwrap();
    fs::create_dir_all(target.join("media")).unwrap();
    fs::copy(orig.join("message/zalo.db"), target.join("message/zalo.db")).unwrap();
    fs::copy(
        orig.join("message/extra.db"),
        target.join("message/extra.db"),
    )
    .unwrap();
    reparse.create_junction(&src, &target).unwrap();

    let now = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 100_000,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now - ChronoDuration::hours(30),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // Intentionally record stale/low counts (file_count=10, message_file_count=1) to verify the
    // gate compares directly against the on-disk original instead of stale DB counts!
    let retained_id = db
        .save_zalo_retained_original(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: orig.clone(),
            size_bytes: 100_000,
            file_count: 10,
            message_file_count: 1,
            health_verified: true,
            created_at: now - ChronoDuration::hours(30),
            purge_after: now - ChronoDuration::hours(1),
            purge_after_valid: true,
            observed_healthy_secs: 25 * 3600,
            last_observed_at: Some(now - ChronoDuration::minutes(1)),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    // Case 1: Missing `extra.db` on target (even though DB says message_file_count=1) -> MUST BLOCK!
    fs::remove_file(target.join("message/extra.db")).unwrap();
    let err_missing =
        ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap_err();
    assert!(
        matches!(err_missing, GreenCError::PreconditionBlocked { .. }),
        "purge must fail when a DB present in on-disk original/message/ is missing on target, got {err_missing:?}"
    );
    fs::copy(
        orig.join("message/extra.db"),
        target.join("message/extra.db"),
    )
    .unwrap();

    // Case 2: Corrupted (non-SQLite) `zalo.db` of same size -> MUST BLOCK!
    let orig_len = fs::metadata(orig.join("message/zalo.db")).unwrap().len() as usize;
    fs::write(target.join("message/zalo.db"), vec![0xDE; orig_len]).unwrap();
    let err_corrupt =
        ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap_err();
    assert!(
        matches!(err_corrupt, GreenCError::PreconditionBlocked { .. }),
        "purge must fail when target DB cannot be opened as valid SQLite, got {err_corrupt:?}"
    );

    // Case 3: Significantly smaller valid SQLite DB (e.g. empty DB replacing 250-row DB) -> MUST BLOCK!
    fs::remove_file(target.join("message/zalo.db")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 0);
    let err_shrunk =
        ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap_err();
    assert!(
        matches!(err_shrunk, GreenCError::PreconditionBlocked { .. }),
        "purge must fail when target DB is significantly smaller than on-disk original, got {err_shrunk:?}"
    );

    // Case 4: Restore intact SQLite DBs; keep ALL `media/` deleted AND `-wal/-shm/-journal` absent -> MUST SUCCEED!
    fs::copy(orig.join("message/zalo.db"), target.join("message/zalo.db")).unwrap();
    let ok = ZaloRelocator::purge_retained_original(retained_id, &db, &reparse).unwrap();
    assert_eq!(ok.status, ZaloRetainedStatus::Purged);
    assert!(!orig.exists());
}

#[test]
fn item2_health_verified_requires_real_zalo_write_not_greenc_probe() {
    // Subcase A: Zalo stays alive and GreenC's own write probe succeeds,
    //            but Zalo NEVER writes anything to target -> health_verified MUST be false!
    {
        let tmp = TempDir::new().unwrap();
        let (disc, _zalo_data, exe_path) = build_zalo_env_sqlite(tmp.path());
        let db = GreenCDb::open(tmp.path().join("item2_a.db")).unwrap();
        let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        let reparse = NativeReparsePointApi;
        let shell = MockShellFolderApi::new();
        let lock = MockFileLockProbe::new();
        let proc = MockProcessServiceProbe::new();
        let lifecycle = MockAppLifecycleApi::new();
        lifecycle.add_running_exe(exe_path);

        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_a");
        let report = ZaloRelocator::move_zalo_data(
            &disc,
            &ZaloMoveOptions {
                target_drive: 'D',
                custom_target_path: Some(target),
                reopen_zalo: true,
                post_reopen_watch_duration: Duration::from_millis(25),
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
            "health_verified MUST be false when only GreenC's probe wrote to target and Zalo never wrote"
        );
    }

    // Subcase B: Zalo reopens and writes its own file (`message/zalo.db-wal`) through the junction ->
    //            health_verified MUST be true!
    {
        let tmp = TempDir::new().unwrap();
        let (disc, zalo_data, exe_path) = build_zalo_env_sqlite(tmp.path());
        let db = GreenCDb::open(tmp.path().join("item2_b.db")).unwrap();
        let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        let reparse = NativeReparsePointApi;
        let shell = MockShellFolderApi::new();
        let lock = MockFileLockProbe::new();
        let proc = MockProcessServiceProbe::new();
        let lifecycle = MockAppLifecycleApi::new();
        lifecycle.add_running_exe(exe_path);
        let zalo_data_hook = zalo_data.clone();
        lifecycle.set_on_restart_hook(move || {
            // Simulate real Zalo writing WAL file through the junction into target
            let _ = fs::write(zalo_data_hook.join("message/zalo.db-wal"), b"ZALO-WAL-LIVE");
        });

        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_b");
        let report = ZaloRelocator::move_zalo_data(
            &disc,
            &ZaloMoveOptions {
                target_drive: 'D',
                custom_target_path: Some(target),
                reopen_zalo: true,
                post_reopen_watch_duration: Duration::from_millis(25),
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
            report.health_verified,
            "health_verified MUST be true when Zalo stays alive and actually writes to target"
        );
    }
}

// =========================================================================
// Item 3: restore_safety_copy only accepts copy_id from discover_zalo_safety_copies,
//         requires message/, refuses zalo-restoring.*, checks Zalo closed +
//         fingerprint right before swap, checks locks across full target tree,
//         and marks NeedsReview (never swallows error) if target rename fails.
// =========================================================================

#[test]
fn item3_restore_safety_copy_by_id_rejects_unknown_id_missing_message_and_restoring_dir() {
    use greenc_relocate::zalo_move::discover_zalo_safety_copies;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item3_a.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    fs::create_dir_all(src.join("message")).unwrap();
    create_real_sqlite_db(&src.join("message/zalo.db"), 5);

    let no_msg_conflict =
        c_dir.join("Zalo Data.greenc-conflict.aaaa1111-1111-1111-1111-111111111111");
    fs::create_dir_all(&no_msg_conflict).unwrap();
    fs::write(no_msg_conflict.join("junk.txt"), b"NO-MESSAGE-DIR").unwrap();

    let partial_restoring = c_dir.join("zalo-restoring.bbbb2222-2222-2222-2222-222222222222");
    fs::create_dir_all(partial_restoring.join("message")).unwrap();
    create_real_sqlite_db(&partial_restoring.join("message/zalo.db"), 5);

    let valid_conflict =
        c_dir.join("Zalo Data.greenc-conflict.cccc3333-3333-3333-3333-333333333333");
    fs::create_dir_all(valid_conflict.join("message")).unwrap();
    create_real_sqlite_db(&valid_conflict.join("message/zalo.db"), 10);
    fs::write(valid_conflict.join("message/marker.txt"), b"VALID-CONFLICT").unwrap();

    let copies = discover_zalo_safety_copies(Some(&src), &db);
    let no_msg_info = copies
        .iter()
        .find(|c| c.path == no_msg_conflict)
        .expect("no-msg conflict must be discovered");
    let restoring_info = copies
        .iter()
        .find(|c| c.path == partial_restoring)
        .expect("zalo-restoring must be discovered");
    let valid_info = copies
        .iter()
        .find(|c| c.path == valid_conflict)
        .expect("valid conflict must be discovered");

    assert!(!no_msg_info.copy_id.is_empty());
    assert!(!restoring_info.copy_id.is_empty());
    assert!(!valid_info.copy_id.is_empty());
    assert!(
        !no_msg_info.restorable,
        "copy without message/ MUST NOT be marked restorable"
    );
    assert!(
        !restoring_info.restorable,
        "partial zalo-restoring.* dir MUST NOT be offered for restore"
    );
    assert!(
        valid_info.restorable,
        "valid conflict copy with message/ should be restorable"
    );

    // 1. Unknown/arbitrary path passed as ID -> MUST fail!
    let err_unknown = ZaloRelocator::restore_safety_copy_by_id(
        &valid_conflict.to_string_lossy(),
        Some(&src),
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(
        matches!(
            err_unknown,
            GreenCError::NotFound { .. } | GreenCError::PreconditionBlocked { .. }
        ),
        "raw path MUST NOT be accepted as copy_id, got {err_unknown:?}"
    );

    // 2. Copy missing `message/` -> MUST fail with PreconditionBlocked!
    let err_no_msg = ZaloRelocator::restore_safety_copy_by_id(
        &no_msg_info.copy_id,
        Some(&src),
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(
        matches!(err_no_msg, GreenCError::PreconditionBlocked { .. }),
        "copy without message/ MUST be blocked, got {err_no_msg:?}"
    );

    // 3. Partial `zalo-restoring.*` -> MUST fail with PreconditionBlocked!
    let err_restoring = ZaloRelocator::restore_safety_copy_by_id(
        &restoring_info.copy_id,
        Some(&src),
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(
        matches!(err_restoring, GreenCError::PreconditionBlocked { .. }),
        "partial zalo-restoring.* MUST be refused, got {err_restoring:?}"
    );

    // 4. Valid discovered copy_id -> succeeds and resolves source from discovery!
    let msg = ZaloRelocator::restore_safety_copy_by_id(
        &valid_info.copy_id,
        Some(&src),
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(!msg.is_empty());
    assert_eq!(
        fs::read(src.join("message/marker.txt")).unwrap(),
        b"VALID-CONFLICT"
    );
}

#[test]
fn item3_restore_safety_copy_checks_target_tree_lock_pre_swap_fingerprint_and_target_rename_error()
{
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item3_b.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let target = d_dir.join("zalo");
    let copy_on_d = d_dir.join("Zalo Data.greenc-conflict.dddd4444-4444-4444-4444-444444444444");

    fs::create_dir_all(&c_dir).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 5);
    fs::create_dir_all(copy_on_d.join("message")).unwrap();
    create_real_sqlite_db(&copy_on_d.join("message/zalo.db"), 8);
    reparse.create_junction(&src, &target).unwrap();

    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 1024,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // Subcase A: File inside linked target tree is locked -> MUST block with FileLocked!
    let lock_target = MockFileLockProbe::new();
    lock_target.lock(&target.join("message/zalo.db"));
    let err_locked = ZaloRelocator::restore_safety_copy_by_path(
        &copy_on_d,
        &src,
        &db,
        &reparse,
        &lock_target,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(
        matches!(err_locked, GreenCError::FileLocked { .. }),
        "restore MUST check locks across entire linked target tree, got {err_locked:?}"
    );
    assert!(
        reparse.is_reparse_point(&src),
        "junction must remain untouched when target tree is locked"
    );

    // Subcase B: Copy mutates right before swap (during staging) -> MUST abort with IntegrityMismatch!
    let lock_free = MockFileLockProbe::new();
    let copy_mut = copy_on_d.clone();
    let err_drift = ZaloRelocator::restore_safety_copy_by_path_with_hook(
        &copy_on_d,
        &src,
        &db,
        &reparse,
        &lock_free,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
        Some(&move || {
            let _ = fs::write(copy_mut.join("message/drift.tmp"), b"MUTATED-BEFORE-SWAP");
        }),
    )
    .unwrap_err();
    assert!(
        matches!(err_drift, GreenCError::IntegrityMismatch { .. }),
        "restore MUST verify fingerprint right before swap, got {err_drift:?}"
    );
    assert!(
        reparse.is_reparse_point(&src),
        "junction must remain untouched on pre-swap fingerprint drift"
    );
    let _ = fs::remove_file(copy_on_d.join("message/drift.tmp"));

    // Subcase C (Unix non-root): Target parent is read-only so renaming linked `target` to
    // `.greenc-conflict.*` FAILS -> MUST NOT swallow error, MUST mark relocation `NeedsReview`!
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Skip only if running as root (chmod 0555 does not restrict root)
        let probe_dir = tmp.path().join("ro_probe_3");
        fs::create_dir_all(&probe_dir).unwrap();
        fs::set_permissions(&probe_dir, fs::Permissions::from_mode(0o555)).unwrap();
        let is_non_root = fs::write(probe_dir.join("t"), b"x").is_err();
        let _ = fs::set_permissions(&probe_dir, fs::Permissions::from_mode(0o755));

        if is_non_root {
            let copy_on_c =
                c_dir.join("Zalo Data.greenc-conflict.eeee5555-5555-5555-5555-555555555555");
            fs::create_dir_all(copy_on_c.join("message")).unwrap();
            create_real_sqlite_db(&copy_on_c.join("message/zalo.db"), 8);

            // Make `d_dir` read-only so `fs::rename(&target, &tgt_conflict)` fails!
            fs::set_permissions(&d_dir, fs::Permissions::from_mode(0o555)).unwrap();

            let res = ZaloRelocator::restore_safety_copy_by_path(
                &copy_on_c,
                &src,
                &db,
                &reparse,
                &lock_free,
                &proc,
                &lifecycle,
                Duration::from_secs(2),
            );

            let _ = fs::set_permissions(&d_dir, fs::Permissions::from_mode(0o755));

            assert!(
                res.is_err(),
                "target rename failure MUST NOT be swallowed, got {res:?}"
            );
            let updated = db
                .list_relocations(None)
                .unwrap()
                .into_iter()
                .find(|r| r.id == Some(reloc_id))
                .unwrap();
            assert_eq!(
                updated.status,
                RelocationStatus::NeedsReview,
                "target rename failure MUST mark relocation NeedsReview, not Undone"
            );
        }
    }
}

// =========================================================================
// Item 4: Restore/undo sets relocation Moving durably before removing junction;
//         post-crash recovery rebuilds junction to target when moving_path is
//         undo temp dir or retained row is Restoring and source is missing;
//         only matches restoring dir belonging to that record; create_junction
//         failure returns conflict to place and NEVER saves Done.
// =========================================================================

#[test]
fn item4_crash_recovery_rebuilds_junction_for_undo_temp_and_restoring_row_and_scopes_restoring_dir()
{
    use greenc_relocate::recovery::RecoveryService;

    // Subcase A: Crash during undo (`moving_path` is `zalo-restoring.<uuid>`, source missing,
    //            target online) + unrelated `restoring.<other>` in same parent dir.
    {
        let tmp = TempDir::new().unwrap();
        let db = GreenCDb::open(tmp.path().join("item4_a.db")).unwrap();
        let reparse = NativeReparsePointApi;

        let c_dir = tmp.path().join("C_Drive");
        let src = c_dir.join("Zalo Data");
        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
        let my_undo_tmp = c_dir.join("zalo-restoring.11111111-1111-1111-1111-111111111111");
        let unrelated_restoring = c_dir.join("restoring.99999999-9999-9999-9999-999999999999");

        fs::create_dir_all(target.join("message")).unwrap();
        create_real_sqlite_db(&target.join("message/zalo.db"), 10);
        fs::create_dir_all(my_undo_tmp.join("message")).unwrap();
        create_real_sqlite_db(&my_undo_tmp.join("message/zalo.db"), 10);
        fs::create_dir_all(&unrelated_restoring).unwrap();
        fs::write(unrelated_restoring.join("other.bin"), b"OTHER-APP").unwrap();

        let rec_id = db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: "zalo".to_string(),
                app_name: "Zalo PC".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: src.clone(),
                target_path: target.clone(),
                size_bytes: 1024,
                status: RelocationStatus::Moving,
                health_status: RelocationHealthStatus::Healthy,
                process_names: vec!["Zalo.exe".to_string()],
                created_at: Utc::now(),
                detail: Some("Đang hoàn tác...".to_string()),
                moving_path: Some(my_undo_tmp.clone()),
                target_user_repaired: false,
            })
            .unwrap();

        let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, Some(rec_id));
        assert_eq!(
            recovered[0].status,
            RelocationStatus::Done,
            "undo crash with online target must rebuild junction and return to Done, got {:?}",
            recovered[0]
        );
        assert_eq!(recovered[0].health_status, RelocationHealthStatus::Healthy);
        assert!(
            reparse.is_reparse_point(&src),
            "junction at source MUST be rebuilt to target"
        );
        assert_eq!(reparse.get_junction_target(&src).unwrap(), target);
        assert!(
            !my_undo_tmp.exists(),
            "own undo staging dir should be cleaned after verified junction rebuild"
        );
        assert!(
            unrelated_restoring.join("other.bin").exists(),
            "unrelated restoring.* dir from another record MUST NOT be touched or block recovery"
        );
    }

    // Subcase B: Crash during `restore_retained_original` after `delete_junction` (retained row
    //            is `Restoring`, source is missing, target is online).
    {
        let tmp = TempDir::new().unwrap();
        let db = GreenCDb::open(tmp.path().join("item4_b.db")).unwrap();
        let reparse = NativeReparsePointApi;

        let c_dir = tmp.path().join("C_Drive");
        let src = c_dir.join("Zalo Data");
        let orig = c_dir.join("Zalo Data.greenc-original.22222222-2222-2222-2222-222222222222");
        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");

        fs::create_dir_all(orig.join("message")).unwrap();
        create_real_sqlite_db(&orig.join("message/zalo.db"), 5);
        fs::create_dir_all(target.join("message")).unwrap();
        create_real_sqlite_db(&target.join("message/zalo.db"), 8);

        let now = Utc::now();
        let reloc_id = db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: "zalo".to_string(),
                app_name: "Zalo PC".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: src.clone(),
                target_path: target.clone(),
                size_bytes: 1024,
                status: RelocationStatus::Moving,
                health_status: RelocationHealthStatus::Healthy,
                process_names: vec!["Zalo.exe".to_string()],
                created_at: now,
                detail: Some("Đang khôi phục bản gốc...".to_string()),
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
                retained_path: orig.clone(),
                size_bytes: 1024,
                file_count: 1,
                message_file_count: 1,
                health_verified: false,
                created_at: now,
                purge_after: now + ChronoDuration::hours(24),
                purge_after_valid: true,
                observed_healthy_secs: 0,
                last_observed_at: Some(now),
                status: ZaloRetainedStatus::Restoring,
                resolved_at: None,
                detail: Some("Đang khôi phục...".to_string()),
            })
            .unwrap();

        let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
        assert!(!recovered.is_empty());
        assert!(
            reparse.is_reparse_point(&src),
            "crash in Restoring with missing source MUST rebuild junction to target"
        );
        assert_eq!(reparse.get_junction_target(&src).unwrap(), target);
        assert!(orig.join("message/zalo.db").exists());
        let ret_after = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
        assert_eq!(
            ret_after.status,
            ZaloRetainedStatus::Retained,
            "retained row must revert from Restoring to Retained after junction rebuild"
        );
        let reloc_after = db
            .list_relocations(None)
            .unwrap()
            .into_iter()
            .find(|r| r.id == Some(reloc_id))
            .unwrap();
        assert_eq!(reloc_after.status, RelocationStatus::Done);
    }
}

#[cfg(unix)]
#[test]
fn item4_undo_rename_failure_never_saves_done_on_create_junction_err_and_restores_conflict() {
    use greenc_relocate::mover::TransactionalMover;
    use std::os::unix::fs::PermissionsExt;

    // Skip if running as root (chmod 0555 does not block rename/create)
    let root_probe = TempDir::new().unwrap();
    let pdir = root_probe.path().join("p");
    fs::create_dir_all(&pdir).unwrap();
    fs::set_permissions(&pdir, fs::Permissions::from_mode(0o555)).unwrap();
    let is_non_root = fs::write(pdir.join("x"), b"1").is_err();
    let _ = fs::set_permissions(&pdir, fs::Permissions::from_mode(0o755));
    if !is_non_root {
        return;
    }

    // Subcase A: `source` is a junction; `fs::rename(restore_tmp, source)` fails AND
    //            `create_junction` ALSO fails -> record MUST be `NeedsReview` (NEVER `Done`!).
    {
        let tmp = TempDir::new().unwrap();
        let db = GreenCDb::open(tmp.path().join("item4_undo_a.db")).unwrap();
        let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        let real_reparse = NativeReparsePointApi;
        let shell = MockShellFolderApi::new();
        let proc = MockProcessServiceProbe::new();
        let lock = MockFileLockProbe::new();

        let c_dir = tmp.path().join("C_Drive");
        let src = c_dir.join("Zalo Data");
        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
        fs::create_dir_all(&c_dir).unwrap();
        fs::create_dir_all(target.join("message")).unwrap();
        create_real_sqlite_db(&target.join("message/zalo.db"), 5);
        real_reparse.create_junction(&src, &target).unwrap();

        let rec_id = db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: "zalo".to_string(),
                app_name: "Zalo PC".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: src.clone(),
                target_path: target.clone(),
                size_bytes: 1024,
                status: RelocationStatus::Done,
                health_status: RelocationHealthStatus::Healthy,
                process_names: vec!["Zalo.exe".to_string()],
                created_at: Utc::now(),
                detail: None,
                moving_path: None,
                target_user_repaired: false,
            })
            .unwrap();

        struct DeleteAndObstructReparse {
            inner: NativeReparsePointApi,
        }
        impl ReparsePointApi for DeleteAndObstructReparse {
            fn is_reparse_point(&self, path: &Path) -> bool {
                self.inner.is_reparse_point(path)
            }
            fn get_junction_target(&self, path: &Path) -> greenc_core::error::Result<PathBuf> {
                self.inner.get_junction_target(path)
            }
            fn create_junction(
                &self,
                _link: &Path,
                _target: &Path,
            ) -> greenc_core::error::Result<()> {
                Err(GreenCError::Relocation(
                    "simulated create_junction failure".to_string(),
                ))
            }
            fn delete_junction(&self, link: &Path) -> greenc_core::error::Result<()> {
                self.inner.delete_junction(link)?;
                fs::create_dir_all(link)?;
                fs::write(link.join("blocker.txt"), b"BLOCK").unwrap();
                Ok(())
            }
        }

        let failing = DeleteAndObstructReparse {
            inner: NativeReparsePointApi,
        };
        let res = TransactionalMover::undo_relocation_with_deps(
            rec_id,
            &db,
            &vol,
            &failing,
            &shell,
            &proc,
            &lock,
            None,
            &TaskControl::new(),
            None,
        );
        assert!(res.is_err(), "undo must fail when rename fails");
        let after = db
            .list_relocations(None)
            .unwrap()
            .into_iter()
            .find(|r| r.id == Some(rec_id))
            .unwrap();
        assert_eq!(
            after.status,
            RelocationStatus::NeedsReview,
            "when create_junction rollback fails, record MUST NOT be saved as Done — must be NeedsReview, got {:?}",
            after.status
        );
        assert_eq!(after.health_status, RelocationHealthStatus::LinkBroken);
    }

    // Subcase B: `restore_retained_original` persists `relocations.status = Moving` durably
    //            BEFORE calling `delete_junction`!
    {
        struct AssertMovingOnDeleteReparse {
            inner: NativeReparsePointApi,
            db_path: PathBuf,
            reloc_id: i64,
            saw_moving: std::sync::atomic::AtomicBool,
        }
        impl ReparsePointApi for AssertMovingOnDeleteReparse {
            fn is_reparse_point(&self, path: &Path) -> bool {
                self.inner.is_reparse_point(path)
            }
            fn get_junction_target(&self, path: &Path) -> greenc_core::error::Result<PathBuf> {
                self.inner.get_junction_target(path)
            }
            fn create_junction(
                &self,
                link: &Path,
                target: &Path,
            ) -> greenc_core::error::Result<()> {
                self.inner.create_junction(link, target)
            }
            fn delete_junction(&self, link: &Path) -> greenc_core::error::Result<()> {
                let db2 = GreenCDb::open(&self.db_path).unwrap();
                let st = db2
                    .list_relocations(None)
                    .unwrap()
                    .into_iter()
                    .find(|r| r.id == Some(self.reloc_id))
                    .unwrap()
                    .status;
                if st == RelocationStatus::Moving {
                    self.saw_moving
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                self.inner.delete_junction(link)
            }
        }

        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("item4_durable_moving.db");
        let db = GreenCDb::open(&db_path).unwrap();
        let real_reparse = NativeReparsePointApi;
        let lock = MockFileLockProbe::new();
        let proc = MockProcessServiceProbe::new();
        let lifecycle = MockAppLifecycleApi::new();

        let c_dir = tmp.path().join("C_Drive");
        let src = c_dir.join("Zalo Data");
        let orig = c_dir.join("Zalo Data.greenc-original.33333333-3333-3333-3333-333333333333");
        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
        fs::create_dir_all(orig.join("message")).unwrap();
        create_real_sqlite_db(&orig.join("message/zalo.db"), 5);
        fs::create_dir_all(target.join("message")).unwrap();
        create_real_sqlite_db(&target.join("message/zalo.db"), 5);
        real_reparse.create_junction(&src, &target).unwrap();

        let now = Utc::now();
        let reloc_id = db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: "zalo".to_string(),
                app_name: "Zalo PC".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: src.clone(),
                target_path: target.clone(),
                size_bytes: 1024,
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
                retained_path: orig.clone(),
                size_bytes: 1024,
                file_count: 1,
                message_file_count: 1,
                health_verified: false,
                created_at: now,
                purge_after: now + ChronoDuration::hours(24),
                purge_after_valid: true,
                observed_healthy_secs: 0,
                last_observed_at: Some(now),
                status: ZaloRetainedStatus::Retained,
                resolved_at: None,
                detail: None,
            })
            .unwrap();

        let spy = AssertMovingOnDeleteReparse {
            inner: NativeReparsePointApi,
            db_path: db_path.clone(),
            reloc_id,
            saw_moving: std::sync::atomic::AtomicBool::new(false),
        };
        ZaloRelocator::restore_retained_original(
            retained_id,
            &db,
            &spy,
            &lock,
            &proc,
            &lifecycle,
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(
            spy.saw_moving.load(std::sync::atomic::Ordering::SeqCst),
            "restore_retained_original MUST persist relocation status=Moving durably BEFORE delete_junction"
        );
    }
}

// =========================================================================
// Item 5: Restore when target is offline refuses unless explicitly confirmed;
//         when confirmed (or on offline rollback), keeps NeedsReview with
//         target path, and discover_zalo_safety_copies lists the target.
// =========================================================================

#[test]
fn item5_restore_offline_target_refuses_without_confirm_and_keeps_needs_review_and_lists_target() {
    use greenc_relocate::zalo_move::discover_zalo_safety_copies;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item5_a.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    let orig = c_dir.join("Zalo Data.greenc-original.55555555-5555-5555-5555-555555555555");
    let offline_target = tmp.path().join("D_Offline/GreenC/Relocated/zalo");

    fs::create_dir_all(orig.join("message")).unwrap();
    create_real_sqlite_db(&orig.join("message/zalo.db"), 5);
    fs::create_dir_all(offline_target.join("message")).unwrap();
    reparse.create_junction(&src, &offline_target).unwrap();
    // Simulate drive D: going offline
    fs::remove_dir_all(tmp.path().join("D_Offline")).unwrap();

    let now = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: offline_target.clone(),
            size_bytes: 1024,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::TargetOffline,
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
            target_path: offline_target.clone(),
            retained_path: orig.clone(),
            size_bytes: 1024,
            file_count: 1,
            message_file_count: 1,
            health_verified: false,
            created_at: now,
            purge_after: now + ChronoDuration::hours(24),
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    // 1. Default restore (allow_offline_target = false) MUST REFUSE!
    let err = ZaloRelocator::restore_retained_original(
        retained_id,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(
        matches!(err, GreenCError::PreconditionBlocked { .. }),
        "restore when target is offline MUST refuse without explicit confirmation, got {err:?}"
    );
    assert!(
        reparse.is_reparse_point(&src),
        "junction must remain untouched when offline restore is refused"
    );
    assert!(orig.join("message/zalo.db").exists());

    // 2. Explicitly confirmed restore (`allow_offline_target = true`) succeeds BUT keeps
    //    `NeedsReview` + `TargetOffline` + target path in detail (NEVER `Undone`)!
    let restored = ZaloRelocator::restore_retained_original_with_options(
        retained_id,
        true,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(restored.status, ZaloRetainedStatus::Restored);
    assert!(!reparse.is_reparse_point(&src));
    assert!(src.join("message/zalo.db").exists());

    let reloc_after = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == Some(reloc_id))
        .unwrap();
    assert_eq!(
        reloc_after.status,
        RelocationStatus::NeedsReview,
        "confirmed offline-target restore MUST keep relocation in NeedsReview, got {:?}",
        reloc_after.status
    );
    assert_eq!(
        reloc_after.health_status,
        RelocationHealthStatus::TargetOffline
    );
    assert!(
        reloc_after
            .detail
            .as_deref()
            .unwrap_or("")
            .contains(&offline_target.display().to_string()),
        "NeedsReview detail must include offline target path"
    );

    // 3. `discover_zalo_safety_copies` MUST list the offline target path!
    let copies_offline = discover_zalo_safety_copies(Some(&src), &db);
    let listed_offline = copies_offline
        .iter()
        .find(|c| c.path == offline_target)
        .expect("offline target MUST be listed by discover_zalo_safety_copies");
    assert!(!listed_offline.exists_on_disk);
    assert_eq!(listed_offline.status, "target_offline");

    // 4. When target drive reconnects, `discover_zalo_safety_copies` lists it as online & restorable!
    fs::create_dir_all(offline_target.join("message")).unwrap();
    create_real_sqlite_db(&offline_target.join("message/zalo.db"), 12);
    let copies_online = discover_zalo_safety_copies(Some(&src), &db);
    let listed_online = copies_online
        .iter()
        .find(|c| c.path == offline_target)
        .expect("reconnected target MUST still be listed by discover_zalo_safety_copies");
    assert!(listed_online.exists_on_disk);
    assert!(listed_online.restorable);
}

#[test]
fn item5_rollback_when_target_offline_keeps_needs_review_and_lists_target() {
    use greenc_relocate::zalo_move::discover_zalo_safety_copies;

    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item5_b.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Offline_Watch/GreenC/Relocated/zalo");
    let target_root = tmp.path().join("D_Offline_Watch");
    let src_hook = zalo_data.clone();
    lifecycle.set_on_restart_hook(move || {
        let _ = fs::remove_dir_all(&target_root);
        #[cfg(windows)]
        let _ = fs::remove_dir(&src_hook);
        #[cfg(not(windows))]
        let _ = fs::remove_file(&src_hook);
    });

    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(30),
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
    assert!(err.to_string().contains(&target.display().to_string()));

    let reloc = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        reloc.status,
        RelocationStatus::NeedsReview,
        "offline-target rollback MUST keep NeedsReview (not Undone), got {:?}",
        reloc.status
    );
    assert_eq!(reloc.health_status, RelocationHealthStatus::TargetOffline);
    assert!(reloc
        .detail
        .as_deref()
        .unwrap_or("")
        .contains(&target.display().to_string()));

    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    assert!(
        copies.iter().any(|c| c.path == target),
        "offline target from rollback MUST be listed in discover_zalo_safety_copies"
    );
}

// =========================================================================
// Item 6: Dead-end fixes:
//   - real source + no moving dir + recorded moving_path missing -> Failed,
//     target renamed to .greenc-conflict.<uuid>
//   - `data_is_known_relocation` is true ONLY when source is ACTUALLY a
//     reparse point
//   - Zalo reinstalled (real dir replaced junction): Undo refuses clearly,
//     preserving both real source and old target; target is listed
// =========================================================================

#[test]
fn item6_recovery_real_source_no_moving_dir_marks_failed_and_renames_target_to_conflict() {
    use greenc_relocate::recovery::RecoveryService;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item6_a.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let missing_moving = c_dir.join("Zalo Data.greenc-moving.66666666-6666-6666-6666-666666666666");
    let target = d_dir.join("zalo");

    fs::create_dir_all(src.join("message")).unwrap();
    create_real_sqlite_db(&src.join("message/zalo.db"), 10);
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 10);

    // Crash right after persisting `moving_path` and finalizing `target`, BEFORE `fs::rename(src, moving)`!
    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 4096,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: Some("Đang đổi chỗ thư mục...".to_string()),
            moving_path: Some(missing_moving),
            target_user_repaired: false,
        })
        .unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, Some(rec_id));
    assert_eq!(
        recovered[0].status,
        RelocationStatus::Failed,
        "real source + missing recorded moving_path MUST resolve to Failed (not NeedsReview dead-end), got {:?}",
        recovered[0]
    );
    assert_eq!(recovered[0].health_status, RelocationHealthStatus::Healthy);
    assert!(
        src.join("message/zalo.db").exists() && !reparse.is_reparse_point(&src),
        "real source must remain intact"
    );
    assert!(!target.exists(), "target should be renamed to conflict");
    let has_conflict = fs::read_dir(&d_dir).unwrap().flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .contains(".greenc-conflict.")
    });
    assert!(
        has_conflict,
        "target MUST be renamed to .greenc-conflict.* (never deleted)"
    );
}

#[test]
fn item6_known_relocation_requires_actual_reparse_point_and_undo_refuses_reinstalled_zalo() {
    use greenc_relocate::mover::TransactionalMover;
    use greenc_relocate::zalo_move::discover_zalo_safety_copies;
    use std::collections::HashSet;

    let tmp = TempDir::new().unwrap();
    let (disc0, zalo_data, _exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item6_b.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();

    // 1. Even if `zalo_data` is listed in `known_relocated_sources`, because `zalo_data` is a
    //    REAL directory (NOT a reparse point!), `data_is_known_relocation` MUST be `false`!
    let mut known = HashSet::new();
    known.insert(zalo_data.clone());
    let disc_real = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(disc0.config_path.clone()),
        local_appdata: Some(tmp.path().join("Local")),
        system_drive_root: Some(tmp.path().join("C_Drive")),
        known_relocated_sources: known.clone(),
        ..Default::default()
    })
    .unwrap();
    assert!(
        !disc_real.data_is_known_relocation,
        "a real (non-reparse) directory MUST NEVER be treated as already relocated!"
    );

    // 2. Simulate Zalo reinstalled: DB has `Done` relocation to `target`, while `zalo_data` on C:
    //    is now a REAL directory containing newly created messages (not a junction)!
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 20);
    fs::write(
        zalo_data.join("message/reinstalled_marker.txt"),
        b"NEW-ZALO",
    )
    .unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: zalo_data.clone(),
            target_path: target.clone(),
            size_bytes: 4096,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // Undo from history MUST REFUSE when source was replaced by a real directory!
    let err = TransactionalMover::undo_relocation(
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
        "Zalo undo MUST refuse when source is a real dir (reinstalled Zalo), got {err:?}"
    );
    assert_eq!(
        fs::read(zalo_data.join("message/reinstalled_marker.txt")).unwrap(),
        b"NEW-ZALO",
        "reinstalled real Zalo Data dir MUST remain completely untouched"
    );
    assert!(
        target.join("message/zalo.db").exists(),
        "old target copy on D: MUST remain completely untouched"
    );

    // Old target on D: must also be discoverable in safety copies so user can inspect/restore it!
    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    assert!(
        copies.iter().any(|c| c.path == target),
        "old target copy on D: must be listed when junction was replaced by a real dir"
    );
}

// =========================================================================
// Item 7: Rollback safety:
//   - recover_after_rename_failure NEVER deletes target if rename fails
//   - rollback_broken_zalo_relocation uses the 24h-retention undo path,
//     persists durably, and reopens Zalo after rollback
// =========================================================================

#[test]
fn item7_rollback_retains_target_24h_durably_and_reopens_zalo() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item7_a.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_rb");
    let fired = Arc::new(AtomicBool::new(false));
    let fired_cb = Arc::clone(&fired);
    let link_cb = zalo_data.clone();
    let target_cb = target.clone();
    lifecycle.set_on_restart_hook(move || {
        if fired_cb.swap(true, Ordering::SeqCst) {
            return;
        }
        // Write a new message to target, then break the junction so rollback triggers!
        let _ = fs::write(target_cb.join("message/after_move.txt"), b"LATEST-MSG");
        #[cfg(windows)]
        let _ = fs::remove_dir(&link_cb);
        #[cfg(not(windows))]
        let _ = fs::remove_file(&link_cb);
        fs::create_dir_all(link_cb.join("message")).unwrap();
        fs::write(link_cb.join("message/conflict.txt"), b"CONFLICT-MSG").unwrap();
    });

    let err = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: true,
            post_reopen_watch_duration: Duration::from_millis(30),
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

    // 1. Zalo MUST have been reopened after rollback (1st restart for move, 2nd after rollback)!
    assert_eq!(
        lifecycle.restart_count(),
        2,
        "rollback MUST reopen Zalo after restoring data"
    );

    // 2. Target copy MUST be retained for 24h in DB (renamed to .greenc-original.*), NOT deleted!
    assert!(!target.exists(), "original target path should be renamed");
    let retained_rows = db.list_zalo_retained_originals(Some("retained")).unwrap();
    let retained_target = retained_rows
        .iter()
        .find(|r| {
            r.retained_path
                .parent()
                .is_some_and(|p| p == target.parent().unwrap())
                && r.retained_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(".greenc-original."))
        })
        .expect("rollback MUST retain target as .greenc-original.* for 24h in DB");
    assert!(
        retained_target
            .retained_path
            .join("message/after_move.txt")
            .exists(),
        "24h retained target copy must still exist intact on disk"
    );
    let hours = (retained_target.purge_after - retained_target.created_at).num_hours();
    assert!(hours >= 23, "target copy must be retained for ~24h");
}

#[cfg(unix)]
#[test]
fn item7_recover_after_rename_failure_never_deletes_target_when_rename_fails() {
    use greenc_platform::traits::MockReparsePointApi;
    use greenc_relocate::zalo_move::ZaloMoveHooks;
    use std::os::unix::fs::PermissionsExt;

    let root_probe = TempDir::new().unwrap();
    let pdir = root_probe.path().join("p");
    fs::create_dir_all(&pdir).unwrap();
    fs::set_permissions(&pdir, fs::Permissions::from_mode(0o555)).unwrap();
    let is_non_root = fs::write(pdir.join("x"), b"1").is_err();
    let _ = fs::set_permissions(&pdir, fs::Permissions::from_mode(0o755));
    if !is_non_root {
        return;
    }

    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, _exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item7_b.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let mock_reparse = MockReparsePointApi::new();
    mock_reparse.set_fail_create(true);
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let d_parent = tmp.path().join("D_Drive/GreenC/Relocated");
    let target = d_parent.join("zalo_ro");
    let d_parent_cb = d_parent.clone();
    let after_rename = move |_src: &Path, _moving: &Path, _tgt: &Path| {
        // Make target parent read-only so `fs::rename(target, target_cleanup)` FAILS!
        fs::set_permissions(&d_parent_cb, fs::Permissions::from_mode(0o555)).unwrap();
    };

    let res = ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
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
        &ZaloMoveHooks {
            after_rename: Some(&after_rename),
            ..Default::default()
        },
    );

    // Restore permissions for inspection & cleanup
    let _ = fs::set_permissions(&d_parent, fs::Permissions::from_mode(0o755));

    assert!(res.is_err());
    assert!(zalo_data.join("message/zalo.db").exists());
    // CRITICAL: Because `fs::rename(target, target_cleanup)` failed, `target` MUST NOT have been
    // deleted directly!
    assert!(
        target.join("message/zalo.db").exists(),
        "when rename of target fails during rollback, target MUST NEVER be deleted directly!"
    );
}

// =========================================================================
// Item 9: Post-undo/conflict confirmed delete for ANY listed safety copy
//         (only when live source covers it), AND undo checks C: free space
//         BEFORE shutting down Zalo.
// =========================================================================

#[test]
fn item9_delete_safety_copy_works_for_post_undo_and_conflict_only_when_live_source_covers_copy() {
    use greenc_relocate::zalo_move::discover_zalo_safety_copies;

    let tmp = TempDir::new().unwrap();
    let (_disc, zalo_data, _exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item9_a.db")).unwrap();
    let reparse = NativeReparsePointApi;

    // 1. Create a conflict copy on C: that has an EXTRA message DB NOT present in live `zalo_data`
    let conflict_copy = zalo_data
        .parent()
        .unwrap()
        .join("Zalo Data.greenc-conflict.item9");
    fs::create_dir_all(conflict_copy.join("message")).unwrap();
    create_real_sqlite_db(&conflict_copy.join("message/zalo.db"), 10);
    create_real_sqlite_db(&conflict_copy.join("message/only_in_conflict.db"), 10);

    // 2. Create a post-undo 24h retained target copy on D: (relocation status = Undone!)
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    fs::create_dir_all(&d_dir).unwrap();
    let post_undo_retained = d_dir.join("zalo.greenc-original.postundo");
    fs::create_dir_all(post_undo_retained.join("message")).unwrap();
    create_real_sqlite_db(&post_undo_retained.join("message/zalo.db"), 10);

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: zalo_data.clone(),
            target_path: d_dir.join("zalo"),
            size_bytes: 4096,
            status: RelocationStatus::Undone,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let now = Utc::now();
    let retained_id = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: rec_id,
            source_path: zalo_data.clone(),
            target_path: d_dir.join("zalo"),
            retained_path: post_undo_retained.clone(),
            size_bytes: 4096,
            file_count: 1,
            message_file_count: 1,
            health_verified: false,
            created_at: now,
            purge_after: now + ChronoDuration::hours(24),
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    // 3. Also create an untracked `.greenc-purging.*` dir
    let purging_copy = zalo_data
        .parent()
        .unwrap()
        .join("Zalo Data.greenc-purging.leftover");
    fs::create_dir_all(&purging_copy).unwrap();
    fs::write(purging_copy.join("half.tmp"), b"half").unwrap();

    let copies = discover_zalo_safety_copies(Some(&zalo_data), &db);
    let conflict_info = copies
        .iter()
        .find(|c| c.path == conflict_copy)
        .expect("conflict copy must be listed");
    let post_undo_info = copies
        .iter()
        .find(|c| c.path == post_undo_retained)
        .expect("post-undo retained copy must be listed");
    let purging_info = copies
        .iter()
        .find(|c| c.path == purging_copy)
        .expect("purging copy must be listed");

    // A. Refuses to delete conflict copy when live `zalo_data` is missing `only_in_conflict.db`!
    let err = ZaloRelocator::delete_safety_copy_by_id(
        &conflict_info.copy_id,
        Some(&zalo_data),
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(err, GreenCError::PreconditionBlocked { .. }),
        "must refuse deleting conflict copy when live source lacks one of its DBs, got {err:?}"
    );
    assert!(
        conflict_copy.join("message/only_in_conflict.db").exists(),
        "conflict copy must remain intact after refusal"
    );

    // B. Once live `zalo_data` covers `only_in_conflict.db`, confirmed delete SUCCEEDS!
    create_real_sqlite_db(&zalo_data.join("message/only_in_conflict.db"), 12);
    ZaloRelocator::delete_safety_copy_by_id(
        &conflict_info.copy_id,
        Some(&zalo_data),
        &db,
        &reparse,
    )
    .expect("confirmed delete of conflict copy must succeed once live source covers all DBs");
    assert!(!conflict_copy.exists(), "conflict copy should be removed");

    // C. Post-undo 24h retained copy on D: can be deleted via confirmed delete once live C: covers it!
    ZaloRelocator::delete_safety_copy_by_id(
        &post_undo_info.copy_id,
        Some(&zalo_data),
        &db,
        &reparse,
    )
    .expect("confirmed delete of post-undo retained copy must succeed when live C: covers it");
    assert!(
        !post_undo_retained.exists(),
        "post-undo retained copy should be removed"
    );
    let row = db.get_zalo_retained_original(retained_id).unwrap().unwrap();
    assert_eq!(row.status, ZaloRetainedStatus::Purged);

    // D. Untracked `.greenc-purging.*` leftover can be cleaned up via delete_safety_copy_by_id!
    ZaloRelocator::delete_safety_copy_by_id(&purging_info.copy_id, Some(&zalo_data), &db, &reparse)
        .expect("finishing untracked .greenc-purging leftover must succeed");
    assert!(!purging_copy.exists());
}

#[test]
fn item9_zalo_undo_checks_c_free_space_before_closing_zalo() {
    use greenc_relocate::mover::TransactionalMover;

    let tmp = TempDir::new().unwrap();
    let (_disc, zalo_data, exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item9_b.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();
    let lock = MockFileLockProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path);

    // Set up a valid relocated Zalo (junction zalo_data -> target)
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_space");
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 20);
    fs::remove_dir_all(&zalo_data).unwrap();
    reparse.create_junction(&zalo_data, &target).unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: zalo_data.clone(),
            target_path: target.clone(),
            size_bytes: 100 * 1024 * 1024,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // C: only has 200 MiB free (< 100 MiB + 512 MiB reserve)!
    let vol = make_drives(200 * 1024 * 1024, 100 * 1024 * 1024 * 1024);

    let err = TransactionalMover::undo_relocation_with_deps(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &lock,
        Some(&lifecycle),
        &TaskControl::new(),
        None,
    )
    .unwrap_err();
    assert!(
        matches!(err, GreenCError::InsufficientVaultSpace(_)),
        "undo must reject when C: lacks free space + 512 MiB reserve, got {err:?}"
    );
    assert_eq!(
        lifecycle.shutdown_count(),
        0,
        "undo MUST check C: free space BEFORE shutting down Zalo!"
    );
}

// =========================================================================
// Item 10: Non-Zalo app regressions:
//   - Generic undo normal failure reverts record to Done + rebuilds junction
//   - `restoring.*` only blocks the matching record
//   - `repair_junction_target` ignores `Failed` records during overlap check
// =========================================================================

#[test]
fn item10_generic_undo_error_reverts_to_done_and_rebuilds_junction() {
    use greenc_platform::traits::ReparsePointApi;
    use greenc_relocate::mover::TransactionalMover;
    use std::path::Path;

    struct SabotageRestoreTmpReparse {
        inner: NativeReparsePointApi,
        source_parent: PathBuf,
        source_name: String,
    }

    impl ReparsePointApi for SabotageRestoreTmpReparse {
        fn create_junction(
            &self,
            link_path: &Path,
            target_path: &Path,
        ) -> greenc_core::error::Result<()> {
            self.inner.create_junction(link_path, target_path)
        }
        fn delete_junction(&self, link_path: &Path) -> greenc_core::error::Result<()> {
            self.inner.delete_junction(link_path)?;
            // Simulate a rename failure of `restore_tmp -> source_path` by removing the temp dir
            // right after junction deletion while leaving `source_path` vacant so junction rebuild
            // can succeed!
            if let Ok(rd) = fs::read_dir(&self.source_parent) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("restoring.")
                        || name.starts_with(&format!("{}.greenc-restoring.", self.source_name))
                    {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
            Ok(())
        }
        fn get_junction_target(&self, link_path: &Path) -> greenc_core::error::Result<PathBuf> {
            self.inner.get_junction_target(link_path)
        }
        fn is_reparse_point(&self, path: &Path) -> bool {
            self.inner.is_reparse_point(path)
        }
    }

    let tmp = TempDir::new().unwrap();
    let c_apps = tmp.path().join("C_Drive/Apps");
    let src = c_apps.join("MyGame");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/MyGame");
    fs::create_dir_all(&c_apps).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("save.dat"), b"GAME-SAVE-123").unwrap();

    let native_reparse = NativeReparsePointApi;
    native_reparse.create_junction(&src, &target).unwrap();

    let db = GreenCDb::open(tmp.path().join("item10_a.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "my_game".to_string(),
            app_name: "MyGame".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 13,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: Some("Moved ok".to_string()),
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let sabotage_reparse = SabotageRestoreTmpReparse {
        inner: NativeReparsePointApi,
        source_parent: c_apps.clone(),
        source_name: "MyGame".to_string(),
    };

    let err = TransactionalMover::undo_relocation(
        rec_id,
        &db,
        &vol,
        &sabotage_reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .unwrap_err();
    let _ = err;

    // Record MUST be reverted to Done (not stuck in Moving!) AND junction MUST be rebuilt!
    let rec = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == Some(rec_id))
        .unwrap();
    assert_eq!(
        rec.status,
        RelocationStatus::Done,
        "generic undo failure must revert record to Done after rebuilding junction, got {:?}",
        rec.status
    );
    assert!(
        rec.moving_path.is_none(),
        "moving_path must be cleared after rollback to Done"
    );
    assert!(
        native_reparse.is_reparse_point(&src),
        "junction at source_path MUST be rebuilt when generic undo rename fails"
    );
    assert_eq!(native_reparse.get_junction_target(&src).unwrap(), target);
}

#[test]
fn item10_restoring_dir_only_blocks_matching_record_and_repair_ignores_failed() {
    use greenc_relocate::recovery::RecoveryService;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item10_b.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_apps = tmp.path().join("C_Drive/Apps");
    let app_a_src = c_apps.join("AppA");
    let app_b_src = c_apps.join("AppB");
    let d_root = tmp.path().join("D_Drive/GreenC/Relocated");
    let app_a_tgt = d_root.join("AppA");
    let app_b_tgt = d_root.join("AppB");
    fs::create_dir_all(&c_apps).unwrap();
    fs::create_dir_all(&app_a_tgt).unwrap();
    fs::create_dir_all(&app_b_tgt).unwrap();
    fs::write(app_a_tgt.join("a.bin"), b"A").unwrap();
    fs::write(app_b_tgt.join("b.bin"), b"B").unwrap();

    // AppA has a claimed `restoring.*` temp dir in the SAME parent directory `C_Drive/Apps`!
    let app_a_restoring = c_apps.join("restoring.app-a-1234");
    fs::create_dir_all(&app_a_restoring).unwrap();
    fs::write(app_a_restoring.join("a.bin"), b"A").unwrap();
    let _rec_a = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "app_a".to_string(),
            app_name: "AppA".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_a_src.clone(),
            target_path: app_a_tgt.clone(),
            size_bytes: 1,
            status: RelocationStatus::NeedsReview,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: Some(app_a_restoring.clone()),
            target_user_repaired: false,
        })
        .unwrap();

    // AppB has a valid junction to `app_b_tgt` and its own `.greenc-moving.*`, interrupted in Moving.
    reparse.create_junction(&app_b_src, &app_b_tgt).unwrap();
    let app_b_moving = c_apps.join("AppB.greenc-moving.b1");
    fs::create_dir_all(&app_b_moving).unwrap();
    fs::write(app_b_moving.join("b.bin"), b"B").unwrap();

    let rec_b_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "app_b".to_string(),
            app_name: "AppB".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_b_src.clone(),
            target_path: app_b_tgt.clone(),
            size_bytes: 1,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: Some(app_b_moving),
            target_user_repaired: false,
        })
        .unwrap();

    // AppA's restoring dir MUST NOT block AppB from recovering cleanly to Done!
    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    let rec_b = recovered
        .into_iter()
        .find(|r| r.id == Some(rec_b_id))
        .expect("AppB should be recovered");
    assert_eq!(
        rec_b.status,
        RelocationStatus::Done,
        "AppA's restoring dir must NOT block AppB recovery, got {:?}",
        rec_b
    );

    // Now test `repair_junction_target` ignores a `Failed` record pointing to the same target!
    let repair_tgt = d_root.join("AppB_Repaired");
    fs::create_dir_all(&repair_tgt).unwrap();
    fs::write(repair_tgt.join("b.bin"), b"B2").unwrap();

    let _failed_prev = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "app_b_old".to_string(),
            app_name: "AppB Old Failed Attempt".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: c_apps.join("AppB_Old"),
            target_path: repair_tgt.clone(),
            size_bytes: 1,
            status: RelocationStatus::Failed,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: Some("Previous attempt failed".to_string()),
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let repaired = RecoveryService::repair_junction_target(rec_b_id, &repair_tgt, &db, &reparse)
        .expect("repair_junction_target MUST ignore Failed records during overlap check");
    assert_eq!(repaired.target_path, repair_tgt);
    assert_eq!(repaired.status, RelocationStatus::Done);
}

// =========================================================================
// Item 11:
//   - Guardian/purge_expired_originals runs cheap checks per tick and full
//     SQLite check ONLY when due
//   - Checkpoints 2-3 check processes ONLY (no recursive file lock walk)
//   - RmShutdown error waits for processes to exit before concluding
//   - Always reopen Zalo when aborted/cancelled after shutdown
//   - Crash between rename(moving->original) and DB save keeps tracking
// =========================================================================

#[test]
fn item11_guardian_tick_only_runs_cheap_checks_until_due() {
    let tmp = TempDir::new().unwrap();
    let (_disc, zalo_data, _exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item11_guardian.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_g");
    fs::create_dir_all(target.join("message")).unwrap();
    // Put a CORRUPTED SQLite file on target right now!
    // During an early (not-yet-due) tick, cheap check should NOT open SQLite so observation
    // advances cheaply; ONLY when due should full SQLite validation run and refuse purge!
    fs::write(
        target.join("message/zalo.db"),
        b"NOT-A-VALID-SQLITE-HEADER-BYTES-12345",
    )
    .unwrap();

    let retained = zalo_data
        .parent()
        .unwrap()
        .join("Zalo Data.greenc-original.g1");
    fs::rename(&zalo_data, &retained).unwrap();
    reparse.create_junction(&zalo_data, &target).unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: zalo_data.clone(),
            target_path: target.clone(),
            size_bytes: 4096,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let t0 = Utc::now();
    let rid = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: rec_id,
            source_path: zalo_data.clone(),
            target_path: target.clone(),
            retained_path: retained.clone(),
            size_bytes: 4096,
            file_count: 1,
            message_file_count: 1,
            health_verified: true,
            created_at: t0,
            purge_after: t0 + ChronoDuration::hours(24),
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(t0),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    // Tick 1 at +60s (NOT due yet): cheap check runs (junction healthy + target/message exists)
    // WITHOUT doing heavy SQLite verification, so `observed_healthy_secs` advances by 60s!
    let purged_early =
        ZaloRelocator::purge_expired_originals(&db, &reparse, t0 + ChronoDuration::seconds(60))
            .unwrap();
    assert!(purged_early.is_empty());
    let after_early = db.get_zalo_retained_original(rid).unwrap().unwrap();
    assert_eq!(
        after_early.observed_healthy_secs, 60,
        "non-due Guardian tick must only run cheap checks (not full SQLite scan) and advance observation"
    );

    // Tick 2 when DUE (24h elapsed AND observed_healthy_secs >= 24h): NOW full SQLite check
    // MUST run and REFUSE purge because target DB is corrupt!
    db.update_zalo_retained_observation(rid, 24 * 3600, t0 + ChronoDuration::hours(24))
        .unwrap();
    let purged_due =
        ZaloRelocator::purge_expired_originals(&db, &reparse, t0 + ChronoDuration::hours(25))
            .unwrap();
    assert!(
        purged_due.is_empty(),
        "due tick MUST run full SQLite validation and refuse corrupt target"
    );
    assert!(retained.exists(), "retained original must remain intact");
}

#[test]
fn item11_checkpoints_2_3_only_check_processes_and_post_shutdown_abort_reopens_zalo() {
    use greenc_platform::traits::FileLockProbe;
    use greenc_relocate::zalo_move::ZaloMoveHooks;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    // Part A: Counting FileLockProbe verifies Checkpoints 2 & 3 do NOT walk/lock-probe `moving_path`!
    struct CountingLockProbe {
        moving_probes: Arc<AtomicUsize>,
        block_all: Arc<AtomicBool>,
    }
    impl FileLockProbe for CountingLockProbe {
        fn is_file_locked(&self, path: &Path) -> bool {
            if path.to_string_lossy().contains(".greenc-moving.") {
                self.moving_probes.fetch_add(1, Ordering::SeqCst);
            }
            self.block_all.load(Ordering::SeqCst)
        }
    }

    let tmp = TempDir::new().unwrap();
    let (disc, zalo_data, exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item11_cp.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    lifecycle.add_running_exe(exe_path.clone());

    let moving_probes = Arc::new(AtomicUsize::new(0));
    let lock = CountingLockProbe {
        moving_probes: Arc::clone(&moving_probes),
        block_all: Arc::new(AtomicBool::new(false)),
    };

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_cp");
    let target_cb = target.clone();
    lifecycle.set_on_restart_hook(move || {
        let _ = fs::write(target_cb.join("message/zalo.db-wal"), b"WAL");
    });

    ZaloRelocator::move_zalo_data_with_hooks(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target),
            reopen_zalo: true,
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
        &ZaloMoveHooks::default(),
    )
    .unwrap();
    assert_eq!(
        moving_probes.load(Ordering::SeqCst),
        0,
        "Checkpoints 2 & 3 MUST only check Zalo processes, NEVER lock-probe moving_path files!"
    );

    // Part B: If post-shutdown check fails (e.g. a file is locked by another process right after
    // Zalo was shut down), GreenC MUST reopen Zalo!
    let tmp2 = TempDir::new().unwrap();
    let (disc2, _zalo_data2, exe_path2) = build_zalo_env_sqlite(tmp2.path());
    let db2 = GreenCDb::open(tmp2.path().join("item11_reopen.db")).unwrap();
    let lifecycle2 = MockAppLifecycleApi::new();
    lifecycle2.add_running_exe(exe_path2);
    let lock2 = CountingLockProbe {
        moving_probes: Arc::new(AtomicUsize::new(0)),
        block_all: Arc::new(AtomicBool::new(true)),
    };

    let err = ZaloRelocator::move_zalo_data(
        &disc2,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(tmp2.path().join("D_Drive/GreenC/Relocated/zalo_reopen")),
            reopen_zalo: true,
            enforce_platform_support: false,
            ..Default::default()
        },
        &db2,
        &vol,
        &reparse,
        &shell,
        &lock2,
        &proc,
        &lifecycle2,
        &TaskControl::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, GreenCError::FileLocked { .. }));
    assert_eq!(
        lifecycle2.restart_count(),
        1,
        "when move aborts right after shutdown (locked file), GreenC MUST reopen Zalo!"
    );
    let _ = zalo_data;
}

#[test]
fn item11_crash_between_original_rename_and_db_save_stays_tracked() {
    use greenc_relocate::recovery::RecoveryService;

    let tmp = TempDir::new().unwrap();
    let (_disc, zalo_data, _exe_path) = build_zalo_env_sqlite(tmp.path());
    let db = GreenCDb::open(tmp.path().join("item11_crash.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo_crash");
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 10);

    // Simulate crash right AFTER `fs::rename(moving_path, original_path)` and BEFORE
    // `save_zalo_retained_original_durable` / `record.status = Done`:
    // - `zalo_data` is a valid junction -> `target`
    // - `moving_path` recorded in `relocations` no longer exists (it was ALREADY renamed!)
    // - `Zalo Data.greenc-original.<uuid>` sits on disk UNTRACKED in `zalo_retained_originals`
    let old_moving = zalo_data
        .parent()
        .unwrap()
        .join("Zalo Data.greenc-moving.pre-crash");
    let renamed_original = zalo_data
        .parent()
        .unwrap()
        .join("Zalo Data.greenc-original.post-rename-crash");
    fs::rename(&zalo_data, &renamed_original).unwrap();
    reparse.create_junction(&zalo_data, &target).unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: zalo_data.clone(),
            target_path: target.clone(),
            size_bytes: 4096,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: Some("Đang đổi chỗ...".to_string()),
            moving_path: Some(old_moving),
            target_user_repaired: false,
        })
        .unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, Some(rec_id));
    assert_eq!(recovered[0].status, RelocationStatus::Done);

    // CRITICAL: `renamed_original` MUST be adopted and tracked in `zalo_retained_originals`!
    let retained_rows = db.list_zalo_retained_originals(None).unwrap();
    assert!(
        retained_rows
            .iter()
            .any(|r| r.retained_path == renamed_original && r.status == ZaloRetainedStatus::Retained && !r.health_verified),
        "crash between rename(moving->original) and DB save MUST adopt and track the .greenc-original.* dir in DB, got {:?}",
        retained_rows
    );
}

// =========================================================================
// Item 12: Known folder relocation refuses if tree contains a Zalo junction
// =========================================================================

#[test]
fn item12_known_folder_relocation_refuses_tree_containing_zalo_junction() {
    use greenc_platform::{FOLDERID_DOCUMENTS, FOLDERID_DOWNLOADS, FOLDERID_PICTURES};
    use greenc_relocate::known_folders::KnownFolderService;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item12.db")).unwrap();
    let vol = make_drives(50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();

    // Case A: Tracked Zalo junction inside Documents (`Documents/Zalo Data -> D:/.../zalo`)
    let docs_dir = tmp.path().join("C_Drive/Users/u/Documents");
    let docs_zalo_link = docs_dir.join("Zalo Data");
    let zalo_target_a = tmp.path().join("D_Drive/GreenC/Relocated/zalo_docs");
    fs::create_dir_all(&docs_dir).unwrap();
    fs::create_dir_all(zalo_target_a.join("message")).unwrap();
    create_real_sqlite_db(&zalo_target_a.join("message/zalo.db"), 10);
    reparse
        .create_junction(&docs_zalo_link, &zalo_target_a)
        .unwrap();
    fs::write(docs_dir.join("report.txt"), b"doc").unwrap();
    shell.register_folder(FOLDERID_DOCUMENTS, docs_dir.clone());

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: docs_zalo_link.clone(),
        target_path: zalo_target_a.clone(),
        size_bytes: 4096,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    })
    .unwrap();

    let docs_dest = tmp.path().join("D_Drive/Documents");
    let err_a = KnownFolderService::relocate_known_folder(
        "documents",
        'D',
        Some(docs_dest.clone()),
        &db,
        &shell,
        &vol,
        &TaskControl::new(),
        |_, _| {},
    )
    .unwrap_err();
    assert!(
        matches!(err_a, GreenCError::PreconditionBlocked { .. }),
        "KnownFolder relocation MUST refuse when tree contains a DB-tracked Zalo junction, got {err_a:?}"
    );
    assert!(!docs_dest.exists(), "must not create target when blocked");
    assert!(
        reparse.is_reparse_point(&docs_zalo_link),
        "Zalo junction inside Documents must remain untouched"
    );

    // Case B: UNTRACKED nested Zalo junction inside Downloads (`Downloads/Sub/Zalo Data`)
    let dl_dir = tmp.path().join("C_Drive/Users/u/Downloads");
    let dl_nested_zalo = dl_dir.join("Sub/Zalo Data");
    let zalo_target_b = tmp.path().join("D_Drive/ManualZaloTarget");
    fs::create_dir_all(dl_dir.join("Sub")).unwrap();
    fs::create_dir_all(zalo_target_b.join("message")).unwrap();
    create_real_sqlite_db(&zalo_target_b.join("message/zalo.db"), 10);
    reparse
        .create_junction(&dl_nested_zalo, &zalo_target_b)
        .unwrap();
    shell.register_folder(FOLDERID_DOWNLOADS, dl_dir.clone());

    let dl_dest = tmp.path().join("D_Drive/Downloads");
    let err_b = KnownFolderService::relocate_known_folder(
        "downloads",
        'D',
        Some(dl_dest.clone()),
        &db,
        &shell,
        &vol,
        &TaskControl::new(),
        |_, _| {},
    )
    .unwrap_err();
    assert!(
        matches!(err_b, GreenCError::PreconditionBlocked { .. }),
        "KnownFolder relocation MUST refuse when tree contains an untracked Zalo junction, got {err_b:?}"
    );
    assert!(!dl_dest.exists(), "must not create target when blocked");
    assert!(
        reparse.is_reparse_point(&dl_nested_zalo),
        "nested Zalo junction inside Downloads must remain untouched"
    );

    // Case C: Normal Known Folder with NO Zalo junction still succeeds!
    let pic_dir = tmp.path().join("C_Drive/Users/u/Pictures");
    fs::create_dir_all(&pic_dir).unwrap();
    fs::write(pic_dir.join("photo.jpg"), b"JPEG").unwrap();
    shell.register_folder(FOLDERID_PICTURES, pic_dir);
    let pic_dest = tmp.path().join("D_Drive/Pictures");
    let rec_c = KnownFolderService::relocate_known_folder(
        "pictures",
        'D',
        Some(pic_dest.clone()),
        &db,
        &shell,
        &vol,
        &TaskControl::new(),
        |_, _| {},
    )
    .expect("normal Known Folder without Zalo junction must still succeed");
    assert_eq!(rec_c.status, RelocationStatus::Done);
    assert!(pic_dest.join("photo.jpg").exists());
}
