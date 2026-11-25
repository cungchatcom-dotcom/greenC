//! Regression & TDD tests for GC-GUARD2 review fixes (Items 1-7).
#![cfg_attr(not(unix), allow(unused_imports, dead_code, unused_variables))]

use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::Result;
use greenc_core::models::{CleanCandidate, DriveInfo, DriveQuota, RegenerableCacheMode};
use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool, DevToolProbe, MockDevToolProbe};
use greenc_platform::handle::query_file_identity;
use greenc_platform::traits::{FileLockProbe, MockFileLockProbe, Win32VolumeApi};
use greenc_platform::{detect_atime_reliability_from_proc_mounts, AtimeReliability};
use greenc_storage::db::GreenCDb;
use greenc_vault::cache_units::{
    compute_unit_latest_used, scan_dev_cache_root, scan_dev_cache_root_with_atime_reliability,
    CleanUnit, CleanUnitKind, DEFAULT_DEV_CACHE_MIN_AGE_SECS,
};
use greenc_vault::guard::{
    enforce_drive_quota_units, recover_staging_dir_with_options, GuardRunOptions,
    StagingJournalEntry, StagingMode, StagingRecoveryOptions, StagingState,
};
use greenc_vault::profiles::PresetProfiles;
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tempfile::tempdir;

#[derive(Clone)]
struct DirAccountingVolumeApi {
    root: PathBuf,
    total_bytes: u64,
    base_free_bytes: u64,
    initial_tree_bytes: Arc<Mutex<u64>>,
}

impl DirAccountingVolumeApi {
    fn new(root: PathBuf, total_bytes: u64, base_free_bytes: u64) -> Self {
        let initial = dir_bytes(&root);
        Self {
            root,
            total_bytes,
            base_free_bytes,
            initial_tree_bytes: Arc::new(Mutex::new(initial)),
        }
    }
}

fn dir_bytes(root: &Path) -> u64 {
    let mut sum = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if let Ok(m) = std::fs::symlink_metadata(&p) {
                    if m.file_type().is_symlink() {
                        continue;
                    }
                    if m.is_dir() {
                        stack.push(p);
                    } else if m.is_file() {
                        sum = sum.saturating_add(m.len());
                    }
                }
            }
        }
    }
    sum
}

impl Win32VolumeApi for DirAccountingVolumeApi {
    fn list_drives(&self) -> Result<Vec<DriveInfo>> {
        let (t, f) = self.get_disk_free_space(&self.root)?;
        Ok(vec![DriveInfo {
            root: self.root.clone(),
            drive_letter: 'H',
            label: "Home".to_string(),
            filesystem: "ext4".to_string(),
            total_bytes: t,
            free_bytes: f,
            is_system: false,
            is_eligible: true,
            ineligible_reason: None,
        }])
    }

    fn get_disk_free_space(&self, _root: &Path) -> Result<(u64, u64)> {
        let init = *self.initial_tree_bytes.lock().unwrap();
        let cur = dir_bytes(&self.root);
        let delta = init as i64 - cur as i64;
        let free = (self.base_free_bytes as i64 + delta).max(0) as u64;
        Ok((self.total_bytes, free))
    }

    fn get_volume_info(&self, _root: &Path) -> Result<(String, String, u32)> {
        Ok(("Home".to_string(), "ext4".to_string(), 3))
    }
}

fn write_aged(path: &Path, data: &[u8], age_days: u64, now: SystemTime) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, data).unwrap();
    let ts = now - Duration::from_secs(age_days * 86_400);
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_times(FileTimes::new().set_modified(ts).set_accessed(ts))
        .unwrap();
}

fn backdate_dir(path: &Path, age_days: u64, now: SystemTime) {
    let ts = now - Duration::from_secs(age_days * 86_400);
    #[cfg(unix)]
    {
        let _ = std::fs::File::open(path)
            .and_then(|f| f.set_times(FileTimes::new().set_modified(ts).set_accessed(ts)));
    }
}

/// Lock probe that unlocks on the first N checks of `target` (scan + TOCTOU),
/// then returns locked=true on subsequent checks (simulating mid-archive lock in Vault mode).
struct DelayedLockProbe {
    target: PathBuf,
    allow_before_lock: usize,
    hits: AtomicUsize,
}

impl FileLockProbe for DelayedLockProbe {
    fn is_file_locked(&self, path: &Path) -> bool {
        if path == self.target {
            let n = self.hits.fetch_add(1, Ordering::SeqCst);
            return n >= self.allow_before_lock;
        }
        false
    }
}

struct CountingDevToolProbe {
    inner: MockDevToolProbe,
    calls: AtomicUsize,
}

impl CountingDevToolProbe {
    fn idle() -> Self {
        Self {
            inner: MockDevToolProbe::idle(),
            calls: AtomicUsize::new(0),
        }
    }

