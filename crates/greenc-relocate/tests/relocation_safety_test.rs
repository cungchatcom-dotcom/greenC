//! Safety of link repair, crash recovery and rollback: a failed or interrupted step must never
//! leave the user with fewer copies, or with a link GreenC removed and could not replace.

use greenc_core::error::{GreenCError, Result};
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
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Env {
    tmp: TempDir,
    db: GreenCDb,
    reparse: MockReparsePointApi,
}

fn env() -> Env {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();
    Env {
        tmp,
        db,
        reparse: MockReparsePointApi::new(),
    }
}

impl Env {
    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn write(&self, rel: &str, data: &[u8]) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, data).unwrap();
        p
    }

    fn record(&self, status: RelocationStatus) -> i64 {
        self.db
            .save_relocation(&RelocationRecord {
                id: None,
                app_id: "slack".to_string(),
                app_name: "Slack".to_string(),
                strategy: RelocationStrategyKind::Junction,
                source_path: self.path("C/Slack"),
                target_path: self.path("D/Slack"),
                size_bytes: 64,
                status,
                health_status: RelocationHealthStatus::Healthy,
                process_names: vec![],
                created_at: chrono::Utc::now(),
                detail: None,
            })
            .unwrap()
    }

    fn link(&self) -> Option<PathBuf> {
        self.reparse.get_junction_target(&self.path("C/Slack")).ok()
    }

    fn reload(&self, id: i64) -> RelocationRecord {
        self.db
            .list_relocations(None)
            .unwrap()
            .into_iter()
            .find(|r| r.id == Some(id))
            .unwrap()
    }
}

fn volumes() -> MockVolumeApi {
    let drive = |letter: char, is_system: bool| greenc_core::models::DriveInfo {
        root: PathBuf::from(format!("{letter}:\\")),
        drive_letter: letter,
        label: String::new(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 << 30,
        free_bytes: 200 << 30,
        is_system,
        is_eligible: !is_system,
        ineligible_reason: None,
    };
    MockVolumeApi::new(vec![drive('C', true), drive('D', false)])
}

/// Fails `create_junction` and leaves a non-empty directory behind, as an interrupted link
/// creation or an app recreating its folder would.
struct BlockingReparse;

impl ReparsePointApi for BlockingReparse {
    fn is_reparse_point(&self, _path: &Path) -> bool {
        false
    }
    fn create_junction(&self, link: &Path, _target: &Path) -> Result<()> {
        fs::create_dir_all(link)?;
        fs::write(link.join("recreated-by-app.txt"), b"new")?;
        Err(GreenCError::Relocation("simulated".to_string()))
    }
    fn get_junction_target(&self, link: &Path) -> Result<PathBuf> {
        Err(GreenCError::Relocation(link.display().to_string()))
    }
    fn delete_junction(&self, _link: &Path) -> Result<()> {
        Ok(())
    }
}

/// Behaves like the mock until the link is removed, when the app recreates its folder.
struct RecreatedAfterUnlink(MockReparsePointApi);

impl ReparsePointApi for RecreatedAfterUnlink {
    fn is_reparse_point(&self, path: &Path) -> bool {
        self.0.is_reparse_point(path)
    }
    fn create_junction(&self, link: &Path, target: &Path) -> Result<()> {
        self.0.create_junction(link, target)
    }
    fn get_junction_target(&self, link: &Path) -> Result<PathBuf> {
        self.0.get_junction_target(link)
    }
    fn delete_junction(&self, link: &Path) -> Result<()> {
        self.0.delete_junction(link)?;
        fs::create_dir_all(link)?;
        fs::write(link.join("recreated-by-app.txt"), b"new")?;
        Ok(())
    }
}

fn candidate(e: &Env) -> RelocationCandidate {
    RelocationCandidate {
        id: "slack".to_string(),
        name: "Slack".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: e.path("C/Slack"),
        default_target_path: e.path("D/Slack"),
        size_bytes: 64,
        file_count: 1,
        is_locked: false,
        locked_processes: vec![],
        how_description: String::new(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    }
}

fn relocate(e: &Env, reparse: &dyn ReparsePointApi) -> Result<RelocationRecord> {
    TransactionalMover::relocate_junction(
        &candidate(e),
        MoveOptions {
            target_drive: 'D',
            custom_target_path: Some(e.path("D/Slack")),
        },
        &e.db,
        &volumes(),
        reparse,
        &MockProcessServiceProbe::new(),
        &MockShellFolderApi::new(),
        &TaskControl::new(),
        |_, _| {},
    )
}

fn undo(e: &Env, id: i64, reparse: &dyn ReparsePointApi) -> Result<RelocationRecord> {
    TransactionalMover::undo_relocation(
        id,
        &e.db,
        &volumes(),
        reparse,
        &MockShellFolderApi::new(),
        &MockProcessServiceProbe::new(),
        &TaskControl::new(),
    )
}

fn leftovers(e: &Env, needle: &str) -> Vec<String> {
    fs::read_dir(e.path("C"))
        .unwrap()
        .flatten()
        .map(|d| d.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(needle))
        .collect()
}

// --- Link repair ---------------------------------------------------------------------------

#[test]
fn repair_keeps_the_old_link_when_the_new_one_cannot_be_created() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    e.write("E/Slack/a.db", b"a");
    fs::create_dir_all(e.path("C")).unwrap();
    let id = e.record(RelocationStatus::Done);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("D/Slack"))
        .unwrap();
    e.reparse.set_fail_create(true);

    let res = RecoveryService::repair_junction_target(id, &e.path("E/Slack"), &e.db, &e.reparse);

    assert!(res.is_err());
    assert_eq!(e.link(), Some(e.path("D/Slack")));
    assert_eq!(e.reload(id).target_path, e.path("D/Slack"));
}

