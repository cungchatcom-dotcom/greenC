//! Integration and unit tests for `BackgroundGuardian` scheduler preemption (`< 100 ms`),
//! battery-saver / D3D fullscreen deferral, Emergency Low-Disk Shield alerts, and rolling
//! `QuotaRule` enforcement into the Canary Vault (Task T058).

#![deny(unsafe_code)]

use chrono::Utc;
use greenc_app::guardian::{BackgroundGuardian, GuardianDeps};
use greenc_app::state::AppState;
use greenc_core::models::{DriveInfo, QuotaRule};
use greenc_core::task::TaskControl;
use greenc_win32::power_session::{MockPowerSessionApi, PowerSessionState};
use greenc_win32::traits::{
    MockFileLockProbe, MockProcessServiceProbe, MockReparsePointApi, MockVolumeApi,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tempfile::tempdir;

fn make_test_guardian(
    temp_root: &std::path::Path,
    free_gb: u64,
    power_state: PowerSessionState,
) -> (AppState, BackgroundGuardian, MockPowerSessionApi) {
    let db_path = temp_root.join("greenc_test.db");
    let state = AppState::new(db_path).expect("AppState should initialize in temp dir");
    let data_root = temp_root.join("DataRoot");
    std::fs::create_dir_all(data_root.join("Vault")).unwrap();
    state.settings().set_data_root(&data_root).unwrap();

    let sys_drive = temp_root.join("SysDrive");
    std::fs::create_dir_all(&sys_drive).unwrap();

    let mock_power = MockPowerSessionApi::new(power_state);
    let mock_volume = MockVolumeApi::new(vec![
        DriveInfo {
            root: sys_drive.clone(),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 500 * 1024 * 1024 * 1024,
            free_bytes: free_gb * 1024 * 1024 * 1024,
            is_system: true,
            is_eligible: false,
            ineligible_reason: Some("System drive".to_string()),
        },
        DriveInfo {
            root: data_root.clone(),
            drive_letter: 'D',
            label: "Data".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 1000 * 1024 * 1024 * 1024,
            free_bytes: 200 * 1024 * 1024 * 1024,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        },
    ]);

    let deps = GuardianDeps {
        power_api: Arc::new(mock_power.clone()),
        volume_api: Arc::new(mock_volume),
        lock_probe: Arc::new(MockFileLockProbe::new()),
        proc_probe: Arc::new(MockProcessServiceProbe::new()),
        reparse_api: Arc::new(MockReparsePointApi::new()),
    };

    let guardian = BackgroundGuardian::with_deps(state.clone(), deps, Duration::from_secs(45))
        .with_system_drive_root(sys_drive);

    (state, guardian, mock_power)
}

#[tokio::test]
async fn guardian_yields_under_100ms_when_foreground_task_is_active() {
    let dir = tempdir().unwrap();
    let (state, guardian, _) = make_test_guardian(dir.path(), 80, PowerSessionState::default());

    // Register an active foreground user task (e.g., Space Lens scan or 1-click Cleanup)
    let fg_ctrl = Arc::new(TaskControl::new());
    state.register_task("fg-cleanup-001".to_string(), fg_ctrl);

    let start = Instant::now();
    let report = guardian.run_cycle().await.unwrap();
    let elapsed = start.elapsed();

    assert!(report.preempted_by_foreground);
    assert_eq!(report.rules_evaluated, 0);
    assert!(
        elapsed < Duration::from_millis(100),
        "Guardian must yield in < 100 ms when foreground task is running, took {:?}",
        elapsed
    );

    state.unregister_task("fg-cleanup-001");
}

#[tokio::test]
async fn guardian_defers_quota_compression_on_battery_or_fullscreen() {
    let dir = tempdir().unwrap();
    let (state, guardian, mock_power) = make_test_guardian(
        dir.path(),
        60,
        PowerSessionState {
            on_battery: true,
            battery_saver_on: false,
            battery_percent: 55,
            is_fullscreen_or_busy: false,
        },
    );

    // Seed an exceeded quota rule so it WOULD enforce if not deferred
    let app_cache = dir.path().join("ZaloCache");
    std::fs::create_dir_all(&app_cache).unwrap();
    let old_file = app_cache.join("media_01.mp4");
    std::fs::write(&old_file, vec![0xAB; 2 * 1024 * 1024]).unwrap();

    state
        .db
        .save_quota_rule(&QuotaRule {
            id: None,
            app_name: "Zalo".to_string(),
            process_name: "Zalo.exe".to_string(),
            service_name: String::new(),
            target_paths: vec![app_cache.clone()],
            max_bytes: 1024 * 1024, // 1 MiB limit, currently 2 MiB
            watermark_ratio: 0.85,
            retention_days: 7,
            min_file_age_seconds: 0,
            exclude_patterns: Vec::new(),
            protected_files: Vec::new(),
            enabled: true,
            created_at: Utc::now(),
        })
        .unwrap();

    // 1. On battery -> deferred! File remains untouched
    let rep_battery = guardian.run_cycle().await.unwrap();
    assert!(rep_battery.deferred_for_power_or_fullscreen);
    assert_eq!(rep_battery.rules_enforced, 0);
    assert!(old_file.exists());

    // 2. AC power but D3D fullscreen game active -> also deferred!
    mock_power.set_state(PowerSessionState {
        on_battery: false,
        battery_saver_on: false,
        battery_percent: 100,
        is_fullscreen_or_busy: true,
    });
    let rep_fullscreen = guardian.run_cycle().await.unwrap();
    assert!(rep_fullscreen.deferred_for_power_or_fullscreen);
    assert!(old_file.exists());

    // 3. AC power + normal desktop session -> enforces rule down to <= 85% watermark!
    mock_power.set_state(PowerSessionState::default());
    let rep_normal = guardian.run_cycle().await.unwrap();
    assert!(!rep_normal.deferred_for_power_or_fullscreen);
    assert_eq!(rep_normal.rules_enforced, 1);
    assert!(rep_normal.bytes_reclaimed >= 2 * 1024 * 1024);
    assert!(!old_file.exists());
}

#[tokio::test]
async fn guardian_triggers_emergency_low_disk_shield_alert() {
    let dir = tempdir().unwrap();
    // Free space = 4 GiB < default 10 GiB shield threshold
    let (state, guardian, _) = make_test_guardian(dir.path(), 4, PowerSessionState::default());
    state.settings().set_low_disk_shield_gb(10).unwrap();

    let report = guardian.run_cycle().await.unwrap();
    assert!(report.low_disk_triggered);

    let alerts = guardian.recorded_alerts();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, "low_disk_warning");
    assert_eq!(alerts[0].severity, "critical");

    // Second immediate tick respects cooldown so user isn't spammed every 45s
    let report_second = guardian.run_cycle().await.unwrap();
    assert!(report_second.low_disk_triggered);
    assert_eq!(guardian.recorded_alerts().len(), 1);
}

