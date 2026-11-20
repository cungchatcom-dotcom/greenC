//! CLI contract integration tests (`status --json`, `analyze`, `clean --dry-run`,
//! `vault list`, `policy export/import`, and exit codes `0 / 1 / 2 / 130`) (Task T063).

#![deny(unsafe_code)]

use clap::Parser;
use greenc_cli::{
    execute_cli, Cli, EXIT_CANCELLED, EXIT_OPERATIONAL_ERROR, EXIT_PRECONDITION_BLOCKED,
    EXIT_SUCCESS,
};
use greenc_core::models::QuotaRule;
use greenc_core::task::TaskControl;
use greenc_storage::db::GreenCDb;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

fn run_args(args: &[&str], db_path: &std::path::Path) -> greenc_cli::CliOutcome {
    let cli = Cli::try_parse_from(args).expect("CLI arguments should parse cleanly");
    let ctrl = Arc::new(TaskControl::new());
    execute_cli(cli, db_path, &ctrl)
}

#[test]
fn cli_status_json_matches_contract() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");

    let out = run_args(&["greenc", "status", "--json"], &db_path);
    assert_eq!(out.exit_code, EXIT_SUCCESS);

    let parsed: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(parsed.get("drives").and_then(|v| v.as_array()).is_some());
    assert!(parsed.get("burn_rate_forecast").is_some());
    assert!(parsed.get("vault_summary").is_some());
    assert!(parsed.get("active_quota_rules").is_some());
    assert!(parsed.get("relocation_health").is_some());
}

