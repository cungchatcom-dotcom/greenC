//! Comprehensive TDD safety tests for (Items 1-12).

use chrono::Utc;
use greenc_core::error::GreenCError;
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_relocate::zalo_move::{
    discover_zalo_safety_copies, ZaloDeleteSafetyCopyOptions, ZaloRelocator,
};
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
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
            [format!("gc-zalo5-msg-{i:06}-{}", "x".repeat(120))],
        )
        .unwrap();
    }
    tx.commit().unwrap();
}

fn set_mtime(path: &Path, t: SystemTime) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(t).unwrap();
}

// =========================================================================
// Item 1 (F2): Refuse deleting any safety copy (including kind == "target")
// when ANY file in copy/message/ is newer (mtime) or larger than live;
// require separate "bản này mới hơn" confirmation; honest verification text.
// =========================================================================

#[test]
fn item1_delete_safety_copy_refuses_newer_mtime_or_larger_size_including_target_kind() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item1_f2.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let live_src = c_dir.join("Zalo Data");
    let conflict_newer_mtime = c_dir.join("Zalo Data.greenc-conflict.newer-mtime");
    let conflict_larger_size = c_dir.join("Zalo Data.greenc-conflict.larger-size");
    let target_copy = d_dir.join("zalo");

    fs::create_dir_all(live_src.join("message")).unwrap();
    fs::create_dir_all(conflict_newer_mtime.join("message")).unwrap();
    fs::create_dir_all(conflict_larger_size.join("message")).unwrap();
    fs::create_dir_all(target_copy.join("message")).unwrap();

    let t_old = SystemTime::now() - Duration::from_secs(3600);
    let t_mid = SystemTime::now() - Duration::from_secs(600);
    let t_new = SystemTime::now() - Duration::from_secs(60);

    // Live has 50 rows at t_mid
    create_real_sqlite_db(&live_src.join("message/zalo.db"), 50);
    set_mtime(&live_src.join("message/zalo.db"), t_mid);

    // Copy A: smaller (10 rows) but NEWER mtime (t_new > t_mid)
    create_real_sqlite_db(&conflict_newer_mtime.join("message/zalo.db"), 10);
    set_mtime(&conflict_newer_mtime.join("message/zalo.db"), t_new);

    // Copy B: OLDER mtime (t_old < t_mid) but slightly LARGER (60 rows > 50 rows; within old 80% threshold!)
    create_real_sqlite_db(&conflict_larger_size.join("message/zalo.db"), 60);
    set_mtime(&conflict_larger_size.join("message/zalo.db"), t_old);

    // Copy C (kind == "target"): NEWER mtime AND larger (80 rows at t_new)
    create_real_sqlite_db(&target_copy.join("message/zalo.db"), 80);
    set_mtime(&target_copy.join("message/zalo.db"), t_new);

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "zalo".to_string(),
        app_name: "Zalo PC".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: live_src.clone(),
        target_path: target_copy.clone(),
        size_bytes: 4096,
        status: RelocationStatus::NeedsReview,
        health_status: RelocationHealthStatus::TargetOffline,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: Utc::now(),
        detail: None,
        moving_path: None,
        target_user_repaired: false,
    })
    .unwrap();

    let copies = discover_zalo_safety_copies(Some(&live_src), &db);
    let newer_info = copies
        .iter()
        .find(|c| c.path == conflict_newer_mtime)
        .expect("newer conflict copy listed");
    let larger_info = copies
        .iter()
        .find(|c| c.path == conflict_larger_size)
        .expect("larger conflict copy listed");
    let target_info = copies
        .iter()
        .find(|c| c.path == target_copy && c.kind == "target")
        .expect("kind==target copy listed");

    // 1. Standard delete MUST refuse copy with newer mtime
    let err_mtime = ZaloRelocator::delete_safety_copy_by_id(
        &newer_info.copy_id,
        Some(&live_src),
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(
            err_mtime,
            GreenCError::PreconditionBlocked { ref operation, .. } if operation == "zalo_delete_copy_newer"
        ),
        "must refuse copy with newer mtime using zalo_delete_copy_newer, got {err_mtime:?}"
    );
    assert!(conflict_newer_mtime.exists());

    // 2. Standard delete MUST refuse copy with larger size (even slightly larger!)
    let err_size = ZaloRelocator::delete_safety_copy_by_id(
        &larger_info.copy_id,
        Some(&live_src),
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(
            err_size,
            GreenCError::PreconditionBlocked { ref operation, .. } if operation == "zalo_delete_copy_newer"
        ),
        "must refuse copy with larger size using zalo_delete_copy_newer, got {err_size:?}"
    );
    assert!(conflict_larger_size.exists());

    // 3. Standard delete MUST ALSO refuse kind == "target" when newer/larger
    let err_target = ZaloRelocator::delete_safety_copy_by_id(
        &target_info.copy_id,
        Some(&live_src),
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(
            err_target,
            GreenCError::PreconditionBlocked { ref operation, .. } if operation == "zalo_delete_copy_newer"
        ),
        "must refuse kind==target copy when newer/larger, got {err_target:?}"
    );
    assert!(target_copy.exists());

    // 4. Separate explicit confirmation ("bản này mới hơn") succeeds AND returns honest message
    let msg_newer = ZaloRelocator::delete_safety_copy_by_id_with_options(
        &newer_info.copy_id,
        Some(&live_src),
        &ZaloDeleteSafetyCopyOptions {
            confirm_copy_may_be_newer: true,
            ..Default::default()
        },
        &db,
        &reparse,
    )
    .expect("separate confirmation must allow deleting newer copy when live is intact");
    assert!(!conflict_newer_mtime.exists());
    assert!(
        !msg_newer.contains("đối chiếu đầy đủ"),
        "message must not overclaim full message-level verification: {msg_newer}"
    );
    assert!(
        msg_newer.contains("bản này mới hơn") && msg_newer.contains("không so sánh từng dòng"),
        "message should honestly reflect actual checks: {msg_newer}"
    );

    // 5. Same for kind == "target" with separate confirmation
    let msg_target = ZaloRelocator::delete_safety_copy_by_id_with_options(
        &target_info.copy_id,
        Some(&live_src),
        &ZaloDeleteSafetyCopyOptions {
            confirm_copy_may_be_newer: true,
            ..Default::default()
        },
        &db,
        &reparse,
    )
    .expect("separate confirmation must allow deleting kind==target copy when live is intact");
    assert!(!target_copy.exists());
    assert!(!msg_target.contains("đối chiếu đầy đủ"));

    // 6. Even WITH confirm_copy_may_be_newer == true, corrupt live DB MUST STILL BLOCK!
    fs::write(live_src.join("message/zalo.db"), b"CORRUPTED-LIVE-DB").unwrap();
    let err_corrupt_live = ZaloRelocator::delete_safety_copy_by_id_with_options(
        &larger_info.copy_id,
        Some(&live_src),
        &ZaloDeleteSafetyCopyOptions {
            confirm_copy_may_be_newer: true,
            ..Default::default()
        },
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(err_corrupt_live, GreenCError::PreconditionBlocked { .. }),
        "must NEVER delete copy over corrupt live DB even with confirm_copy_may_be_newer, got {err_corrupt_live:?}"
    );
    assert!(conflict_larger_size.exists());
}

// =========================================================================
// Item 2 (F4): Purge gate NEVER opens live SQLite DB directly (no -wal/-shm
// created in Zalo dir); copies db+wal outside Zalo dir only when Zalo is
// closed; rejects emptied DBs (even with same file size & newer mtime);
// backs off exponentially on gate failure instead of retrying every 45s.
// =========================================================================

