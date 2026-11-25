//! Security & crash-recovery TDD tests for GC-GUARD3 (Items 1-5).

use chrono::Utc;
use greenc_core::error::Result;
use greenc_core::models::{
    DriveInfo, DriveQuota, RegenerableCacheMode, VaultArchive, VaultFileItem, VaultStatus,
};
use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool, MockDevToolProbe};
use greenc_platform::handle::query_file_identity;
use greenc_platform::traits::{MockFileLockProbe, ProcessServiceProbe, Win32VolumeApi};
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::AppSettings;
use greenc_vault::cache_units::{
    scan_dev_cache_root, CleanUnitKind, DEFAULT_DEV_CACHE_MIN_AGE_SECS,
};
use greenc_vault::guard::{
    enforce_drive_quota_units, is_valid_staging_unit_id, recover_staging_dir,
    recover_staging_dir_with_options, resolve_staging_dir_checked, GuardRunOptions,
    StagingJournalEntry, StagingMode, StagingRecoveryOptions, StagingState,
};
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use std::fs::{File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

struct IdleProcessProbe;

impl ProcessServiceProbe for IdleProcessProbe {
    fn is_process_running(&self, _exe: &str) -> bool {
        false
    }
    fn is_service_running(&self, _svc: &str) -> Result<bool> {
        Ok(true)
    }
}

struct StaticVol(u64, u64);

impl Win32VolumeApi for StaticVol {
    fn list_drives(&self) -> Result<Vec<DriveInfo>> {
        Ok(Vec::new())
    }
    fn get_disk_free_space(&self, _root: &Path) -> Result<(u64, u64)> {
        Ok((self.0, self.1))
    }
    fn get_volume_info(&self, _root: &Path) -> Result<(String, String, u32)> {
        Ok(("Test".to_string(), "ext4".to_string(), 3))
    }
}

fn make_journal(
    id: &str,
    original_path: PathBuf,
    staged_path: PathBuf,
    mode: StagingMode,
    state: StagingState,
    drive_root: &Path,
    unit_kind: Option<CleanUnitKind>,
) -> StagingJournalEntry {
    let phys = query_file_identity(&staged_path);
    StagingJournalEntry {
        version: 2,
        id: id.to_string(),
        original_path,
        staged_path,
        mode,
        state,
        created_at: Utc::now(),
        drive_root: drive_root.to_string_lossy().to_string(),
        unit_label: "gradle_transform".to_string(),
        dev_tool: Some(DevTool::Gradle),
        note: None,
        staged_dev: phys.map(|p| p.volume_serial),
        staged_ino: phys.map(|p| p.file_index),
        unit_kind,
    }
}

#[test]
fn gc_guard3_item1_unit_id_regex_validator() {
    assert!(is_valid_staging_unit_id("unit_1700000000000_1234_1"));
    assert!(is_valid_staging_unit_id("unit_0_0_0"));
    assert!(!is_valid_staging_unit_id("unit_a"));
    assert!(!is_valid_staging_unit_id("unit_1_2"));
    assert!(!is_valid_staging_unit_id("unit_1_2_3_4"));
    assert!(!is_valid_staging_unit_id("unit_1_2_"));
    assert!(!is_valid_staging_unit_id("../unit_1_2_3"));
    assert!(!is_valid_staging_unit_id("unit_1_2_3/sub"));
    assert!(!is_valid_staging_unit_id("evil_id"));
}

/// Item 1: A forged journal inside staging pointing `staged_path` to an arbitrary directory
/// outside staging MUST NEVER delete or touch the victim directory.
#[test]
fn gc_guard3_item1_forged_journal_staged_path_outside_staging_never_deletes() {
    let dir = tempdir().unwrap();
    let staging_dir = dir.path().join(".greenc-staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    #[cfg(unix)]
    set_mode(&staging_dir, 0o700);

    let victim_dir = dir.path().join("victim_important_data");
    std::fs::create_dir_all(&victim_dir).unwrap();
    let victim_file = victim_dir.join("secrets.txt");
    std::fs::write(&victim_file, b"do not delete me").unwrap();

    let forged = make_journal(
        "unit_1700000000000_1234_1",
        dir.path().join("orig_dummy"),
        victim_dir.clone(),
        StagingMode::Direct,
        StagingState::Deleting,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    let jpath = staging_dir.join("unit_1700000000000_1234_1.journal.json");
    std::fs::write(&jpath, serde_json::to_vec(&forged).unwrap()).unwrap();

    let rep = recover_staging_dir(&staging_dir, &BusyToolsStatus::idle());

    assert!(
        victim_file.exists(),
        "forged journal pointing staged_path outside staging MUST NOT delete victim file"
    );
    assert!(
        victim_dir.exists(),
        "forged journal pointing staged_path outside staging MUST NOT delete victim dir"
    );
    assert!(
        jpath.exists(),
        "rejected forged journal should not be silently removed as stale"
    );
    assert!(
        !rep.rejected.is_empty(),
        "forged journal must be recorded in rejected"
    );
}

/// Item 1: Forged journal with `..` in `staged_path`, invalid `id`, mismatched filename/id,
/// dev/inode mismatch, or symlink at `staging_dir/<id>` MUST NOT delete anything.
#[test]
fn gc_guard3_item1_forged_journal_dotdot_bad_id_dev_ino_and_symlink_never_deletes() {
    let dir = tempdir().unwrap();
    let staging_dir = dir.path().join(".greenc-staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    #[cfg(unix)]
    set_mode(&staging_dir, 0o700);

    // Case A: `..` in staged_path
    let victim_dotdot = dir.path().join("victim_dotdot");
    std::fs::create_dir_all(&victim_dotdot).unwrap();
    std::fs::write(victim_dotdot.join("keep.txt"), b"keep").unwrap();
    let j_dotdot = make_journal(
        "unit_1700000000000_1234_2",
        dir.path().join("orig"),
        staging_dir.join("../victim_dotdot"),
        StagingMode::Direct,
        StagingState::Deleting,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join("unit_1700000000000_1234_2.journal.json"),
        serde_json::to_vec(&j_dotdot).unwrap(),
    )
    .unwrap();

    // Case B: Invalid id (does not match ^unit_[0-9]+_[0-9]+_[0-9]+$)
    let bad_id_dir = staging_dir.join("evil_id");
    std::fs::create_dir_all(&bad_id_dir).unwrap();
    std::fs::write(bad_id_dir.join("keep.txt"), b"keep").unwrap();
    let j_bad_id = make_journal(
        "evil_id",
        dir.path().join("orig"),
        bad_id_dir.clone(),
        StagingMode::Direct,
        StagingState::Deleting,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join("evil_id.journal.json"),
        serde_json::to_vec(&j_bad_id).unwrap(),
    )
    .unwrap();

    // Case C: Filename id != journal.id
    let mismatch_dir = staging_dir.join("unit_1700000000000_1234_99");
    std::fs::create_dir_all(&mismatch_dir).unwrap();
    std::fs::write(mismatch_dir.join("keep.txt"), b"keep").unwrap();
    let j_mismatch = make_journal(
        "unit_1700000000000_1234_99",
        dir.path().join("orig"),
        mismatch_dir.clone(),
        StagingMode::Direct,
        StagingState::Deleting,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join("unit_1700000000000_1234_3.journal.json"),
        serde_json::to_vec(&j_mismatch).unwrap(),
    )
    .unwrap();

    // Case D: Dev/inode mismatch (journal records wrong inode for staging_dir/<id>)
    let swapped_dir = staging_dir.join("unit_1700000000000_1234_5");
    std::fs::create_dir_all(&swapped_dir).unwrap();
    std::fs::write(swapped_dir.join("keep.txt"), b"keep").unwrap();
    let mut j_bad_ino = make_journal(
        "unit_1700000000000_1234_5",
        dir.path().join("orig"),
        swapped_dir.clone(),
        StagingMode::Direct,
        StagingState::Deleting,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    j_bad_ino.staged_ino = Some(j_bad_ino.staged_ino.unwrap_or(1).wrapping_add(999_999));
    std::fs::write(
        staging_dir.join("unit_1700000000000_1234_5.journal.json"),
        serde_json::to_vec(&j_bad_ino).unwrap(),
    )
    .unwrap();

    // Case E: Symlink at staging_dir/<id> pointing outside
    #[cfg(unix)]
    let victim_sym = dir.path().join("victim_symlink_target");
    #[cfg(unix)]
    {
        std::fs::create_dir_all(&victim_sym).unwrap();
        std::fs::write(victim_sym.join("keep.txt"), b"keep").unwrap();
        let sym_staged = staging_dir.join("unit_1700000000000_1234_4");
        std::os::unix::fs::symlink(&victim_sym, &sym_staged).unwrap();
        let j_sym = make_journal(
            "unit_1700000000000_1234_4",
            dir.path().join("orig"),
            sym_staged,
            StagingMode::Direct,
            StagingState::Deleting,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        std::fs::write(
            staging_dir.join("unit_1700000000000_1234_4.journal.json"),
            serde_json::to_vec(&j_sym).unwrap(),
        )
        .unwrap();
    }

    let rep = recover_staging_dir(&staging_dir, &BusyToolsStatus::idle());

    assert!(
        victim_dotdot.join("keep.txt").exists(),
        "`..` staged_path must be rejected without deleting anything"
    );
    assert!(
        bad_id_dir.join("keep.txt").exists(),
        "invalid unit id must be rejected without deleting anything"
    );
    assert!(
        mismatch_dir.join("keep.txt").exists(),
        "mismatched journal filename vs id must be rejected without deleting anything"
    );
    assert!(
        swapped_dir.join("keep.txt").exists(),
        "dev/inode mismatch must be rejected without deleting anything"
    );
    #[cfg(unix)]
    assert!(
        victim_sym.join("keep.txt").exists(),
        "symlink staged target must never be deleted"
    );
    assert!(
        rep.rejected.len() >= 4,
        "all forged entries must be reported in rejected: {:?}",
        rep.rejected
    );
}

/// Item 1: Forged journal trying to restore (`rename`) to an `original_path` outside managed
/// developer cache roots, or containing `..`, or traversing a symlinked parent, MUST NOT rename.
#[test]
fn gc_guard3_item1_forged_journal_original_path_outside_managed_cache_never_renames() {
    let dir = tempdir().unwrap();
    let staging_dir = dir.path().join(".greenc-staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    #[cfg(unix)]
    set_mode(&staging_dir, 0o700);

    let managed_root = dir.path().join("managed_gradle_caches");
    std::fs::create_dir_all(&managed_root).unwrap();
    let rec_opts = StagingRecoveryOptions {
        allowed_cache_roots: Some(vec![managed_root.clone()]),
        ..Default::default()
    };

    // Case A: Outside managed developer cache root
    let staged_a = staging_dir.join("unit_1700000000000_1234_10");
    std::fs::create_dir_all(&staged_a).unwrap();
    std::fs::write(staged_a.join("payload.bin"), b"payload").unwrap();
    let outside_dest = dir.path().join("unmanaged_folder/injected_dir");
    std::fs::create_dir_all(outside_dest.parent().unwrap()).unwrap();
    let j_a = make_journal(
        "unit_1700000000000_1234_10",
        outside_dest.clone(),
        staged_a.clone(),
        StagingMode::Vault,
        StagingState::Staged,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join("unit_1700000000000_1234_10.journal.json"),
        serde_json::to_vec(&j_a).unwrap(),
    )
    .unwrap();

    // Case B: `..` escape from managed cache root
    let staged_b = staging_dir.join("unit_1700000000000_1234_11");
    std::fs::create_dir_all(&staged_b).unwrap();
    std::fs::write(staged_b.join("payload.bin"), b"payload").unwrap();
    let dotdot_dest = managed_root.join("../escaped_via_dotdot");
    let j_b = make_journal(
        "unit_1700000000000_1234_11",
        dotdot_dest,
        staged_b.clone(),
        StagingMode::Vault,
        StagingState::Staged,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join("unit_1700000000000_1234_11.journal.json"),
        serde_json::to_vec(&j_b).unwrap(),
    )
    .unwrap();

    // Case C: Symlink parent inside managed root pointing outside
    #[cfg(unix)]
    let sym_outside_target = dir.path().join("outside_via_symlink");
    #[cfg(unix)]
    let staged_c = staging_dir.join("unit_1700000000000_1234_12");
    #[cfg(unix)]
    {
        std::fs::create_dir_all(&sym_outside_target).unwrap();
        let sym_parent = managed_root.join("sym_link_parent");
        std::os::unix::fs::symlink(&sym_outside_target, &sym_parent).unwrap();
        std::fs::create_dir_all(&staged_c).unwrap();
        std::fs::write(staged_c.join("payload.bin"), b"payload").unwrap();
        let j_c = make_journal(
            "unit_1700000000000_1234_12",
            sym_parent.join("injected"),
            staged_c.clone(),
            StagingMode::Vault,
            StagingState::Staged,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        std::fs::write(
            staging_dir.join("unit_1700000000000_1234_12.journal.json"),
            serde_json::to_vec(&j_c).unwrap(),
        )
        .unwrap();
    }

    let rep = recover_staging_dir_with_options(&staging_dir, &BusyToolsStatus::idle(), &rec_opts);

    assert!(
        !outside_dest.exists(),
        "recover_staging_dir MUST NOT rename staged dir outside managed developer cache roots"
    );
    assert!(
        !dir.path().join("escaped_via_dotdot").exists(),
        "recover_staging_dir MUST NOT rename via `..` escape"
    );
    assert!(staged_a.exists() && staged_b.exists());
    #[cfg(unix)]
    {
        assert!(
            !sym_outside_target.join("injected").exists(),
            "recover_staging_dir MUST NOT rename through symlink parent escaping managed cache root"
        );
        assert!(staged_c.exists());
    }
    assert!(
        !rep.conflicts.is_empty() && !rep.rejected.is_empty(),
        "rejections must be reported in recovery report"
    );
}

/// Item 1: Staging dir with wrong mode (not 0700), symlink staging dir, wrong owner UID,
/// hardlinked journal (nlink > 1), or > 64 KB journal MUST be rejected without deleting anything.
#[cfg(unix)]
#[test]
fn gc_guard3_item1_rejects_bad_mode_owner_symlink_hardlink_and_oversized_journal() {
    // 1. Bad mode (0755 instead of 0700)
    {
        let dir = tempdir().unwrap();
        let staging_dir = dir.path().join(".greenc-staging");
        std::fs::create_dir_all(&staging_dir).unwrap();
        set_mode(&staging_dir, 0o755);

        let staged = staging_dir.join("unit_1700000000000_1234_20");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("keep.txt"), b"keep").unwrap();

        let j = make_journal(
            "unit_1700000000000_1234_20",
            dir.path().join("orig"),
            staged.clone(),
            StagingMode::Direct,
            StagingState::Deleting,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        std::fs::write(
            staging_dir.join("unit_1700000000000_1234_20.journal.json"),
            serde_json::to_vec(&j).unwrap(),
        )
        .unwrap();

        let rep = recover_staging_dir(&staging_dir, &BusyToolsStatus::idle());
        assert!(
            staged.join("keep.txt").exists(),
            "staging dir with mode 0755 (not 0700) must be skipped without deleting anything"
        );
        assert!(!rep.rejected.is_empty());
    }

    // 2. Symlink staging dir
    {
        let dir = tempdir().unwrap();
        let real_dir = dir.path().join("real_staging");
        std::fs::create_dir_all(&real_dir).unwrap();
        set_mode(&real_dir, 0o700);
        let sym_staging = dir.path().join("sym_staging");
        std::os::unix::fs::symlink(&real_dir, &sym_staging).unwrap();

        let staged = real_dir.join("unit_1700000000000_1234_25");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("keep.txt"), b"keep").unwrap();

        let j = make_journal(
            "unit_1700000000000_1234_25",
            dir.path().join("orig"),
            sym_staging.join("unit_1700000000000_1234_25"),
            StagingMode::Direct,
            StagingState::Deleting,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        std::fs::write(
            real_dir.join("unit_1700000000000_1234_25.journal.json"),
            serde_json::to_vec(&j).unwrap(),
        )
        .unwrap();

        let rep = recover_staging_dir(&sym_staging, &BusyToolsStatus::idle());
        assert!(
            staged.join("keep.txt").exists(),
            "symlink staging_dir must be rejected without deleting anything"
        );
        assert!(!rep.rejected.is_empty());
    }

    // 3. Wrong owner UID (simulated expected_uid != dir owner, plus real root-owned `/`)
    {
        let dir = tempdir().unwrap();
        let staging_dir = dir.path().join(".greenc-staging");
        std::fs::create_dir_all(&staging_dir).unwrap();
        set_mode(&staging_dir, 0o700);

        let staged = staging_dir.join("unit_1700000000000_1234_26");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("keep.txt"), b"keep").unwrap();

        let j = make_journal(
            "unit_1700000000000_1234_26",
            dir.path().join("orig"),
            staged.clone(),
            StagingMode::Direct,
            StagingState::Deleting,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        std::fs::write(
            staging_dir.join("unit_1700000000000_1234_26.journal.json"),
            serde_json::to_vec(&j).unwrap(),
        )
        .unwrap();

        let wrong_uid = greenc_platform::current_user_uid().wrapping_add(1);
        let rep = recover_staging_dir_with_options(
            &staging_dir,
            &BusyToolsStatus::idle(),
            &StagingRecoveryOptions {
                expected_uid: Some(wrong_uid),
                ..Default::default()
            },
        );
        assert!(
            staged.join("keep.txt").exists(),
            "staging dir with wrong owner UID must be rejected without deleting anything"
        );
        assert!(!rep.rejected.is_empty());

        // Also test a real root-owned directory (`/`) when running non-root
        if greenc_platform::current_user_uid() != 0 {
            let root_rep = recover_staging_dir(Path::new("/"), &BusyToolsStatus::idle());
            assert!(!root_rep.rejected.is_empty());
        }
    }

    // 4. Hardlinked journal (nlink == 2)
    {
        let dir = tempdir().unwrap();
        let staging_dir = dir.path().join(".greenc-staging");
        std::fs::create_dir_all(&staging_dir).unwrap();
        set_mode(&staging_dir, 0o700);

        let staged = staging_dir.join("unit_1700000000000_1234_21");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("keep.txt"), b"keep").unwrap();

        let j = make_journal(
            "unit_1700000000000_1234_21",
            dir.path().join("orig"),
            staged.clone(),
            StagingMode::Direct,
            StagingState::Deleting,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        let jpath = staging_dir.join("unit_1700000000000_1234_21.journal.json");
        std::fs::write(&jpath, serde_json::to_vec(&j).unwrap()).unwrap();
        std::fs::hard_link(&jpath, dir.path().join("external_hardlink.json")).unwrap();

        let rep = recover_staging_dir(&staging_dir, &BusyToolsStatus::idle());
        assert!(
            staged.join("keep.txt").exists(),
            "hardlinked journal (nlink > 1) must be rejected without deleting anything"
        );
        assert!(!rep.rejected.is_empty());
    }

    // 5. Oversized journal (> 64 KB)
    {
        let dir = tempdir().unwrap();
        let staging_dir = dir.path().join(".greenc-staging");
        std::fs::create_dir_all(&staging_dir).unwrap();
        set_mode(&staging_dir, 0o700);

        let staged = staging_dir.join("unit_1700000000000_1234_22");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("keep.txt"), b"keep").unwrap();

        let mut j = make_journal(
            "unit_1700000000000_1234_22",
            dir.path().join("orig"),
            staged.clone(),
            StagingMode::Direct,
            StagingState::Deleting,
            dir.path(),
            Some(CleanUnitKind::RegenerableDevCache),
        );
        j.note = Some("A".repeat(70 * 1024));
        let jpath = staging_dir.join("unit_1700000000000_1234_22.journal.json");
        std::fs::write(&jpath, serde_json::to_vec(&j).unwrap()).unwrap();

        let rep = recover_staging_dir(&staging_dir, &BusyToolsStatus::idle());
        assert!(
            staged.join("keep.txt").exists(),
            "oversized journal (> 64 KB) must be rejected without deleting anything"
        );
        assert!(!rep.rejected.is_empty());
    }
}

/// Item 2: `enforce_drive_quota_units` and `resolve_staging_dir_checked` MUST NEVER create or use
/// `drive_root/.greenc-staging` or a world-writable/foreign ancestor `anc/.greenc-staging`.
/// When NO safe same-drive staging dir is available, the unit is skipped and the reason is reported.
#[cfg(unix)]
#[test]
fn gc_guard3_item2_never_uses_drive_root_or_world_writable_ancestor_and_skips_with_reason() {
    let tmp = tempdir().unwrap();
    let drive_root = tmp.path().join("fake_drive");
    let shared_anc = drive_root.join("shared_world_writable");
    let gradle_root = shared_anc.join(".gradle/caches");
    let unit_dir = gradle_root.join("8.11.1/transforms/old_hash_item2");
    std::fs::create_dir_all(&unit_dir).unwrap();
    let now = SystemTime::now();
    let old = now - Duration::from_secs(10 * 86_400);
    let meta_bin = unit_dir.join("metadata.bin");
    std::fs::write(&meta_bin, vec![b'X'; 64 * 1024]).unwrap();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&meta_bin)
        .unwrap();
    f.set_times(FileTimes::new().set_modified(old).set_accessed(old))
        .unwrap();
    let _ = std::fs::File::open(&unit_dir)
        .and_then(|d| d.set_times(FileTimes::new().set_modified(old).set_accessed(old)));

    // Make `shared_anc`, `vault_dir`, AND `fake_home` world-writable (0777) so NO safe staging dir exists!
    set_mode(&shared_anc, 0o777);
    let vault_dir = tmp.path().join("untrusted_vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    set_mode(&vault_dir, 0o777);
    let untrusted_home = tmp.path().join("untrusted_home");
    std::fs::create_dir_all(&untrusted_home).unwrap();
    set_mode(&untrusted_home, 0o777);

    // Direct check on `resolve_staging_dir_checked`
    let res = resolve_staging_dir_checked(
        &unit_dir,
        &gradle_root,
        &vault_dir,
        None,
        Some(&untrusted_home),
        None,
    );
    assert!(
        res.is_err(),
        "must fail when no trusted user-owned staging dir exists, got {:?}",
        res
    );

    // End-to-end check on `enforce_drive_quota_units`
    let units = scan_dev_cache_root(
        &gradle_root,
        Duration::from_secs(DEFAULT_DEV_CACHE_MIN_AGE_SECS),
        now,
        &BusyToolsStatus::idle(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
    );
    assert_eq!(units.len(), 1);

    let db = GreenCDb::open(tmp.path().join("db.sqlite")).unwrap();
    let total = 10u64 * 1024 * 1024 * 1024;
    let free = total / 10; // 90% used -> critical
    let mut quota = DriveQuota::new(drive_root.to_string_lossy().to_string());
    quota.clean_ratio = 0.80;
    quota.target_ratio = 0.75;
    quota.auto_clean = true;
    quota.regenerable_cache_mode = RegenerableCacheMode::Direct;

    let out = enforce_drive_quota_units(
        &quota,
        units,
        &vault_dir,
        &db,
        &StaticVol(total, free),
        &MockFileLockProbe::new(),
        &MockDevToolProbe::idle(),
        &GuardRunOptions {
            now_sys: now,
            staging_home_override: Some(untrusted_home.clone()),
            ..Default::default()
        },
        &TaskControl::new(),
    )
    .unwrap();

    // Restore permissions before assertions for clean tempdir drop
    set_mode(&shared_anc, 0o755);
    set_mode(&vault_dir, 0o755);
    set_mode(&untrusted_home, 0o755);

    assert!(
        !drive_root.join(".greenc-staging").exists(),
        "MUST NEVER create drive_root/.greenc-staging"
    );
    assert!(
        !shared_anc.join(".greenc-staging").exists(),
        "MUST NEVER create .greenc-staging in a world-writable ancestor"
    );
    assert!(
        !vault_dir.join(".greenc-staging").exists(),
        "MUST NEVER create .greenc-staging inside a world-writable Vault dir"
    );
    assert!(
        unit_dir.exists() && meta_bin.exists(),
        "unit MUST be skipped untouched when no safe staging dir is available"
    );
    assert!(
        !out.staging_warnings.is_empty(),
        "skipping unit due to missing safe staging dir MUST report a reason in staging_warnings"
    );
    assert!(
        out.reasons.iter().any(|r| r.contains("khu chờ")),
        "skipping unit MUST report reason in outcome.reasons: {:?}",
        out.reasons
    );
}

/// Item 3: `RestoreConflict` for a regenerable dev cache whose `original_path` was recreated MUST
/// be deleted and logged, whereas a non-regenerable unit (`StandardFile` / `None`) MUST be kept.
#[test]
fn gc_guard3_item3_restore_conflict_cleans_regenerable_and_keeps_non_regenerable() {
    let dir = tempdir().unwrap();
    let gradle_root = dir.path().join(".gradle/caches");
    let orig_regen = gradle_root.join("8.11.1/transforms/recreated_regen");
    let orig_nonregen = gradle_root.join("8.11.1/transforms/recreated_nonregen");
    std::fs::create_dir_all(&orig_regen).unwrap();
    std::fs::write(orig_regen.join("metadata.bin"), b"fresh_from_gradle").unwrap();
    std::fs::create_dir_all(&orig_nonregen).unwrap();
    std::fs::write(orig_nonregen.join("data.bin"), b"important_user_data").unwrap();

    let staging_dir = dir.path().join(".greenc-staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    #[cfg(unix)]
    set_mode(&staging_dir, 0o700);

    // 1. Regenerable dev cache in RestoreConflict -> MUST be deleted & logged
    let id_regen = "unit_1700000000000_9999_1";
    let staged_regen = staging_dir.join(id_regen);
    std::fs::create_dir_all(&staged_regen).unwrap();
    std::fs::write(staged_regen.join("metadata.bin"), b"stale_staged_copy").unwrap();
    let j_regen = make_journal(
        id_regen,
        orig_regen.clone(),
        staged_regen.clone(),
        StagingMode::Vault,
        StagingState::RestoreConflict,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    let jpath_regen = staging_dir.join(format!("{id_regen}.journal.json"));
    std::fs::write(&jpath_regen, serde_json::to_vec(&j_regen).unwrap()).unwrap();

    // 2. Non-regenerable unit (`StandardFile`) in RestoreConflict -> MUST NOT be deleted
    let id_nonregen = "unit_1700000000000_9999_2";
    let staged_nonregen = staging_dir.join(id_nonregen);
    std::fs::create_dir_all(&staged_nonregen).unwrap();
    std::fs::write(staged_nonregen.join("data.bin"), b"keep_in_staging").unwrap();
    let j_nonregen = make_journal(
        id_nonregen,
        orig_nonregen.clone(),
        staged_nonregen.clone(),
        StagingMode::Vault,
        StagingState::RestoreConflict,
        dir.path(),
        Some(CleanUnitKind::StandardFile),
    );
    let jpath_nonregen = staging_dir.join(format!("{id_nonregen}.journal.json"));
    std::fs::write(&jpath_nonregen, serde_json::to_vec(&j_nonregen).unwrap()).unwrap();

    let rep = recover_staging_dir_with_options(
        &staging_dir,
        &BusyToolsStatus::idle(),
        &StagingRecoveryOptions {
            allowed_cache_roots: Some(vec![gradle_root]),
            ..Default::default()
        },
    );

    assert_eq!(
        std::fs::read(orig_regen.join("metadata.bin")).unwrap(),
        b"fresh_from_gradle",
        "recreated original dir must remain untouched"
    );
    assert!(
        !staged_regen.exists() && !jpath_regen.exists(),
        "redundant RestoreConflict staged copy of regenerable cache MUST be deleted"
    );
    assert_eq!(rep.redundant_conflicts_cleaned, 1);
    assert_eq!(rep.cleaned_conflicts.len(), 1);

    assert!(
        staged_nonregen.exists() && jpath_nonregen.exists(),
        "non-regenerable RestoreConflict MUST remain safely in staging"
    );
    assert_eq!(rep.conflicts.len(), 1);
}

/// Item 5: Power loss at EVERY lifecycle state recovers deterministically and accurately:
/// - `Staging` (before rename -> stale journal cleaned, original untouched)
/// - `Staging` (after rename, Vault -> restored to original)
/// - `Staging` (after rename, Direct -> finished deleting)
/// - `Staged` (Vault, original missing -> restored)
/// - `Archived` (Vault -> finished deleting)
/// - `Deleting` (Direct/Vault -> finished deleting)
/// - `RestoreConflict` (RegenerableDevCache + orig exists -> deleted)
#[test]
fn gc_guard3_item5_power_loss_recovery_at_every_staging_state() {
    let dir = tempdir().unwrap();
    let staging_dir = dir.path().join(".greenc-staging");
    std::fs::create_dir_all(&staging_dir).unwrap();
    #[cfg(unix)]
    set_mode(&staging_dir, 0o700);

    let gradle_root = dir.path().join(".gradle/caches");
    let transforms = gradle_root.join("8.11.1/transforms");
    std::fs::create_dir_all(&transforms).unwrap();

    // 1. Staging (Vault) BEFORE rename: staged dir doesn't exist, original still intact
    let orig_1 = transforms.join("state_1_pre_rename");
    std::fs::create_dir_all(&orig_1).unwrap();
    std::fs::write(orig_1.join("a.bin"), b"orig1").unwrap();
    let phys_1 = query_file_identity(&orig_1).unwrap();
    let id_1 = "unit_1700000000100_55_1";
    let j_1 = StagingJournalEntry {
        version: 2,
        id: id_1.to_string(),
        original_path: orig_1.clone(),
        staged_path: staging_dir.join(id_1),
        mode: StagingMode::Vault,
        state: StagingState::Staging,
        created_at: Utc::now(),
        drive_root: dir.path().to_string_lossy().to_string(),
        unit_label: "gradle_transform".to_string(),
        dev_tool: Some(DevTool::Gradle),
        note: None,
        staged_dev: Some(phys_1.volume_serial),
        staged_ino: Some(phys_1.file_index),
        unit_kind: Some(CleanUnitKind::RegenerableDevCache),
    };
    std::fs::write(
        staging_dir.join(format!("{id_1}.journal.json")),
        serde_json::to_vec(&j_1).unwrap(),
    )
    .unwrap();

    // 2. Staging (Vault) AFTER rename: staged dir exists with original's (dev, ino), orig absent
    let orig_2 = transforms.join("state_2_post_rename_vault");
    let id_2 = "unit_1700000000100_55_2";
    let staged_2 = staging_dir.join(id_2);
    std::fs::create_dir_all(&staged_2).unwrap();
    std::fs::write(staged_2.join("b.bin"), b"orig2").unwrap();
    let j_2 = make_journal(
        id_2,
        orig_2.clone(),
        staged_2.clone(),
        StagingMode::Vault,
        StagingState::Staging,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join(format!("{id_2}.journal.json")),
        serde_json::to_vec(&j_2).unwrap(),
    )
    .unwrap();

    // 3. Staging (Direct) AFTER rename: must finish deleting
    let orig_3 = transforms.join("state_3_post_rename_direct");
    let id_3 = "unit_1700000000100_55_3";
    let staged_3 = staging_dir.join(id_3);
    std::fs::create_dir_all(&staged_3).unwrap();
    std::fs::write(staged_3.join("c.bin"), b"del3").unwrap();
    let j_3 = make_journal(
        id_3,
        orig_3,
        staged_3.clone(),
        StagingMode::Direct,
        StagingState::Staging,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join(format!("{id_3}.journal.json")),
        serde_json::to_vec(&j_3).unwrap(),
    )
    .unwrap();

    // 4. Staged (Vault) -> restored to original
    let orig_4 = transforms.join("state_4_staged_vault");
    let id_4 = "unit_1700000000100_55_4";
    let staged_4 = staging_dir.join(id_4);
    std::fs::create_dir_all(&staged_4).unwrap();
    std::fs::write(staged_4.join("d.bin"), b"orig4").unwrap();
    let j_4 = make_journal(
        id_4,
        orig_4.clone(),
        staged_4.clone(),
        StagingMode::Vault,
        StagingState::Staged,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join(format!("{id_4}.journal.json")),
        serde_json::to_vec(&j_4).unwrap(),
    )
    .unwrap();

    // 5. Archived (Vault) -> finished deleting
    let orig_5 = transforms.join("state_5_archived");
    let id_5 = "unit_1700000000100_55_5";
    let staged_5 = staging_dir.join(id_5);
    std::fs::create_dir_all(&staged_5).unwrap();
    std::fs::write(staged_5.join("e.bin"), b"del5").unwrap();
    let j_5 = make_journal(
        id_5,
        orig_5,
        staged_5.clone(),
        StagingMode::Vault,
        StagingState::Archived,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join(format!("{id_5}.journal.json")),
        serde_json::to_vec(&j_5).unwrap(),
    )
    .unwrap();

    // 6. Deleting -> finished deleting
    let orig_6 = transforms.join("state_6_deleting");
    let id_6 = "unit_1700000000100_55_6";
    let staged_6 = staging_dir.join(id_6);
    std::fs::create_dir_all(&staged_6).unwrap();
    std::fs::write(staged_6.join("f.bin"), b"del6").unwrap();
    let j_6 = make_journal(
        id_6,
        orig_6,
        staged_6.clone(),
        StagingMode::Direct,
        StagingState::Deleting,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join(format!("{id_6}.journal.json")),
        serde_json::to_vec(&j_6).unwrap(),
    )
    .unwrap();

    // 7. RestoreConflict (RegenerableDevCache, recreated at orig_7) -> deleted
    let orig_7 = transforms.join("state_7_conflict_regen");
    std::fs::create_dir_all(&orig_7).unwrap();
    std::fs::write(orig_7.join("g.bin"), b"fresh7").unwrap();
    let id_7 = "unit_1700000000100_55_7";
    let staged_7 = staging_dir.join(id_7);
    std::fs::create_dir_all(&staged_7).unwrap();
    std::fs::write(staged_7.join("g.bin"), b"stale7").unwrap();
    let j_7 = make_journal(
        id_7,
        orig_7.clone(),
        staged_7.clone(),
        StagingMode::Vault,
        StagingState::RestoreConflict,
        dir.path(),
        Some(CleanUnitKind::RegenerableDevCache),
    );
    std::fs::write(
        staging_dir.join(format!("{id_7}.journal.json")),
        serde_json::to_vec(&j_7).unwrap(),
    )
    .unwrap();

    let rep = recover_staging_dir_with_options(
        &staging_dir,
        &BusyToolsStatus::idle(),
        &StagingRecoveryOptions {
            allowed_cache_roots: Some(vec![gradle_root]),
            ..Default::default()
        },
    );

    assert_eq!(rep.stale_journals_removed, 1, "case 1 stale journal");
    assert_eq!(rep.restored_to_original, 2, "cases 2 & 4 restored");
    assert_eq!(
        rep.completed_deletions, 4,
        "cases 3, 5, 6, 7 completed deletions"
    );
    assert_eq!(
        rep.redundant_conflicts_cleaned, 1,
        "case 7 conflict cleaned"
    );
    assert!(
        rep.rejected.is_empty(),
        "no rejections expected: {:?}",
        rep.rejected
    );
    assert!(
        rep.conflicts.is_empty(),
        "no remaining conflicts expected: {:?}",
        rep.conflicts
    );

    assert_eq!(std::fs::read(orig_1.join("a.bin")).unwrap(), b"orig1");
    assert_eq!(std::fs::read(orig_2.join("b.bin")).unwrap(), b"orig2");
    assert!(!staged_3.exists());
    assert_eq!(std::fs::read(orig_4.join("d.bin")).unwrap(), b"orig4");
    assert!(!staged_5.exists());
    assert!(!staged_6.exists());
    assert!(!staged_7.exists());
    assert_eq!(std::fs::read(orig_7.join("g.bin")).unwrap(), b"fresh7");
    assert_eq!(
        std::fs::read_dir(&staging_dir).unwrap().count(),
        0,
        "staging dir must be completely clean after recovery"
    );
}

/// Item 4: Vault purge & rollback MUST refuse `zip_path` / `manifest_path` outside `vault_dir`
/// (including `..` traversal and symlinks inside `vault_dir` pointing outside).
#[test]
fn gc_guard3_item4_vault_purge_and_rollback_reject_paths_outside_vault_dir() {
    let tmp = tempdir().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();

    let vault_dir = tmp.path().join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    AppSettings::new(&db).set_data_root(tmp.path()).unwrap();

    let outside_dir = tmp.path().join("outside_victim");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let victim_zip = outside_dir.join("victim.zip");
    let victim_man = outside_dir.join("victim.manifest.json");
    {
        let mut zw = ZipWriter::new(File::create(&victim_zip).unwrap());
        zw.start_file("item.txt", SimpleFileOptions::default())
            .unwrap();
        zw.write_all(b"victim content").unwrap();
        zw.finish().unwrap();
    }
    std::fs::write(&victim_man, b"{}").unwrap();

    // Case A: Expired archive in DB pointing outside `vault_dir` -> watchdog auto-purge MUST NOT delete it!
    let exp_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "TamperedExpired".to_string(),
                zip_path: victim_zip.clone(),
                manifest_path: victim_man.clone(),
                file_count: 0,
                original_bytes: 10,
                compressed_bytes: 10,
                created_at: Utc::now() - chrono::Duration::days(10),
                expires_at: Utc::now() - chrono::Duration::days(1),
                status: VaultStatus::Quarantined,
                status_reason: "Tampered".to_string(),
                resolved_at: None,
            },
            &[],
        )
        .unwrap();

    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    let _ = engine.run_watchdog_cycle(&IdleProcessProbe).unwrap();

    assert!(
        victim_zip.exists(),
        "watchdog auto-purge MUST NOT delete zip_path outside vault_dir"
    );
    assert!(
        victim_man.exists(),
        "watchdog auto-purge MUST NOT delete manifest_path outside vault_dir"
    );
    assert_eq!(
        db.get_vault_archive(exp_id).unwrap().unwrap().status,
        VaultStatus::Quarantined,
        "refused purge must not mark tampered archive as Purged"
    );

    // Case B: Rollback with zip_path escaping vault_dir via `..` MUST be rejected!
    let traversal_zip = vault_dir.join("../outside_victim/victim.zip");
    let traversal_man = vault_dir.join("../outside_victim/victim.manifest.json");
    let restore_target = tmp.path().join("restored_out.txt");
    let trav_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "TamperedRollback".to_string(),
                zip_path: traversal_zip,
                manifest_path: traversal_man,
                file_count: 1,
                original_bytes: 14,
                compressed_bytes: 100,
                created_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::days(7),
                status: VaultStatus::Quarantined,
                status_reason: "Tampered".to_string(),
                resolved_at: None,
            },
            &[VaultFileItem {
                id: None,
                archive_id: None,
                original_path: restore_target.clone(),
                rel_arcname: "item.txt".to_string(),
                size_bytes: 14,
                sha256: greenc_vault::manifest::compute_bytes_sha256(b"victim content"),
                mtime: 1731760000.0,
                atime: 1731760000.0,
                hardlink_paths: vec![restore_target.clone()],
                restored_individually: false,
            }],
        )
        .unwrap();

    let rb_res = engine.rollback_archive(trav_id, "tampered", false);
    assert!(
        rb_res.is_err(),
        "rollback_archive MUST reject zip_path/manifest_path outside vault_dir"
    );
    assert!(victim_zip.exists() && victim_man.exists());

    // Case C: Symlink inside `vault_dir` pointing to outside file MUST be rejected!
    #[cfg(unix)]
    {
        let sym_zip = vault_dir.join("sneaky.zip");
        let sym_man = vault_dir.join("sneaky.manifest.json");
        std::os::unix::fs::symlink(&victim_zip, &sym_zip).unwrap();
        std::os::unix::fs::symlink(&victim_man, &sym_man).unwrap();

        let sym_id = db
            .create_vault_archive(
                &VaultArchive {
                    id: None,
                    rule_id: 1,
                    app_name: "TamperedSymlink".to_string(),
                    zip_path: sym_zip.clone(),
                    manifest_path: sym_man.clone(),
                    file_count: 1,
                    original_bytes: 14,
                    compressed_bytes: 100,
                    created_at: Utc::now() - chrono::Duration::days(10),
                    expires_at: Utc::now() - chrono::Duration::days(1),
                    status: VaultStatus::Quarantined,
                    status_reason: "TamperedSymlink".to_string(),
                    resolved_at: None,
                },
                &[],
            )
            .unwrap();

        let _ = engine.run_watchdog_cycle(&IdleProcessProbe).unwrap();
        assert!(victim_zip.exists() && victim_man.exists());
        assert!(sym_zip.symlink_metadata().is_ok());
        assert!(engine
            .rollback_archive(sym_id, "tampered symlink", false)
            .is_err());
    }
}
