//! TDD tests for Microsoft Store MSIX breakaway Vault restoration, outside-package SHA-256
//! verification, package-escape failure fallback guidance, and alternate non-AppData target folder.

use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{CleanCandidate, VaultStatus};
use greenc_core::task::TaskControl;
use greenc_platform::breakaway::{
    BreakawayExecOutput, BreakawayHelperClient, HelperConflictStrategy, PrivateBreakawayJobDir,
    RestoreFileJob, RestoreHelperRequest, RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE,
};
use greenc_platform::traits::Win32VolumeApi;
use greenc_storage::db::GreenCDb;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::manifest::compute_file_sha256;
use greenc_vault::rollback::{
    execute_restore_files_job, map_path_into_restore_dir, RestoreConflictStrategy, RestoreOptions,
    RollbackEngine,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

/// The default temp dir sits under AppData\Local\Temp on Windows, which is deliberately excluded from
/// the AppData rule, so fake AppData trees live in the user profile there.
fn fake_profile() -> tempfile::TempDir {
    #[cfg(windows)]
    return tempfile::Builder::new()
        .prefix(".greenc-test-")
        .tempdir_in(std::env::var_os("USERPROFILE").unwrap())
        .unwrap();
    #[cfg(not(windows))]
    tempfile::tempdir().unwrap()
}

#[derive(Clone)]
#[cfg_attr(not(feature = "store"), allow(dead_code))]
enum MockHelperBehavior {
    /// Delegates to the real `execute_restore_files_job(..., running_in_package = false)`.
    RealUnpackaged,
    /// Simulates breakaway failing because the helper is still inside the MSIX package.
    StillInPackage,
    /// Simulates Windows refusing to spawn the breakaway process (policy / older OS).
    SpawnFailed,
    /// Simulates a helper claiming ok=true while reporting a wrong SHA-256 (or missing file).
    TamperedVerification {
        verified_exists: bool,
        fake_sha256: String,
    },
}

struct MockBreakawayClient {
    packaged: bool,
    behavior: MockHelperBehavior,
    calls: Arc<AtomicUsize>,
}

impl BreakawayHelperClient for MockBreakawayClient {
    fn is_packaged(&self) -> bool {
        self.packaged
    }

    fn execute(
        &self,
        _job_dir: &PrivateBreakawayJobDir,
        stdin_json: &[u8],
    ) -> Result<BreakawayExecOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let req: RestoreHelperRequest = serde_json::from_slice(stdin_json)?;
        match &self.behavior {
            MockHelperBehavior::RealUnpackaged => {
                let (code, resp) = match req {
                    RestoreHelperRequest::RestoreFiles {
                        payload_root,
                        conflict_strategy,
                        verify_sha256,
                        restore_hardlinks,
                        restore_timestamps,
                        items,
                    } => execute_restore_files_job(
                        &payload_root,
                        conflict_strategy,
                        verify_sha256,
                        restore_hardlinks,
                        restore_timestamps,
                        &items,
                        false,
                    ),
                    RestoreHelperRequest::RestoreStagedDir {
                        staged_dir,
                        original_path,
                        expected_dev,
                        expected_ino,
                    } => greenc_vault::guard::execute_restore_staged_dir_job(
                        &staged_dir,
                        &original_path,
                        expected_dev,
                        expected_ino,
                        false,
                    ),
                    _ => (
                        1,
                        RestoreHelperResponse::failed("unsupported", "unexpected job kind"),
                    ),
                };
                Ok(BreakawayExecOutput {
                    exit_code: code,
                    stdout: serde_json::to_string(&resp)?,
                    stderr: String::new(),
                })
            }
            MockHelperBehavior::StillInPackage => {
                let (code, resp) = match req {
                    RestoreHelperRequest::RestoreFiles {
                        payload_root,
                        conflict_strategy,
                        verify_sha256,
                        restore_hardlinks,
                        restore_timestamps,
                        items,
                    } => execute_restore_files_job(
                        &payload_root,
                        conflict_strategy,
                        verify_sha256,
                        restore_hardlinks,
                        restore_timestamps,
                        &items,
                        true,
                    ),
                    RestoreHelperRequest::RestoreStagedDir {
                        staged_dir,
                        original_path,
                        expected_dev,
                        expected_ino,
                    } => greenc_vault::guard::execute_restore_staged_dir_job(
                        &staged_dir,
                        &original_path,
                        expected_dev,
                        expected_ino,
                        true,
                    ),
                    _ => (
                        EXIT_HELPER_STILL_IN_PACKAGE,
                        RestoreHelperResponse::still_in_package(Some("CungChat.GreenC")),
                    ),
                };
                Ok(BreakawayExecOutput {
                    exit_code: code,
                    stdout: serde_json::to_string(&resp)?,
                    stderr: String::new(),
                })
            }
            MockHelperBehavior::SpawnFailed => Err(GreenCError::Win32 {
                code: 5,
                message: "CreateProcessW denied by desktop app breakaway policy".to_string(),
            }),
            MockHelperBehavior::TamperedVerification {
                verified_exists,
                fake_sha256,
            } => {
                let RestoreHelperRequest::RestoreFiles { items, .. } = req else {
                    unreachable!();
                };
                let resp = RestoreHelperResponse {
                    ok: true,
                    running_outside_package: true,
                    error_code: None,
                    error: None,
                    files: items
                        .into_iter()
                        .map(|job| greenc_platform::breakaway::RestoreFileOutcome {
                            original_path: job.original_path,
                            restored_path: job.dest_path,
                            bytes_written: job.expected_size,
                            verified_exists: *verified_exists,
                            verified_sha256: fake_sha256.clone(),
                            restored_siblings: vec![],
                        })
                        .collect(),
                    verified_dir: None,
                    verified_file_count: 1,
                    verified_bytes: 16,
                };
                Ok(BreakawayExecOutput {
                    exit_code: 0,
                    stdout: serde_json::to_string(&resp)?,
                    stderr: String::new(),
                })
            }
        }
    }
}