#[test]
fn item2_purge_gate_isolates_db_copy_rejects_emptied_db_and_backs_off() {
    use chrono::Duration as ChronoDuration;
    use greenc_core::models::{ZaloRetainedOriginal, ZaloRetainedStatus};
    use greenc_platform::traits::{MockProcessServiceProbe, ReparsePointApi};
    use greenc_relocate::zalo_move::purge_gate_backoff_state;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item2_f4.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let src = tmp.path().join("C_Drive/Zalo Data");
    let orig = tmp.path().join("C_Drive/Zalo Data.greenc-original.f4-1111");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");

    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(orig.join("message")).unwrap();
    fs::create_dir_all(target.join("message")).unwrap();

    create_real_sqlite_db(&orig.join("message/zalo.db"), 200);
    fs::copy(orig.join("message/zalo.db"), target.join("message/zalo.db")).unwrap();
    reparse.create_junction(&src, &target).unwrap();

    let t0 = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 50_000,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: t0 - ChronoDuration::hours(30),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let rid = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: orig.clone(),
            size_bytes: 50_000,
            file_count: 1,
            message_file_count: 1,
            health_verified: true,
            created_at: t0 - ChronoDuration::hours(25),
            purge_after: t0 - ChronoDuration::hours(1),
            purge_after_valid: true,
            observed_healthy_secs: 25 * 3600,
            last_observed_at: Some(t0 - ChronoDuration::minutes(1)),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    // A. When Zalo is running, purge gate MUST refuse/defer (never open live DB or copy while running)
    let proc_running = MockProcessServiceProbe::new();
    proc_running.set_process_running("Zalo.exe", true);
    let err_running =
        ZaloRelocator::purge_retained_original_with_probe(rid, &db, &reparse, &proc_running)
            .unwrap_err();
    assert!(
        matches!(err_running, GreenCError::PreconditionBlocked { .. }),
        "purge gate must refuse while Zalo is running, got {err_running:?}"
    );
    assert!(orig.exists());

    // B. Empty target DB WITHOUT VACUUM (file size stays IDENTICAL, mtime is NEWER, quick_check == ok!)
    let orig_size = fs::metadata(orig.join("message/zalo.db")).unwrap().len();
    {
        let conn = rusqlite::Connection::open(target.join("message/zalo.db")).unwrap();
        conn.execute("DELETE FROM messages;", []).unwrap();
    }
    let emptied_size = fs::metadata(target.join("message/zalo.db")).unwrap().len();
    assert_eq!(
        orig_size, emptied_size,
        "DELETE without VACUUM keeps identical file size"
    );

    let proc_closed = MockProcessServiceProbe::new();
    let err_emptied =
        ZaloRelocator::purge_retained_original_with_probe(rid, &db, &reparse, &proc_closed)
            .unwrap_err();
    assert!(
        matches!(err_emptied, GreenCError::PreconditionBlocked { .. }),
        "emptied DB (0 rows, same size) MUST be rejected by page/content gate, got {err_emptied:?}"
    );
    assert!(orig.exists());

    // C. Auto-purge gate failure records exponential backoff (does NOT retry every 45s!)
    let p1 =
        ZaloRelocator::purge_expired_originals_with_probe(&db, &reparse, &proc_closed, t0).unwrap();
    assert!(p1.is_empty());
    let (fails1, next1) =
        purge_gate_backoff_state(&db, rid).expect("backoff must be persisted after gate failure");
    assert_eq!(fails1, 1);
    assert_eq!(next1, t0 + ChronoDuration::seconds(300));

    // Even if target DB is still broken at t0 + 300s, second failure escalates backoff to 900s!
    let p2 = ZaloRelocator::purge_expired_originals_with_probe(
        &db,
        &reparse,
        &proc_closed,
        t0 + ChronoDuration::seconds(300),
    )
    .unwrap();
    assert!(p2.is_empty());
    let (fails2, next2) = purge_gate_backoff_state(&db, rid).unwrap();
    assert_eq!(fails2, 2);
    assert_eq!(next2, t0 + ChronoDuration::seconds(300 + 900));

    // Restore intact WAL-mode DB on target (with NO -shm file!)
    fs::copy(orig.join("message/zalo.db"), target.join("message/zalo.db")).unwrap();
    // Put a WAL-mode DB without -shm in target to prove SQLite never opens live dir & creates -shm
    {
        let conn = rusqlite::Connection::open(target.join("message/zalo.db")).unwrap();
        let _: String = conn
            .query_row("PRAGMA journal_mode=WAL;", [], |r| r.get(0))
            .unwrap();
        conn.execute("INSERT INTO messages (body) VALUES ('wal-row');", [])
            .unwrap();
    }
    let _ = fs::remove_file(target.join("message/zalo.db-shm"));
    assert!(
        !target.join("message/zalo.db-shm").exists(),
        "precondition: no -shm in live target before gate"
    );

    // At t0 + 300 + 45s (still inside 900s backoff): gate MUST NOT run even though target is now fixed!
    let p_during_backoff = ZaloRelocator::purge_expired_originals_with_probe(
        &db,
        &reparse,
        &proc_closed,
        t0 + ChronoDuration::seconds(345),
    )
    .unwrap();
    assert!(
        p_during_backoff.is_empty(),
        "purge gate MUST NOT re-run every 45s during backoff"
    );
    assert!(orig.exists());

    // Once backoff expires (t0 + 1200s): gate runs via isolated copy, succeeds, and creates NO -shm in live dir!
    let p_after_backoff = ZaloRelocator::purge_expired_originals_with_probe(
        &db,
        &reparse,
        &proc_closed,
        t0 + ChronoDuration::seconds(1200),
    )
    .unwrap();
    assert_eq!(p_after_backoff.len(), 1);
    assert_eq!(p_after_backoff[0].status, ZaloRetainedStatus::Purged);
    assert!(!orig.exists());
    assert!(
        !target.join("message/zalo.db-shm").exists(),
        "isolated copy check MUST NOT create -shm inside live Zalo directory"
    );
    assert!(
        purge_gate_backoff_state(&db, rid).is_none(),
        "backoff state should be cleared after successful purge"
    );
}

// =========================================================================
// Item 3 (F5): Detect SQLite strictly by 16-byte magic header (NOT extension);
// encrypted/non-SQLite DBs use size + mtime rule; typed confirmation phrase
// escape hatch prevents users from ever being permanently stuck.
// =========================================================================

