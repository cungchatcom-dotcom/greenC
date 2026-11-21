//! End-to-end integration tests for fast Settings persistence, native host drive enumeration,
//! Windows HKCU autostart roundtrip, and updater semver checks.

#![deny(unsafe_code)]

use greenc_app::commands::updater::is_newer_version;
use greenc_app::state::AppState;
use greenc_platform::volume::list_drives_native;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn settings_autostart_and_guardian_fields_persist_across_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc_e2e.db");

    {
        let state = AppState::new(db_path.clone()).expect("AppState should open");
        let s = state.settings();
        assert!(!s.get_autostart_enabled().unwrap());
        assert!(s.get_pause_on_battery_fullscreen().unwrap());
        assert_eq!(s.get_low_disk_shield_gb().unwrap(), 10);

        s.set_autostart_enabled(true).unwrap();
        s.set_pause_on_battery_fullscreen(false).unwrap();
        s.set_low_disk_shield_gb(25).unwrap();
    }

    // Reopen fresh SQLite connection to prove durable persistence
    let reopened = AppState::new(db_path).expect("AppState should reopen");
    let start = Instant::now();
    let s = reopened.settings();
    assert!(s.get_autostart_enabled().unwrap());
    assert!(!s.get_pause_on_battery_fullscreen().unwrap());
    assert_eq!(s.get_low_disk_shield_gb().unwrap(), 25);
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "reading settings must complete in < 100 ms"
    );
}

#[test]
fn native_drive_enumeration_is_fast_and_finds_host_volumes() {
    let start = Instant::now();
    let drives = list_drives_native().expect("list_drives_native must succeed");
    assert!(
        !drives.is_empty(),
        "host must expose at least one logical drive"
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "enumerating host drives must be fast, took {:?}",
        start.elapsed()
    );
}

#[cfg(windows)]
#[test]
fn windows_hkcu_autostart_registry_roundtrip() {
    use greenc_platform::power_session::{
        is_autostart_enabled_native, set_autostart_registry_native,
    };

    const TEST_KEY: &str = "GreenC_E2E_Autostart_Probe";
    let fake_exe = std::path::Path::new(r"C:\Program Files\GreenC\GreenC.exe");

    // Ensure clean starting state
    let _ = set_autostart_registry_native(TEST_KEY, fake_exe, false);
    assert!(!is_autostart_enabled_native(TEST_KEY).unwrap());

    // Enable -> must exist in HKCU\...\Run
    set_autostart_registry_native(TEST_KEY, fake_exe, true).unwrap();
    assert!(is_autostart_enabled_native(TEST_KEY).unwrap());

    // Disable -> must be removed cleanly
    set_autostart_registry_native(TEST_KEY, fake_exe, false).unwrap();
    assert!(!is_autostart_enabled_native(TEST_KEY).unwrap());
}

#[test]
fn updater_semver_detects_v212_over_v211() {
    assert!(is_newer_version("2.1.1", "v2.1.2"));
    assert!(is_newer_version("2.1.1", "2.2.0"));
    assert!(!is_newer_version("2.1.2", "v2.1.2"));
    assert!(!is_newer_version("2.1.2", "v2.1.1"));
}
