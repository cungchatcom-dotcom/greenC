//! Incident-reproduction and invariant tests for `greenc-vault::guard` and `greenc-vault::suggest`
//! (`GC-GUARD` Items 3, 4, 5, 6, 8 + TOCTOU safety).

use chrono::{Duration as ChronoDuration, Utc};
use greenc_core::error::Result;
use greenc_core::models::{DriveInfo, DriveQuota, RegenerableCacheMode};
use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool, MockDevToolProbe};
use greenc_platform::traits::{MockFileLockProbe, Win32VolumeApi};
use greenc_storage::db::GreenCDb;
use greenc_vault::cache_units::{
    scan_dev_cache_root, CleanUnit, CleanUnitKind, DEFAULT_DEV_CACHE_MIN_AGE_SECS,
};
use greenc_vault::guard::{
    cooldown_duration_for_failures, enforce_drive_quota_units, GuardRunOptions,
};
use greenc_vault::profiles::PresetProfiles;
use greenc_vault::suggest::{suggest_large_build_outputs, SuggestOptions};
use std::fs::FileTimes;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tempfile::tempdir;

/// Volume API backed by actual directory size deltas inside a tempdir so `free_after - free_before`
/// faithfully reflects both unlinked source bytes AND same-drive Vault archive bytes!
#[derive(Clone)]
struct DirAccountingVolumeApi {
    root: PathBuf,
    total_bytes: u64,
    base_free_bytes: u64,
    initial_tree_bytes: Arc<Mutex<Option<u64>>>,
}

impl DirAccountingVolumeApi {
    fn new(root: PathBuf, total_bytes: u64, base_free_bytes: u64) -> Self {
        let initial = dir_bytes(&root);
        Self {
            root,
            total_bytes,
            base_free_bytes,
            initial_tree_bytes: Arc::new(Mutex::new(Some(initial))),
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
        let init = self.initial_tree_bytes.lock().unwrap().unwrap_or(0);
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
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES + FILE_FLAG_BACKUP_SEMANTICS: required to set times on a directory.
        let _ = std::fs::OpenOptions::new()
            .access_mode(0x0100)
            .share_mode(7)
            .custom_flags(0x0200_0000)
            .open(path)
            .and_then(|f| f.set_times(FileTimes::new().set_modified(ts).set_accessed(ts)));
    }
}

#[test]
fn same_drive_vault_reports_accurate_net_freed_and_target_reached() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

    // Place a 2 MiB compressible log file inside the protected root (so Vault on same drive shrinks it ~99%!).
    let state_dir = dir.path().join("home_sim").join(".local/state");
    let log_file = state_dir.join("app/big.log");
    write_aged(&log_file, &vec![b'A'; 2 * 1024 * 1024], 10, now);

    let vault_dir = dir.path().join("home_sim").join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();

    // Drive budget: 20 MiB total, 3 MiB free (85% used >= 80% clean, target 75% => need 2 MiB?
    // Set 3.2 MiB free (84% used, need 1.8 MiB to reach 75% = 15 MiB used / 5 MiB free).
    // Wait: reserve check requires >= 512 MiB free for ArchiverPipeline!
    // Use 10 GiB total, 1.999 GiB free -> need ~0.501 GiB? No, set limit_bytes = 20 MiB on a 10 GiB drive!
    // No: `effective_limit` vs `total_bytes`: DriveQuota::usage_ratio = (total - free) / limit.
    // Easiest: total = 10 GiB, clean_ratio = 0.80, target_ratio = 0.7999 -> need ~1 MiB!
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = total - (total as f64 * 0.8000) as u64; // 80.00% used, ~2 GiB free (> 512 MiB reserve!)
    let mut quota = DriveQuota::new(dir.path().join("home_sim").to_string_lossy().to_string());
    quota.warn_ratio = 0.70;
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.7999; // needs ~1.07 MiB freed to reach target
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Vault;

    let vol = DirAccountingVolumeApi::new(dir.path().join("home_sim"), total, free_before);
    let profile = PresetProfiles::get_by_id("app_logs").unwrap();
    let units = PresetProfiles::scan_profile_dirs_as_units(
        &profile,
        &[state_dir],
        now,
        &TaskControl::new(),
        &MockFileLockProbe::new(),
        &BusyToolsStatus::idle(),
    );
    assert_eq!(units.len(), 1);

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

    assert!(out.vault_same_drive);
    assert_eq!(out.logical_bytes, 2 * 1024 * 1024);
    assert!(out.vault_bytes > 0 && out.vault_bytes < out.logical_bytes);
    // Net freed must closely equal logical_bytes - vault_bytes!
    let expected_net = out.logical_bytes as i64 - out.vault_bytes as i64;
    assert_eq!(out.net_freed_bytes, expected_net);
    assert!(out.target_reached);
    assert_eq!(out.status, "cleaned");
    assert!(!log_file.exists());
}

