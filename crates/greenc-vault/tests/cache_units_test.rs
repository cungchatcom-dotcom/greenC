//! TDD tests for atomic developer cache units, forbidden control entries, reliable atime/mtime
//! aging, busy-tool gating, and Linux `app_logs` filtering (`GC-GUARD` Item 2).

use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool};
use greenc_platform::traits::MockFileLockProbe;
use greenc_vault::cache_units::{
    is_forbidden_dev_cache_path, is_linux_app_log_file, scan_dev_cache_root, CleanUnitKind,
    DEFAULT_DEV_CACHE_MIN_AGE_SECS,
};
#[cfg(all(unix, not(target_os = "macos")))]
use greenc_vault::profiles::PresetProfiles;
use std::fs::FileTimes;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::tempdir;

fn set_times(path: &Path, mtime: SystemTime, atime: SystemTime) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let times = FileTimes::new().set_modified(mtime).set_accessed(atime);
    f.set_times(times).unwrap();
}

fn write_aged_file(path: &Path, bytes: &[u8], mtime: SystemTime, atime: SystemTime) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
    set_times(path, mtime, atime);
}

/// Builds a realistic fake `~/.gradle/caches` tree mirroring the 2026-11-24 incident.
fn build_fake_gradle_tree(root: &Path, now: SystemTime) -> std::path::PathBuf {
    let caches = root.join(".gradle").join("caches");
    let ten_days_ago = now - Duration::from_secs(10 * 86_400);
    let twenty_days_ago = now - Duration::from_secs(20 * 86_400);
    let one_day_ago = now - Duration::from_secs(86_400);

    // Active/latest version 8.11.1
    write_aged_file(
        &caches.join("8.11.1/transforms/old_hash_01/transformed/classes.jar"),
        &vec![1u8; 4096],
        ten_days_ago,
        ten_days_ago,
    );
    write_aged_file(
        &caches.join("8.11.1/transforms/old_hash_01/metadata.bin"),
        &vec![2u8; 512],
        ten_days_ago,
        ten_days_ago,
    );
    // Transform whose mtime is 10d old, but atime is 1d old (recently read!) -> must be protected!
    write_aged_file(
        &caches.join("8.11.1/transforms/recently_read_hash/classes.jar"),
        &vec![3u8; 4096],
        ten_days_ago,
        one_day_ago,
    );
    // Fresh transform (1d old) -> must be protected!
    write_aged_file(
        &caches.join("8.11.1/transforms/fresh_hash/classes.jar"),
        &vec![4u8; 4096],
        one_day_ago,
        one_day_ago,
    );
    // Forbidden control files inside 8.11.1 and top-level
    write_aged_file(
        &caches.join("8.11.1/transforms/transforms.lock"),
        b"lock",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("8.11.1/fileHashes/fileHashes.bin"),
        b"hashes",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("8.11.1/kotlin-dsl/scripts.jar"),
        b"dsl",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("8.11.1/gc.properties"),
        b"gc",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("journal-1/journal-1.lock"),
        b"jlock",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("journal-1/file-access.bin"),
        b"access",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("modules-2/metadata-2.106/descriptors.bin"),
        b"meta",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("modules-2/file-access.bin"),
        b"maccess",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("modules-2/modules-2.lock"),
        b"mlock",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55",
        twenty_days_ago,
        twenty_days_ago,
    );
    write_aged_file(
        &caches.join("gc.properties"),
        b"gc",
        twenty_days_ago,
        twenty_days_ago,
    );

    // Obsolete older Gradle version 7.6 (20 days old, no control locks inside)
    write_aged_file(
        &caches.join("7.6/transforms/legacy_hash/lib.jar"),
        &vec![9u8; 8192],
        twenty_days_ago,
        twenty_days_ago,
    );

    // Backdate directory mtimes/atimes for old_hash_01 and 7.6 so dir metadata matches files.
    // (Directory mtime is bumped when child files are created.)
    #[cfg(unix)]
    {
        for rel in [
            "8.11.1/transforms/old_hash_01/transformed",
            "8.11.1/transforms/old_hash_01",
            "7.6/transforms/legacy_hash",
            "7.6/transforms",
            "7.6",
        ] {
            let dir_p = caches.join(rel);
            let _ = std::fs::File::open(&dir_p).and_then(|f| {
                f.set_times(
                    FileTimes::new()
                        .set_modified(twenty_days_ago)
                        .set_accessed(twenty_days_ago),
                )
            });
        }
    }

    caches
}

