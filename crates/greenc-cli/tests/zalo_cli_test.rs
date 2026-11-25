//! Integration tests for `greenc-cli zalo status|clean|move` and exit-code contract compliance.

#![deny(unsafe_code)]

use clap::Parser;
use greenc_cli::{
    execute_cli, Cli, EXIT_CANCELLED, EXIT_OPERATIONAL_ERROR, EXIT_PRECONDITION_BLOCKED,
    EXIT_SUCCESS,
};
use greenc_core::task::TaskControl;
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::AppSettings;
use std::sync::Arc;
use tempfile::tempdir;

/// Points every profile lookup at an empty temp directory so these tests never read (or act on) the
/// developer's real Zalo installation.
fn isolate_profile_env() {
    static ENV: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ENV.get_or_init(|| {
        let dir = tempdir().unwrap();
        for key in ["APPDATA", "LOCALAPPDATA", "HOME", "USERPROFILE"] {
            std::env::set_var(key, dir.path());
        }
        dir
    });
}

fn run_args(args: &[&str], db_path: &std::path::Path) -> greenc_cli::CliOutcome {
    isolate_profile_env();
    let cli = Cli::try_parse_from(args).expect("CLI arguments should parse cleanly");
    let ctrl = Arc::new(TaskControl::new());
    execute_cli(cli, db_path, &ctrl)
}

#[test]
fn cli_zalo_status_and_clean_dry_run_json_contract() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("zalo_cli.db");

    let status_out = run_args(&["greenc", "zalo", "status", "--json"], &db_path);
    assert_eq!(status_out.exit_code, EXIT_SUCCESS);
    let parsed: serde_json::Value = serde_json::from_str(&status_out.stdout).unwrap();
    assert!(parsed.get("installed").and_then(|v| v.as_bool()).is_some());
    assert!(parsed
        .get("auto_move_supported")
        .and_then(|v| v.as_bool())
        .is_some());

    let clean_out = run_args(
        &["greenc", "zalo", "clean", "--dry-run", "--json"],
        &db_path,
    );
    assert_eq!(clean_out.exit_code, EXIT_SUCCESS);
    let clean_parsed: serde_json::Value = serde_json::from_str(&clean_out.stdout).unwrap();
    assert_eq!(
        clean_parsed.get("dry_run").and_then(|v| v.as_bool()),
        Some(true)
    );
}