#[test]
fn direct_mode_deletes_only_regenerable_dev_cache_and_vaults_non_regenerable_data() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

    // 1. Regenerable Gradle transform (10 days old, 2 MiB)
    let gradle_root = home.join(".gradle/caches");
    let old_t = gradle_root.join("8.11.1/transforms/old_t1/classes.jar");
    write_aged(&old_t, &vec![0x5Au8; 2 * 1024 * 1024], 10, now);
    backdate_dir(&gradle_root.join("8.11.1/transforms/old_t1"), 10, now);

    // 2. Non-regenerable app log (10 days old, 2 MiB)
    let state_dir = home.join(".local/state");
    let log_file = state_dir.join("svc/app.log");
    write_aged(&log_file, &vec![b'L'; 2 * 1024 * 1024], 10, now);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();

    let mut units = scan_dev_cache_root(
        &gradle_root,
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

    // Require 3 MiB freed so BOTH units are selected (dev cache 2 MiB + log 2 MiB).
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = total - (total as f64 * 0.8005) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.warn_ratio = 0.70;
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.8002; // needs ~3.2 MiB freed
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

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

    // Gradle transform MUST be direct-deleted with DB audit log, NOT archived in Vault!
    assert!(!old_t.exists());
    assert_eq!(out.direct_deleted_bytes, 2 * 1024 * 1024);
    let direct_logs = db.list_direct_deletes(Some(&quota.drive_root), 10).unwrap();
    assert_eq!(direct_logs.len(), 1);
    assert_eq!(direct_logs[0].unit_kind, "gradle_transform");
    assert_eq!(direct_logs[0].logical_bytes, 2 * 1024 * 1024);

    // Non-regenerable log MUST ALWAYS go through Vault even in `direct` mode!
    assert!(!log_file.exists());
    assert!(out.vault_zip.is_some());
    assert!(out.vault_bytes > 0);
    let archives = db.list_vault_archives(None).unwrap();
    assert_eq!(archives.len(), 1);
    let vault_files = db.get_vault_files(archives[0].id.unwrap()).unwrap();
    assert_eq!(vault_files.len(), 1);
    assert_eq!(vault_files[0].original_path, log_file);
}

