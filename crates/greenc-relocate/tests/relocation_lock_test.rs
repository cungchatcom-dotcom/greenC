//! Only one move, undo, link repair or crash recovery may run at a time on one GreenC database,
//! across the app and the command line: recovering a move that is still running would undo it.

use greenc_core::error::GreenCError;
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    MockProcessServiceProbe, MockReparsePointApi, MockShellFolderApi, MockVolumeApi,
    ReparsePointApi,
};
use greenc_platform::RelocationLock;
use greenc_relocate::candidate::RelocationCandidate;
use greenc_relocate::known_folders::KnownFolderService;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::recovery::RecoveryService;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn record(root: &Path, status: RelocationStatus) -> RelocationRecord {
    RelocationRecord {
        id: None,
        app_id: "slack".to_string(),
        app_name: "Slack".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: root.join("C/Slack"),
        target_path: root.join("D/Slack"),
        size_bytes: 1,
        status,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec![],
        created_at: chrono::Utc::now(),
        detail: None,
    }
}

fn is_lock_error<T: std::fmt::Debug>(res: Result<T, GreenCError>) -> bool {
    matches!(
        res,
        Err(GreenCError::PreconditionBlocked { ref operation, .. }) if operation == "relocation_lock"
    )
}

fn setup() -> (TempDir, GreenCDb) {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();
    fs::create_dir_all(tmp.path().join("C")).unwrap();
    fs::create_dir_all(tmp.path().join("D/Slack")).unwrap();
    fs::write(tmp.path().join("D/Slack/a.db"), b"a").unwrap();
    (tmp, db)
}

#[test]
fn lock_is_exclusive_per_database_and_released_on_drop() {
    let (tmp, db) = setup();
    let held = RelocationLock::acquire_for_db(db.path()).unwrap();
    assert!(RelocationLock::try_acquire_for_db(db.path())
        .unwrap()
        .is_none());
    let other = tmp.path().join("other.db");
    assert!(RelocationLock::try_acquire_for_db(&other)
        .unwrap()
        .is_some());
    drop(held);
    assert!(RelocationLock::try_acquire_for_db(db.path())
        .unwrap()
        .is_some());
}

#[test]
fn recovery_does_not_touch_a_move_that_is_still_running() {
    let (tmp, db) = setup();
    let moving = tmp.path().join("C/Slack.greenc-moving.abc");
    fs::create_dir_all(&moving).unwrap();
    fs::write(moving.join("a.db"), b"a").unwrap();
    db.save_relocation(&record(tmp.path(), RelocationStatus::Moving))
        .unwrap();
    let held = RelocationLock::acquire_for_db(db.path()).unwrap();
    let reparse = MockReparsePointApi::new();

    assert!(RecoveryService::recover_interrupted(&db, &reparse)
        .unwrap()
        .is_empty());
    assert!(moving.exists());

    drop(held);
    assert_eq!(
        RecoveryService::recover_interrupted(&db, &reparse)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn move_undo_repair_and_known_folder_are_refused_while_another_one_runs() {
    let (tmp, db) = setup();
    let reparse = MockReparsePointApi::new();
    let (source, target) = (tmp.path().join("C/Slack"), tmp.path().join("D/Slack"));
    let id = db
        .save_relocation(&record(tmp.path(), RelocationStatus::Done))
        .unwrap();
    reparse.create_junction(&source, &target).unwrap();
    let fresh = tmp.path().join("C/Discord");
    fs::create_dir_all(&fresh).unwrap();
    let drive = |letter: char, is_system: bool| greenc_core::models::DriveInfo {
        root: format!("{letter}:\\").into(),
        drive_letter: letter,
        label: String::new(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 << 30,
        free_bytes: 200 << 30,
        is_system,
        is_eligible: !is_system,
        ineligible_reason: None,
    };
    let volumes = MockVolumeApi::new(vec![drive('C', true), drive('D', false)]);
    let shell = MockShellFolderApi::new();
    shell.register_folder(greenc_platform::FOLDERID_MUSIC, fresh.clone());
    let probe = MockProcessServiceProbe::new();
    let ctrl = TaskControl::new();
    let _held = RelocationLock::acquire_for_db(db.path()).unwrap();

    let candidate = RelocationCandidate {
        id: "discord".to_string(),
        name: "Discord".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: fresh.clone(),
        default_target_path: tmp.path().join("D/Discord"),
        size_bytes: 1,
        file_count: 1,
        is_locked: false,
        locked_processes: vec![],
        how_description: String::new(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    };
    let options = MoveOptions {
        target_drive: 'D',
        custom_target_path: None,
    };
    assert!(is_lock_error(TransactionalMover::relocate_junction(
        &candidate,
        options,
        &db,
        &volumes,
        &reparse,
        &probe,
        &shell,
        &ctrl,
        |_, _| {},
    )));
    assert!(is_lock_error(TransactionalMover::undo_relocation(
        id, &db, &volumes, &reparse, &shell, &probe, &ctrl,
    )));
    assert!(is_lock_error(RecoveryService::repair_junction_target(
        id, &target, &db, &reparse,
    )));
    assert!(is_lock_error(KnownFolderService::relocate_known_folder(
        "music",
        'D',
        Some(tmp.path().join("D/Music")),
        &db,
        &shell,
        &volumes,
        &ctrl,
        |_, _| {},
    )));
    assert!(fresh.exists());
    assert_eq!(reparse.get_junction_target(&source).unwrap(), target);
}