#[test]
fn cli_zalo_move_parses_flags_and_enforces_exit_codes() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("zalo_move.db");

    // 1. Invalid target drive -> exit 2 (PreconditionBlocked)
    for bad in ["123", "invalid", "D:\\foo"] {
        let bad_drive = run_args(
            &["greenc", "zalo", "move", "--to", bad, "--dry-run", "--json"],
            &db_path,
        );
        assert_eq!(bad_drive.exit_code, EXIT_PRECONDITION_BLOCKED);
    }

    // 2. Non-NTFS argument -> exit 2 (PreconditionBlocked)
    let fat32 = run_args(
        &[
            "greenc",
            "zalo",
            "move",
            "--to",
            "fat32",
            "--dry-run",
            "--no-reopen",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(fat32.exit_code, EXIT_PRECONDITION_BLOCKED);

    // 3. Valid flags parse cleanly (`--to D: --dry-run --no-reopen --json`)
    let res = run_args(
        &[
            "greenc",
            "zalo",
            "move",
            "--to",
            "D:",
            "--dry-run",
            "--no-reopen",
            "--json",
        ],
        &db_path,
    );
    // On a host without Zalo installed, NotFound maps to exit 1; on non-Windows/Store with Zalo,
    // PreconditionBlocked maps to exit 2; never panics.
    assert!(
        res.exit_code == EXIT_OPERATIONAL_ERROR || res.exit_code == EXIT_PRECONDITION_BLOCKED,
        "unexpected exit code {} (stdout={}, stderr={})",
        res.exit_code,
        res.stdout,
        res.stderr
    );

    // 4. Pre-cancelled TaskControl -> exit 130
    let cli = Cli::try_parse_from([
        "greenc",
        "zalo",
        "move",
        "--to",
        "D:",
        "--dry-run",
        "--no-reopen",
        "--json",
    ])
    .unwrap();
    let ctrl = Arc::new(TaskControl::new());
    ctrl.cancel();
    let cancelled = execute_cli(cli, &db_path, &ctrl);
    assert_eq!(cancelled.exit_code, EXIT_CANCELLED);
}

#[test]
fn cli_policy_roundtrips_zalo_auto_clean_setting() {
    let dir = tempdir().unwrap();
    let db_a = dir.path().join("a.db");
    let db_b = dir.path().join("b.db");
    let policy_path = dir.path().join("policy.json");

    {
        let db = GreenCDb::open(&db_a).unwrap();
        let settings = AppSettings::new(&db);
        // Default must be OFF (false)
        assert!(!settings.get_zalo_auto_clean_enabled().unwrap());
        settings.set_zalo_auto_clean_enabled(true).unwrap();
    }

    let exp = run_args(
        &[
            "greenc",
            "policy",
            "export",
            policy_path.to_str().unwrap(),
            "--json",
        ],
        &db_a,
    );
    assert_eq!(exp.exit_code, EXIT_SUCCESS);

    let imp = run_args(
        &[
            "greenc",
            "policy",
            "import",
            policy_path.to_str().unwrap(),
            "--json",
        ],
        &db_b,
    );
    assert_eq!(imp.exit_code, EXIT_SUCCESS);

    let db2 = GreenCDb::open(&db_b).unwrap();
    let settings2 = AppSettings::new(&db2);
    assert!(settings2.get_zalo_auto_clean_enabled().unwrap());
}

#[test]
fn cli_zalo_status_lists_retained_original_conflict_and_moving_copies() {
    use chrono::{Duration as ChronoDuration, Utc};
    use greenc_core::models::{
        RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
        ZaloRetainedOriginal, ZaloRetainedStatus,
    };
    use greenc_platform::reparse::NativeReparsePointApi;
    use greenc_platform::traits::ReparsePointApi;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("zalo_copies.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let src = dir.path().join("Zalo Data");
    let target = dir.path().join("D_Target/zalo");
    let orig = dir.path().join("Zalo Data.greenc-original.test-1");
    let conflict = dir.path().join("Zalo Data.greenc-conflict.test-2");
    let moving = dir.path().join("Zalo Data.greenc-moving.test-3");

    std::fs::create_dir_all(target.join("message")).unwrap();
    std::fs::write(target.join("message/zalo.db"), b"TARGET-CHAT").unwrap();
    std::fs::create_dir_all(orig.join("message")).unwrap();
    std::fs::write(orig.join("message/zalo.db"), b"ORIGINAL-CHAT-PAYLOAD").unwrap();
    std::fs::create_dir_all(conflict.join("message")).unwrap();
    std::fs::write(conflict.join("message/new.db"), b"CONFLICT-CHAT").unwrap();
    std::fs::create_dir_all(moving.join("message")).unwrap();
    std::fs::write(moving.join("message/tmp.db"), b"MOVING-CHAT").unwrap();

    let reparse = NativeReparsePointApi;
    reparse.create_junction(&src, &target).unwrap();

    let now = Utc::now();
    let reloc_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: src.clone(),
            target_path: target.clone(),
            size_bytes: 21,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec!["Zalo.exe".to_string()],
            created_at: now,
            detail: None,
        })
        .unwrap();

    let rid = db
        .save_zalo_retained_original(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: orig.clone(),
            size_bytes: 21,
            file_count: 1,
            health_verified: true,
            created_at: now,
            purge_after: now + ChronoDuration::hours(24),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: Some("Giữ tạm 24 giờ".to_string()),
        })
        .unwrap();

    // 1. `zalo status --json` must report all 3 safety copies (original, conflict, moving)
    let st_json = run_args(&["greenc", "zalo", "status", "--json"], &db_path);
    assert_eq!(st_json.exit_code, EXIT_SUCCESS);
    let parsed: serde_json::Value = serde_json::from_str(&st_json.stdout).unwrap();
    let copies = parsed
        .get("safety_copies")
        .and_then(|v| v.as_array())
        .expect("zalo status must emit safety_copies array");
    assert!(copies
        .iter()
        .any(|c| c.get("kind").and_then(|k| k.as_str()) == Some("original")));
    assert!(copies
        .iter()
        .any(|c| c.get("kind").and_then(|k| k.as_str()) == Some("conflict")));
    assert!(copies
        .iter()
        .any(|c| c.get("kind").and_then(|k| k.as_str()) == Some("moving")));
    assert!(
        parsed
            .get("retained_original_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 21
    );

    // 2. Human output prints the safety copies clearly in Vietnamese
    let st_human = run_args(&["greenc", "zalo", "status"], &db_path);
    assert_eq!(st_human.exit_code, EXIT_SUCCESS);
    assert!(st_human.stdout.contains(".greenc-original.test-1"));
    assert!(st_human.stdout.contains(".greenc-conflict.test-2"));
    assert!(st_human.stdout.contains(".greenc-moving.test-3"));

    // 3. `zalo originals --json` lists retained originals
    let orig_list = run_args(&["greenc", "zalo", "originals", "--json"], &db_path);
    assert_eq!(orig_list.exit_code, EXIT_SUCCESS);
    let orig_val: serde_json::Value = serde_json::from_str(&orig_list.stdout).unwrap();
    assert!(!orig_val["safety_copies"].as_array().unwrap().is_empty());

    // 4. `zalo purge-original --id <rid> --json` purges the retained original cleanly
    let rid_str = rid.to_string();
    let purge_out = run_args(
        &[
            "greenc",
            "zalo",
            "purge-original",
            "--id",
            &rid_str,
            "--json",
        ],
        &db_path,
    );
    assert_eq!(
        purge_out.exit_code, EXIT_SUCCESS,
        "stderr={}",
        purge_out.stderr
    );
    assert!(!orig.exists());
    assert!(target.join("message/zalo.db").exists());

    // 5. `zalo restore-original --id <rid2> --json` restores original and preserves target as conflict
    let orig2 = dir.path().join("Zalo Data.greenc-original.restore-test");
    std::fs::create_dir_all(orig2.join("message")).unwrap();
    std::fs::write(orig2.join("message/zalo.db"), b"ORIGINAL-RESTORE-DATA").unwrap();
    let rid2 = db
        .save_zalo_retained_original(&ZaloRetainedOriginal {
            id: None,
            relocation_id: reloc_id,
            source_path: src.clone(),
            target_path: target.clone(),
            retained_path: orig2.clone(),
            size_bytes: 21,
            file_count: 1,
            health_verified: true,
            created_at: now,
            purge_after: now + ChronoDuration::hours(24),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: None,
        })
        .unwrap();
    let rid2_str = rid2.to_string();
    let restore_out = run_args(
        &[
            "greenc",
            "zalo",
            "restore-original",
            "--id",
            &rid2_str,
            "--json",
        ],
        &db_path,
    );
    // Restoring is refused while the real Zalo is running on the developer's machine.
    if restore_out.exit_code == EXIT_PRECONDITION_BLOCKED && restore_out.stderr.contains("Zalo") {
        return;
    }
    assert_eq!(
        restore_out.exit_code, EXIT_SUCCESS,
        "stderr={}",
        restore_out.stderr
    );
    assert!(!reparse.is_reparse_point(&src));
    assert_eq!(
        std::fs::read(src.join("message/zalo.db")).unwrap(),
        b"ORIGINAL-RESTORE-DATA"
    );
    assert!(!orig2.exists());
    // Target preserved as `.greenc-conflict.*` in D_Target!
    let target_parent = target.parent().unwrap();
    let has_target_conflict = std::fs::read_dir(target_parent)
        .unwrap()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(".greenc-conflict.")
                && e.path().join("message/zalo.db").exists()
        });
    assert!(
        has_target_conflict,
        "restore-original must keep post-move target as a conflict copy"
    );
}
