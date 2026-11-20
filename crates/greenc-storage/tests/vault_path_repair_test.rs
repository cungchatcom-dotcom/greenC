//! Re-pointing Vault archives whose ZIP moved (e.g. `C:\DiskGuard\vault` → `C:\Green C\Vault` after the
//! project rename) and flagging archives whose ZIP is gone everywhere.

use chrono::Utc;
use greenc_core::models::{VaultArchive, VaultFileItem, VaultStatus};
use greenc_storage::db::GreenCDb;
use greenc_storage::vault_repair::repair_vault_paths;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn add_archive(db: &GreenCDb, zip: &Path, status: VaultStatus) -> i64 {
    let manifest = zip.with_file_name(format!(
        "{}.manifest.json",
        zip.file_stem().unwrap().to_string_lossy()
    ));
    db.create_vault_archive(
        &VaultArchive {
            id: None,
            rule_id: 1,
            app_name: "App".to_string(),
            zip_path: zip.to_path_buf(),
            manifest_path: manifest,
            file_count: 1,
            original_bytes: 10,
            compressed_bytes: 5,
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::days(7),
            status,
            status_reason: String::new(),
            resolved_at: None,
        },
        &[VaultFileItem {
            id: None,
            archive_id: None,
            original_path: PathBuf::from(r"C:\x\a.bin"),
            rel_arcname: "item_00001_a.bin".to_string(),
            size_bytes: 10,
            sha256: "00".to_string(),
            mtime: 0.0,
            atime: 0.0,
            hardlink_paths: vec![],
            restored_individually: false,
        }],
    )
    .unwrap()
}

#[test]
fn relocates_moved_zips_and_flags_missing_ones() {
    let tmp = tempdir().unwrap();
    let db = GreenCDb::open(tmp.path().join("g.db")).unwrap();

    let old_dir = tmp.path().join("DiskGuard").join("vault"); // where the DB still points
    let new_dir = tmp.path().join("Green C").join("Vault"); // where the files are now
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::write(new_dir.join("vault_rule1_a.zip"), b"zip").unwrap();
    std::fs::write(new_dir.join("vault_rule1_a.manifest.json"), b"{}").unwrap();

    let moved = add_archive(
        &db,
        &old_dir.join("vault_rule1_a.zip"),
        VaultStatus::Quarantined,
    );
    let gone = add_archive(
        &db,
        &old_dir.join("vault_rule2_b.zip"),
        VaultStatus::Quarantined,
    );
    let fine_zip = new_dir.join("vault_rule1_a.zip");
    let fine = add_archive(&db, &fine_zip, VaultStatus::Quarantined);
    // Already finished archives are history: never touched.
    let purged = add_archive(&db, &old_dir.join("vault_rule3_c.zip"), VaultStatus::Purged);

    let report = repair_vault_paths(&db, &[old_dir.clone(), new_dir.clone()]).unwrap();

    assert_eq!(report.relocated.len(), 1);
    assert_eq!(report.missing, vec![gone]);

    let a = db.get_vault_archive(moved).unwrap().unwrap();
    assert_eq!(a.zip_path, new_dir.join("vault_rule1_a.zip"));
    assert_eq!(a.manifest_path, new_dir.join("vault_rule1_a.manifest.json"));
    assert_eq!(a.status, VaultStatus::Quarantined);

    let g = db.get_vault_archive(gone).unwrap().unwrap();
    assert_eq!(g.status, VaultStatus::Missing);
    assert!(g.status_reason.contains("vault_rule2_b.zip"));

    assert_eq!(
        db.get_vault_archive(fine).unwrap().unwrap().zip_path,
        fine_zip
    );
    assert_eq!(
        db.get_vault_archive(purged).unwrap().unwrap().status,
        VaultStatus::Purged
    );

    // Missing archives are no longer offered as restorable quarantine.
    let quarantined: Vec<i64> = db
        .list_vault_archives(Some(VaultStatus::Quarantined.as_str()))
        .unwrap()
        .into_iter()
        .filter_map(|a| a.id)
        .collect();
    assert!(!quarantined.contains(&gone));

    // Idempotent: a second pass changes nothing.
    let again = repair_vault_paths(&db, &[old_dir, new_dir]).unwrap();
    assert!(again.relocated.is_empty() && again.missing.is_empty());
}

#[test]
fn missing_status_round_trips() {
    assert_eq!(VaultStatus::from_str_loose("MISSING"), VaultStatus::Missing);
    assert_eq!(VaultStatus::Missing.as_str(), "MISSING");
}