struct TestVolApi;

impl Win32VolumeApi for TestVolApi {
    fn list_drives(&self) -> Result<Vec<greenc_core::models::DriveInfo>> {
        Ok(vec![])
    }

    fn get_disk_free_space(&self, _path: &Path) -> Result<(u64, u64)> {
        Ok((100 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024))
    }

    fn get_volume_info(&self, _drive_root: &Path) -> Result<(String, String, u32)> {
        Ok(("Data".to_string(), "NTFS".to_string(), 3))
    }

    fn is_elevated(&self) -> bool {
        false
    }
}

fn seed_appdata_vault_archive(
    root: &Path,
    payload: &[u8],
) -> (GreenCDb, PathBuf, PathBuf, PathBuf, PathBuf, i64, String) {
    let vault_dir = root.join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let appdata_cache = root
        .join("Users")
        .join("Alice")
        .join("AppData")
        .join("Local")
        .join("Google")
        .join("Chrome")
        .join("Cache");
    std::fs::create_dir_all(&appdata_cache).unwrap();

    let victim = appdata_cache.join("f_000001");
    std::fs::write(&victim, payload).unwrap();
    let expected_sha = compute_file_sha256(&victim).unwrap();

    let db = GreenCDb::open(root.join("greenc.db")).unwrap();
    let vol_api = TestVolApi;
    let ctrl = TaskControl::new();
    let candidate = CleanCandidate {
        path: victim.clone(),
        size_bytes: payload.len() as u64,
        mtime: SystemTime::now(),
        is_locked: false,
        profile_id: "browser_cache".to_string(),
        is_canary: false,
        is_orphan: false,
        hardlink_siblings: vec![victim.clone()],
    };
    let pipeline = ArchiverPipeline::new(ArchiveOptions {
        vault_dir: vault_dir.clone(),
        retention_days: 7,
        min_vault_reserve_bytes: 0,
        ..Default::default()
    });
    let manifest = pipeline
        .archive_candidates(0, "BrowserCache", &[candidate], &vol_api, &ctrl, |_| {})
        .unwrap();
    let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
    let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
    let archive_id = manifest.persist_to_db(&db, &zip_p, &man_p).unwrap();

    // Delete original file just like `clean` does.
    std::fs::remove_file(&victim).unwrap();
    assert!(!victim.exists());

    (
        db,
        vault_dir,
        victim,
        zip_p,
        man_p,
        archive_id,
        expected_sha,
    )
}