#[test]
fn cli_analyze_and_clean_dry_run_do_not_mutate_disk() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");

    let analyze = run_args(&["greenc", "analyze", "--json"], &db_path);
    assert_eq!(analyze.exit_code, EXIT_SUCCESS);
    let analyze_json: serde_json::Value = serde_json::from_str(&analyze.stdout).unwrap();
    assert!(analyze_json
        .get("proposals")
        .and_then(|v| v.as_array())
        .is_some());

    let clean_dry = run_args(
        &[
            "greenc",
            "clean",
            "--apps",
            "windows_temp",
            "--dry-run",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(clean_dry.exit_code, EXIT_SUCCESS);
    let clean_json: serde_json::Value = serde_json::from_str(&clean_dry.stdout).unwrap();
    assert_eq!(
        clean_json.get("dry_run").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(clean_json.get("archive_id").unwrap().is_null());
}

#[test]
fn cli_vault_list_and_policy_export_import_roundtrip() {
    let dir = tempdir().unwrap();
    let db_a = dir.path().join("source.db");
    let db_b = dir.path().join("dest.db");
    let policy_file = dir.path().join("greenc_policy.json");

    // Seed a rule in source DB
    {
        let db = GreenCDb::open(&db_a).unwrap();
        db.save_quota_rule(&QuotaRule {
            id: None,
            app_name: "Zalo".to_string(),
            process_name: "Zalo.exe".to_string(),
            service_name: String::new(),
            target_paths: vec![PathBuf::from(r"C:\Temp\ZaloCache")],
            max_bytes: 5 * 1024 * 1024 * 1024,
            watermark_ratio: 0.85,
            retention_days: 14,
            min_file_age_seconds: 60,
            exclude_patterns: Vec::new(),
            protected_files: Vec::new(),
            enabled: true,
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    }

    let vlist = run_args(&["greenc", "vault", "list", "--json"], &db_a);
    assert_eq!(vlist.exit_code, EXIT_SUCCESS);
    let vlist_json: serde_json::Value = serde_json::from_str(&vlist.stdout).unwrap();
    assert!(vlist_json
        .get("archives")
        .and_then(|v| v.as_array())
        .is_some());

    // Export policy from db_a
    let export_res = run_args(
        &[
            "greenc",
            "policy",
            "export",
            &policy_file.to_string_lossy(),
            "--json",
        ],
        &db_a,
    );
    assert_eq!(export_res.exit_code, EXIT_SUCCESS);
    assert!(policy_file.exists());

    // Import policy into db_b
    let import_res = run_args(
        &[
            "greenc",
            "policy",
            "import",
            &policy_file.to_string_lossy(),
            "--json",
        ],
        &db_b,
    );
    assert_eq!(import_res.exit_code, EXIT_SUCCESS);

    let db_dest = GreenCDb::open(&db_b).unwrap();
    let rules = db_dest.list_quota_rules(false).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].app_name, "Zalo");
    assert_eq!(rules[0].retention_days, 14);
}

#[test]
fn cli_exit_codes_match_contract_for_preconditions_errors_and_ctrl_c() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");

    // Precondition blocked (exit code 2): OneDrive path
    let onedrive_block = run_args(
        &[
            "greenc",
            "compact",
            "apply",
            r"C:\Users\Test\OneDrive\Docs",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(onedrive_block.exit_code, EXIT_PRECONDITION_BLOCKED);

    // Operational error (exit code 1): missing policy file
    let missing_policy = run_args(
        &[
            "greenc",
            "policy",
            "import",
            r"C:\NonExistent\missing_policy_999.json",
        ],
        &db_path,
    );
    assert_eq!(missing_policy.exit_code, EXIT_OPERATIONAL_ERROR);

    // Cooperative Ctrl+C cancellation (exit code 130)
    let cli = Cli::try_parse_from(["greenc", "status", "--json"]).unwrap();
    let cancelled_ctrl = Arc::new(TaskControl::new());
    cancelled_ctrl.cancel();
    let cancelled_out = execute_cli(cli, &db_path, &cancelled_ctrl);
    assert_eq!(cancelled_out.exit_code, EXIT_CANCELLED);
}

#[test]
fn cli_quota_set_list_remove_and_validation() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");

    // Empty to start.
    let empty = run_args(&["greenc", "quota", "list", "--json"], &db_path);
    assert_eq!(empty.exit_code, EXIT_SUCCESS);
    let v: serde_json::Value = serde_json::from_str(&empty.stdout).unwrap();
    assert_eq!(v["quotas"].as_array().unwrap().len(), 0);

    // Set (drive letter is normalized to "C:\").
    let set = run_args(
        &[
            "greenc",
            "quota",
            "set",
            "--drive",
            "c",
            "--warn",
            "75",
            "--clean",
            "88",
            "--target",
            "80",
            "--auto-clean",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(set.exit_code, EXIT_SUCCESS, "stderr: {}", set.stderr);

    let list = run_args(&["greenc", "quota", "list", "--json"], &db_path);
    let v: serde_json::Value = serde_json::from_str(&list.stdout).unwrap();
    let rows = v["quotas"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["drive_root"], "C:\\");
    assert_eq!(rows[0]["auto_clean"], true);
    assert!((rows[0]["warn_ratio"].as_f64().unwrap() - 0.75).abs() < 1e-9);

    // Same drive again updates instead of duplicating.
    let again = run_args(
        &["greenc", "quota", "set", "--drive", "C:", "--json"],
        &db_path,
    );
    assert_eq!(again.exit_code, EXIT_SUCCESS);
    let list = run_args(&["greenc", "quota", "list", "--json"], &db_path);
    let v: serde_json::Value = serde_json::from_str(&list.stdout).unwrap();
    assert_eq!(v["quotas"].as_array().unwrap().len(), 1);

    // Invalid drive and backup on the protected drive are refused.
    let bad_drive = run_args(
        &["greenc", "quota", "set", "--drive", "C:/Users", "--json"],
        &db_path,
    );
    assert_eq!(bad_drive.exit_code, EXIT_PRECONDITION_BLOCKED);
    let same_drive = run_args(
        &[
            "greenc",
            "quota",
            "set",
            "--drive",
            "C",
            "--auto-backup",
            "--backup-dir",
            "C:/bk",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(same_drive.exit_code, EXIT_PRECONDITION_BLOCKED);

    // Remove, then removing again is an error.
    let rm = run_args(
        &["greenc", "quota", "remove", "--drive", "C", "--json"],
        &db_path,
    );
    assert_eq!(rm.exit_code, EXIT_SUCCESS);
    let rm2 = run_args(
        &["greenc", "quota", "remove", "--drive", "C", "--json"],
        &db_path,
    );
    assert_ne!(rm2.exit_code, EXIT_SUCCESS);
}