#[test]
fn item3_sqlite_header_only_encrypted_db_size_mtime_and_typed_force_delete() {
    use greenc_relocate::zalo_move::ZALO_FORCE_DELETE_CONFIRM_PHRASE;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item3_f5.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let live_src = c_dir.join("Zalo Data");
    let enc_copy_ok = c_dir.join("Zalo Data.greenc-conflict.enc-ok");
    let enc_copy_newer = c_dir.join("Zalo Data.greenc-conflict.enc-newer");
    let dat_sqlite_copy = c_dir.join("Zalo Data.greenc-conflict.dat-sqlite");
    let stuck_copy = c_dir.join("Zalo Data.greenc-conflict.stuck");

    fs::create_dir_all(live_src.join("message")).unwrap();
    fs::create_dir_all(enc_copy_ok.join("message")).unwrap();
    fs::create_dir_all(enc_copy_newer.join("message")).unwrap();
    fs::create_dir_all(dat_sqlite_copy.join("message")).unwrap();
    fs::create_dir_all(stuck_copy.join("message")).unwrap();

    let t_old = SystemTime::now() - Duration::from_secs(3600);
    let t_mid = SystemTime::now() - Duration::from_secs(600);
    let t_new = SystemTime::now() - Duration::from_secs(60);

    // 1. Encrypted (SQLCipher-like) `.db` file: `.db` extension, NO "SQLite format 3\0" header!
    let enc_payload_old: Vec<u8> = (0..4096).map(|i| ((i * 37 + 13) & 0xFF) as u8).collect();
    let enc_payload_live: Vec<u8> = (0..8192).map(|i| ((i * 53 + 7) & 0xFF) as u8).collect();
    fs::write(live_src.join("message/encrypted.db"), &enc_payload_live).unwrap();
    set_mtime(&live_src.join("message/encrypted.db"), t_mid);

    fs::write(enc_copy_ok.join("message/encrypted.db"), &enc_payload_old).unwrap();
    set_mtime(&enc_copy_ok.join("message/encrypted.db"), t_old);

    fs::write(
        enc_copy_newer.join("message/encrypted.db"),
        &enc_payload_old,
    )
    .unwrap();
    set_mtime(&enc_copy_newer.join("message/encrypted.db"), t_new);

    // 2. Plaintext SQLite file with `.dat` extension (NOT `.db`) -> MUST be sniffed by header!
    create_real_sqlite_db(&dat_sqlite_copy.join("message/msgstore.dat"), 50);
    set_mtime(&dat_sqlite_copy.join("message/msgstore.dat"), t_old);
    fs::copy(
        dat_sqlite_copy.join("message/msgstore.dat"),
        live_src.join("message/msgstore.dat"),
    )
    .unwrap();
    // Corrupt live `msgstore.dat` with same size & newer mtime -> header sniff on copy must catch it!
    let dat_len = fs::metadata(dat_sqlite_copy.join("message/msgstore.dat"))
        .unwrap()
        .len() as usize;
    fs::write(live_src.join("message/msgstore.dat"), vec![0x99; dat_len]).unwrap();
    set_mtime(&live_src.join("message/msgstore.dat"), t_mid);

    // 3. Copy we will force-delete via typed phrase when live is broken/missing
    create_real_sqlite_db(&stuck_copy.join("message/zalo.db"), 20);

    let copies = discover_zalo_safety_copies(Some(&live_src), &db);
    let enc_ok_info = copies.iter().find(|c| c.path == enc_copy_ok).unwrap();
    let enc_newer_info = copies.iter().find(|c| c.path == enc_copy_newer).unwrap();
    let dat_info = copies.iter().find(|c| c.path == dat_sqlite_copy).unwrap();
    let stuck_info = copies.iter().find(|c| c.path == stuck_copy).unwrap();

    // A. Encrypted `.db` copy with smaller size + older mtime SUCCEEDS (not stuck on rusqlite!)
    let msg_enc = ZaloRelocator::delete_safety_copy_by_id(
        &enc_ok_info.copy_id,
        Some(&live_src),
        &db,
        &reparse,
    )
    .expect("encrypted .db must pass via size+mtime rule instead of failing rusqlite");
    assert!(!enc_copy_ok.exists());
    assert!(
        msg_enc.contains("mã hoá/khác theo kích thước và mtime"),
        "message should honestly mention encrypted/opaque size+mtime check: {msg_enc}"
    );

    // B. Encrypted `.db` copy with NEWER mtime is REFUSED by size+mtime rule!
    let err_enc_newer = ZaloRelocator::delete_safety_copy_by_id(
        &enc_newer_info.copy_id,
        Some(&live_src),
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(
            err_enc_newer,
            GreenCError::PreconditionBlocked { ref operation, .. } if operation == "zalo_delete_copy_newer"
        ),
        "encrypted .db with newer mtime must be refused, got {err_enc_newer:?}"
    );
    assert!(enc_copy_newer.exists());

    // C. Plaintext SQLite `.dat` file is detected by MAGIC HEADER -> corrupt live `.dat` MUST BLOCK!
    let err_dat =
        ZaloRelocator::delete_safety_copy_by_id(&dat_info.copy_id, Some(&live_src), &db, &reparse)
            .unwrap_err();
    assert!(
        matches!(err_dat, GreenCError::PreconditionBlocked { .. }),
        "SQLite file with .dat extension must be detected by header and reject corrupt live DB, got {err_dat:?}"
    );
    assert!(dat_sqlite_copy.exists());

    // D. Break live dir completely (remove live_src) -> standard delete blocks, wrong typed phrase
    //    blocks, exact typed phrase `ZALO_FORCE_DELETE_CONFIRM_PHRASE` SUCCEEDS (never stuck!)
    fs::remove_dir_all(&live_src).unwrap();

    let err_wrong_phrase = ZaloRelocator::delete_safety_copy_by_id_with_options(
        &stuck_info.copy_id,
        Some(&live_src),
        &ZaloDeleteSafetyCopyOptions {
            confirm_copy_may_be_newer: false,
            force_delete_confirmation_text: Some("xoa dai di".to_string()),
        },
        &db,
        &reparse,
    )
    .unwrap_err();
    assert!(
        matches!(
            err_wrong_phrase,
            GreenCError::PreconditionBlocked { ref operation, .. } if operation == "zalo_delete_force_confirm_mismatch"
        ),
        "wrong typed phrase must be rejected, got {err_wrong_phrase:?}"
    );
    assert!(stuck_copy.exists());

    let msg_force = ZaloRelocator::delete_safety_copy_by_id_with_options(
        &stuck_info.copy_id,
        Some(&live_src),
        &ZaloDeleteSafetyCopyOptions {
            confirm_copy_may_be_newer: false,
            force_delete_confirmation_text: Some(ZALO_FORCE_DELETE_CONFIRM_PHRASE.to_string()),
        },
        &db,
        &reparse,
    )
    .expect("exact typed confirmation phrase must let user escape-hatch delete a stuck copy");
    assert!(!stuck_copy.exists());
    assert!(msg_force.contains("xác nhận gõ chữ"));
}

// =========================================================================
// Item 4 (F6): Restoring an untracked safety copy MUST handle every matching
// active relocation row whose target_path exists on disk and is not the live
// dir: rename target_path -> .greenc-conflict.* (always listed, unblocks next
// move), or keep NeedsReview if rename fails.
// =========================================================================