#[test]
fn ineffective_cleanup_escalates_cooldown_30m_2h_12h_and_skips_between_runs() {
    assert_eq!(
        cooldown_duration_for_failures(1),
        ChronoDuration::minutes(30)
    );
    assert_eq!(cooldown_duration_for_failures(2), ChronoDuration::hours(2));
    assert_eq!(cooldown_duration_for_failures(3), ChronoDuration::hours(12));
    assert_eq!(cooldown_duration_for_failures(5), ChronoDuration::hours(12));

    let dir = tempdir().unwrap();
    let home = dir.path().join("home_sim");
    std::fs::create_dir_all(&home).unwrap();
    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

    let total = 70u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.19) as u64; // 81% used -> needs ~4.2 GB
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.warn_ratio = 0.70;
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;

    let vol = DirAccountingVolumeApi::new(home.clone(), total, free_before);
    // Tools are idle, so 0 bytes freed (< 5% of needed) is genuinely ineffective and must escalate
    // 30m -> 2h -> 12h. (Busy-tool deferrals are tested separately to stay at fixed 30m.)
    let idle_probe = MockDevToolProbe::idle();
    let t0 = Utc::now();

    // Run 1 at t0: tools idle, 0 freed -> target not reached -> 30m cooldown
    let r1 = enforce_drive_quota_units(
        &quota,
        vec![],
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &idle_probe,
        &GuardRunOptions {
            now_utc: t0,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    assert!(!r1.target_reached);
    assert_eq!(r1.consecutive_ineffective, 1);
    assert_eq!(r1.cooldown_until, Some(t0 + ChronoDuration::minutes(30)));
    assert!(!r1.reasons.is_empty());

    // Run after 15 minutes (cron cadence!): MUST be skipped by cooldown!
    let r_15m = enforce_drive_quota_units(
        &quota,
        vec![],
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &idle_probe,
        &GuardRunOptions {
            now_utc: t0 + ChronoDuration::minutes(15),
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    assert_eq!(r_15m.status, "skipped_cooldown");
    assert!(r_15m.cooldown_active);

    // Run 2 after 31 minutes: still not reaching target -> escalates to 2 hours
    let t2 = t0 + ChronoDuration::minutes(31);
    let r2 = enforce_drive_quota_units(
        &quota,
        vec![],
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &idle_probe,
        &GuardRunOptions {
            now_utc: t2,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    assert_eq!(r2.consecutive_ineffective, 2);
    assert_eq!(r2.cooldown_until, Some(t2 + ChronoDuration::hours(2)));

    // Run 3 after 2h + 1m: escalates to 12 hours
    let t3 = t2 + ChronoDuration::hours(2) + ChronoDuration::minutes(1);
    let r3 = enforce_drive_quota_units(
        &quota,
        vec![],
        &vault_dir,
        &db,
        &vol,
        &MockFileLockProbe::new(),
        &idle_probe,
        &GuardRunOptions {
            now_utc: t3,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();
    assert_eq!(r3.consecutive_ineffective, 3);
    assert_eq!(r3.cooldown_until, Some(t3 + ChronoDuration::hours(12)));
}

#[test]
fn toctou_stops_immediately_when_tool_starts_mid_run() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let gradle_root = home.join(".gradle/caches");
    let t1 = gradle_root.join("8.11.1/transforms/old_t1/a.jar");
    let t2 = gradle_root.join("8.11.1/transforms/old_t2/b.jar");
    write_aged(&t1, &vec![1u8; 1024 * 1024], 15, now);
    write_aged(&t2, &vec![2u8; 1024 * 1024], 10, now);
    backdate_dir(&gradle_root.join("8.11.1/transforms/old_t1"), 15, now);
    backdate_dir(&gradle_root.join("8.11.1/transforms/old_t2"), 10, now);

    let units = scan_dev_cache_root(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 2);

    // Probe returns:
    // 1) Initial check: idle
    // 2) TOCTOU before unit 1: idle -> deletes unit 1
    // 3) TOCTOU before unit 2: Gradle BUSY -> stops immediately, unit 2 MUST survive!
    let probe = MockDevToolProbe::idle();
    probe.push_sequence([
        BusyToolsStatus::idle(),
        BusyToolsStatus::idle(),
        BusyToolsStatus::known([DevTool::Gradle].into_iter().collect()),
    ]);

    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

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

    assert_eq!(out.status, "stopped_tool_started");
    assert_eq!(out.stopped_mid_run_tool.as_deref(), Some("gradle"));
    assert_eq!(out.cleaned_units, 1);
    assert!(!t1.exists(), "oldest unit 1 deleted before Gradle started");
    assert!(
        t2.exists(),
        "unit 2 MUST survive when Gradle starts mid-run"
    );
}

#[test]
fn drive_isolation_refuses_units_from_another_drive() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let vault_dir = dir.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();

    // Synthetic cross-drive unit on D:\ while protecting C:\
    let foreign_unit = CleanUnit {
        root_path: PathBuf::from(r"D:\.gradle\caches\8.11.1\transforms\abc"),
        allowed_parent: PathBuf::from(r"D:\.gradle\caches"),
        files: vec![],
        total_bytes: 10 * 1024 * 1024,
        last_used: now - Duration::from_secs(10 * 86_400),
        profile_id: "package_managers".to_string(),
        kind: CleanUnitKind::RegenerableDevCache,
        dev_tool: Some(DevTool::Gradle),
        unit_label: "gradle_transform".to_string(),
        root_identity: None,
        min_age: Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
    };

    let mut quota = DriveQuota::new("C:\\");
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

    let vol = DirAccountingVolumeApi::new(
        dir.path().to_path_buf(),
        100 * 1024 * 1024 * 1024,
        10 * 1024 * 1024 * 1024,
    );
    let out = enforce_drive_quota_units(
        &quota,
        vec![foreign_unit],
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
    assert_eq!(out.cleaned_units, 0);
    assert_eq!(out.logical_bytes, 0);
}

#[test]
fn guard_suggest_reports_stale_gitignored_build_outputs_read_only_and_skips_symlinks_and_busy() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();

    // Create a git worktree-style repo (`.git` as a file!) with `.gitignore` ignoring `build/` and `target/`.
    let repo = dir.path().join("projects/worktree_android");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join(".git"),
        "gitdir: /nonexistent/.git/worktrees/wt1\n",
    )
    .unwrap();
    std::fs::write(repo.join(".gitignore"), "build/\ntarget/\nnode_modules/\n").unwrap();

    // 5-day-old ignored `app/build` -> MUST be suggested
    let build_out = repo.join("app/build/outputs/app.apk");
    write_aged(&build_out, &vec![0xABu8; 64 * 1024], 5, now);
    backdate_dir(&repo.join("app/build/outputs"), 5, now);
    backdate_dir(&repo.join("app/build"), 5, now);

    // 1-day-old ignored `fresh/build` -> too new (< 3 days) -> MUST NOT be suggested
    let fresh_out = repo.join("fresh/build/out.o");
    write_aged(&fresh_out, &vec![0xCDu8; 32 * 1024], 1, now);

    // 5-day-old NOT-ignored `src_build` -> MUST NOT be suggested
    let tracked = repo.join("docs/tracked.txt");
    write_aged(&tracked, b"keep me", 10, now);

    // Vault dir with 1 archive
    let vault_dir = dir.path().join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    std::fs::write(vault_dir.join("vault_1.zip"), vec![0u8; 16 * 1024]).unwrap();

    // Symlink pointing outside -> MUST NOT be traversed
    #[cfg(unix)]
    {
        let outside = tempdir().unwrap();
        let outside_build = outside.path().join("build/huge.bin");
        write_aged(&outside_build, &vec![0xEEu8; 128 * 1024], 10, now);
        let _ = std::os::unix::fs::symlink(outside.path(), repo.join("linked_outside"));
    }

    let mut opts = SuggestOptions::with_defaults(vec![dir.path().to_path_buf()], vault_dir);
    opts.now = now;

    // 1. Idle tools -> suggests app/build and does NOT delete anything!
    let report = suggest_large_build_outputs(&opts, &MockDevToolProbe::idle(), &TaskControl::new());
    assert_eq!(report.suggestions.len(), 1, "got: {:?}", report.suggestions);
    assert_eq!(report.suggestions[0].path, repo.join("app/build"));
    assert_eq!(report.suggestions[0].size_bytes, 64 * 1024);
    assert!(report.suggestions[0].git_ignored);
    assert_eq!(report.vault.archive_count, 1);
    assert_eq!(report.vault.total_bytes, 16 * 1024);
    assert!(build_out.exists(), "suggest MUST NEVER delete files!");

    // 2. Busy Gradle -> skips Gradle `build` suggestion!
    let busy_report = suggest_large_build_outputs(
        &opts,
        &MockDevToolProbe::with_busy([DevTool::Gradle]),
        &TaskControl::new(),
    );
    assert!(busy_report.suggestions.is_empty());
}

#[test]
fn toctou_skips_unit_if_replaced_with_new_inode_or_touched_before_delete() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let gradle_root = home.join(".gradle/caches");
    let unit_dir = gradle_root.join("8.11.1/transforms/replaced_hash");
    let jar = unit_dir.join("lib.jar");
    write_aged(&jar, &vec![3u8; 256 * 1024], 10, now);
    backdate_dir(&unit_dir, 10, now);

    // Scan captures old dev/inode identity
    let units = scan_dev_cache_root(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 1);

    // Simulate another process replacing the directory (new inode) or touching it right before delete!
    std::fs::remove_dir_all(&unit_dir).unwrap();
    write_aged(&jar, &vec![4u8; 256 * 1024], 10, now);
    backdate_dir(&unit_dir, 10, now);
    // Also touch mtime to now on a sibling to guarantee fresh timestamp even if filesystem reuses inode
    let fresh_marker = unit_dir.join("fresh.bin");
    std::fs::write(&fresh_marker, b"active").unwrap();

    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();
    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.15) as u64;
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

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

    assert_eq!(out.cleaned_units, 0);
    assert!(
        jar.exists(),
        "unit modified/replaced after scan MUST be preserved"
    );
    assert!(fresh_marker.exists());
}

#[test]
fn same_drive_vault_skips_incompressible_archives_and_warns_with_cooldown() {
    let dir = tempdir().unwrap();
    let now = SystemTime::now();
    let home = dir.path().join("home_sim");
    let cargo_root = home.join(".cargo/registry/cache");
    let crate_file = cargo_root.join("index.crates.io-abc/huge-1.0.0.crate");
    write_aged(&crate_file, &vec![0x77u8; 2 * 1024 * 1024], 14, now);

    let vault_dir = home.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let db = GreenCDb::open(dir.path().join("db.sqlite")).unwrap();

    let units = scan_dev_cache_root(
        &cargo_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 1);

    let total = 70u64 * 1024 * 1024 * 1024;
    let free_before = (total as f64 * 0.19) as u64; // 81% used
    let mut quota = DriveQuota::new(home.to_string_lossy().to_string());
    quota.warn_ratio = 0.70;
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
        &MockDevToolProbe::idle(),
        &GuardRunOptions {
            now_sys: now,
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    assert!(out.vault_same_drive);
    assert_eq!(out.cleaned_units, 0);
    assert_eq!(out.net_freed_bytes, 0);
    assert!(!out.target_reached);
    assert_eq!(out.status, "ineffective");
    assert_eq!(out.consecutive_ineffective, 1);
    assert!(out.cooldown_until.is_some());
    assert!(
        out.reasons
            .iter()
            .any(|r| r.contains("Vault nằm cùng ổ") && r.contains("regenerable_cache_mode=direct")),
        "expected same-drive Vault Vietnamese reason, got: {:?}",
        out.reasons
    );
    assert!(
        crate_file.exists(),
        "incompressible crate must not be churned into same-drive Vault with zero net gain"
    );
}