#[test]
fn gradle_busy_or_unknown_selects_nothing() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let caches = build_fake_gradle_tree(dir.path(), now);
    let min_age = Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS);
    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();

    // 1. Gradle busy -> zero units
    let busy_gradle = BusyToolsStatus::known([DevTool::Gradle].into_iter().collect());
    let units = scan_dev_cache_root(&caches, min_age, now, &busy_gradle, &lock_probe, &ctrl);
    assert!(units.is_empty(), "must select nothing when Gradle is busy");

    // 2. Probe status Unknown -> zero units (fail-safe)
    let unknown = BusyToolsStatus::unknown("cannot read /proc");
    let units_unknown = scan_dev_cache_root(&caches, min_age, now, &unknown, &lock_probe, &ctrl);
    assert!(
        units_unknown.is_empty(),
        "must select nothing when dev tool status is unknown"
    );
}

#[test]
fn gradle_idle_selects_only_atomic_stale_units_and_never_forbidden_control_files() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let caches = build_fake_gradle_tree(dir.path(), now);
    let min_age = Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS);
    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();
    let idle = BusyToolsStatus::idle();

    let units = scan_dev_cache_root(&caches, min_age, now, &idle, &lock_probe, &ctrl);

    let roots: Vec<String> = units
        .iter()
        .map(|u| u.root_path.to_string_lossy().to_string())
        .collect();

    // Must select atomic old transform `old_hash_01` (with BOTH files inside it) and old version `7.6`.
    assert!(
        units
            .iter()
            .any(|u| u.root_path.ends_with("8.11.1/transforms/old_hash_01")
                && u.files.len() == 2
                && u.unit_label == "gradle_transform"
                && u.kind == CleanUnitKind::RegenerableDevCache),
        "expected atomic old_hash_01 transform unit, got: {roots:?}"
    );
    assert!(
        units
            .iter()
            .any(|u| u.root_path.ends_with("7.6") && u.unit_label == "gradle_old_version"),
        "expected obsolete Gradle 7.6 unit, got: {roots:?}"
    );
    assert_eq!(units.len(), 2, "unexpected extra units selected: {roots:?}");

    // Verify NO forbidden files or fresh/recently-read units were selected.
    for u in &units {
        assert!(!is_forbidden_dev_cache_path(&u.root_path));
        for f in &u.files {
            let p = f.path.to_string_lossy();
            assert!(!p.ends_with(".lock"), "lock file selected: {p}");
            assert!(!p.ends_with(".lck"), "lck file selected: {p}");
            assert!(!p.contains("journal-1"), "journal selected: {p}");
            assert!(!p.contains("gc.properties"), "gc.properties selected: {p}");
            assert!(!p.contains("CACHEDIR.TAG"), "CACHEDIR.TAG selected: {p}");
            assert!(!p.contains("fileHashes"), "fileHashes selected: {p}");
            assert!(!p.contains("metadata-"), "modules-2 metadata selected: {p}");
            assert!(
                !p.contains("file-access.bin"),
                "file-access.bin selected: {p}"
            );
            assert!(!p.contains("kotlin-dsl"), "active kotlin-dsl selected: {p}");
            assert!(
                !p.contains("recently_read_hash"),
                "recently read transform (atime 1d) must be protected: {p}"
            );
            assert!(
                !p.contains("fresh_hash"),
                "fresh transform must be protected: {p}"
            );
        }
    }
}