#[test]
fn repair_refuses_a_real_folder_holding_files_at_the_old_location() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    let live = e.write("C/Slack/new.db", b"new");
    let id = e.record(RelocationStatus::Done);

    let res = RecoveryService::repair_junction_target(id, &e.path("D/Slack"), &e.db, &e.reparse);

    assert!(res.is_err());
    assert_eq!(e.link(), None);
    assert_eq!(fs::read(live).unwrap(), b"new");
}

#[test]
fn repair_refuses_a_move_that_was_already_undone() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    fs::create_dir_all(e.path("C/Slack")).unwrap();
    let id = e.record(RelocationStatus::Undone);

    let res = RecoveryService::repair_junction_target(id, &e.path("D/Slack"), &e.db, &e.reparse);

    assert!(res.is_err());
    assert_eq!(e.link(), None);
}

#[test]
fn repair_refuses_a_target_that_contains_the_link() {
    let e = env();
    fs::create_dir_all(e.path("C")).unwrap();
    let id = e.record(RelocationStatus::Done);

    let res = RecoveryService::repair_junction_target(id, &e.path("C"), &e.db, &e.reparse);

    assert!(res.is_err());
    assert_eq!(e.link(), None);
}

#[test]
fn repair_points_the_link_at_the_reconnected_folder() {
    let e = env();
    e.write("E/Slack/a.db", b"a");
    fs::create_dir_all(e.path("C")).unwrap();
    let id = e.record(RelocationStatus::Done);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("D/Slack"))
        .unwrap();

    RecoveryService::repair_junction_target(id, &e.path("E/Slack"), &e.db, &e.reparse).unwrap();

    assert_eq!(e.link(), Some(e.path("E/Slack")));
    assert_eq!(e.reload(id).target_path, e.path("E/Slack"));
    assert_eq!(
        e.reparse.junctions.lock().unwrap().len(),
        1,
        "no probe link left"
    );
}

// --- Crash recovery --------------------------------------------------------------------------

#[test]
fn recovery_leaves_the_link_alone_while_the_target_drive_is_offline() {
    let e = env();
    // Crashed while deleting the original: only part of it is left, the full copy is offline.
    let part = e.write("C/Slack.greenc-moving.abc/part.db", b"part");
    let id = e.record(RelocationStatus::Moving);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("D/Slack"))
        .unwrap();

    let recovered = RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert!(recovered.is_empty(), "nothing was recovered yet");
    assert_eq!(e.link(), Some(e.path("D/Slack")));
    assert!(
        part.exists(),
        "partial original must not become the live folder"
    );
    let rec = e.reload(id);
    assert_eq!(rec.status, RelocationStatus::Moving, "retried once online");
    assert_eq!(rec.health_status, RelocationHealthStatus::TargetOffline);
}

#[test]
fn recovery_never_removes_a_link_that_points_somewhere_else() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    e.write("Users-own/a.db", b"own");
    e.write("C/Slack.greenc-moving.abc/a.db", b"a");
    let id = e.record(RelocationStatus::Moving);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("Users-own"))
        .unwrap();

    RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert_eq!(e.link(), Some(e.path("Users-own")));
    assert!(e.path("D/Slack/a.db").exists());
    assert!(e.path("C/Slack.greenc-moving.abc/a.db").exists());
    assert_eq!(e.reload(id).status, RelocationStatus::NeedsReview);
}

#[test]
fn recovery_keeps_target_files_missing_from_the_restored_original() {
    let e = env();
    // The original was half deleted and the link then lost: the target is the only full copy.
    e.write("D/Slack/a.db", b"a");
    e.write("D/Slack/b.db", b"only here");
    e.write("C/Slack.greenc-moving.abc/a.db", b"a");
    let id = e.record(RelocationStatus::Moving);

    RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert_eq!(fs::read(e.path("D/Slack/b.db")).unwrap(), b"only here");
    assert!(e.path("C/Slack/a.db").exists());
    assert_eq!(e.reload(id).status, RelocationStatus::NeedsReview);
}