    fn from_mock(inner: MockDevToolProbe) -> Self {
        Self {
            inner,
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl DevToolProbe for CountingDevToolProbe {
    fn detect_busy_tools(&self) -> BusyToolsStatus {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.detect_busy_tools()
    }
}

/// Item 1: In Vault mode, a directory unit (Gradle transform) MUST be atomically moved out to a
/// private (`0700`) staging directory outside `.gradle/caches` before archiving, NEVER half-deleted
/// leaving `classes.jar` without `metadata.bin` or an empty dir, and Vault restore MUST restore
/// original paths verbatim.
#[test]
fn item1_vault_mode_moves_gradle_transform_atomically_never_leaves_partial_or_empty_dir() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let gradle_root = home.join(".gradle/caches");
    let unit_dir = gradle_root.join("8.11.1/transforms/old_hash_01");
    let meta_bin = unit_dir.join("metadata.bin");
    let classes_jar = unit_dir.join("transformed/classes.jar");
    let meta_bytes = vec![b'M'; 64 * 1024];
    let jar_bytes = vec![b'J'; 512 * 1024];
    write_aged(&meta_bin, &meta_bytes, 10, now);
    write_aged(&classes_jar, &jar_bytes, 10, now);
    backdate_dir(&unit_dir.join("transformed"), 10, now);
    backdate_dir(&unit_dir, 10, now);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

    let units = scan_dev_cache_root(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 1);

    // Delayed lock triggers if Vault tries to unlink individual files in-place after TOCTOU!
    let lock_probe = DelayedLockProbe {
        target: classes_jar.clone(),
        allow_before_lock: 1, // 1 check in TOCTOU passes; 2nd check during old in-place unlink locks!
        hits: AtomicUsize::new(0),
    };

    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = total - (total as f64 * 0.8005) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.7999;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Vault;

    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let out = enforce_drive_quota_units(
        &quota,
        units,
        &vault_dir,
        &db,
        &vol,
        &lock_probe,
        &MockDevToolProbe::idle(),
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    // The transform dir MUST be completely gone (zero leftover/partial/empty dirs inside cache tree).
    assert!(
        !unit_dir.exists(),
        "transform unit dir must be atomically removed with zero leftover/partial files; meta_exists={}, jar_exists={}",
        meta_bin.exists(),
        classes_jar.exists()
    );
    assert!(out.vault_zip.is_some());

    // Staging dir MUST live outside `.gradle/caches` and have 0700 permissions on Unix, with no leftover staged dirs.
    let staging_dir = vault_dir.join(".greenc-staging");
    assert!(staging_dir.is_dir());
    assert!(!staging_dir.starts_with(&gradle_root));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&staging_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "staging directory must be private 0700");
    }
    assert_eq!(
        std::fs::read_dir(&staging_dir).unwrap().count(),
        0,
        "staging dir must be clean after Vault commit"
    );

    // Vault DB MUST store original paths (not staging paths) and restore cleanly!
    let archives = db.list_vault_archives(None).unwrap();
    assert_eq!(archives.len(), 1);
    let arch_id = archives[0].id.unwrap();
    let vfiles = db.get_vault_files(arch_id).unwrap();
    assert!(vfiles.iter().any(|f| f.original_path == meta_bin));
    assert!(vfiles.iter().any(|f| f.original_path == classes_jar));

    let rollback = RollbackEngine::new(
        &db,
        RestoreOptions {
            vault_dir: Some(vault_dir.clone()),
            ..Default::default()
        },
    );
    let restored = rollback
        .rollback_archive(arch_id, "verify original_path restore", false)
        .unwrap();
    assert_eq!(restored.restored_count, 2);
    assert_eq!(std::fs::read(&meta_bin).unwrap(), meta_bytes);
    assert_eq!(std::fs::read(&classes_jar).unwrap(), jar_bytes);
}

/// Item 1 (staging crash recovery & conflict handling — updated for GC-GUARD3 security &
/// redundant-conflict cleanup):
/// - Power loss before Vault commit + original path absent -> restored to original path.
/// - Power loss before Vault commit + original path already recreated:
///   - Regenerable dev cache -> redundant staged copy deleted and logged.
///   - Non-regenerable unit -> kept safely in staging & reported as conflict.
/// - Power loss after Direct/Archived commit -> staged tree deleted & journal removed.
#[test]
fn item1_staging_crash_recovery_restores_or_reports_conflict_and_cleans_committed() {
    let dir = tempdir().unwrap();
    let staging_dir = dir.path().join(".greenc-staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let gradle_root = dir.path().join(".gradle/caches");
    let rec_opts = StagingRecoveryOptions {
        allowed_cache_roots: Some(vec![gradle_root.clone()]),
        ..Default::default()
    };

    // Case A: Vault staged unit whose original path does NOT exist -> MUST restore back!
    let orig_a = gradle_root.join("8.11.1/transforms/hash_a");
    std::fs::create_dir_all(orig_a.parent().unwrap()).unwrap();
    let id_a = "unit_1700000000001_100_1".to_string();
    let staged_a = staging_dir.join(&id_a);
    std::fs::create_dir_all(&staged_a).unwrap();
    std::fs::write(staged_a.join("metadata.bin"), b"meta_a").unwrap();
    let phys_a = query_file_identity(&staged_a).unwrap();
    let j_a = StagingJournalEntry {
        version: 2,
        id: id_a.clone(),
        original_path: orig_a.clone(),
        staged_path: staged_a.clone(),
        mode: StagingMode::Vault,
        state: StagingState::Staged,
        created_at: Utc::now(),
        drive_root: dir.path().to_string_lossy().to_string(),
        unit_label: "gradle_transform".to_string(),
        dev_tool: Some(DevTool::Gradle),
        note: None,
        staged_dev: Some(phys_a.volume_serial),
        staged_ino: Some(phys_a.file_index),
        unit_kind: Some(CleanUnitKind::RegenerableDevCache),
    };
    std::fs::write(
        staging_dir.join(format!("{id_a}.journal.json")),
        serde_json::to_vec(&j_a).unwrap(),
    )
    .unwrap();

    // Case B1: Non-regenerable unit whose original path ALREADY exists -> MUST keep in staging & warn!
    let orig_b = gradle_root.join("8.11.1/transforms/hash_b");
    std::fs::create_dir_all(&orig_b).unwrap();
    std::fs::write(orig_b.join("metadata.bin"), b"new_from_gradle").unwrap();
    let id_b = "unit_1700000000001_100_2".to_string();
    let staged_b = staging_dir.join(&id_b);
    std::fs::create_dir_all(&staged_b).unwrap();
    std::fs::write(staged_b.join("metadata.bin"), b"old_staged").unwrap();
    let phys_b = query_file_identity(&staged_b).unwrap();
    let j_b = StagingJournalEntry {
        version: 2,
        id: id_b.clone(),
        original_path: orig_b.clone(),
        staged_path: staged_b.clone(),
        mode: StagingMode::Vault,
        state: StagingState::Staged,
        created_at: Utc::now(),
        drive_root: dir.path().to_string_lossy().to_string(),
        unit_label: "standard_file".to_string(),
        dev_tool: Some(DevTool::Gradle),
        note: None,
        staged_dev: Some(phys_b.volume_serial),
        staged_ino: Some(phys_b.file_index),
        unit_kind: Some(CleanUnitKind::StandardFile),
    };
    std::fs::write(
        staging_dir.join(format!("{id_b}.journal.json")),
        serde_json::to_vec(&j_b).unwrap(),
    )
    .unwrap();

    // Case C: Direct-delete staged unit interrupted before deletion finished -> MUST finish deleting!
    let orig_c = gradle_root.join("8.11.1/transforms/hash_c");
    let id_c = "unit_1700000000001_100_3".to_string();
    let staged_c = staging_dir.join(&id_c);
    std::fs::create_dir_all(&staged_c).unwrap();
    std::fs::write(staged_c.join("old.jar"), b"to_delete").unwrap();
    let phys_c = query_file_identity(&staged_c).unwrap();
    let j_c = StagingJournalEntry {
        version: 2,
        id: id_c.clone(),
        original_path: orig_c,
        staged_path: staged_c.clone(),
        mode: StagingMode::Direct,
        state: StagingState::Deleting,
        created_at: Utc::now(),
        drive_root: dir.path().to_string_lossy().to_string(),
        unit_label: "gradle_transform".to_string(),
        dev_tool: Some(DevTool::Gradle),
        note: None,
        staged_dev: Some(phys_c.volume_serial),
        staged_ino: Some(phys_c.file_index),
        unit_kind: Some(CleanUnitKind::RegenerableDevCache),
    };
    std::fs::write(
        staging_dir.join(format!("{id_c}.journal.json")),
        serde_json::to_vec(&j_c).unwrap(),
    )
    .unwrap();

    // First run while Gradle is BUSY: Case A & B deferred, Case C still safely deleted!
    let busy_rec = recover_staging_dir_with_options(
        &staging_dir,
        &BusyToolsStatus::known([DevTool::Gradle].into_iter().collect()),
        &rec_opts,
    );
    assert_eq!(busy_rec.completed_deletions, 1);
    assert_eq!(busy_rec.deferred_busy, 2);
    assert!(!staged_c.exists());
    assert!(staged_a.exists());
    assert!(!orig_a.exists());

    // Next run when Gradle is IDLE: Case A restored, Case B (non-regenerable) kept in staging with conflict warning!
    let idle_rec =
        recover_staging_dir_with_options(&staging_dir, &BusyToolsStatus::idle(), &rec_opts);
    assert_eq!(idle_rec.restored_to_original, 1);
    assert_eq!(idle_rec.conflicts.len(), 1);
    assert_eq!(
        std::fs::read(orig_a.join("metadata.bin")).unwrap(),
        b"meta_a"
    );
    assert!(!staged_a.exists());
    assert!(!staging_dir.join(format!("{id_a}.journal.json")).exists());
    assert_eq!(
        std::fs::read(orig_b.join("metadata.bin")).unwrap(),
        b"new_from_gradle",
        "recreated original dir must NOT be overwritten"
    );
    assert!(
        staged_b.exists(),
        "conflicting non-regenerable staged dir must be kept safely in staging"
    );
}

/// Item 1 (npm multi-file in Direct & Vault modes): if removing index-v5 fails, content-v2 MUST NOT
/// be removed (never leave an index pointing to missing content!).
#[cfg(unix)]
#[test]
fn item1_npm_multifile_never_deletes_content_if_index_removal_fails() {
    use std::os::unix::fs::PermissionsExt;

    for mode in [RegenerableCacheMode::Direct, RegenerableCacheMode::Vault] {
        let dir = tempdir().unwrap();
        let now = SystemTime::now();
        let home = dir.path().join("home_sim");
        let cacache = home.join(".npm/_cacache");
        let content_blob = cacache.join("content-v2/sha512/01/02/03040506");
        let idx_dir = cacache.join("index-v5/11/22");
        let idx_file = idx_dir.join("old_entry");
        write_aged(&content_blob, &vec![b'C'; 128 * 1024], 10, now);
        write_aged(
            &idx_file,
            b"deadbeef\t{\"key\":\"pkg\",\"integrity\":\"sha512-AQIDBAUG\"}\n",
            10,
            now,
        );

        let units = scan_dev_cache_root(
            &cacache,
            Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
            now,
            &BusyToolsStatus::idle(),
            &MockFileLockProbe::new(),
            &TaskControl::new(),
        );
        assert_eq!(units.len(), 1);

        // Make index parent directory read-only so unlinking `idx_file` fails!
        std::fs::set_permissions(&idx_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let vault_dir = home.join("Vault");
        std::fs::create_dir_all(&vault_dir).unwrap();
        let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
        let total = 10u64 * 1024 * 1024 * 1024;
        let free_before = (total as f64 * 0.15) as u64;
        let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
        quota.clean_ratio = 0.80;
        quota.target_ratio = 0.75;
        quota.auto_clean = true;
        quota.regenerable_cache_mode = mode;

        let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
        let out = enforce_drive_quota_units(
            &quota,
            units,
            &vault_dir,
            &db,
            &vol,
            &MockFileLockProbe::new(),
            &MockDevToolProbe::idle(),
            &GuardRunOptions {
                now_sys: now,
                ..Default::default()
            },
            &TaskControl::new(),
        )
        .unwrap();

        // Restore write permission so tempdir cleans up cleanly
        let _ = std::fs::set_permissions(&idx_dir, std::fs::Permissions::from_mode(0o755));

        assert!(
            idx_file.exists(),
            "mode={mode:?}: index removal was blocked by read-only dir"
        );
        assert!(
            content_blob.exists(),
            "mode={mode:?}: content-v2 MUST NOT be deleted when index-v5 could not be removed!"
        );
        if mode == RegenerableCacheMode::Vault {
            // Item 2 bonus check: since 0 files were deleted, no orphan Vault zip/DB row!
            assert!(out.vault_zip.is_none());
            assert!(db.list_vault_archives(None).unwrap().is_empty());
        }
    }
}

/// Item 2: When tool starts (or status becomes Unknown) during Vault archiving and 0 originals
/// are unlinked, the newly created Vault zip, manifest, and DB row MUST be deleted, and Unknown
/// MUST populate `reasons` and `stopped_mid_run_tool`.
#[test]
fn item2_unknown_or_tool_started_mid_archive_deletes_orphan_vault_and_reports_unknown() {
    for second_status in [
        BusyToolsStatus::unknown("cannot read /proc"),
        BusyToolsStatus::known([DevTool::Pip].into_iter().collect()),
    ] {
        let is_unknown = second_status.is_unknown();
        let dir = tempdir().unwrap();
        let now = SystemTime::now();
        let home = dir.path().join("home_sim");
        let pip_root = home.join(".cache/pip");
        let pip_file = pip_root.join("http-v2/ab/cd/cached_response.txt");
        write_aged(&pip_file, &vec![b'P'; 512 * 1024], 10, now);

        let vault_dir = home.join("Vault");
        std::fs::create_dir_all(&vault_dir).unwrap();
        let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

        let units = scan_dev_cache_root(
            &pip_root,
            Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
            now,
            &BusyToolsStatus::idle(),
            &MockFileLockProbe::new(),
            &TaskControl::new(),
        );
        assert_eq!(units.len(), 1);

        // Probe sequence with <= 1s cache:
        // 1) Initial check: idle (seeds cache for TOCTOU)
        // 2) Post-archive fresh check (after archiving pip_file): Unknown or Pip busy!
        let probe = MockDevToolProbe::idle();
        probe.push_sequence([BusyToolsStatus::idle(), second_status]);

        let total = 10u64 * 1024 * 1024 * 1024;
        let free_before = (total as f64 * 0.15) as u64;
        let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
        quota.clean_ratio = 0.80;
        quota.target_ratio = 0.75;
        quota.auto_clean = true;
        quota.regenerable_cache_mode = RegenerableCacheMode::Vault;

        let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
        let out = enforce_drive_quota_units(
            &quota,
            units,
            &vault_dir,
            &db,
            &vol,
            &MockFileLockProbe::new(),
            &probe,
            &GuardRunOptions {
                now_sys: now,
                ..Default::default()
            },
            &TaskControl::new(),
        )
        .unwrap();

        assert!(pip_file.exists(), "original pip file must remain untouched");
        assert!(
            out.vault_zip.is_none(),
            "orphan vault_zip must be cleared when 0 originals were deleted"
        );
        assert_eq!(out.vault_bytes, 0, "vault_bytes must be 0");
        assert!(
            db.list_vault_archives(None).unwrap().is_empty(),
            "orphan DB vault_archives row must be deleted"
        );
        let remaining_zips: Vec<_> = std::fs::read_dir(&vault_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("zip"))
            .collect();
        assert!(
            remaining_zips.is_empty(),
            "orphan .zip file must be removed from disk"
        );
        assert!(
            out.stopped_mid_run_tool.is_some(),
            "stopped_mid_run_tool must be populated"
        );
        if is_unknown {
            assert!(out.busy_tools_unknown, "busy_tools_unknown must be true");
            assert!(
                out.reasons.iter().any(|r| {
                    r.to_lowercase().contains("không xác định")
                        || r.to_lowercase().contains("unknown")
                }),
                "reasons must explain Unknown stop, got: {:?}",
                out.reasons
            );
        } else {
            assert_eq!(out.stopped_mid_run_tool.as_deref(), Some("pip"));
        }
    }
}

/// Item 2 (partial unlink): when a non-dev file IS unlinked while a dev-cache file is preserved
/// because its tool started during archiving, the Vault ZIP, manifest, and DB MUST contain ONLY
/// the removed file — never the still-present dev-cache file!
#[test]
fn item2_partial_unlink_keeps_only_removed_units_in_vault_zip_manifest_and_db() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");

    let pip_root = home.join(".cache/pip");
    let pip_file = pip_root.join("http-v2/ab/cd/cached_response.txt");
    write_aged(&pip_file, &vec![b'P'; 256 * 1024], 10, now);

    let state_dir = home.join(".local/state");
    let log_file = state_dir.join("svc/old.log");
    let log_bytes = vec![b'L'; 256 * 1024];
    write_aged(&log_file, &log_bytes, 10, now);

    let mut units = scan_dev_cache_root(
        &pip_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    let log_profile = PresetProfiles::get_by_id("app_logs").unwrap();
    units.extend(PresetProfiles::scan_profile_dirs_as_units(
        &log_profile,
        &[state_dir],
        now,
        &TaskControl::new(),
        &MockFileLockProbe::new(),
        &BusyToolsStatus::idle(),
    ));
    assert_eq!(units.len(), 2);

    // 1) Initial probe: idle
    // 2) Post-archive probe: Pip started! -> pip_file preserved, log_file deleted!
    let probe = MockDevToolProbe::idle();
    probe.push_sequence([
        BusyToolsStatus::idle(),
        BusyToolsStatus::known([DevTool::Pip].into_iter().collect()),
    ]);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Vault;

    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let out = enforce_drive_quota_units(
        &quota,
        units,
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &probe,
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    assert!(
        pip_file.exists(),
        "pip file must survive when pip starts mid-archive"
    );
    assert!(!log_file.exists(), "non-dev log file must be unlinked");
    assert_eq!(out.cleaned_units, 1);
    assert_eq!(out.cleaned_files, 1);
    let zip_path = out
        .vault_zip
        .expect("partial Vault archive must exist for log_file");

    // DB must list ONLY log_file (not pip_file)!
    let archives = db.list_vault_archives(None).unwrap();
    assert_eq!(archives.len(), 1);
    assert_eq!(archives[0].file_count, 1);
    let arch_id = archives[0].id.unwrap();
    let vfiles = db.get_vault_files(arch_id).unwrap();
    assert_eq!(vfiles.len(), 1);
    assert_eq!(vfiles[0].original_path, log_file);

    // Filtered ZIP itself must contain ONLY 1 entry (not pip_file)!
    let zf = std::fs::File::open(&zip_path).unwrap();
    let za = zip::ZipArchive::new(zf).unwrap();
    assert_eq!(za.len(), 1, "filtered ZIP must only contain removed files");

    // And RollbackEngine must restore log_file with verified SHA-256!
    let rollback = RollbackEngine::new(
        &db,
        RestoreOptions {
            vault_dir: Some(vault_dir.clone()),
            ..Default::default()
        },
    );
    let res = rollback
        .rollback_archive(arch_id, "test partial restore", false)
        .unwrap();
    assert_eq!(res.restored_count, 1);
    assert_eq!(std::fs::read(&log_file).unwrap(), log_bytes);
}

/// Item 3: Cooldown rules:
/// - Net freed >= 5% of bytes_needed (partial progress) -> fixed 30m cooldown, NEVER escalates to 2h/12h.
/// - Deferred because tool is busy -> fixed 30m cooldown, NEVER escalates `consecutive_ineffective`.
#[test]
fn item3_partial_progress_and_busy_deferral_do_not_escalate_cooldown() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    std::fs::create_dir_all(&home).unwrap();
    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

    // 10 GiB drive, 80.10% used, target 80.00% -> needs ~10.7 MiB; 5% = ~550 KiB.
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = total - (total as f64 * 0.8010) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.8000;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

    let t0 = Utc::now();

    // Part A: Deferred because Gradle is busy across 3 consecutive runs -> must stay 30m, NOT 2h/12h!
    let busy_gradle = MockDevToolProbe::with_busy([DevTool::Gradle]);
    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    for i in 0..3 {
        let ti = t0 + ChronoDuration::minutes(31 * i);
        let r = enforce_drive_quota_units(
            &quota,
            vec![],
            &vault_dir,
            &db,
            &vol,
            &MockFileLockProbe::new(),
            &busy_gradle,
            &GuardRunOptions {
                now_utc: ti,
                now_sys: now,
                ..Default::default()
            },
            &TaskControl::new(),
        )
        .unwrap();
        assert_eq!(
            r.cooldown_until,
            Some(ti + ChronoDuration::minutes(30)),
            "busy deferral run {} must retry in 30m, not escalate",
            i + 1
        );
        assert_eq!(
            r.consecutive_ineffective, 0,
            "busy deferral must not escalate consecutive_ineffective"
        );
    }

    // Part B: Partial progress (>= 5% freed, ~2 MiB of ~10.7 MiB needed) across 2 runs -> fixed 30m!
    let cargo_root = home.join(".cargo/registry/cache");
    let c1 = cargo_root.join("idx/a-1.0.0.crate");
    let c2 = cargo_root.join("idx/b-1.0.0.crate");
    write_aged(&c1, &vec![1u8; 2 * 1024 * 1024], 10, now);
    let vol_p1 = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let units1 = scan_dev_cache_root(
        &cargo_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    let tp1 = t0 + ChronoDuration::hours(5);
    let rp1 = enforce_drive_quota_units(
        &quota,
        units1,
        &vault_dir,
        &db,
        &vol_p1,
        &MockFileLockProbe::new(),
        &MockDevToolProbe::idle(),
        &GuardRunOptions {
            now_utc: tp1,
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    assert!(!rp1.target_reached);
    assert_eq!(rp1.cooldown_until, Some(tp1 + ChronoDuration::minutes(30)));
    assert_eq!(rp1.consecutive_ineffective, 0);

    write_aged(&c2, &vec![2u8; 2 * 1024 * 1024], 10, now);
    let vol_p2 = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let units2 = scan_dev_cache_root(
        &cargo_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    let tp2 = tp1 + ChronoDuration::minutes(31);
    let rp2 = enforce_drive_quota_units(
        &quota,
        units2,
        &vault_dir,
        &db,
        &vol_p2,
        &MockFileLockProbe::new(),
        &MockDevToolProbe::idle(),
        &GuardRunOptions {
            now_utc: tp2,
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    assert!(!rp2.target_reached);
    assert_eq!(
        rp2.cooldown_until,
        Some(tp2 + ChronoDuration::minutes(30)),
        "second run with >= 5% progress MUST stay at 30m, NOT escalate to 2h"
    );
    assert_eq!(rp2.consecutive_ineffective, 0);
}

/// Item 4: When a filesystem is mounted with `noatime` (detected from mount options), active
/// Gradle version transforms MUST NOT be selected (even if mtime is old), and other dev caches
/// MUST enforce the conservative >= 30-day threshold without parsing Gradle's binary journal.
#[test]
fn item4_noatime_protects_active_gradle_version_and_enforces_30d_threshold() {
    let mounts = "/dev/sda1 / ext4 rw,relatime 0 0\n/dev/sda2 /home ext4 rw,noatime 0 0\n";
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/home/u/.gradle/caches"), mounts),
        AtimeReliability::UnreliableNoAtime
    );
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/var/cache"), mounts),
        AtimeReliability::Reliable
    );

    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let gradle_root = dir.path().join(".gradle/caches");

    // Active Gradle 8.11.1 transforms: 15d old AND 45d old -> BOTH protected on `noatime`!
    write_aged(
        &gradle_root.join("8.11.1/transforms/daily_used_15d/classes.jar"),
        &vec![1u8; 4096],
        15,
        now,
    );
    backdate_dir(
        &gradle_root.join("8.11.1/transforms/daily_used_15d"),
        15,
        now,
    );
    write_aged(
        &gradle_root.join("8.11.1/transforms/daily_used_45d/classes.jar"),
        &vec![2u8; 4096],
        45,
        now,
    );
    backdate_dir(
        &gradle_root.join("8.11.1/transforms/daily_used_45d"),
        45,
        now,
    );

    // Obsolete Gradle 7.6 (15d old < 30d) -> protected on `noatime`!
    write_aged(
        &gradle_root.join("7.6/transforms/old15/lib.jar"),
        &vec![3u8; 4096],
        15,
        now,
    );
    backdate_dir(&gradle_root.join("7.6/transforms/old15"), 15, now);
    backdate_dir(&gradle_root.join("7.6/transforms"), 15, now);
    backdate_dir(&gradle_root.join("7.6"), 15, now);

    // Obsolete Gradle 7.5 (35d old >= 30d) -> eligible even on `noatime`!
    write_aged(
        &gradle_root.join("7.5/transforms/old35/lib.jar"),
        &vec![4u8; 4096],
        35,
        now,
    );
    backdate_dir(&gradle_root.join("7.5/transforms/old35"), 35, now);
    backdate_dir(&gradle_root.join("7.5/transforms"), 35, now);
    backdate_dir(&gradle_root.join("7.5"), 35, now);

    let noatime_units = scan_dev_cache_root_with_atime_reliability(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
        AtimeReliability::UnreliableNoAtime,
    );
    assert_eq!(
        noatime_units.len(),
        1,
        "only obsolete >= 30d Gradle 7.5 should be selected on noatime; got: {:?}",
        noatime_units
            .iter()
            .map(|u| &u.root_path)
            .collect::<Vec<_>>()
    );
    assert!(noatime_units[0].root_path.ends_with("7.5"));

    // Contrast: on reliable atime (relatime), 15d & 45d active transforms + 7.6 + 7.5 are all eligible.
    let reliable_units = scan_dev_cache_root_with_atime_reliability(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
        AtimeReliability::Reliable,
    );
    assert_eq!(reliable_units.len(), 4);

    // Cargo crate under `noatime`: 15d skipped, 35d selected!
    let cargo_root = dir.path().join(".cargo/registry/cache");
    write_aged(&cargo_root.join("idx/mid-1.0.0.crate"), b"15d", 15, now);
    write_aged(&cargo_root.join("idx/old-1.0.0.crate"), b"35d", 35, now);
    let cargo_noatime = scan_dev_cache_root_with_atime_reliability(
        &cargo_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
        AtimeReliability::UnreliableNoAtime,
    );
    assert_eq!(cargo_noatime.len(), 1);
    assert!(cargo_noatime[0].root_path.ends_with("old-1.0.0.crate"));
}

/// Item 5: Busy-tool status must be cached (<= 1s) for bulk file units instead of scanning `/proc`
/// once per unit, while directory units STILL run a fresh live check right before atomic rename!
#[test]
fn item5_busy_tools_probe_is_cached_for_bulk_file_units() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let cargo_root = home.join(".cargo/registry/cache");
    for i in 0..50 {
        let p = cargo_root.join(format!("idx/crate_{i:03}-1.0.0.crate"));
        write_aged(&p, &vec![b'X'; 8 * 1024], 10, now);
    }

    let units = scan_dev_cache_root(
        &cargo_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 50);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

    let probe = CountingDevToolProbe::idle();
    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let out = enforce_drive_quota_units(
        &quota,
        units,
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &probe,
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    assert_eq!(out.cleaned_units, 50);
    assert!(
        probe.call_count() <= 3,
        "expected cached busy-tool checks (<= 3 calls) for 50 file units, got {} calls",
        probe.call_count()
    );
}

/// Item 5b: Directory units MUST still run a fresh live busy-tool check right before atomic rename
/// even when within the 1-second cache window!
#[test]
fn item5_directory_unit_checks_busy_tools_fresh_right_before_rename() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let gradle_root = home.join(".gradle/caches");
    let d1 = gradle_root.join("8.11.1/transforms/dir_1");
    let d2 = gradle_root.join("8.11.1/transforms/dir_2");
    write_aged(&d1.join("metadata.bin"), &vec![1u8; 32 * 1024], 15, now);
    write_aged(&d1.join("a.jar"), &vec![1u8; 32 * 1024], 15, now);
    write_aged(&d2.join("metadata.bin"), &vec![2u8; 32 * 1024], 10, now);
    write_aged(&d2.join("b.jar"), &vec![2u8; 32 * 1024], 10, now);
    backdate_dir(&d1, 15, now);
    backdate_dir(&d2, 10, now);

    let units = scan_dev_cache_root(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 2);

    // 1) Initial check: idle
    // 2) Fresh check right before renaming dir_1: idle -> dir_1 removed
    // 3) Fresh check right before renaming dir_2: Gradle BUSY -> stops immediately, dir_2 survives!
    let mock = MockDevToolProbe::idle();
    mock.push_sequence([
        BusyToolsStatus::idle(),
        BusyToolsStatus::idle(),
        BusyToolsStatus::known([DevTool::Gradle].into_iter().collect()),
    ]);
    let probe = CountingDevToolProbe::from_mock(mock);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Vault;

    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let out = enforce_drive_quota_units(
        &quota,
        units,
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &probe,
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    assert_eq!(
        probe.call_count(),
        3,
        "initial + 1 fresh probe per dir rename attempt"
    );
    assert_eq!(out.cleaned_units, 1);
    assert!(!d1.exists());
    assert!(d2.exists());
    assert_eq!(out.stopped_mid_run_tool.as_deref(), Some("gradle"));
}

/// Item 5c: Verify Vault mode scales linearly (O(n) HashMap) on hundreds of files without O(n^2)
/// stall and without re-probing `/proc` per file.
#[test]
fn item5_vault_mode_scales_linearly_with_hashmap_and_cached_probe() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let pip_root = home.join(".cache/pip");
    for i in 0..300 {
        let p = pip_root.join(format!(
            "http-v2/{:02}/{:02}/resp_{i:04}.txt",
            i % 10,
            (i / 10) % 10
        ));
        write_aged(
            &p,
            b"compressible_pip_response_payload_repeat_repeat",
            10,
            now,
        );
    }

    let units = scan_dev_cache_root(
        &pip_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 300);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Vault;

    let probe = CountingDevToolProbe::idle();
    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let t0 = Instant::now();
    let out = enforce_drive_quota_units(
        &quota,
        units,
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &probe,
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    let elapsed = t0.elapsed();

    assert_eq!(out.cleaned_units, 300);
    assert_eq!(out.cleaned_files, 300);
    // Initial probe + 1 post-archive fresh probe = 2 calls (not 302!).
    assert!(
        probe.call_count() <= 4,
        "expected <= 4 busy-tool probes for 300 Vault file units, got {}",
        probe.call_count()
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "300-file Vault run took too long: {elapsed:?}"
    );
}

/// Item 6: Trees deeper than 32 levels must NOT be half-deleted by `break` in delete or silently
/// truncated in `compute_unit_latest_used`, while preserving no-symlink-follow safety.
#[test]
fn item6_deep_tree_over_32_levels_is_not_truncated_or_half_deleted() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let gradle_root = home.join(".gradle/caches");
    let unit_root = gradle_root.join("8.11.1/transforms/deep_hash");

    // Build a 38-level nested directory tree with a leaf file at depth 38
    let mut deep = unit_root.clone();
    for i in 0..38 {
        deep = deep.join(format!("d{i}"));
    }
    let leaf = deep.join("leaf.bin");
    write_aged(&leaf, &vec![b'D'; 4096], 10, now);

    // Backdate every dir in the chain
    let mut cur = deep.as_path();
    while cur != gradle_root.as_path() {
        backdate_dir(cur, 10, now);
        cur = cur.parent().unwrap();
    }

    // 1. Scan rejects trees > 32 levels (never half-scanned)
    let scanned = scan_dev_cache_root(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert!(
        scanned.is_empty(),
        "scan must reject directory trees deeper than 32 levels"
    );

    // 2. `compute_unit_latest_used` must NOT silently `break` at 32 and return a stale timestamp!
    std::fs::write(&leaf, b"freshly written at depth 38").unwrap();
    let latest = compute_unit_latest_used(&unit_root, now);
    if let Some(ts) = latest {
        let age = now.duration_since(ts).unwrap_or(Duration::ZERO);
        assert!(
            age < Duration::from_secs(60),
            "compute_unit_latest_used must not ignore fresh files past depth 32 (got age {:?})",
            age
        );
    }

    // 3. Even if a >32-level unit is handed directly to enforce_drive_quota_units (with min_age=0),
    // deletion MUST remove the ENTIRE tree (0 remnants), never leave half behind!
    write_aged(&leaf, &vec![b'D'; 4096], 10, now);
    let mut cur2 = deep.as_path();
    while cur2 != gradle_root.as_path() {
        backdate_dir(cur2, 10, now);
        cur2 = cur2.parent().unwrap();
    }
    let synthetic_unit = CleanUnit {
        root_path: unit_root.clone(),
        allowed_parent: gradle_root.clone(),
        files: vec![CleanCandidate {
            path: leaf.clone(),
            size_bytes: 4096,
            mtime: now - Duration::from_secs(10 * 86_400),
            is_locked: false,
            profile_id: "package_managers".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: vec![leaf.clone()],
        }],
        total_bytes: 4096,
        last_used: now - Duration::from_secs(10 * 86_400),
        profile_id: "package_managers".to_string(),
        kind: CleanUnitKind::RegenerableDevCache,
        dev_tool: Some(DevTool::Gradle),
        unit_label: "gradle_transform".to_string(),
        root_identity: query_file_identity(&unit_root),
        min_age: Duration::ZERO,
    };

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    let _ = enforce_drive_quota_units(
        &quota,
        vec![synthetic_unit],
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &MockDevToolProbe::idle(),
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    assert!(
        !unit_root.exists(),
        "deep directory unit (>32 levels) must be deleted completely with zero remnants!"
    );
}

/// Item 7: Verify `packaging/linux/greenc-disk-guard.sh` handles `@reboot`, missing CLI,
/// failing `ps`, high-volume `ps` (no `ps | awk` SIGPIPE), unwritable lockfile, and exit codes
/// without EVER losing its log line. Runs strictly inside an isolated `tempdir` (never touches real HOME).
#[cfg(target_os = "linux")]
#[test]
fn item7_disk_guard_shell_script_handles_reboot_sigpipe_and_early_errors() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/linux/greenc-disk-guard.sh")
        .canonicalize()
        .unwrap();

    // 1. Verify no raw `ps ... | awk` pipe remains in the script.
    let src = std::fs::read_to_string(&script).unwrap();
    assert!(
        !src.contains("ps -u \"$(id -u)\" -o pid=,comm=,args= 2>/dev/null | awk"),
        "script must not use a raw `ps | awk` pipe susceptible to SIGPIPE"
    );

    // 2. Normal `--reboot` run with fake CLI + fake high-volume `ps` (5000 lines + GradleDaemon):
    let dir = tempdir().unwrap();
    let fake_home = dir.path().join("fake_home");
    let fake_bin = fake_home.join(".local/bin");
    let log_dir = fake_home.join(".local/share/greenc");
    let log_file = log_dir.join("guardian.log");
    let lock_file = dir.path().join("guard.lock");
    let args_capture = dir.path().join("cli_args.txt");
    std::fs::create_dir_all(&fake_bin).unwrap();

    let fake_cli = fake_bin.join("greenc-cli");
    std::fs::write(
        &fake_cli,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> {:?}\nif [[ \"$1\" == \"vault\" ]]; then\n  echo '{{\"purged_archives\":0,\"freed_bytes\":0}}'\n  exit 0\nfi\necho '{{\"drives\":[{{\"drive_root\":\"/home\",\"usage_after_ratio\":0.74,\"status\":\"ok\",\"logical_bytes\":0,\"vault_bytes\":0,\"net_freed_bytes\":0,\"target_reached\":true,\"reasons\":[]}}]}}'\n",
            args_capture
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake_cli, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Fake `ps` outputting 5000 lines + GradleDaemon to prove no SIGPIPE & busy detection works!
    let fake_ps = fake_bin.join("ps");
    std::fs::write(
        &fake_ps,
        "#!/usr/bin/env bash\nfor i in $(seq 1 5000); do echo \"$i bash /bin/bash\"; done\necho \"99999 java /usr/bin/java org.gradle.launcher.daemon.bootstrap.GradleDaemon\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake_ps, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new("bash")
        .arg(&script)
        .arg("--reboot")
        .env("HOME", &fake_home)
        .env("GREENC_CLI", &fake_cli)
        .env("GREENC_LOG_DIR", &log_dir)
        .env("GREENC_LOG_FILE", &log_file)
        .env("GREENC_LOCK_FILE", &lock_file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "reboot script failed: status={:?}, stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let log_text = std::fs::read_to_string(&log_file).unwrap();
    assert!(
        log_text.contains("REBOOT: greenc-disk-guard started"),
        "log={log_text}"
    );
    assert!(
        log_text.contains("GUARD: /home=74.0% status=ok"),
        "log={log_text}"
    );
    let captured = std::fs::read_to_string(&args_capture).unwrap();
    assert!(
        captured.contains("--extra-busy-tools gradle"),
        "expected gradle passed via --extra-busy-tools, got: {captured}"
    );

    // 3. Failing `ps` (exit 1) + unwritable lock file -> MUST log warnings AND still log GUARD!
    std::fs::write(&fake_ps, "#!/usr/bin/env bash\nexit 1\n").unwrap();
    let log_file2 = dir.path().join("guardian2.log");
    let bad_lock = dir.path().join("nonexistent_dir/locked.lock");
    let out2 = Command::new("bash")
        .arg(&script)
        .arg("--reboot")
        .env("HOME", &fake_home)
        .env("GREENC_CLI", &fake_cli)
        .env("GREENC_LOG_DIR", &log_dir)
        .env("GREENC_LOG_FILE", &log_file2)
        .env("GREENC_LOCK_FILE", &bad_lock)
        .output()
        .unwrap();
    assert!(out2.status.success());
    let log_text2 = std::fs::read_to_string(&log_file2).unwrap();
    assert!(
        log_text2.contains("WARN: cannot open lock file"),
        "log2={log_text2}"
    );
    assert!(
        log_text2.contains("WARN: ps process check failed"),
        "log2={log_text2}"
    );
    assert!(
        log_text2.contains("REBOOT: greenc-disk-guard started"),
        "log2={log_text2}"
    );
    assert!(
        log_text2.contains("GUARD: /home=74.0% status=ok"),
        "log2={log_text2}"
    );

    // 4. Failing `greenc-cli` (exit 2) -> MUST log ERROR and preserve exit code 2!
    let fail_cli = fake_bin.join("fail-cli");
    std::fs::write(
        &fail_cli,
        "#!/usr/bin/env bash\nif [[ \"$1\" == \"vault\" ]]; then exit 0; fi\necho 'simulated failure' >&2\nexit 2\n",
    )
    .unwrap();
    std::fs::set_permissions(&fail_cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log_file3 = dir.path().join("guardian3.log");
    let out3 = Command::new("bash")
        .arg(&script)
        .arg("--reboot")
        .env("HOME", &fake_home)
        .env("GREENC_CLI", &fail_cli)
        .env("GREENC_LOG_DIR", &log_dir)
        .env("GREENC_LOG_FILE", &log_file3)
        .env("GREENC_LOCK_FILE", &lock_file)
        .output()
        .unwrap();
    assert_eq!(out3.status.code(), Some(2));
    let log_text3 = std::fs::read_to_string(&log_file3).unwrap();
    assert!(
        log_text3.contains("ERROR: greenc-cli guard run failed (exit=2)"),
        "log3={log_text3}"
    );
}
