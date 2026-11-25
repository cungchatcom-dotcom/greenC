//! End-to-end tests for hidden `greenc-cli __restore-helper` subprocess protocol and
//! `greenc-cli vault restore --to <non-AppData dir>` fallback restoration.

#![deny(unsafe_code)]

use clap::Parser;
use greenc_cli::{execute_cli, run_restore_helper_request, Cli, EXIT_SUCCESS};
use greenc_core::models::{CleanCandidate, VaultStatus};
use greenc_core::task::TaskControl;
use greenc_platform::breakaway::{
    spawn_restore_helper_breakaway, HelperConflictStrategy, PrivateBreakawayJobDir, RestoreFileJob,
    RestoreHelperRequest, RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE,
};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{ReparsePointApi, Win32VolumeApi};
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::AppSettings;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::manifest::compute_file_sha256;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;
use tempfile::tempdir;

const CLI_BIN: &str = env!("CARGO_BIN_EXE_greenc-cli");

struct DummyVol;
impl Win32VolumeApi for DummyVol {
    fn list_drives(&self) -> greenc_core::error::Result<Vec<greenc_core::models::DriveInfo>> {
        Ok(vec![])
    }
    fn get_disk_free_space(&self, _path: &Path) -> greenc_core::error::Result<(u64, u64)> {
        Ok((100 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024))
    }
    fn get_volume_info(
        &self,
        _drive_root: &Path,
    ) -> greenc_core::error::Result<(String, String, u32)> {
        Ok(("Data".to_string(), "NTFS".to_string(), 3))
    }
    fn is_elevated(&self) -> bool {
        false
    }
}

#[test]
fn restore_helper_rejects_arbitrary_cli_path_arguments_and_is_hidden_from_help() {
    // 1. Must NOT appear in `--help` output.
    let help = Command::new(CLI_BIN).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        !help_text.contains("__restore-helper"),
        "hidden helper must not be advertised in --help"
    );

    // 2. Must reject any positional or flag path arguments on the command line.
    let bad = Command::new(CLI_BIN)
        .args(["__restore-helper", "/tmp/arbitrary.txt"])
        .output()
        .unwrap();
    assert!(
        !bad.status.success(),
        "__restore-helper must refuse arbitrary CLI path arguments"
    );
}

