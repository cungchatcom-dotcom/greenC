//! Integration and unit tests for NTFS Junction relocation, 4-phase transaction,
//! atomic rollback on failure, crash recovery in both directions, and 1-click Undo (Tasks T039, T046).

use greenc_core::error::GreenCError;
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    MockProcessServiceProbe, MockReparsePointApi, MockShellFolderApi, MockVolumeApi,
    ReparsePointApi,
};
use greenc_relocate::candidate::RelocationCandidate;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::recovery::RecoveryService;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn setup_test_env() -> (
    TempDir,
    GreenCDb,
    MockVolumeApi,
    MockReparsePointApi,
    MockProcessServiceProbe,
    MockShellFolderApi,
) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test_greenc.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let drives = vec![
        greenc_core::models::DriveInfo {
            root: PathBuf::from("C:\\"),
            drive_letter: 'C',
            label: "System".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 200 * 1024 * 1024 * 1024,
            free_bytes: 50 * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: None,
        },
        greenc_core::models::DriveInfo {
            root: PathBuf::from("D:\\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 500 * 1024 * 1024 * 1024,
            free_bytes: 200 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ];

    let volume_api = MockVolumeApi::new(drives);
    let reparse_api = MockReparsePointApi::new();
    let proc_probe = MockProcessServiceProbe::new();
    let shell_api = MockShellFolderApi::new();

    (tmp, db, volume_api, reparse_api, proc_probe, shell_api)
}

#[test]
fn test_transactional_move_happy_path() {
    let (tmp, db, vol_api, reparse_api, proc_probe, shell_api) = setup_test_env();

    // Create source directory with real test files
    let src_dir = tmp.path().join("Telegram Desktop");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("data.bin"),
        b"Telegram binary chat media payload",
    )
    .unwrap();
    fs::write(src_dir.join("config.json"), b"{\"user\":\"test\"}").unwrap();

    let target_dir = tmp.path().join("D_Drive_Target").join("Telegram");

    let candidate = RelocationCandidate {
        id: "telegram".to_string(),
        name: "Telegram Desktop".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        default_target_path: target_dir.clone(),
        size_bytes: 1024,
        file_count: 2,
        is_locked: false,
        locked_processes: vec!["telegram.exe".to_string()],
        how_description: "NTFS Junction".to_string(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let ctrl = TaskControl::new();
    let options = MoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target_dir.clone()),
    };

    let record = TransactionalMover::relocate_junction(
        &candidate,
        options,
        &db,
        &vol_api,
        &reparse_api,
        &proc_probe,
        &shell_api,
        &ctrl,
        |_, _| {},
    )
    .expect("Relocation must succeed");

    assert_eq!(record.status, RelocationStatus::Done);
    assert_eq!(record.health_status, RelocationHealthStatus::Healthy);

    // Verify files exist in target
    assert!(target_dir.join("data.bin").exists());
    assert!(target_dir.join("config.json").exists());

    // Verify junction is registered
    assert!(reparse_api.is_reparse_point(&src_dir));
    assert_eq!(
        reparse_api.get_junction_target(&src_dir).unwrap(),
        target_dir
    );

    // Verify SQLite record updated
    let list = db.list_relocations(Some("done")).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].app_id, "telegram");
}

#[test]
fn test_move_blocked_when_process_running() {
    let (tmp, db, vol_api, reparse_api, proc_probe, shell_api) = setup_test_env();

    let src_dir = tmp.path().join("Telegram Desktop");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("data.bin"), b"test").unwrap();

    let target_dir = tmp.path().join("D_Drive_Target").join("Telegram");

    // Simulate active process running
    proc_probe.set_process_running("telegram.exe", true);

    let candidate = RelocationCandidate {
        id: "telegram".to_string(),
        name: "Telegram Desktop".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        default_target_path: target_dir.clone(),
        size_bytes: 1024,
        file_count: 1,
        is_locked: true,
        locked_processes: vec!["telegram.exe".to_string()],
        how_description: "NTFS Junction".to_string(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let ctrl = TaskControl::new();
    let options = MoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target_dir.clone()),
    };

    let res = TransactionalMover::relocate_junction(
        &candidate,
        options,
        &db,
        &vol_api,
        &reparse_api,
        &proc_probe,
        &shell_api,
        &ctrl,
        |_, _| {},
    );

    assert!(res.is_err());
    match res.err().unwrap() {
        GreenCError::FileLocked { reason: msg, .. } => {
            assert!(msg.contains("telegram.exe"));
        }
        other => panic!("Expected FileLocked, got {:?}", other),
    }

    // Source must remain completely untouched
    assert!(src_dir.join("data.bin").exists());
    assert!(!target_dir.exists());
}