#[test]
fn item4_restore_untracked_copy_renames_matching_target_to_conflict_and_unblocks_next_move() {
    use greenc_core::models::DriveInfo;
    use greenc_core::task::TaskControl;
    use greenc_platform::app_lifecycle::MockAppLifecycleApi;
    use greenc_platform::traits::{
        MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi,
    };
    use greenc_relocate::zalo_move::ZaloMoveOptions;
    use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscoveryOptions};

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item4_f6.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    let shell = MockShellFolderApi::new();

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let target = d_dir.join("zalo");
    let untracked_conflict = c_dir.join("Zalo Data.greenc-conflict.f6-restore");

    // Source is a REAL directory (not a junction, e.g. Zalo reinstalled), while a matching active
    // relocation row still points to an existing `target` on D:.
    fs::create_dir_all(src.join("message")).unwrap();
    create_real_sqlite_db(&src.join("message/zalo.db"), 5);
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 40);
    fs::create_dir_all(untracked_conflict.join("message")).unwrap();
    create_real_sqlite_db(&untracked_conflict.join("message/zalo.db"), 25);

    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 16_384,
            status: RelocationStatus::NeedsReview,
            health_status: RelocationHealthStatus::LinkBroken,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    ZaloRelocator::restore_safety_copy_by_path(
        &untracked_conflict,
        &src,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .expect("restoring untracked conflict copy should succeed");

    // 1. Target on D: MUST have been renamed to `.greenc-conflict.*` (not left blocking `target`!)
    assert!(
        !target.exists(),
        "original target_path must be vacated so it does not block the next move"
    );
    let copies_after = discover_zalo_safety_copies(Some(&src), &db);
    assert!(
        copies_after.iter().any(|c| c.kind == "conflict"
            && c.path.parent() == Some(d_dir.as_path())
            && c.exists_on_disk
            && c.restorable),
        "renamed D: target must always remain listed in discover_zalo_safety_copies: {copies_after:?}"
    );
    let reloc_after = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == Some(reloc_id))
        .unwrap();
    assert_eq!(reloc_after.status, RelocationStatus::Undone);

    // 2. Subsequent move_zalo_data to the exact same `target` path MUST NOT be blocked!
    let roaming = tmp.path().join("Roaming/ZaloData");
    fs::create_dir_all(&roaming).unwrap();
    let cfg = roaming.join("config.json");
    let db_loc = src.join("message").to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &cfg,
        format!(r#"{{"db_path": {{"location": "{db_loc}"}}}}"#),
    )
    .unwrap();
    let local = tmp.path().join("Local");
    let exe_path = local.join("Programs/Zalo/Zalo-26.11.12/Zalo.exe");
    fs::create_dir_all(exe_path.parent().unwrap()).unwrap();
    fs::write(&exe_path, b"FAKE-ZALO-EXE").unwrap();
    let disc = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg),
        local_appdata: Some(local),
        system_drive_root: Some(c_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    let vol = MockVolumeApi::new(vec![
        DriveInfo {
            root: std::path::PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System".to_string()),
        },
        DriveInfo {
            root: std::path::PathBuf::from(r"D:\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 512 * 1024 * 1024 * 1024,
            free_bytes: 100 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ]);
    let report = ZaloRelocator::move_zalo_data(
        &disc,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(target.clone()),
            reopen_zalo: false,
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
    .expect("subsequent move to same target_path must not be blocked");
    assert_eq!(report.status, "done");
}

#[cfg(unix)]
#[test]
fn item4_restore_untracked_copy_keeps_needs_review_when_target_rename_fails() {
    use greenc_platform::app_lifecycle::MockAppLifecycleApi;
    use greenc_platform::traits::{MockFileLockProbe, MockProcessServiceProbe};
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let probe_dir = tmp.path().join("ro_probe_f6");
    fs::create_dir_all(&probe_dir).unwrap();
    fs::set_permissions(&probe_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let is_non_root = fs::write(probe_dir.join("t"), b"x").is_err();
    let _ = fs::set_permissions(&probe_dir, fs::Permissions::from_mode(0o755));
    if !is_non_root {
        return;
    }

    let db = GreenCDb::open(tmp.path().join("item4_f6_ro.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let target = d_dir.join("zalo");
    let conflict = c_dir.join("Zalo Data.greenc-conflict.f6-ro");

    fs::create_dir_all(src.join("message")).unwrap();
    create_real_sqlite_db(&src.join("message/zalo.db"), 5);
    fs::create_dir_all(target.join("message")).unwrap();
    create_real_sqlite_db(&target.join("message/zalo.db"), 30);
    fs::create_dir_all(conflict.join("message")).unwrap();
    create_real_sqlite_db(&conflict.join("message/zalo.db"), 15);

    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 4096,
            status: RelocationStatus::NeedsReview,
            health_status: RelocationHealthStatus::LinkBroken,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    fs::set_permissions(&d_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let res = ZaloRelocator::restore_safety_copy_by_path(
        &conflict,
        &src,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    );
    let _ = fs::set_permissions(&d_dir, fs::Permissions::from_mode(0o755));

    assert!(res.is_err(), "target rename failure must return Err");
    let row = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == Some(reloc_id))
        .unwrap();
    assert_eq!(row.status, RelocationStatus::NeedsReview);
    let copies = discover_zalo_safety_copies(Some(&src), &db);
    assert!(
        copies
            .iter()
            .any(|c| c.path == target && c.kind == "target"),
        "unrenamed target MUST remain listed in discover_zalo_safety_copies"
    );
}

// =========================================================================
// Item 5 (F7): Resume-purge MUST re-check junction + target coverage in ALL
// paths (auto-purge, manual purge, delete_safety_copy_by_id); intact
// `.greenc-purging.*` (file count & size match) can be renamed back to
// `.greenc-original.*`; EVERY purge path refuses `retained_path == target_path`.
// =========================================================================

#[test]
fn item5_resume_purge_rechecks_junction_and_coverage_recovers_intact_purging_and_guards_target_eq()
{
    use chrono::Duration as ChronoDuration;
    use greenc_core::models::{ZaloRetainedOriginal, ZaloRetainedStatus};
    use greenc_platform::traits::ReparsePointApi;
    use greenc_relocate::candidate::CandidateScanner;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item5_f7.db")).unwrap();
    let reparse = NativeReparsePointApi;

    let c_dir = tmp.path().join("C_Drive");
    let src = c_dir.join("Zalo Data");
    let purging = c_dir.join("Zalo Data.greenc-purging.f7-intact");
    let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");

    fs::create_dir_all(purging.join("message")).unwrap();
    create_real_sqlite_db(&purging.join("message/zalo.db"), 40);
    let (intact_bytes, intact_files) = CandidateScanner::compute_dir_size(&purging);

    fs::create_dir_all(target.join("message")).unwrap();
    fs::copy(
        purging.join("message/zalo.db"),
        target.join("message/zalo.db"),
    )
    .unwrap();
    // Break junction (no junction at `src`)!
    let now = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: intact_bytes,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now - ChronoDuration::hours(30),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let rid = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: purging.clone(),
            size_bytes: intact_bytes,
            file_count: intact_files,
            message_file_count: 1,
            health_verified: true,
            created_at: now - ChronoDuration::hours(25),
            purge_after: now - ChronoDuration::hours(1),
            purge_after_valid: true,
            observed_healthy_secs: 25 * 3600,
            last_observed_at: Some(now - ChronoDuration::minutes(1)),
            status: ZaloRetainedStatus::Purging,
            resolved_at: None,
            detail: Some("Crash right after rename to .greenc-purging".to_string()),
        })
        .unwrap();

    // 1. Manual purge MUST refuse resuming `.greenc-purging.*` when junction is broken!
    let err_manual = ZaloRelocator::purge_retained_original(rid, &db, &reparse).unwrap_err();
    assert!(
        matches!(err_manual, GreenCError::PreconditionBlocked { .. }),
        "manual resume purge must refuse when junction is broken, got {err_manual:?}"
    );
    assert!(purging.join("message/zalo.db").exists());

    // 2. delete_safety_copy_by_id MUST ALSO refuse deleting `.greenc-purging.*` when live is missing!
    let copies = discover_zalo_safety_copies(Some(&src), &db);
    let p_info = copies.iter().find(|c| c.id == Some(rid)).unwrap();
    let err_del =
        ZaloRelocator::delete_safety_copy_by_id(&p_info.copy_id, Some(&src), &db, &reparse)
            .unwrap_err();
    assert!(
        matches!(err_del, GreenCError::PreconditionBlocked { .. }),
        "delete_safety_copy_by_id must refuse .greenc-purging when live source is missing, got {err_del:?}"
    );
    assert!(purging.join("message/zalo.db").exists());

    // 3. Auto-purge MUST NOT delete `.greenc-purging.*` when junction is broken; because it is
    //    still 100% intact (file count & size match), auto-purge renames it BACK to `.greenc-original.*`!
    let auto_purged = ZaloRelocator::purge_expired_originals(&db, &reparse, now).unwrap();
    assert!(auto_purged.is_empty());
    let recovered_row = db.get_zalo_retained_original(rid).unwrap().unwrap();
    assert_eq!(recovered_row.status, ZaloRetainedStatus::Retained);
    assert!(!recovered_row.health_verified);
    assert!(
        recovered_row
            .retained_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".greenc-original."),
        "intact .greenc-purging should be renamed back to .greenc-original, got {:?}",
        recovered_row.retained_path
    );
    assert!(recovered_row.retained_path.join("message/zalo.db").exists());

    // 4. Direct API `recover_intact_purging_original` works when intact and REFUSES when half-deleted!
    let purging2 = c_dir.join("Zalo Data.greenc-purging.f7-half");
    fs::create_dir_all(purging2.join("message")).unwrap();
    create_real_sqlite_db(&purging2.join("message/zalo.db"), 20);
    let (bytes2, files2) = CandidateScanner::compute_dir_size(&purging2);
    let rid2 = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: purging2.clone(),
            size_bytes: bytes2 + 512, // mismatch -> half deleted!
            file_count: files2 + 1,
            message_file_count: 1,
            health_verified: true,
            created_at: now - ChronoDuration::hours(25),
            purge_after: now - ChronoDuration::hours(1),
            purge_after_valid: true,
            observed_healthy_secs: 25 * 3600,
            last_observed_at: Some(now - ChronoDuration::minutes(1)),
            status: ZaloRetainedStatus::Purging,
            resolved_at: None,
            detail: None,
        })
        .unwrap();
    let err_half = ZaloRelocator::recover_intact_purging_original(rid2, &db).unwrap_err();
    assert!(
        matches!(err_half, GreenCError::PreconditionBlocked { .. }),
        "half-deleted .greenc-purging MUST NOT be renamed back, got {err_half:?}"
    );

    // 5. EVERY purge path MUST refuse when `retained_path == target_path`!
    reparse.create_junction(&src, &target).unwrap();
    let rid_eq = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: target.clone(), // EQUAL to target_path!
            size_bytes: intact_bytes,
            file_count: intact_files,
            message_file_count: 1,
            health_verified: true,
            created_at: now - ChronoDuration::hours(25),
            purge_after: now - ChronoDuration::hours(1),
            purge_after_valid: true,
            observed_healthy_secs: 25 * 3600,
            last_observed_at: Some(now - ChronoDuration::minutes(1)),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    let err_eq_manual = ZaloRelocator::purge_retained_original(rid_eq, &db, &reparse).unwrap_err();
    assert!(matches!(
        err_eq_manual,
        GreenCError::PreconditionBlocked { .. }
    ));
    let eq_auto = ZaloRelocator::purge_expired_originals(&db, &reparse, now).unwrap();
    assert!(eq_auto.is_empty());
    let copies_eq = discover_zalo_safety_copies(Some(&src), &db);
    let eq_info = copies_eq.iter().find(|c| c.id == Some(rid_eq)).unwrap();
    let err_eq_del =
        ZaloRelocator::delete_safety_copy_by_id(&eq_info.copy_id, Some(&src), &db, &reparse)
            .unwrap_err();
    assert!(matches!(
        err_eq_del,
        GreenCError::PreconditionBlocked { .. }
    ));
    assert!(
        target.join("message/zalo.db").exists(),
        "target_path MUST remain completely untouched when retained_path == target_path"
    );
}

// =========================================================================
// Item 6 (F8): Retained copy on D: (after undo/rollback) restores via
// cross-volume staged copy like `restore_safety_copy_internal`; "offline"
// means drive/volume absent (NOT target renamed); durable conflict path
// allows crash recovery to heal interruption after vacating live `source`.
// =========================================================================

#[test]
fn item6_post_undo_copy_on_d_restores_via_staged_copy_not_offline_and_crash_recovers_conflict() {
    use chrono::Duration as ChronoDuration;
    use greenc_core::models::{ZaloRetainedOriginal, ZaloRetainedStatus};
    use greenc_platform::app_lifecycle::MockAppLifecycleApi;
    use greenc_platform::traits::{MockFileLockProbe, MockProcessServiceProbe};
    use greenc_relocate::recovery::RecoveryService;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item6_f8.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let old_target = d_dir.join("zalo"); // Renamed after undo -> does NOT exist, while `d_dir` DOES!
    let retained_on_d = d_dir.join("zalo.greenc-original.post-undo-f8");

    fs::create_dir_all(src.join("message")).unwrap();
    create_real_sqlite_db(&src.join("message/zalo.db"), 5);
    fs::create_dir_all(retained_on_d.join("message")).unwrap();
    create_real_sqlite_db(&retained_on_d.join("message/zalo.db"), 35);

    let now = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: old_target.clone(),
            size_bytes: 16_384,
            status: RelocationStatus::Undone,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now - ChronoDuration::hours(2),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let retained_id = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: old_target.clone(),
            retained_path: retained_on_d.clone(),
            size_bytes: 16_384,
            file_count: 1,
            message_file_count: 1,
            health_verified: false,
            created_at: now - ChronoDuration::hours(1),
            purge_after: now + ChronoDuration::hours(23),
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();

    // A. Default restore (allow_offline_target = false!) MUST SUCCEED because D: is online
    //    (old_target was simply renamed to `retained_on_d`, NOT offline!), and MUST stage a
    //    verified copy into C: leaving `retained_on_d` intact on D: and preserving old C: as conflict!
    let restored = ZaloRelocator::restore_retained_original(
        retained_id,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .expect("post-undo retained copy on online D: must restore via staged copy without false offline error");
    assert_eq!(restored.status, ZaloRetainedStatus::Restored);
    assert!(src.join("message/zalo.db").exists());
    // Staged copy preserved `retained_on_d` on D: AND preserved previous `src` as `.greenc-conflict.*` on C:
    assert!(
        retained_on_d.join("message/zalo.db").exists(),
        "cross-parent staged copy must leave source copy on D: intact"
    );
    let c_conflict_exists = fs::read_dir(&c_dir).unwrap().flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .contains(".greenc-conflict.")
    });
    assert!(
        c_conflict_exists,
        "previous live dir at C: must be preserved as .greenc-conflict.*"
    );

    // B. Simulate crash mid-restore right AFTER `src` was renamed to `src_conflict` and durable
    //    `Restoring` row persisted with conflict metadata -> `RecoveryService::recover_interrupted`
    //    MUST automatically restore `src_conflict -> src` and clean up `zalo-restoring.*`!
    let src2 = c_dir.join("Zalo Data 2");
    let conflict2 = c_dir.join("Zalo Data 2.greenc-conflict.crash-f8");
    let staged2 = c_dir.join("zalo-restoring.crash-f8");
    let ret2 = d_dir.join("zalo2.greenc-original.crash-f8");
    fs::create_dir_all(conflict2.join("message")).unwrap();
    fs::write(conflict2.join("message/live.txt"), b"LIVE-BEFORE-CRASH").unwrap();
    fs::create_dir_all(staged2.join("message")).unwrap();
    fs::write(staged2.join("message/staged.txt"), b"STAGED").unwrap();
    fs::create_dir_all(ret2.join("message")).unwrap();
    create_real_sqlite_db(&ret2.join("message/zalo.db"), 10);

    let meta_json = serde_json::json!({
        "conflict": conflict2,
        "staged": staged2,
        "prev_reloc_status": "undone"
    });
    let rid2 = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src2.clone(),
            target_path: d_dir.join("zalo2"),
            retained_path: ret2.clone(),
            size_bytes: 4096,
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
            detail: Some(format!(
                "Đang khôi phục bản gốc Zalo... GREENC_RESTORE_META:{meta_json}"
            )),
        })
        .unwrap();

    let _ = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert!(
        src2.join("message/live.txt").exists(),
        "recovery MUST move durable conflict path back to source_path after crash"
    );
    assert_eq!(
        fs::read(src2.join("message/live.txt")).unwrap(),
        b"LIVE-BEFORE-CRASH"
    );
    assert!(
        !staged2.exists(),
        "abandoned zalo-restoring staging dir should be cleaned once source is restored"
    );
    let ro2 = db.get_zalo_retained_original(rid2).unwrap().unwrap();
    assert_eq!(ro2.status, ZaloRetainedStatus::Retained);
}