#[test]
fn restore_helper_real_subprocess_restores_and_verifies_files_staged_dir_and_undo_relocation() {
    let tmp = tempdir().unwrap();

    // --- A. RestoreFiles end-to-end via real subprocess ---
    let job_dir = PrivateBreakawayJobDir::create_in(&tmp.path().join("jobs")).unwrap();
    let payload_file = job_dir.payload_dir().join("item_0.bin");
    let content = b"MSIX-breakaway-subprocess-verified-content-2026";
    std::fs::write(&payload_file, content).unwrap();
    let expected_sha = compute_file_sha256(&payload_file).unwrap();

    let dest = tmp
        .path()
        .join("Users")
        .join("Alice")
        .join("AppData")
        .join("Local")
        .join("Google")
        .join("Chrome")
        .join("Cache")
        .join("f_000001");
    let req = RestoreHelperRequest::RestoreFiles {
        payload_root: job_dir.payload_dir().to_path_buf(),
        conflict_strategy: HelperConflictStrategy::OverwriteIfZeroLength,
        verify_sha256: true,
        restore_hardlinks: false,
        restore_timestamps: true,
        items: vec![RestoreFileJob {
            original_path: dest.clone(),
            staged_source: payload_file,
            dest_path: dest.clone(),
            expected_sha256: expected_sha.clone(),
            expected_size: content.len() as u64,
            atime: 1_700_000_000.0,
            mtime: 1_700_000_000.0,
            hardlink_siblings: vec![],
        }],
    };
    let exec_out = spawn_restore_helper_breakaway(
        Path::new(CLI_BIN),
        &job_dir,
        &serde_json::to_vec(&req).unwrap(),
    )
    .unwrap();
    assert_eq!(
        exec_out.exit_code, 0,
        "stdout={} stderr={}",
        exec_out.stdout, exec_out.stderr
    );
    let resp: RestoreHelperResponse = serde_json::from_str(&exec_out.stdout).unwrap();
    assert!(resp.ok);
    assert!(resp.running_outside_package);
    assert_eq!(resp.files.len(), 1);
    assert!(resp.files[0].verified_exists);
    assert_eq!(resp.files[0].verified_sha256, expected_sha);
    assert_eq!(std::fs::read(&dest).unwrap(), content);

    // --- B. RestoreFiles rejects tampered SHA-256 ---
    let bad_dest = tmp.path().join("bad_dest.bin");
    let bad_payload = job_dir.payload_dir().join("item_bad.bin");
    std::fs::write(&bad_payload, b"actual-bytes").unwrap();
    let bad_req = RestoreHelperRequest::RestoreFiles {
        payload_root: job_dir.payload_dir().to_path_buf(),
        conflict_strategy: HelperConflictStrategy::Overwrite,
        verify_sha256: true,
        restore_hardlinks: false,
        restore_timestamps: false,
        items: vec![RestoreFileJob {
            original_path: bad_dest.clone(),
            staged_source: bad_payload,
            dest_path: bad_dest.clone(),
            expected_sha256: "0".repeat(64),
            expected_size: 12,
            atime: 0.0,
            mtime: 0.0,
            hardlink_siblings: vec![],
        }],
    };
    let bad_out = spawn_restore_helper_breakaway(
        Path::new(CLI_BIN),
        &job_dir,
        &serde_json::to_vec(&bad_req).unwrap(),
    )
    .unwrap();
    assert_ne!(bad_out.exit_code, 0);
    let bad_resp: RestoreHelperResponse = serde_json::from_str(&bad_out.stdout).unwrap();
    assert!(!bad_resp.ok);
    assert!(!bad_dest.exists());

    // --- C. RestoreStagedDir end-to-end via real subprocess ---
    let staged_dir = tmp.path().join("staging_out").join("unit_1_2_3");
    std::fs::create_dir_all(&staged_dir).unwrap();
    std::fs::write(staged_dir.join("data.bin"), b"staged-cache").unwrap();
    let orig_dir = tmp
        .path()
        .join("AppData")
        .join("Local")
        .join("pip")
        .join("unit");
    let staged_req = RestoreHelperRequest::RestoreStagedDir {
        staged_dir: staged_dir.clone(),
        original_path: orig_dir.clone(),
        expected_dev: None,
        expected_ino: None,
    };
    let staged_out = spawn_restore_helper_breakaway(
        Path::new(CLI_BIN),
        &job_dir,
        &serde_json::to_vec(&staged_req).unwrap(),
    )
    .unwrap();
    assert_eq!(staged_out.exit_code, 0, "stdout={}", staged_out.stdout);
    let staged_resp: RestoreHelperResponse = serde_json::from_str(&staged_out.stdout).unwrap();
    assert!(staged_resp.ok);
    assert_eq!(
        staged_resp.verified_dir.as_deref(),
        Some(orig_dir.as_path())
    );
    assert_eq!(
        std::fs::read(orig_dir.join("data.bin")).unwrap(),
        b"staged-cache"
    );

    // --- D. UndoRelocationDir end-to-end via real subprocess ---
    let reparse = NativeReparsePointApi;
    let reloc_src = tmp.path().join("AppData").join("Roaming").join("MyApp");
    let reloc_tgt = tmp.path().join("D_Drive").join("MyApp");
    std::fs::create_dir_all(reloc_src.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&reloc_tgt).unwrap();
    std::fs::write(reloc_tgt.join("state.dat"), b"relocated-state").unwrap();
    reparse.create_junction(&reloc_src, &reloc_tgt).unwrap();

    let undo_req = RestoreHelperRequest::UndoRelocationDir {
        source_path: reloc_src.clone(),
        target_path: reloc_tgt.clone(),
    };
    let undo_out = spawn_restore_helper_breakaway(
        Path::new(CLI_BIN),
        &job_dir,
        &serde_json::to_vec(&undo_req).unwrap(),
    )
    .unwrap();
    assert_eq!(undo_out.exit_code, 0, "stdout={}", undo_out.stdout);
    let undo_resp: RestoreHelperResponse = serde_json::from_str(&undo_out.stdout).unwrap();
    assert!(undo_resp.ok);
    assert_eq!(undo_resp.verified_dir.as_deref(), Some(reloc_src.as_path()));
    assert!(!reparse.is_reparse_point(&reloc_src));
    assert_eq!(
        std::fs::read(reloc_src.join("state.dat")).unwrap(),
        b"relocated-state"
    );
}