#[test]
fn test_rollback_on_junction_failure() {
    let (tmp, db, vol_api, reparse_api, proc_probe, shell_api) = setup_test_env();

    let src_dir = tmp.path().join("Discord App");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("cache.dat"), b"Discord cache").unwrap();

    let target_dir = tmp.path().join("D_Drive_Target").join("Discord");

    // Force junction creation to fail (e.g. access denied / antivirus)
    reparse_api.set_fail_create(true);

    let candidate = RelocationCandidate {
        id: "discord".to_string(),
        name: "Discord".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        default_target_path: target_dir.clone(),
        size_bytes: 1024,
        file_count: 1,
        is_locked: false,
        locked_processes: vec!["discord.exe".to_string()],
        how_description: "NTFS Junction".to_string(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let ctrl = TaskControl::new();
    let options = MoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target_dir.clone()),
    };

    let res = TransactionalMover::relocate_junction(
        &candidate,
        options,
        &db,
        &vol_api,
        &reparse_api,
        &proc_probe,
        &shell_api,
        &ctrl,
        |_, _| {},
    );

    assert!(res.is_err());

    // Source must be completely restored!
    assert!(src_dir.exists());
    assert!(src_dir.join("cache.dat").exists());

    // Partial target must be deleted!
    assert!(!target_dir.exists());

    // SQLite record marked Failed
    let list = db.list_relocations(Some("failed")).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].app_id, "discord");
}

#[test]
fn test_crash_recovery_direction_a_valid_junction() {
    let (tmp, db, _vol_api, reparse_api, _proc_probe, _shell_api) = setup_test_env();

    let src_dir = tmp.path().join("Slack");
    let target_dir = tmp.path().join("D_Drive").join("Slack");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("slack.dat"), b"Slack files").unwrap();

    // Create valid junction pointing to target
    reparse_api.create_junction(&src_dir, &target_dir).unwrap();

    // Create leftover .greenc-moving folder from crash
    let moving_dir = tmp.path().join("Slack.greenc-moving.abc-123");
    fs::create_dir_all(&moving_dir).unwrap();
    fs::write(moving_dir.join("slack_old.dat"), b"old").unwrap();

    // Insert database record stuck in "moving"
    let record = RelocationRecord {
        id: None,
        app_id: "slack".to_string(),
        app_name: "Slack".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        target_path: target_dir.clone(),
        size_bytes: 1024,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["slack.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
    };
    db.save_relocation(&record).unwrap();

    // Run crash recovery on startup
    let recovered = RecoveryService::recover_interrupted(&db, &reparse_api).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, RelocationStatus::Done);

    // Leftover .greenc-moving folder must be purged
    assert!(!moving_dir.exists());

    // Target files remain valid
    assert!(target_dir.join("slack.dat").exists());
}