// =========================================================================
// Item 7 (F9): undo/restore/recovery ONLY remove a junction when its target
// == record.target_path; mark & finalize the same set of rows; refuse a
// second active row for the same source; recovery.rs:482 never removes a
// foreign reparse point; repair crash between 1069 and 1073 self-heals.
// =========================================================================

#[test]
fn item7_junction_ownership_duplicate_active_row_same_set_finalize_and_repair_crash_heal() {
    use chrono::Duration as ChronoDuration;
    use greenc_core::models::{DriveInfo, ZaloRetainedOriginal, ZaloRetainedStatus};
    use greenc_core::task::TaskControl;
    use greenc_platform::app_lifecycle::MockAppLifecycleApi;
    use greenc_platform::traits::{
        MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi,
        ReparsePointApi,
    };
    use greenc_relocate::mover::TransactionalMover;
    use greenc_relocate::recovery::RecoveryService;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item7_f9.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    let shell = MockShellFolderApi::new();
    let vol = MockVolumeApi::new(vec![
        DriveInfo {
            root: std::path::PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System".to_string()),
        },
        DriveInfo {
            root: std::path::PathBuf::from(r"D:\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 512 * 1024 * 1024 * 1024,
            free_bytes: 100 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ]);

    let c_dir = tmp.path().join("C_Drive");
    let d_dir = tmp.path().join("D_Drive/GreenC/Relocated");
    let src = c_dir.join("Zalo Data");
    let rec_target = d_dir.join("zalo");
    let foreign_target = tmp.path().join("Foreign/OtherZalo");
    let orig = c_dir.join("Zalo Data.greenc-original.f9-1");
    let conflict = c_dir.join("Zalo Data.greenc-conflict.f9-1");

    fs::create_dir_all(&c_dir).unwrap();
    fs::create_dir_all(rec_target.join("message")).unwrap();
    create_real_sqlite_db(&rec_target.join("message/zalo.db"), 10);
    fs::create_dir_all(foreign_target.join("message")).unwrap();
    create_real_sqlite_db(&foreign_target.join("message/zalo.db"), 10);
    fs::create_dir_all(orig.join("message")).unwrap();
    create_real_sqlite_db(&orig.join("message/zalo.db"), 10);
    fs::create_dir_all(conflict.join("message")).unwrap();
    create_real_sqlite_db(&conflict.join("message/zalo.db"), 10);

    // Point `src` junction at `foreign_target` (NOT `rec_target`!)
    reparse.create_junction(&src, &foreign_target).unwrap();

    let now = Utc::now();
    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: rec_target.clone(),
            size_bytes: 4096,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now,
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    // 1. Refuse a second active relocation row for the exact same `source_path`!
    let dup_err = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: d_dir.join("zalo_second"),
            size_bytes: 4096,
            status: RelocationStatus::NeedsReview,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now,
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap_err();
    assert!(
        matches!(
            dup_err,
            GreenCError::PreconditionBlocked { ref operation, .. }
                if operation == "save_relocation_duplicate_active"
        ),
        "second active relocation for same source must be refused, got {dup_err:?}"
    );

    // 2. Undo MUST refuse to remove junction pointing to `foreign_target`!
    let err_undo = TransactionalMover::undo_relocation(
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
        matches!(
            err_undo,
            GreenCError::PreconditionBlocked { ref operation, .. }
                if operation == "zalo_undo_junction_mismatch"
        ),
        "undo must refuse foreign junction, got {err_undo:?}"
    );
    assert!(reparse.is_reparse_point(&src));
    assert_eq!(reparse.get_junction_target(&src).unwrap(), foreign_target);

    // 3. restore_retained_original MUST refuse to remove foreign junction!
    let rid = db
        .save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: rec_id,
            source_path: src.clone(),
            target_path: rec_target.clone(),
            retained_path: orig.clone(),
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
    let err_rest_orig = ZaloRelocator::restore_retained_original(
        rid,
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
            err_rest_orig,
            GreenCError::PreconditionBlocked { ref operation, .. }
                if operation == "zalo_restore_junction_mismatch"
        ),
        "restore_retained_original must refuse foreign junction, got {err_rest_orig:?}"
    );
    assert_eq!(reparse.get_junction_target(&src).unwrap(), foreign_target);

    // 4. restore_safety_copy_by_path MUST refuse to remove foreign junction!
    let err_rest_copy = ZaloRelocator::restore_safety_copy_by_path(
        &conflict,
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
        matches!(
            err_rest_copy,
            GreenCError::PreconditionBlocked { ref operation, .. }
                if operation == "zalo_restore_junction_mismatch"
        ),
        "restore_safety_copy_by_path must refuse foreign junction, got {err_rest_copy:?}"
    );
    assert_eq!(reparse.get_junction_target(&src).unwrap(), foreign_target);

    // 5. RecoveryService::recover_interrupted (recovery.rs:482) MUST NOT delete foreign junction!
    db.with_conn(|c| {
        c.execute(
            "UPDATE relocations SET status = 'moving' WHERE id = ?1;",
            [rec_id],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        Ok(())
    })
    .unwrap();
    let _ = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert!(
        reparse.is_reparse_point(&src),
        "recovery MUST NOT delete a foreign reparse point pointing elsewhere"
    );
    assert_eq!(reparse.get_junction_target(&src).unwrap(), foreign_target);

    // 6. Legacy DB with 2 active rows for same source -> restore_retained_original marks AND
    //    finalizes BOTH rows (neither left stuck in Moving)!
    reparse.delete_junction(&src).unwrap();
    reparse.create_junction(&src, &rec_target).unwrap();
    db.with_conn(|c| {
        c.execute(
            "UPDATE relocations SET status = 'done' WHERE id = ?1;",
            [rec_id],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        c.execute(
            "INSERT INTO relocations (app_id, app_name, strategy, source_path, target_path, size_bytes, status, health_status, process_names, created_at, detail, moving_path, target_user_repaired)
             VALUES ('zalo', 'Zalo PC', 'junction', ?1, ?2, 4096, 'needs_review', 'healthy', 'Zalo.exe', ?3, '', '', 0);",
            rusqlite::params![
                src.to_string_lossy().to_string(),
                d_dir.join("zalo_legacy").to_string_lossy().to_string(),
                now.to_rfc3339()
            ],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        Ok(())
    })
    .unwrap();
    ZaloRelocator::restore_retained_original(
        rid,
        &db,
        &reparse,
        &lock,
        &proc,
        &lifecycle,
        Duration::from_secs(2),
    )
    .expect("restore_retained_original with matching junction must succeed");
    let rows_after = db.list_relocations(None).unwrap();
    assert!(
        rows_after
            .iter()
            .filter(|r| r.source_path == src)
            .all(|r| r.status == RelocationStatus::Undone),
        "ALL rows marked Moving by restore_retained_original MUST be finalized (none stuck in Moving): {rows_after:?}"
    );

    // 7. Repair crash between line 1069 (source -> .greenc-oldlink) and line 1073 (.greenc-repair -> source)
    //    MUST self-heal on RecoveryService::recover_interrupted!
    let rep_src = c_dir.join("RepairApp");
    let rep_old_tgt = d_dir.join("repair_old");
    let rep_new_tgt = d_dir.join("repair_new");
    fs::create_dir_all(&rep_old_tgt).unwrap();
    fs::create_dir_all(&rep_new_tgt).unwrap();
    let old_backup = c_dir.join("RepairApp.greenc-oldlink.crash1069");
    let staging_link = c_dir.join("RepairApp.greenc-repair.crash1069");
    reparse.create_junction(&old_backup, &rep_old_tgt).unwrap();
    reparse
        .create_junction(&staging_link, &rep_new_tgt)
        .unwrap();
    // Note: `rep_src` is ABSENT (simulating crash right after line 1069 rename before line 1073)!
    let repair_meta = serde_json::json!({
        "old_backup": old_backup,
        "staging_link": staging_link,
        "new_target": rep_new_tgt,
        "prev_status": "done",
        "prev_detail": null
    });
    let rep_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "discord".to_string(),
            app_name: "Discord".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: rep_src.clone(),
            target_path: rep_old_tgt.clone(),
            size_bytes: 1024,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: now,
            detail: Some(format!(
                "Đang cập nhật liên kết... GREENC_REPAIR_META:{repair_meta}"
            )),
            moving_path: Some(staging_link.clone()),
            target_user_repaired: false,
        })
        .unwrap();

    let healed = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert!(
        healed.iter().any(|r| r.id == Some(rep_id)),
        "interrupted repair must be recovered"
    );
    assert!(
        reparse.is_reparse_point(&rep_src),
        "repair crash recovery must place junction back at source_path"
    );
    assert_eq!(reparse.get_junction_target(&rep_src).unwrap(), rep_new_tgt);
    assert!(!old_backup.exists(), "old backup link should be cleaned up");
    assert!(
        !staging_link.exists(),
        "staging link should be moved into place"
    );
    let rep_row = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == Some(rep_id))
        .unwrap();
    assert_eq!(rep_row.status, RelocationStatus::Done);
    assert_eq!(rep_row.target_path, rep_new_tgt);
    assert!(rep_row.target_user_repaired);
}

// =========================================================================
// Item 9 (F11): Store build blocks AppData mutations for ALL Zalo operations
// (undo, restore original, restore safety copy, purge, delete).
// =========================================================================

#[test]
fn item9_store_blocks_appdata_for_all_zalo_operations() {
    use greenc_relocate::zalo_move::ensure_not_store_appdata_zalo_for_build;

    let appdata_zalo = Path::new(r"C:\Users\Alice\AppData\Local\ZaloPC\Zalo Data");
    let non_appdata_zalo = Path::new(r"D:\GreenC\Relocated\zalo");

    for op in [
        "zalo_undo",
        "zalo_restore_original",
        "zalo_restore_safety_copy",
        "zalo_purge_original",
        "zalo_delete_safety_copy",
    ] {
        // Store build MUST block AppData Zalo paths for EVERY operation!
        let err = ensure_not_store_appdata_zalo_for_build(true, &[appdata_zalo], op).unwrap_err();
        assert!(
            matches!(err, GreenCError::PreconditionBlocked { ref operation, .. } if operation == op),
            "Store build must block AppData for {op}, got {err:?}"
        );
        // Non-store build allows AppData paths
        ensure_not_store_appdata_zalo_for_build(false, &[appdata_zalo], op).unwrap();
        // Store build allows non-AppData paths
        ensure_not_store_appdata_zalo_for_build(true, &[non_appdata_zalo], op).unwrap();
    }

    #[cfg(feature = "store")]
    {
        use greenc_core::models::DriveInfo;
        use greenc_core::task::TaskControl;
        use greenc_platform::app_lifecycle::MockAppLifecycleApi;
        use greenc_platform::traits::{
            MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi,
            ReparsePointApi,
        };
        use greenc_relocate::mover::TransactionalMover;

        let tmp = TempDir::new().unwrap();
        let db = GreenCDb::open(tmp.path().join("item9_store.db")).unwrap();
        let reparse = NativeReparsePointApi;
        let lock = MockFileLockProbe::new();
        let proc = MockProcessServiceProbe::new();
        let lifecycle = MockAppLifecycleApi::new();
        let shell = MockShellFolderApi::new();
        let vol = MockVolumeApi::new(vec![DriveInfo {
            root: std::path::PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System".to_string()),
        }]);

        let appdata_src = tmp
            .path()
            .join("Users/Alice/AppData/Local/ZaloPC/Zalo Data");
        let target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
        let conflict = tmp
            .path()
            .join("Users/Alice/AppData/Local/ZaloPC/Zalo Data.greenc-conflict.store-f11");
        fs::create_dir_all(appdata_src.parent().unwrap()).unwrap();
        fs::create_dir_all(target.join("message")).unwrap();
        create_real_sqlite_db(&target.join("message/zalo.db"), 10);
        fs::create_dir_all(conflict.join("message")).unwrap();
        create_real_sqlite_db(&conflict.join("message/zalo.db"), 10);
        reparse.create_junction(&appdata_src, &target).unwrap();

        let rec_id = db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: "zalo".to_string(),
                app_name: "Zalo PC".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: appdata_src.clone(),
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

        // Store build MUST block Zalo undo on AppData
        let err_undo = TransactionalMover::undo_relocation(
            rec_id,
            &db,
            &vol,
            &reparse,
            &shell,
            &proc,
            &TaskControl::new(),
        )
        .unwrap_err();
        assert!(matches!(err_undo, GreenCError::PreconditionBlocked { .. }));

        // Store build MUST block restore safety copy on AppData
        let err_restore = ZaloRelocator::restore_safety_copy_by_path(
            &conflict,
            &appdata_src,
            &db,
            &reparse,
            &lock,
            &proc,
            &lifecycle,
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(matches!(
            err_restore,
            GreenCError::PreconditionBlocked { .. }
        ));

        // Store build MUST block delete safety copy on AppData
        let copies = discover_zalo_safety_copies(Some(&appdata_src), &db);
        let c_info = copies.iter().find(|c| c.path == conflict).unwrap();
        let err_del = ZaloRelocator::delete_safety_copy_by_id(
            &c_info.copy_id,
            Some(&appdata_src),
            &db,
            &reparse,
        )
        .unwrap_err();
        assert!(matches!(err_del, GreenCError::PreconditionBlocked { .. }));
        assert!(conflict.exists());
    }
}

/// Item 11 (F3 phần Zalo):
/// `mover.rs:426` dispatches Zalo undo using `holds_zalo_data(source/target)` and `advanced_zalo*`
/// (not only `app_id == "zalo"`), so target data is retained for 24h instead of deleted.
#[test]
fn item11_undo_dispatches_by_holds_zalo_data_and_advanced_zalo_never_deletes_target() {
    use greenc_core::models::DriveInfo;
    use greenc_core::task::TaskControl;
    use greenc_platform::traits::{
        MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi, ReparsePointApi,
    };
    use greenc_relocate::mover::TransactionalMover;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("f3_undo.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let shell = MockShellFolderApi::new();
    let proc = MockProcessServiceProbe::new();
    let vol = MockVolumeApi::new(vec![
        DriveInfo {
            root: std::path::PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System".to_string()),
        },
        DriveInfo {
            root: std::path::PathBuf::from(r"D:\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ]);

    // Sub-case 1: `app_id = "advanced_zalo_pc"` even without "zalo" in folder names.
    let src1 = tmp.path().join("C_Drive/CustomUser/AppDataMoved1");
    let tgt1 = tmp.path().join("D_Drive/Relocated/CustomBucket1");
    fs::create_dir_all(src1.parent().unwrap()).unwrap();
    fs::create_dir_all(tgt1.join("message")).unwrap();
    create_real_sqlite_db(&tgt1.join("message/zalo.db"), 25);
    reparse.create_junction(&src1, &tgt1).unwrap();

    let rec1_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "advanced_zalo_pc".to_string(),
            app_name: "Advanced Zalo".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src1.clone(),
            target_path: tgt1.clone(),
            size_bytes: 8192,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let undone1 = TransactionalMover::undo_relocation(
        rec1_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .expect("undo of advanced_zalo_pc must succeed via Zalo safe undo path");
    assert_eq!(undone1.status, RelocationStatus::Undone);
    assert!(
        !reparse.is_reparse_point(&src1) && src1.join("message/zalo.db").exists(),
        "source must be restored as real directory"
    );
    let retained1 = db.list_zalo_retained_originals(None).unwrap();
    let row1 = retained1
        .iter()
        .find(|r| r.relocation_id == rec1_id)
        .expect(
        "advanced_zalo_pc undo MUST retain target in zalo_retained_originals instead of deleting",
    );
    assert!(
        row1.retained_path.join("message/zalo.db").exists(),
        "retained target safety copy must remain intact on disk"
    );

    // Sub-case 2: generic `app_id = "custom_folder"` whose `source_path` holds Zalo data.
    let src2 = tmp.path().join("C_Drive/Users/Alice/ZaloPC/Zalo Data");
    let tgt2 = tmp.path().join("D_Drive/Relocated/CustomBucket2");
    fs::create_dir_all(src2.parent().unwrap()).unwrap();
    fs::create_dir_all(tgt2.join("message")).unwrap();
    create_real_sqlite_db(&tgt2.join("message/zalo.db"), 30);
    reparse.create_junction(&src2, &tgt2).unwrap();

    let rec2_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "custom_folder".to_string(),
            app_name: "Custom Folder".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src2.clone(),
            target_path: tgt2.clone(),
            size_bytes: 8192,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
            moving_path: None,
            target_user_repaired: false,
        })
        .unwrap();

    let undone2 = TransactionalMover::undo_relocation(
        rec2_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc,
        &TaskControl::new(),
    )
    .expect("undo of custom_folder holding Zalo data must route through Zalo safe undo");
    assert_eq!(undone2.status, RelocationStatus::Undone);
    assert!(!reparse.is_reparse_point(&src2) && src2.join("message/zalo.db").exists());
    let retained2 = db.list_zalo_retained_originals(None).unwrap();
    let row2 = retained2
        .iter()
        .find(|r| r.relocation_id == rec2_id)
        .expect("generic app_id holding Zalo data MUST retain target instead of deleting");
    assert!(row2.retained_path.join("message/zalo.db").exists());
}

/// Item 11 (F3 phần Zalo):
/// `recovery.rs` dispatches by `holds_zalo_data(source/target)` and `advanced_zalo*` so neither
/// Case A (valid junction -> `.greenc-moving.*`) nor Case B (broken junction -> `target`) ever
/// deletes Zalo data.
#[test]
fn item11_recovery_dispatches_by_holds_zalo_data_never_deletes_advanced_zalo_moving_or_target() {
    use greenc_platform::traits::ReparsePointApi;
    use greenc_relocate::recovery::RecoveryService;

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("f3_recovery.db")).unwrap();
    let reparse = NativeReparsePointApi;

    // Case A: valid junction + `.greenc-moving.*` for `advanced_zalo_data` -> MUST retain, NOT delete!
    let src_a = tmp.path().join("C_Drive/CaseA/AppDataDir");
    let moving_a = tmp
        .path()
        .join("C_Drive/CaseA/AppDataDir.greenc-moving.case-a");
    let tgt_a = tmp.path().join("D_Drive/CaseA/TargetDir");
    fs::create_dir_all(moving_a.join("message")).unwrap();
    create_real_sqlite_db(&moving_a.join("message/zalo.db"), 15);
    fs::create_dir_all(tgt_a.join("message")).unwrap();
    create_real_sqlite_db(&tgt_a.join("message/zalo.db"), 15);
    reparse.create_junction(&src_a, &tgt_a).unwrap();

    let rec_a_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "advanced_zalo_data".to_string(),
            app_name: "Advanced Zalo Case A".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src_a.clone(),
            target_path: tgt_a.clone(),
            size_bytes: 4096,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: Utc::now(),
            detail: None,
            moving_path: Some(moving_a.clone()),
            target_user_repaired: false,
        })
        .unwrap();

    // Case B1: broken/absent junction + `.greenc-moving.*` + `target` for `advanced_zalo` ->
    // MUST rename `target` to `.greenc-conflict.*`, NEVER delete `target`!
    let src_b1 = tmp.path().join("C_Drive/CaseB1/AppDataDir");
    let moving_b1 = tmp
        .path()
        .join("C_Drive/CaseB1/AppDataDir.greenc-moving.case-b1");
    let tgt_b1 = tmp.path().join("D_Drive/CaseB1/TargetDir");
    fs::create_dir_all(moving_b1.join("message")).unwrap();
    create_real_sqlite_db(&moving_b1.join("message/zalo.db"), 12);
    fs::create_dir_all(tgt_b1.join("message")).unwrap();
    create_real_sqlite_db(&tgt_b1.join("message/zalo.db"), 18);

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "advanced_zalo".to_string(),
        app_name: "Advanced Zalo Case B1".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_b1.clone(),
        target_path: tgt_b1.clone(),
        size_bytes: 4096,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["Zalo.exe".to_string()],
        created_at: Utc::now(),
        detail: None,
        moving_path: Some(moving_b1.clone()),
        target_user_repaired: false,
    })
    .unwrap();

    // Case B2: generic `app_id = "custom_generic"` whose `target_path` holds ZaloPC ->
    // MUST ALSO rename `target` to `.greenc-conflict.*`, NEVER delete!
    let src_b2 = tmp.path().join("C_Drive/CaseB2/DataDir");
    let moving_b2 = tmp
        .path()
        .join("C_Drive/CaseB2/DataDir.greenc-moving.case-b2");
    let tgt_b2 = tmp.path().join("D_Drive/CaseB2/ZaloPC");
    fs::create_dir_all(moving_b2.join("message")).unwrap();
    create_real_sqlite_db(&moving_b2.join("message/zalo.db"), 10);
    fs::create_dir_all(tgt_b2.join("message")).unwrap();
    create_real_sqlite_db(&tgt_b2.join("message/zalo.db"), 22);

    db.save_relocation(&RelocationRecord {
        id: None,
        app_id: "custom_generic".to_string(),
        app_name: "Custom Generic".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_b2.clone(),
        target_path: tgt_b2.clone(),
        size_bytes: 4096,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec![],
        created_at: Utc::now(),
        detail: None,
        moving_path: Some(moving_b2.clone()),
        target_user_repaired: false,
    })
    .unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse).unwrap();
    assert_eq!(recovered.len(), 3);

    // Assert Case A retained `.greenc-moving.*` as `.greenc-original.*` instead of deleting it.
    let retained = db.list_zalo_retained_originals(None).unwrap();
    let row_a = retained
        .iter()
        .find(|r| r.relocation_id == rec_a_id)
        .expect("Case A for advanced_zalo_data must retain moving_dir as .greenc-original.*");
    assert!(
        row_a.retained_path.join("message/zalo.db").exists(),
        "Case A retained original must NOT be deleted"
    );

    // Assert Case B1 restored source AND renamed target to `.greenc-conflict.*` (never deleted).
    assert!(src_b1.join("message/zalo.db").exists());
    let b1_conflicts: Vec<_> = fs::read_dir(tgt_b1.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("TargetDir.greenc-conflict.")
        })
        .collect();
    assert_eq!(
        b1_conflicts.len(),
        1,
        "Case B1 target for advanced_zalo MUST be renamed to .greenc-conflict.*, not deleted"
    );
    assert!(b1_conflicts[0].path().join("message/zalo.db").exists());

    // Assert Case B2 restored source AND renamed target to `.greenc-conflict.*` (never deleted).
    assert!(src_b2.join("message/zalo.db").exists());
    let b2_conflicts: Vec<_> = fs::read_dir(tgt_b2.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("ZaloPC.greenc-conflict.")
        })
        .collect();
    assert_eq!(
        b2_conflicts.len(),
        1,
        "Case B2 target holding ZaloPC MUST be renamed to .greenc-conflict.*, not deleted"
    );
    assert!(b2_conflicts[0].path().join("message/zalo.db").exists());
}