#[tokio::test]
async fn guardian_spawn_stops_cleanly_via_cancellation_token() {
    let dir = tempdir().unwrap();
    let (_state, guardian, _) = make_test_guardian(dir.path(), 50, PowerSessionState::default());
    let arc_guardian = Arc::new(guardian);
    let token = arc_guardian.spawn();
    tokio::time::sleep(Duration::from_millis(25)).await;
    token.cancel();

    // Ensure mtime helper compiles cleanly without warnings
    let _ = SystemTime::now();
    let _ = PathBuf::new();
}

// ---------------------------------------------------------------------------
// Drive quota: warnings and automatic actions
// ---------------------------------------------------------------------------

fn seed_drive_quota(
    state: &AppState,
    dir: &std::path::Path,
    mutate: impl FnOnce(&mut greenc_core::models::DriveQuota),
) {
    let mut q = greenc_core::models::DriveQuota::new(
        dir.join("SysDrive").to_string_lossy().to_string(),
    );
    mutate(&mut q);
    state.db.save_drive_quota(&q).unwrap();
}

#[tokio::test]
async fn drive_quota_warns_once_then_stays_quiet_within_cooldown() {
    let dir = tempdir().unwrap();
    // 500 GB drive, 75 GB free => 85% used => Warning (80%..90%)
    let (state, guardian, _) = make_test_guardian(dir.path(), 75, PowerSessionState::default());
    seed_drive_quota(&state, dir.path(), |_| {});

    let first = guardian.run_cycle().await.unwrap();
    assert_eq!(first.drive_quotas_evaluated, 1);
    assert_eq!(first.drive_quota_warnings, 1);
    assert_eq!(first.drive_quota_cleanups, 0);

    let alerts = guardian.recorded_alerts();
    assert!(alerts
        .iter()
        .any(|a| a.kind == "drive_quota" && a.severity == "warning"));

    let second = guardian.run_cycle().await.unwrap();
    assert_eq!(second.drive_quota_warnings, 0, "cooldown must suppress repeats");
}

#[tokio::test]
async fn drive_quota_escalation_to_critical_alerts_again() {
    let dir = tempdir().unwrap();
    // 25 GB free of 500 GB => 95% used => Critical
    let (state, guardian, _) = make_test_guardian(dir.path(), 25, PowerSessionState::default());
    seed_drive_quota(&state, dir.path(), |_| {});

    let report = guardian.run_cycle().await.unwrap();
    assert_eq!(report.drive_quota_warnings, 1);
    assert!(guardian
        .recorded_alerts()
        .iter()
        .any(|a| a.kind == "drive_quota" && a.severity == "critical"));
}

#[tokio::test]
async fn drive_quota_ok_or_disabled_is_silent() {
    let dir = tempdir().unwrap();
    // 200 GB free of 500 GB => 60% used => Ok
    let (state, guardian, _) = make_test_guardian(dir.path(), 200, PowerSessionState::default());
    seed_drive_quota(&state, dir.path(), |_| {});
    let report = guardian.run_cycle().await.unwrap();
    assert_eq!(report.drive_quotas_evaluated, 1);
    assert_eq!(report.drive_quota_warnings, 0);

    // A disabled quota is not evaluated at all.
    let dir2 = tempdir().unwrap();
    let (state2, guardian2, _) = make_test_guardian(dir2.path(), 25, PowerSessionState::default());
    seed_drive_quota(&state2, dir2.path(), |q| q.enabled = false);
    let report2 = guardian2.run_cycle().await.unwrap();
    assert_eq!(report2.drive_quotas_evaluated, 0);
}