#[test]
fn recovery_keeps_originals_missing_from_the_target() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    e.write("C/Slack.greenc-moving.abc/a.db", b"a");
    let only = e.write("C/Slack.greenc-moving.abc/b.db", b"only here");
    let id = e.record(RelocationStatus::Moving);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("D/Slack"))
        .unwrap();

    RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert_eq!(fs::read(only).unwrap(), b"only here");
    assert!(!e.path("C/Slack.greenc-moving.abc/a.db").exists());
    let rec = e.reload(id);
    assert_eq!(rec.status, RelocationStatus::Done);
    assert!(rec.detail.unwrap().contains("greenc-moving.abc"));
}

#[test]
fn recovery_keeps_an_original_newer_than_the_target_copy() {
    let e = env();
    let copy = e.write("D/Slack/a.db", b"old");
    let original = e.write("C/Slack.greenc-moving.abc/a.db", b"newer");
    fs::File::options()
        .write(true)
        .open(copy)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
    e.record(RelocationStatus::Moving);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("D/Slack"))
        .unwrap();

    RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert_eq!(fs::read(original).unwrap(), b"newer");
}

#[test]
fn recovery_only_restores_the_folder_belonging_to_its_own_record() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    let id = e.record(RelocationStatus::Moving);
    e.write(&format!("C/Slack.greenc-moving.r{id}-x/a.db"), b"mine");
    e.write(
        &format!("C/Slack.greenc-moving.r{}-x/a.db", id + 1),
        b"other",
    );

    RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert_eq!(fs::read(e.path("C/Slack/a.db")).unwrap(), b"mine");
    assert_eq!(leftovers(&e, "greenc-moving").len(), 1);
}

#[test]
fn recovery_does_not_guess_between_several_unlabelled_folders() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    e.write("C/Slack.greenc-moving.aaa/a.db", b"1");
    e.write("C/Slack.greenc-moving.bbb/a.db", b"2");
    let id = e.record(RelocationStatus::Moving);

    RecoveryService::recover_interrupted(&e.db, &e.reparse).unwrap();

    assert_eq!(leftovers(&e, "greenc-moving").len(), 2);
    assert!(e.path("D/Slack/a.db").exists());
    assert!(!e.path("C/Slack").exists());
    assert_eq!(e.reload(id).status, RelocationStatus::NeedsReview);
}

// --- Move and undo ---------------------------------------------------------------------------

#[test]
fn move_keeps_both_copies_when_rollback_cannot_put_the_original_back() {
    let e = env();
    e.write("C/Slack/a.db", b"a");

    assert!(relocate(&e, &BlockingReparse).is_err());

    assert_eq!(leftovers(&e, "greenc-moving").len(), 1);
    assert_eq!(fs::read(e.path("D/Slack/a.db")).unwrap(), b"a");
    let rec = &e.db.list_relocations(None).unwrap()[0];
    assert_eq!(
        rec.status,
        RelocationStatus::Moving,
        "left for crash recovery"
    );
}

#[test]
fn move_labels_the_temporary_folder_with_its_record() {
    let e = env();
    e.write("C/Slack/a.db", b"a");

    assert!(relocate(&e, &BlockingReparse).is_err());

    let id = e.db.list_relocations(None).unwrap()[0].id.unwrap();
    let prefix = format!("Slack.greenc-moving.r{id}-");
    assert!(leftovers(&e, "greenc-moving")[0].starts_with(&prefix));
}

#[test]
fn move_refuses_a_source_that_is_already_a_link() {
    let e = env();
    e.write("C/Slack/a.db", b"a");
    e.write("Elsewhere/a.db", b"a");
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("Elsewhere"))
        .unwrap();

    assert!(relocate(&e, &e.reparse).is_err());

    assert_eq!(e.link(), Some(e.path("Elsewhere")));
    assert!(e.path("C/Slack/a.db").exists());
    assert!(e.db.list_relocations(None).unwrap().is_empty());
}

#[test]
fn undo_puts_the_link_back_when_the_restored_folder_cannot_take_its_place() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    fs::create_dir_all(e.path("C")).unwrap();
    let id = e.record(RelocationStatus::Done);
    let reparse = RecreatedAfterUnlink(e.reparse.clone());
    reparse
        .create_junction(&e.path("C/Slack"), &e.path("D/Slack"))
        .unwrap();

    assert!(undo(&e, id, &reparse).is_err());

    assert_eq!(e.link(), Some(e.path("D/Slack")));
    assert!(e.path("D/Slack/a.db").exists());
    assert_eq!(e.reload(id).status, RelocationStatus::Done);
    assert!(leftovers(&e, "restoring.").is_empty());
}

#[test]
fn undo_refuses_when_the_link_was_changed_by_someone_else() {
    let e = env();
    e.write("D/Slack/a.db", b"a");
    e.write("Elsewhere/a.db", b"other");
    fs::create_dir_all(e.path("C")).unwrap();
    let id = e.record(RelocationStatus::Done);
    e.reparse
        .create_junction(&e.path("C/Slack"), &e.path("Elsewhere"))
        .unwrap();

    assert!(undo(&e, id, &e.reparse).is_err());

    assert_eq!(e.link(), Some(e.path("Elsewhere")));
    assert!(e.path("D/Slack/a.db").exists());
    assert!(leftovers(&e, "restoring.").is_empty());
}
