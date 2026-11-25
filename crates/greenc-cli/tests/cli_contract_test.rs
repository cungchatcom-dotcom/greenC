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

#[test]
fn cli_quota_supports_unix_mount_points() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc_unix.db");

    let set_root = run_args(
        &[
            "greenc",
            "quota",
            "set",
            "--drive",
            "/",
            "--warn",
            "70",
            "--clean",
            "80",
            "--target",
            "75",
            "--auto-clean",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(
        set_root.exit_code, EXIT_SUCCESS,
        "stderr: {}",
        set_root.stderr
    );

    let set_home = run_args(
        &[
            "greenc",
            "quota",
            "set",
            "--drive",
            "/home/",
            "--warn",
            "70",
            "--clean",
            "80",
            "--target",
            "75",
            "--auto-clean",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(
        set_home.exit_code, EXIT_SUCCESS,
        "stderr: {}",
        set_home.stderr
    );

    let list = run_args(&["greenc", "quota", "list", "--json"], &db_path);
    assert_eq!(list.exit_code, EXIT_SUCCESS);
    let v: serde_json::Value = serde_json::from_str(&list.stdout).unwrap();
    let rows = v["quotas"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["drive_root"], "/");
    assert_eq!(rows[1]["drive_root"], "/home");

    let rm = run_args(
        &["greenc", "quota", "remove", "--drive", "/home/", "--json"],
        &db_path,
    );
    assert_eq!(rm.exit_code, EXIT_SUCCESS);
}

#[test]
fn cli_guard_set_mode_run_dry_run_and_suggest_contract() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("guard_cli.db");

    // 1. Configure quota on `/` with default regenerable_cache_mode = vault
    let set = run_args(
        &[
            "greenc",
            "quota",
            "set",
            "--drive",
            "/",
            "--warn",
            "70",
            "--clean",
            "80",
            "--target",
            "75",
            "--auto-clean",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(set.exit_code, EXIT_SUCCESS, "stderr: {}", set.stderr);
    let set_json: serde_json::Value = serde_json::from_str(&set.stdout).unwrap();
    assert_eq!(set_json["regenerable_cache_mode"], "vault");

    // 2. Toggle regenerable_cache_mode to `direct` via `guard set-mode`
    let mode_ok = run_args(
        &[
            "greenc", "guard", "set-mode", "--drive", "/", "--mode", "direct", "--json",
        ],
        &db_path,
    );
    assert_eq!(
        mode_ok.exit_code, EXIT_SUCCESS,
        "stderr: {}",
        mode_ok.stderr
    );
    let mode_json: serde_json::Value = serde_json::from_str(&mode_ok.stdout).unwrap();
    assert_eq!(mode_json["regenerable_cache_mode"], "direct");

    // Re-saving thresholds with `quota set` WITHOUT `--regenerable-cache-mode` preserves `direct`
    let preserve = run_args(
        &[
            "greenc",
            "quota",
            "set",
            "--drive",
            "/",
            "--warn",
            "72",
            "--clean",
            "82",
            "--target",
            "76",
            "--auto-clean",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(
        preserve.exit_code, EXIT_SUCCESS,
        "stderr: {}",
        preserve.stderr
    );
    let preserve_json: serde_json::Value = serde_json::from_str(&preserve.stdout).unwrap();
    assert_eq!(preserve_json["regenerable_cache_mode"], "direct");

    // Invalid mode is rejected with exit code 2
    let mode_bad = run_args(
        &[
            "greenc", "guard", "set-mode", "--drive", "/", "--mode", "invalid", "--json",
        ],
        &db_path,
    );
    assert_eq!(mode_bad.exit_code, EXIT_PRECONDITION_BLOCKED);

    // 3. `guard run --drive / --dry-run --json` returns net-freed contract fields
    let run_out = run_args(
        &[
            "greenc",
            "guard",
            "run",
            "--drive",
            "/",
            "--dry-run",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(
        run_out.exit_code, EXIT_SUCCESS,
        "stderr: {}",
        run_out.stderr
    );
    let run_json: serde_json::Value = serde_json::from_str(&run_out.stdout).unwrap();
    assert_eq!(run_json["dry_run"], true);
    assert!(run_json.get("logical_bytes").is_some());
    assert!(run_json.get("vault_bytes").is_some());
    assert!(run_json.get("net_freed_bytes").is_some());
    assert!(run_json.get("target_reached").is_some());
    assert_eq!(run_json["drives"].as_array().unwrap().len(), 1);

    // 4. `guard suggest --root <tempdir> --json` is read-only and returns Vault + suggestions
    let repo = dir.path().join("fake_repo");
    std::fs::create_dir_all(repo.join("app/build")).unwrap();
    std::fs::write(repo.join(".git"), "gitdir: /tmp/fake\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "build/\n").unwrap();
    let artifact = repo.join("app/build/out.bin");
    std::fs::write(&artifact, vec![1u8; 4096]).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(5 * 86_400);
    let ft = std::fs::FileTimes::new()
        .set_modified(old)
        .set_accessed(old);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&artifact)
        .unwrap()
        .set_times(ft)
        .unwrap();
    #[cfg(unix)]
    {
        let _ = std::fs::File::open(repo.join("app/build")).and_then(|f| f.set_times(ft));
    }

    let sug = run_args(
        &[
            "greenc",
            "guard",
            "suggest",
            "--root",
            &dir.path().to_string_lossy(),
            "--min-days",
            "3",
            "--json",
        ],
        &db_path,
    );
    assert_eq!(sug.exit_code, EXIT_SUCCESS, "stderr: {}", sug.stderr);
    let sug_json: serde_json::Value = serde_json::from_str(&sug.stdout).unwrap();
    assert!(sug_json.get("suggestions").is_some());
    assert!(sug_json.get("vault").is_some());
    assert!(artifact.exists(), "guard suggest must never delete files");
}

#[test]
fn cli_vault_purge_and_restore_reject_paths_outside_vault_dir() {
    use chrono::Utc;
    use greenc_core::models::{VaultArchive, VaultStatus};
    use greenc_storage::db::GreenCDb;
    use greenc_storage::settings::AppSettings;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("vault_sec.db");
    let vault_dir = dir.path().join("Vault");
    std::fs::create_dir_all(&vault_dir).unwrap();

    let outside_dir = dir.path().join("outside_secret");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let victim_zip = outside_dir.join("secret.zip");
    let victim_man = outside_dir.join("secret.manifest.json");
    std::fs::write(&victim_zip, b"not a real vault zip").unwrap();
    std::fs::write(&victim_man, b"{}").unwrap();

    let db = GreenCDb::open(&db_path).unwrap();
    AppSettings::new(&db).set_data_root(dir.path()).unwrap();
    let arch_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "TamperedCli".to_string(),
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
    drop(db);

    // 1. `vault purge --expired` must skip the tampered archive and leave victim untouched
    let exp = run_args(
        &["greenc", "vault", "purge", "--expired", "--json"],
        &db_path,
    );
    assert_eq!(exp.exit_code, EXIT_SUCCESS, "stderr: {}", exp.stderr);
    let v: serde_json::Value = serde_json::from_str(&exp.stdout).unwrap();
    assert_eq!(v["purged_archives"], 0);
    assert!(victim_zip.exists() && victim_man.exists());

    // 2. `vault purge <id>` must refuse with EXIT_PRECONDITION_BLOCKED
    let id_str = arch_id.to_string();
    let manual = run_args(&["greenc", "vault", "purge", &id_str, "--json"], &db_path);
    assert_eq!(
        manual.exit_code, EXIT_PRECONDITION_BLOCKED,
        "manual purge of outside path must be blocked, stdout={}, stderr={}",
        manual.stdout, manual.stderr
    );
    assert!(victim_zip.exists() && victim_man.exists());

    // 3. `vault restore <id>` must also refuse
    let restore = run_args(&["greenc", "vault", "restore", &id_str, "--json"], &db_path);
    assert_eq!(
        restore.exit_code, EXIT_PRECONDITION_BLOCKED,
        "restore of outside zip_path must be blocked"
    );
    assert!(victim_zip.exists() && victim_man.exists());
}