#[test]
fn npm_cacache_selects_via_index_and_protects_shared_blobs_and_tmp() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let ten_days_ago = now - Duration::from_secs(10 * 86_400);
    let one_day_ago = now - Duration::from_secs(86_400);

    let cacache = dir.path().join(".npm").join("_cacache");
    // Base64 of 0x01, 0x02, 0x03, 0x04, 0x05, 0x06 -> "AQIDBAUG" -> hex "010203040506"
    // Base64 of 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff -> "qrvM3e7/" -> hex "aabbccddeeff"
    let exclusive_blob = cacache.join("content-v2/sha512/01/02/03040506");
    let shared_blob = cacache.join("content-v2/sha512/aa/bb/ccddeeff");
    write_aged_file(
        &exclusive_blob,
        &vec![5u8; 2048],
        ten_days_ago,
        ten_days_ago,
    );
    write_aged_file(&shared_blob, &vec![6u8; 4096], ten_days_ago, ten_days_ago);

    // Old index referencing exclusive_blob AND shared_blob
    let old_idx = cacache.join("index-v5/11/22/old_entry");
    let old_idx_body = concat!(
        "deadbeef\t{\"key\":\"pkg-old\",\"integrity\":\"sha512-AQIDBAUG\"}\n",
        "deadbee0\t{\"key\":\"pkg-shared\",\"integrity\":\"sha512-qrvM3e7/\"}\n"
    );
    write_aged_file(
        &old_idx,
        old_idx_body.as_bytes(),
        ten_days_ago,
        ten_days_ago,
    );

    // Fresh index also referencing shared_blob
    let fresh_idx = cacache.join("index-v5/33/44/fresh_entry");
    let fresh_idx_body = "cafebabe\t{\"key\":\"pkg-fresh\",\"integrity\":\"sha512-qrvM3e7/\"}\n";
    write_aged_file(
        &fresh_idx,
        fresh_idx_body.as_bytes(),
        one_day_ago,
        one_day_ago,
    );

    // Forbidden `_cacache/tmp` file (even if old!) must NEVER be touched
    let tmp_file = cacache.join("tmp/inflight_download");
    write_aged_file(&tmp_file, b"inflight", ten_days_ago, ten_days_ago);

    let units = scan_dev_cache_root(
        &cacache,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );

    assert_eq!(units.len(), 1);
    let unit = &units[0];
    assert_eq!(unit.unit_label, "npm_cache_entry");
    assert_eq!(unit.root_path, old_idx);
    let paths: Vec<_> = unit.files.iter().map(|f| f.path.clone()).collect();
    assert!(paths.contains(&old_idx));
    assert!(paths.contains(&exclusive_blob));
    assert!(
        !paths.contains(&shared_blob),
        "shared content blob referenced by fresh index must NEVER be deleted"
    );
    assert!(
        !paths.contains(&tmp_file),
        "_cacache/tmp must never be selected"
    );
}

#[test]
fn cargo_cache_selects_old_crates_and_skips_when_cargo_busy() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let ten_days_ago = now - Duration::from_secs(10 * 86_400);
    let cargo_cache = dir.path().join(".cargo/registry/cache/index.crates.io-abc");
    write_aged_file(
        &cargo_cache.join("serde-1.0.0.crate"),
        &vec![7u8; 4096],
        ten_days_ago,
        ten_days_ago,
    );
    write_aged_file(
        &cargo_cache.join(".package-cache"),
        b"lock",
        ten_days_ago,
        ten_days_ago,
    );

    let root = dir.path().join(".cargo/registry/cache");
    let idle_units = scan_dev_cache_root(
        &root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(idle_units.len(), 1);
    assert_eq!(idle_units[0].unit_label, "cargo_crate");

    let busy_cargo = BusyToolsStatus::known([DevTool::Cargo].into_iter().collect());
    assert!(scan_dev_cache_root(
        &root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &busy_cargo,
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    )
    .is_empty());
}

#[test]
fn linux_app_logs_only_selects_old_log_files_not_arbitrary_local_state() {
    assert!(is_linux_app_log_file("app.log"));
    assert!(is_linux_app_log_file("service.log.1"));
    assert!(is_linux_app_log_file("daemon.log.2026-11-20.gz"));
    assert!(!is_linux_app_log_file("guardian.log"));
    assert!(!is_linux_app_log_file("shada.main"));
    assert!(!is_linux_app_log_file("bash_history"));
    assert!(!is_linux_app_log_file("state.db"));
    assert!(!is_linux_app_log_file("gc-cargo.sh"));

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = tempdir().unwrap();
        let now = SystemTime::now();
        let three_days_ago = now - Duration::from_secs(3 * 86_400);
        let state_dir = dir.path().join(".local/state");
        write_aged_file(
            &state_dir.join("myapp/old.log"),
            b"old log",
            three_days_ago,
            three_days_ago,
        );
        write_aged_file(
            &state_dir.join("myapp/old.log.1"),
            b"rotated log",
            three_days_ago,
            three_days_ago,
        );
        write_aged_file(
            &state_dir.join("nvim/shada/main.shada"),
            b"nvim state",
            three_days_ago,
            three_days_ago,
        );
        write_aged_file(
            &state_dir.join("the build agent-loop/gc-cargo.sh"),
            b"#!/bin/bash",
            three_days_ago,
            three_days_ago,
        );

        let profile = PresetProfiles::get_by_id("app_logs").unwrap();
        let units = PresetProfiles::scan_profile_dirs_as_units(
            &profile,
            &[state_dir],
            now,
            &TaskControl::new(),
            &MockFileLockProbe::new(),
            &BusyToolsStatus::idle(),
        );
        let names: Vec<String> = units
            .iter()
            .map(|u| {
                u.root_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(names.len(), 2, "got: {names:?}");
        assert!(names.contains(&"old.log".to_string()));
        assert!(names.contains(&"old.log.1".to_string()));
    }
}