/// Item 12:
/// `recover_after_rename_failure`: when a junction EVER existed at `source_path` before Phase 3
/// failed (e.g., `create_junction` succeeded and then verification/write-probe failed), `target_path`
/// MUST be renamed to `.greenc-conflict.<uuid>` and NEVER deleted, preserving any data written while
/// the junction existed and keeping it listed in `discover_zalo_safety_copies`.
#[test]
fn item12_recover_after_rename_failure_renames_target_to_conflict_when_junction_existed() {
    use greenc_core::models::DriveInfo;
    use greenc_core::task::TaskControl;
    use greenc_platform::app_lifecycle::MockAppLifecycleApi;
    use greenc_platform::traits::{
        MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi,
        ReparsePointApi,
    };
    use greenc_relocate::zalo_move::ZaloMoveOptions;
    use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscoveryOptions};

    struct PostJunctionFailReparse;
    impl ReparsePointApi for PostJunctionFailReparse {
        fn create_junction(
            &self,
            link_path: &Path,
            target_path: &Path,
        ) -> greenc_core::error::Result<()> {
            NativeReparsePointApi.create_junction(link_path, target_path)?;
            // Simulate Zalo racing in while the junction existed and writing a new message file
            // through the junction into `target_path`!
            fs::write(
                link_path.join("message/written_during_junction.txt"),
                b"new-message-written-while-junction-existed",
            )
            .unwrap();
            Ok(())
        }
        fn delete_junction(&self, link_path: &Path) -> greenc_core::error::Result<()> {
            NativeReparsePointApi.delete_junction(link_path)
        }
        fn is_reparse_point(&self, path: &Path) -> bool {
            NativeReparsePointApi.is_reparse_point(path)
        }
        fn get_junction_target(
            &self,
            _link_path: &Path,
        ) -> greenc_core::error::Result<std::path::PathBuf> {
            // Trigger Phase 3 failure AFTER `create_junction` already succeeded!
            Err(GreenCError::Relocation(
                "simulated get_junction_target failure after junction existed".to_string(),
            ))
        }
    }

    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("item12_rename_recovery.db")).unwrap();
    let c_dir = tmp.path().join("C_Drive");
    let local = c_dir.join("Users/Alice/AppData/Local");
    let roaming = c_dir.join("Users/Alice/AppData/Roaming/ZaloData");
    let src = local.join("ZaloPC/Zalo Data");
    let d_target = tmp.path().join("D_Drive/GreenC/Relocated/zalo");
    fs::create_dir_all(src.join("message")).unwrap();
    fs::create_dir_all(&roaming).unwrap();
    create_real_sqlite_db(&src.join("message/zalo.db"), 20);
    fs::create_dir_all(d_target.parent().unwrap()).unwrap();

    let cfg = roaming.join("config.json");
    let db_loc = src.join("message").to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &cfg,
        format!(r#"{{"db_path": {{"location": "{db_loc}"}}}}"#),
    )
    .unwrap();
    let exe_path = local.join("Programs/Zalo/Zalo-26.11.12/Zalo.exe");
    fs::create_dir_all(exe_path.parent().unwrap()).unwrap();
    fs::write(&exe_path, b"FAKE-ZALO-EXE").unwrap();

    let discovery = discover_zalo_installation(&ZaloDiscoveryOptions {
        config_file: Some(cfg),
        local_appdata: Some(local),
        system_drive_root: Some(c_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(discovery.data_dir.as_deref(), Some(src.as_path()));

    let vol = MockVolumeApi::new(vec![
        DriveInfo {
            root: std::path::PathBuf::from(r"C:\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System".to_string()),
        },
        DriveInfo {
            root: std::path::PathBuf::from(r"D:\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 256 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ]);
    let shell = MockShellFolderApi::new();
    let lock = MockFileLockProbe::new();
    let proc = MockProcessServiceProbe::new();
    let lifecycle = MockAppLifecycleApi::new();
    let reparse = PostJunctionFailReparse;

    let res = ZaloRelocator::move_zalo_data(
        &discovery,
        &ZaloMoveOptions {
            target_drive: 'D',
            custom_target_path: Some(d_target.clone()),
            reopen_zalo: false,
            enforce_platform_support: false,
            skip_write_probe: false,
            post_reopen_watch_duration: Duration::from_millis(10),
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
    );
    assert!(
        res.is_err(),
        "move must fail after simulated post-junction error"
    );

    // 1. Original source must be restored as a real directory (not a junction).
    assert!(!NativeReparsePointApi.is_reparse_point(&src));
    assert!(src.join("message/zalo.db").exists());

    // 2. `target_path` must NOT have been deleted — it must have been renamed to `.greenc-conflict.*`
    //    preserving the file written while the junction existed!
    assert!(
        !d_target.exists(),
        "original target_path should be vacated to conflict sibling so future moves are unblocked"
    );
    let target_conflicts: Vec<_> = fs::read_dir(d_target.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("zalo.greenc-conflict.")
        })
        .collect();
    assert_eq!(
        target_conflicts.len(),
        1,
        "target_path MUST be renamed to .greenc-conflict.* when a junction existed, NEVER deleted"
    );
    let conflict_dir = target_conflicts[0].path();
    assert_eq!(
        fs::read(conflict_dir.join("message/written_during_junction.txt")).unwrap(),
        b"new-message-written-while-junction-existed",
        "data written through the junction before rollback MUST survive in .greenc-conflict.*"
    );

    // 3. The preserved `.greenc-conflict.*` copy MUST be listed by `discover_zalo_safety_copies`.
    let copies = discover_zalo_safety_copies(Some(&src), &db);
    assert!(
        copies.iter().any(|c| c.path == conflict_dir),
        "preserved post-junction conflict copy must appear in discover_zalo_safety_copies: {copies:?}"
    );
}