#[tokio::test]
async fn drive_quota_auto_clean_with_nothing_to_clean_does_not_fail() {
    let dir = tempdir().unwrap();
    let (state, guardian, _) = make_test_guardian(dir.path(), 25, PowerSessionState::default());
    seed_drive_quota(&state, dir.path(), |q| q.auto_clean = true);
    let report = guardian.run_cycle().await.unwrap();
    assert_eq!(report.drive_quota_cleanups, 0);
    assert_eq!(report.bytes_reclaimed, 0);
}

#[test]
fn enforce_drive_quota_moves_oldest_candidates_into_vault_and_flags_same_drive_backup() {
    use greenc_app::guardian::enforce_drive_quota;
    use greenc_core::models::{CleanCandidate, DriveQuota};

    let dir = tempdir().unwrap();
    let (state, _guardian, _) = make_test_guardian(dir.path(), 25, PowerSessionState::default());
    let deps = GuardianDeps {
        power_api: Arc::new(MockPowerSessionApi::new(PowerSessionState::default())),
        volume_api: Arc::new(MockVolumeApi::new(vec![])),
        lock_probe: Arc::new(MockFileLockProbe::new()),
        proc_probe: Arc::new(MockProcessServiceProbe::new()),
        reparse_api: Arc::new(MockReparsePointApi::new()),
    };

    // Fabricated candidates inside the temp dir: never touches real user data.
    let cache = dir.path().join("FakeCache");
    std::fs::create_dir_all(&cache).unwrap();
    let mut pool = Vec::new();
    for (i, age_days) in [(0u64, 30u64), (1, 20), (2, 1)] {
        let f = cache.join(format!("f{i}.bin"));
        std::fs::write(&f, vec![7u8; 4096]).unwrap();
        // Make the real mtime match the candidate snapshot (the cleaner refuses touched files).
        let mtime = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        pool.push(CleanCandidate {
            path: f,
            size_bytes: 4096,
            mtime,
            is_locked: false,
            profile_id: "fake".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: Vec::new(),
        });
    }

    let mut quota = DriveQuota::new(dir.path().to_string_lossy().to_string());
    quota.auto_clean = true;
    quota.auto_backup = true;
    quota.backup_dir = Some(dir.path().join("bk")); // same drive as the protected root => refused

    // 500 GB drive, 25 GB free: far more than the pool can satisfy, so every candidate is taken.
    let total = 500u64 * 1024 * 1024 * 1024;
    let free = 25u64 * 1024 * 1024 * 1024;
    let vault = dir.path().join("DataRoot").join("Vault");
    let out = enforce_drive_quota(
        &quota,
        total,
        free,
        pool,
        &vault,
        &state,
        &deps,
        &TaskControl::new(),
    )
    .unwrap();

    assert_eq!(out.bytes_freed, 3 * 4096);
    assert!(out.vault_zip.as_ref().is_some_and(|z| z.exists()));
    assert!(out.backed_up_to.is_none());
    assert!(out.backup_problem.is_some(), "same-drive backup must be refused");
    for i in 0..3 {
        assert!(!cache.join(format!("f{i}.bin")).exists(), "original removed after archiving");
    }
}

#[test]
fn backup_refuses_same_drive_and_copies_to_other_folder() {
    use greenc_app::guardian::backup_vault_archive;
    let dir = tempdir().unwrap();
    let zip = dir.path().join("arch1.zip");
    std::fs::write(&zip, b"zipdata").unwrap();
    std::fs::write(dir.path().join("arch1.manifest.json"), b"{}").unwrap();

    // Same drive prefix as the protected root => refused.
    let err = backup_vault_archive(&zip, std::path::Path::new("C:\\"), std::path::Path::new("C:/bk"))
        .unwrap_err();
    assert!(err.contains("chính ổ"));

    // Different drive prefix => copied (protected root uses a letter the temp dir is not on).
    let dest_dir = dir.path().join("bk");
    let tmp_letter = dir.path().to_string_lossy().chars().next().unwrap().to_ascii_uppercase();
    let other = if tmp_letter == 'Q' { "R:/" } else { "Q:/" };
    let dest = backup_vault_archive(&zip, std::path::Path::new(other), &dest_dir).unwrap();
    assert!(dest.exists());
    assert!(dest_dir.join("arch1.manifest.json").exists());
}

/// Regression: Tauri's `setup()` runs on the main thread with no Tokio runtime entered.
/// `spawn()` must work there instead of panicking ("there is no reactor running").
#[test]
fn guardian_spawn_works_outside_a_tokio_runtime_like_tauri_setup() {
    let dir = tempdir().unwrap();
    let (_state, guardian, _) = make_test_guardian(dir.path(), 50, PowerSessionState::default());
    let token = Arc::new(guardian).spawn();
    std::thread::sleep(Duration::from_millis(50));
    token.cancel();
}