#[test]
fn test_crash_recovery_direction_b_missing_junction_rollback() {
    let (tmp, db, _vol_api, reparse_api, _proc_probe, _shell_api) = setup_test_env();

    let src_dir = tmp.path().join("CapCut");
    let target_dir = tmp.path().join("D_Drive").join("CapCut");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("draft.bin"), b"partial draft").unwrap();

    // Crash occurred before junction was created: source was renamed to moving
    let moving_dir = tmp.path().join("CapCut.greenc-moving.xyz-789");
    fs::create_dir_all(&moving_dir).unwrap();
    fs::write(moving_dir.join("original_draft.bin"), b"original draft").unwrap();

    // Explicitly create src_dir as an empty directory to simulate this crash state
    fs::create_dir_all(&src_dir).unwrap();
    assert!(src_dir.exists(), "src_dir must exist prior to recovery");
    assert!(
        fs::read_dir(&src_dir).unwrap().next().is_none(),
        "src_dir must be initially empty before recovery"
    );

    // Database record stuck in "moving"
    let record = RelocationRecord {
        id: None,
        app_id: "capcut".to_string(),
        app_name: "CapCut".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        target_path: target_dir.clone(),
        size_bytes: 2048,
        status: RelocationStatus::Moving,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["capcut.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: None,
    };
    db.save_relocation(&record).unwrap();

    let recovered = RecoveryService::recover_interrupted(&db, &reparse_api).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, RelocationStatus::Failed);

    // Source must be restored from moving folder: empty directory shell purged,
    // original payload restored
    assert!(src_dir.exists(), "src_dir must exist after recovery");
    assert!(
        src_dir.join("original_draft.bin").exists(),
        "original_draft.bin must be restored to src_dir"
    );
    assert_eq!(
        fs::read(src_dir.join("original_draft.bin")).unwrap(),
        b"original draft",
        "Restored payload must match original content"
    );

    // Dangling moving folder must be removed (renamed back to src_dir)
    assert!(
        !moving_dir.exists(),
        "Leftover moving_dir must be removed during rollback"
    );

    // Partial target must be removed
    assert!(
        !target_dir.exists(),
        "Partial target directory must be purged during rollback"
    );
}