#[test]
fn restore_helper_self_check_exits_with_still_in_package_code() {
    let tmp = tempdir().unwrap();
    let req = RestoreHelperRequest::RestoreStagedDir {
        staged_dir: tmp.path().join("s"),
        original_path: tmp.path().join("o"),
        expected_dev: None,
        expected_ino: None,
    };
    let (code, resp) = run_restore_helper_request(req, true);
    assert_eq!(code, EXIT_HELPER_STILL_IN_PACKAGE);
    assert!(!resp.ok);
    assert!(!resp.running_outside_package);
    assert_eq!(resp.error_code.as_deref(), Some("still_in_package"));
}

#[test]
fn cli_vault_restore_with_to_flag_restores_outside_appdata_cleanly() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("greenc.db");
    let vault_dir = tmp.path().join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();

    let victim_dir = tmp.path().join("AppData").join("Local").join("Chrome");
    std::fs::create_dir_all(&victim_dir).unwrap();
    let victim = victim_dir.join("cache_01.bin");
    let payload = b"restore-to-documents-fallback-data";
    std::fs::write(&victim, payload).unwrap();
    let expected_sha = compute_file_sha256(&victim).unwrap();

    let archive_id_str = {
        let db = GreenCDb::open(&db_path).unwrap();
        AppSettings::new(&db).set_data_root(tmp.path()).unwrap();
        let ctrl = TaskControl::new();
        let pipeline = ArchiverPipeline::new(ArchiveOptions {
            vault_dir: vault_dir.clone(),
            retention_days: 7,
            min_vault_reserve_bytes: 0,
            ..Default::default()
        });
        let manifest = pipeline
            .archive_candidates(
                0,
                "Chrome",
                &[CleanCandidate {
                    path: victim.clone(),
                    size_bytes: payload.len() as u64,
                    mtime: SystemTime::now(),
                    is_locked: false,
                    profile_id: "browser_cache".to_string(),
                    is_canary: false,
                    is_orphan: false,
                    hardlink_siblings: vec![victim.clone()],
                }],
                &DummyVol,
                &ctrl,
                |_| {},
            )
            .unwrap();
        let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
        let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
        let id = manifest.persist_to_db(&db, &zip_p, &man_p).unwrap();
        std::fs::remove_file(&victim).unwrap();
        id.to_string()
    };

    let alt_dir = tmp.path().join("Documents").join("GreenC Restored");
    let cli = Cli::try_parse_from([
        "greenc",
        "vault",
        "restore",
        &archive_id_str,
        "--to",
        &alt_dir.to_string_lossy(),
        "--json",
    ])
    .unwrap();
    let ctrl = Arc::new(TaskControl::new());
    let outcome = execute_cli(cli, &db_path, &ctrl);
    assert_eq!(outcome.exit_code, EXIT_SUCCESS, "stderr={}", outcome.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(parsed["status"], "RESTORED");
    assert_eq!(parsed["restored_files_count"], 1);
    let restored_path = Path::new(parsed["restored_paths"][0].as_str().unwrap());
    assert!(restored_path.starts_with(&alt_dir));
    assert_eq!(compute_file_sha256(restored_path).unwrap(), expected_sha);

    let db = GreenCDb::open(&db_path).unwrap();
    let arch = db
        .get_vault_archive(archive_id_str.parse::<i64>().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(arch.status, VaultStatus::RolledBack);
}
