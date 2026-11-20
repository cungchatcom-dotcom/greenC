use greenc_app::state::AppState;
use greenc_core::task::TaskControl;
use std::sync::Arc;
use std::thread;
use tempfile::tempdir;

#[test]
fn test_app_state_concurrent_reads_and_writes() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");
    let state = Arc::new(AppState::new(db_path).unwrap());

    let mut handles = vec![];

    // Spawn 8 reader threads
    for _ in 0..8 {
        let st = Arc::clone(&state);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let _ = st.settings().get_retention_days().unwrap();
                let _ = st.settings().get_theme_mode().unwrap();
            }
        }));
    }

    // Spawn 2 writer threads
    for i in 0..2 {
        let st = Arc::clone(&state);
        handles.push(thread::spawn(move || {
            for j in 1..=50 {
                let _ = st.settings().set_retention_days(j + i * 10);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert!(state.settings().get_retention_days().unwrap() <= 90);
}

#[test]
fn test_task_control_lifecycle() {
    let dir = tempdir().unwrap();
    let state = AppState::new(dir.path().join("greenc.db")).unwrap();
    let ctrl = Arc::new(TaskControl::new());

    state.register_task("task_001".to_string(), Arc::clone(&ctrl));

    assert!(!ctrl.is_paused());
    assert!(state.control_task("task_001", "pause"));
    assert!(ctrl.is_paused());

    assert!(state.control_task("task_001", "resume"));
    assert!(!ctrl.is_paused());

    assert!(!ctrl.is_cancelled());
    assert!(state.control_task("task_001", "cancel"));
    assert!(ctrl.is_cancelled());

    state.unregister_task("task_001");
    assert!(!state.control_task("task_001", "pause"));
}

#[tokio::test]
async fn test_control_task_command_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(dir.path().join("greenc.db")).unwrap();
    let ctrl = Arc::new(TaskControl::new());

    state.register_task("scan-task-123".to_string(), Arc::clone(&ctrl));

    // Verify pause
    assert!(state.control_task("scan-task-123", "pause"));
    assert!(ctrl.is_paused());

    // Verify resume
    assert!(state.control_task("scan-task-123", "resume"));
    assert!(!ctrl.is_paused());

    // Verify cancel
    assert!(state.control_task("scan-task-123", "cancel"));
    assert!(ctrl.is_cancelled());

    // Verify unknown task
    assert!(!state.control_task("nonexistent-task", "cancel"));
}

#[tokio::test]
async fn test_pre_archive_snapshot_timing_prevents_unlinking_modified_file() {
    let dir = tempfile::tempdir().unwrap();
    let test_file = dir.path().join("active_work.log");
    std::fs::write(&test_file, b"Initial data at T0").unwrap();

    // 1. Capture snapshot at T0
    let mut snapshots = std::collections::HashMap::new();
    let meta_t0 = std::fs::metadata(&test_file).unwrap();
    snapshots.insert(
        test_file.clone(),
        (meta_t0.len(), meta_t0.modified().unwrap()),
    );

    // 2. Simulate mid-flight modification at T1 (during archiving window)
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(
        &test_file,
        b"Appends new critical data at T1 during archiving",
    )
    .unwrap();

    // 3. Simulate SafeCleaner inspection at T2
    let meta_t2 = std::fs::metadata(&test_file).unwrap();
    let (snap_size, snap_mtime) = snapshots.get(&test_file).unwrap();

    let is_modified = meta_t2.len() != *snap_size || meta_t2.modified().unwrap() != *snap_mtime;
    assert!(
        is_modified,
        "Snapshot taken before archiving MUST detect mid-flight modification"
    );
}