#[test]
fn store_packaged_restore_uses_breakaway_helper_and_verifies_sha256_while_standard_is_unchanged() {
    let tmp = fake_profile();
    let payload = b"real-chrome-cache-payload-for-msix-restore-verification";
    let (db, vault_dir, victim, zip_p, man_p, archive_id, expected_sha) =
        seed_appdata_vault_archive(tmp.path(), payload);

    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(MockBreakawayClient {
        packaged: true,
        behavior: MockHelperBehavior::RealUnpackaged,
        calls: Arc::clone(&calls),
    });

    let engine = RollbackEngine::new(
        &db,
        RestoreOptions {
            vault_dir: Some(vault_dir),
            breakaway_client: Some(client),
            ..Default::default()
        },
    );

    let res = engine
        .rollback_archive(archive_id, "store restore test", false)
        .expect("restore should succeed");

    assert_eq!(res.restored_count, 1);
    assert!(victim.is_file(), "real AppData file must exist on disk");
    assert_eq!(std::fs::read(&victim).unwrap(), payload);
    assert_eq!(compute_file_sha256(&victim).unwrap(), expected_sha);

    if cfg!(feature = "store") {
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "Store packaged build MUST invoke the breakaway helper for AppData restores"
        );
    } else {
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "Standard build MUST NOT change behavior or spawn breakaway helper"
        );
    }

    let arch = db.get_vault_archive(archive_id).unwrap().unwrap();
    assert_eq!(arch.status, VaultStatus::RolledBack);
    assert!(!zip_p.exists());
    assert!(!man_p.exists());
}

#[cfg(feature = "store")]
#[test]
fn store_breakaway_refused_still_in_package_keeps_vault_and_guides_to_non_appdata_folder() {
    let tmp = fake_profile();
    let payload = b"important-appdata-file-must-not-be-lost";
    let (db, vault_dir, victim, zip_p, man_p, archive_id, expected_sha) =
        seed_appdata_vault_archive(tmp.path(), payload);

    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(MockBreakawayClient {
        packaged: true,
        behavior: MockHelperBehavior::StillInPackage,
        calls: Arc::clone(&calls),
    });

    let engine = RollbackEngine::new(
        &db,
        RestoreOptions {
            vault_dir: Some(vault_dir.clone()),
            breakaway_client: Some(Arc::clone(&client) as Arc<dyn BreakawayHelperClient>),
            ..Default::default()
        },
    );

    let err = engine
        .rollback_archive(archive_id, "manual restore", false)
        .expect_err("must fail when breakaway helper is still inside MSIX package");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    match &err {
        GreenCError::PreconditionBlocked { operation, reason } => {
            assert_eq!(operation, "vault_restore_store_breakaway");
            assert!(
                reason.contains("GreenC Restored") && reason.contains("--to"),
                "guidance must tell user to pick a non-AppData folder like Documents\\GreenC Restored: {reason}"
            );
            assert!(reason.contains("GIỮ NGUYÊN"));
        }
        other => panic!("expected PreconditionBlocked with guidance, got: {other:?}"),
    }

    // Archive MUST remain intact and QUARANTINED (never marked RolledBack, never deleted).
    assert!(zip_p.is_file(), "Vault ZIP must be preserved");
    assert!(man_p.is_file(), "Vault manifest must be preserved");
    assert!(!victim.exists(), "must not have written anything to victim");
    let arch = db.get_vault_archive(archive_id).unwrap().unwrap();
    assert_eq!(arch.status, VaultStatus::Quarantined);

    // Fallback: user follows guidance and restores to a folder OUTSIDE AppData (`GreenC Restored`).
    let alt_dir = tmp.path().join("Documents").join("GreenC Restored");
    let fallback_engine = RollbackEngine::new(
        &db,
        RestoreOptions {
            vault_dir: Some(vault_dir),
            restore_dir: Some(alt_dir.clone()),
            breakaway_client: Some(client),
            ..Default::default()
        },
    );
    let fallback_res = fallback_engine
        .rollback_archive(archive_id, "restore to alternate folder", false)
        .expect("restoring outside AppData must succeed even when breakaway is unavailable");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "non-AppData fallback restore must not need breakaway"
    );
    assert_eq!(fallback_res.restored_count, 1);
    let restored_file = &fallback_res.restored_paths[0];
    assert!(restored_file.starts_with(&alt_dir));
    assert_eq!(compute_file_sha256(restored_file).unwrap(), expected_sha);
    assert_eq!(
        db.get_vault_archive(archive_id).unwrap().unwrap().status,
        VaultStatus::RolledBack
    );
}