#[test]
fn test_1_click_undo_restores_data_completely() {
    let (tmp, db, vol_api, reparse_api, proc_probe, shell_api) = setup_test_env();

    let src_dir = tmp.path().join("JetBrains");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("settings.xml"), b"<idea-config/>").unwrap();

    let target_dir = tmp.path().join("D_Drive").join("JetBrains");

    let candidate = RelocationCandidate {
        id: "jetbrains".to_string(),
        name: "JetBrains".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        default_target_path: target_dir.clone(),
        size_bytes: 512,
        file_count: 1,
        is_locked: false,
        locked_processes: vec!["idea64.exe".to_string()],
        how_description: "Junction".to_string(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let ctrl = TaskControl::new();
    let options = MoveOptions {
        target_drive: 'D',
        custom_target_path: Some(target_dir.clone()),
    };

    let done_record = TransactionalMover::relocate_junction(
        &candidate,
        options,
        &db,
        &vol_api,
        &reparse_api,
        &proc_probe,
        &shell_api,
        &ctrl,
        |_, _| {},
    )
    .unwrap();

    let record_id = done_record.id.unwrap();

    // Now execute 1-Click Undo
    let undone_record = TransactionalMover::undo_relocation(
        record_id,
        &db,
        &vol_api,
        &reparse_api,
        &shell_api,
        &proc_probe,
        &ctrl,
    )
    .expect("Undo must succeed");

    assert_eq!(undone_record.status, RelocationStatus::Undone);

    // Verify files restored to src_dir
    assert!(src_dir.exists());
    assert!(src_dir.join("settings.xml").exists());

    // Junction removed
    assert!(!reparse_api.is_reparse_point(&src_dir));

    // Target cleaned
    assert!(!target_dir.exists());
}

#[test]
fn test_live_junction_health_detection() {
    let (tmp, _db, _vol_api, reparse_api, _proc_probe, _shell_api) = setup_test_env();

    let src_healthy = tmp.path().join("App_Healthy");
    let tgt_healthy = tmp.path().join("Tgt_Healthy");
    fs::create_dir_all(&tgt_healthy).unwrap();
    reparse_api
        .create_junction(&src_healthy, &tgt_healthy)
        .unwrap();

    let src_broken = tmp.path().join("App_Broken");
    let tgt_broken = tmp.path().join("Tgt_Broken");
    fs::create_dir_all(&tgt_broken).unwrap();
    // Do not create junction for src_broken

    let src_offline = tmp.path().join("App_Offline");
    let tgt_offline = tmp.path().join("Non_Existent_Drive").join("Target");
    reparse_api
        .create_junction(&src_offline, &tgt_offline)
        .unwrap();

    let records = vec![
        RelocationRecord {
            id: Some(1),
            app_id: "app1".to_string(),
            app_name: "App 1".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src_healthy,
            target_path: tgt_healthy,
            size_bytes: 100,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: chrono::Utc::now(),
            detail: None,
        },
        RelocationRecord {
            id: Some(2),
            app_id: "app2".to_string(),
            app_name: "App 2".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src_broken,
            target_path: tgt_broken,
            size_bytes: 100,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: chrono::Utc::now(),
            detail: None,
        },
        RelocationRecord {
            id: Some(3),
            app_id: "app3".to_string(),
            app_name: "App 3".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src_offline,
            target_path: tgt_offline,
            size_bytes: 100,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: chrono::Utc::now(),
            detail: None,
        },
    ];

    let health_statuses = RecoveryService::check_junction_health(&records, &reparse_api);
    assert_eq!(health_statuses[0], RelocationHealthStatus::Healthy);
    assert_eq!(health_statuses[1], RelocationHealthStatus::LinkBroken);
    assert_eq!(health_statuses[2], RelocationHealthStatus::TargetOffline);
}

#[test]
fn test_undo_relocation_fails_early_when_insufficient_space() {
    let (tmp, db, _, reparse_api, proc_probe, shell_api) = setup_test_env();

    // Destination volume C: has only 100 MiB free
    let low_free_bytes: u64 = 100 * 1024 * 1024; // 100 MiB
    let custom_drives = vec![
        greenc_core::models::DriveInfo {
            root: PathBuf::from("C:\\"),
            drive_letter: 'C',
            label: "System".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 200 * 1024 * 1024 * 1024,
            free_bytes: low_free_bytes,
            is_system: true,
            is_eligible: false,
            ineligible_reason: None,
        },
        greenc_core::models::DriveInfo {
            root: PathBuf::from("D:\\"),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 500 * 1024 * 1024 * 1024,
            free_bytes: 200 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ];
    let vol_api = MockVolumeApi::new(custom_drives);

    // Relocated target files exist on secondary disk D:
    let src_dir = PathBuf::from("C:\\TestApp\\Source");
    let target_dir = tmp.path().join("D_Drive_Target").join("TestApp");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("large_payload.dat"), vec![0u8; 1024]).unwrap();

    // Reparse point registered
    reparse_api.create_junction(&src_dir, &target_dir).unwrap();

    // Data size = 50 MiB; Required space = 50 MiB + 512 MiB reserve = 562 MiB
    let data_size_bytes: u64 = 50 * 1024 * 1024; // 50 MiB
    let record = RelocationRecord {
        id: None,
        app_id: "testapp".to_string(),
        app_name: "Test App".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: src_dir.clone(),
        target_path: target_dir.clone(),
        size_bytes: data_size_bytes,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec![],
        created_at: chrono::Utc::now(),
        detail: None,
    };
    let record_id = db.save_relocation(&record).unwrap();

    let ctrl = TaskControl::new();

    // Execute 1-Click Undo with volume_api passed
    let res = TransactionalMover::undo_relocation(
        record_id,
        &db,
        &vol_api,
        &reparse_api,
        &shell_api,
        &proc_probe,
        &ctrl,
    );

    // Must fail early with InsufficientVaultSpace before copying data
    assert!(
        res.is_err(),
        "undo_relocation must fail when free space is insufficient"
    );
    match res.err().unwrap() {
        GreenCError::InsufficientVaultSpace(msg) => {
            assert!(msg.contains(&low_free_bytes.to_string()));
            let expected_required = data_size_bytes.saturating_add(512 * 1024 * 1024);
            assert!(msg.contains(&expected_required.to_string()));
        }
        other => panic!("Expected InsufficientVaultSpace, got {:?}", other),
    }

    // Target files must remain completely untouched on secondary disk
    assert!(
        target_dir.join("large_payload.dat").exists(),
        "Target files must be preserved when undo aborts on space check"
    );

    // Junction must remain intact
    assert!(
        reparse_api.is_reparse_point(&src_dir),
        "Reparse point must remain intact when undo aborts"
    );

    // Database record status must remain "done"
    let current_rec = db
        .list_relocations(None)
        .unwrap()
        .into_iter()
        .find(|r| r.id == Some(record_id))
        .unwrap();
    assert_eq!(current_rec.status, RelocationStatus::Done);
}

/// `relocate_junction` must only ever junction candidates whose strategy IS a junction and that
/// passed qualification. Known Folders (shell API), tool configs and guided moves (WSL/Docker)
/// must never be copied-and-junctioned; ineligible candidates must never move.
#[test]
fn test_relocate_junction_refuses_known_folders_guided_and_ineligible_candidates() {
    let (tmp, db, vol_api, reparse_api, proc_probe, shell_api) = setup_test_env();

    let cases = [
        (
            RelocationStrategyKind::KnownFolder,
            true,
            "known_folder_desktop",
        ),
        (RelocationStrategyKind::Guided, true, "wsl2"),
        (RelocationStrategyKind::Junction, false, "telegram"),
    ];

    for (strategy, eligible, id) in cases {
        let src_dir = tmp.path().join(format!("src_{id}"));
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("keep.bin"), b"user data").unwrap();
        let target_dir = tmp.path().join("D_Drive_Target").join(id);

        let candidate = RelocationCandidate {
            id: id.to_string(),
            name: id.to_string(),
            strategy,
            source_path: src_dir.clone(),
            default_target_path: target_dir.clone(),
            size_bytes: 9,
            file_count: 1,
            is_locked: false,
            locked_processes: vec![],
            how_description: String::new(),
            is_advanced: false,
            is_eligible: eligible,
            ineligible_reason: (!eligible).then(|| "Thư mục rỗng".to_string()),
        };

        let err = TransactionalMover::relocate_junction(
            &candidate,
            MoveOptions {
                target_drive: 'D',
                custom_target_path: Some(target_dir.clone()),
            },
            &db,
            &vol_api,
            &reparse_api,
            &proc_probe,
            &shell_api,
            &TaskControl::new(),
            |_, _| {},
        )
        .expect_err(&format!("{id} must be refused"));

        assert!(
            matches!(
                err,
                greenc_core::error::GreenCError::PreconditionBlocked { .. }
            ),
            "{id}: expected PreconditionBlocked, got {err:?}"
        );
        assert!(src_dir.join("keep.bin").exists(), "{id}: source untouched");
        assert!(!reparse_api.is_reparse_point(&src_dir), "{id}: no junction");
        assert!(!target_dir.exists(), "{id}: nothing copied");
    }
    assert!(db.list_relocations(None).unwrap().is_empty());
}

/// Tool caches/homes (npm, pip, cargo, ...) have no config-rewrite implementation, so they are
/// relocated with the same reversible junction transaction as app data.
#[test]
fn test_preflight_allows_junction_and_tool_config_strategies() {
    for strategy in [
        RelocationStrategyKind::Junction,
        RelocationStrategyKind::ToolConfig,
    ] {
        let candidate = RelocationCandidate {
            id: "x".to_string(),
            name: "x".to_string(),
            strategy,
            source_path: PathBuf::from("src"),
            default_target_path: PathBuf::from("dst"),
            size_bytes: 1,
            file_count: 1,
            is_locked: false,
            locked_processes: vec![],
            how_description: String::new(),
            is_advanced: false,
            is_eligible: true,
            ineligible_reason: None,
        };
        TransactionalMover::preflight(&candidate).expect("must be allowed");
    }
}