#[cfg(feature = "store")]
#[test]
fn store_breakaway_spawn_failure_also_keeps_vault_and_guides_user() {
    let tmp = fake_profile();
    let (db, vault_dir, _victim, zip_p, man_p, archive_id, _) =
        seed_appdata_vault_archive(tmp.path(), b"older-windows-policy-test");

    let client = Arc::new(MockBreakawayClient {
        packaged: true,
        behavior: MockHelperBehavior::SpawnFailed,
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let engine = RollbackEngine::new(
        &db,
        RestoreOptions {
            vault_dir: Some(vault_dir),
            breakaway_client: Some(client),
            ..Default::default()
        },
    );

    let err = engine
        .rollback_archive(archive_id, "restore", false)
        .unwrap_err();
    assert!(matches!(err, GreenCError::PreconditionBlocked { .. }));
    assert!(zip_p.is_file());
    assert!(man_p.is_file());
    assert_eq!(
        db.get_vault_archive(archive_id).unwrap().unwrap().status,
        VaultStatus::Quarantined
    );
}

#[cfg(feature = "store")]
#[test]
fn store_verification_mismatch_or_missing_file_fails_and_keeps_vault_intact() {
    for (verified_exists, fake_sha) in [(false, "a".repeat(64)), (true, "0".repeat(64))] {
        let tmp = fake_profile();
        let (db, vault_dir, _victim, zip_p, man_p, archive_id, _) =
            seed_appdata_vault_archive(tmp.path(), b"verify-must-catch-mismatch");

        let client = Arc::new(MockBreakawayClient {
            packaged: true,
            behavior: MockHelperBehavior::TamperedVerification {
                verified_exists,
                fake_sha256: fake_sha,
            },
            calls: Arc::new(AtomicUsize::new(0)),
        });
        let engine = RollbackEngine::new(
            &db,
            RestoreOptions {
                vault_dir: Some(vault_dir),
                breakaway_client: Some(client),
                ..Default::default()
            },
        );

        let err = engine
            .rollback_archive(archive_id, "restore", false)
            .expect_err("verification failure must NEVER succeed or report RESTORED");
        if verified_exists {
            assert!(matches!(err, GreenCError::IntegrityMismatch { .. }));
        } else {
            assert!(matches!(err, GreenCError::Vault(_)));
        }

        // Vault MUST stay QUARANTINED and untouched!
        assert!(
            zip_p.is_file(),
            "Vault ZIP must remain when verification fails"
        );
        assert!(
            man_p.is_file(),
            "Vault manifest must remain when verification fails"
        );
        assert_eq!(
            db.get_vault_archive(archive_id).unwrap().unwrap().status,
            VaultStatus::Quarantined
        );
    }
}

#[test]
fn helper_job_rejects_in_package_and_unsafe_paths_and_verifies_real_sha256() {
    let tmp = fake_profile();
    let payload_root = tmp.path().join("payload");
    std::fs::create_dir_all(&payload_root).unwrap();
    let staged = payload_root.join("item_0.bin");
    let content = b"verified-helper-payload";
    std::fs::write(&staged, content).unwrap();
    let good_sha = compute_file_sha256(&staged).unwrap();

    let dest = tmp.path().join("dest").join("restored.bin");
    let job = RestoreFileJob {
        original_path: dest.clone(),
        staged_source: staged.clone(),
        dest_path: dest.clone(),
        expected_sha256: good_sha.clone(),
        expected_size: content.len() as u64,
        atime: 1_700_000_000.0,
        mtime: 1_700_000_000.0,
        hardlink_siblings: vec![],
    };

    // 1. Self-check: still in MSIX package -> exits with EXIT_HELPER_STILL_IN_PACKAGE and writes nothing
    let (code, resp) = execute_restore_files_job(
        &payload_root,
        HelperConflictStrategy::OverwriteIfZeroLength,
        true,
        false,
        true,
        std::slice::from_ref(&job),
        true,
    );
    assert_eq!(code, EXIT_HELPER_STILL_IN_PACKAGE);
    assert!(!resp.ok);
    assert!(!resp.running_outside_package);
    assert!(!dest.exists());

    // 2. Rejects traversal (`..`) destination path
    let mut bad_job = job.clone();
    bad_job.dest_path = tmp.path().join("dest").join("..").join("escape.bin");
    let (code, resp) = execute_restore_files_job(
        &payload_root,
        HelperConflictStrategy::Overwrite,
        true,
        false,
        true,
        &[bad_job],
        false,
    );
    assert_ne!(code, 0);
    assert!(!resp.ok);

    // 3. Rejects staged source outside payload_root
    let outside = tmp.path().join("outside.bin");
    std::fs::write(&outside, content).unwrap();
    let mut escape_src_job = job.clone();
    escape_src_job.staged_source = outside;
    let (code, resp) = execute_restore_files_job(
        &payload_root,
        HelperConflictStrategy::Overwrite,
        true,
        false,
        true,
        &[escape_src_job],
        false,
    );
    assert_ne!(code, 0);
    assert!(!resp.ok);

    // 4. Rejects SHA-256 mismatch and cleans up bad file
    let mut bad_hash_job = job.clone();
    bad_hash_job.expected_sha256 = "f".repeat(64);
    let (code, resp) = execute_restore_files_job(
        &payload_root,
        HelperConflictStrategy::Overwrite,
        true,
        false,
        true,
        &[bad_hash_job],
        false,
    );
    assert_ne!(code, 0);
    assert!(!resp.ok);
    assert!(!dest.exists(), "mismatched output must be deleted");

    // 5. Succeeds outside package and verifies real file + SHA-256
    let (code, resp) = execute_restore_files_job(
        &payload_root,
        HelperConflictStrategy::OverwriteIfZeroLength,
        true,
        false,
        true,
        &[job],
        false,
    );
    assert_eq!(code, 0);
    assert!(resp.ok);
    assert!(resp.running_outside_package);
    assert_eq!(resp.files.len(), 1);
    assert!(resp.files[0].verified_exists);
    assert_eq!(resp.files[0].verified_sha256, good_sha);
    assert_eq!(std::fs::read(&dest).unwrap(), content);
}

#[test]
fn map_path_into_restore_dir_preserves_structure_and_blocks_traversal() {
    let tmp = fake_profile();
    let out_dir = tmp.path().join("GreenC Restored");
    let mapped = map_path_into_restore_dir(
        Path::new(r"C:\Users\Alice\AppData\Local\Google\Chrome\f_000001"),
        &out_dir,
    )
    .unwrap();
    assert_eq!(
        mapped,
        out_dir
            .join("C")
            .join("Users")
            .join("Alice")
            .join("AppData")
            .join("Local")
            .join("Google")
            .join("Chrome")
            .join("f_000001")
    );

    assert!(
        map_path_into_restore_dir(Path::new(r"C:\Users\Alice\..\Bob\secret.dat"), &out_dir)
            .is_err()
    );

    let _ = RestoreConflictStrategy::Overwrite;
}

#[test]
fn store_staging_recovery_into_appdata_uses_breakaway_and_preserves_on_refusal() {
    use greenc_platform::dev_tools::BusyToolsStatus;
    use greenc_platform::handle::query_file_identity;
    use greenc_vault::cache_units::CleanUnitKind;
    use greenc_vault::guard::{
        recover_staging_dir_with_options, resolve_staging_dir_checked, StagingJournalEntry,
        StagingMode, StagingRecoveryOptions, StagingState,
    };

    let tmp = fake_profile();
    let appdata_cache_root = tmp
        .path()
        .join("Users")
        .join("Alice")
        .join("AppData")
        .join("Local")
        .join("pip")
        .join("cache");
    std::fs::create_dir_all(&appdata_cache_root).unwrap();

    // 1. Explicit staging dir inside AppData must be rejected in Store builds.
    let bad_explicit = tmp
        .path()
        .join("Users")
        .join("Alice")
        .join("AppData")
        .join("Local")
        .join("CustomStaging");
    let fake_home = tmp.path().join("UserHome");
    let unit_dir = appdata_cache_root.join("http");
    std::fs::create_dir_all(&bad_explicit).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();
    std::fs::create_dir_all(&unit_dir).unwrap();
    let chosen = resolve_staging_dir_checked(
        &unit_dir,
        &appdata_cache_root,
        &bad_explicit,
        Some(&bad_explicit),
        Some(&fake_home),
        None,
    )
    .expect("staging dir should fall back outside AppData");
    if cfg!(feature = "store") {
        assert!(
            !chosen.starts_with(&bad_explicit),
            "Store build must reject explicit staging dir inside AppData, got {}",
            chosen.display()
        );
    }

    // 2. Recovering a staged unit whose `original_path` is inside AppData:
    let staging_dir = tmp.path().join("store_staging");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&staging_dir)
            .unwrap();
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&staging_dir).unwrap();

    let unit_id = "unit_1700000000000_1234_1";
    let staged_path = staging_dir.join(unit_id);
    std::fs::create_dir_all(&staged_path).unwrap();
    std::fs::write(staged_path.join("wheel.bin"), b"cached-wheel").unwrap();
    let phys = query_file_identity(&staged_path).unwrap();

    let original_path = appdata_cache_root.join("recovered_unit");
    let journal = StagingJournalEntry {
        version: 2,
        id: unit_id.to_string(),
        original_path: original_path.clone(),
        staged_path: staged_path.clone(),
        mode: StagingMode::Vault,
        state: StagingState::Staged,
        created_at: chrono::Utc::now(),
        drive_root: "/".to_string(),
        unit_label: "pip cache".to_string(),
        dev_tool: None,
        note: None,
        staged_dev: Some(phys.volume_serial),
        staged_ino: Some(phys.file_index),
        unit_kind: Some(CleanUnitKind::RegenerableDevCache),
    };
    let jpath = staging_dir.join(format!("{unit_id}.journal.json"));
    std::fs::write(&jpath, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();

    // First test: Store packaged + StillInPackage -> keeps staged unit safely in staging (conflict).
    #[cfg(feature = "store")]
    {
        let calls_blocked = Arc::new(AtomicUsize::new(0));
        let blocked_client = Arc::new(MockBreakawayClient {
            packaged: true,
            behavior: MockHelperBehavior::StillInPackage,
            calls: Arc::clone(&calls_blocked),
        });
        let report_blocked = recover_staging_dir_with_options(
            &staging_dir,
            &BusyToolsStatus::idle(),
            &StagingRecoveryOptions {
                allowed_cache_roots: Some(vec![appdata_cache_root.clone()]),
                #[cfg(unix)]
                expected_uid: None,
                breakaway_client: Some(blocked_client),
            },
        );
        assert_eq!(calls_blocked.load(Ordering::SeqCst), 1);
        assert_eq!(report_blocked.restored_to_original, 0);
        assert!(!report_blocked.conflicts.is_empty());
        assert!(
            staged_path.is_dir(),
            "staged directory must remain intact when breakaway is unavailable"
        );
        assert!(!original_path.exists());

        // Reset journal back to Staged so we can verify successful breakaway restore next.
        std::fs::write(&jpath, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    }

    // Second test: Packaged + RealUnpackaged -> restores to real AppData path via breakaway in Store!
    let calls_ok = Arc::new(AtomicUsize::new(0));
    let ok_client = Arc::new(MockBreakawayClient {
        packaged: true,
        behavior: MockHelperBehavior::RealUnpackaged,
        calls: Arc::clone(&calls_ok),
    });
    let report_ok = recover_staging_dir_with_options(
        &staging_dir,
        &BusyToolsStatus::idle(),
        &StagingRecoveryOptions {
            allowed_cache_roots: Some(vec![appdata_cache_root]),
            #[cfg(unix)]
            expected_uid: None,
            breakaway_client: Some(ok_client),
        },
    );
    assert_eq!(report_ok.restored_to_original, 1);
    assert!(original_path.join("wheel.bin").is_file());
    assert!(!staged_path.exists());
    assert!(!jpath.exists());
    if cfg!(feature = "store") {
        assert_eq!(
            calls_ok.load(Ordering::SeqCst),
            1,
            "Store packaged staging recovery into AppData must run via breakaway helper"
        );
    } else {
        assert_eq!(
            calls_ok.load(Ordering::SeqCst),
            0,
            "Standard build must rename directly without breakaway helper"
        );
    }
}
