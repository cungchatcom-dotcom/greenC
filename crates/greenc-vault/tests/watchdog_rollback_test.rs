//! Integration tests for RollbackEngine and Safety Watchdog auto-rollback.

use chrono::Utc;
use greenc_core::models::{VaultArchive, VaultFileItem, VaultStatus};
use greenc_storage::db::GreenCDb;
use greenc_vault::manifest::VaultManifest;
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use greenc_win32::traits::ProcessServiceProbe;
use std::fs::File;
use std::io::Write;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

struct MockProcessProbe {
    service_running: bool,
}

impl ProcessServiceProbe for MockProcessProbe {
    fn is_process_running(&self, _exe: &str) -> bool {
        true
    }
    fn is_service_running(&self, _svc: &str) -> greenc_core::error::Result<bool> {
        Ok(self.service_running)
    }
}

#[test]
fn test_full_archive_rollback() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("greenc.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let target_dir = tmp.path().join("app_data");
    std::fs::create_dir_all(&target_dir).unwrap();
    let orig_file1 = target_dir.join("chat.dat");
    let orig_file2 = target_dir.join("image.png");

    let content1 = b"Important chat message history";
    let content2 = b"Simulated PNG picture bytes 12345";

    // Write original files
    File::create(&orig_file1)
        .unwrap()
        .write_all(content1)
        .unwrap();
    File::create(&orig_file2)
        .unwrap()
        .write_all(content2)
        .unwrap();

    let sha1 = greenc_vault::manifest::compute_file_sha256(&orig_file1).unwrap();
    let sha2 = greenc_vault::manifest::compute_file_sha256(&orig_file2).unwrap();

    // Create ZIP archive
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let zip_path = vault_dir.join("vault_test_01.zip");
    let manifest_path = vault_dir.join("vault_test_01.manifest.json");

    {
        let zip_file = File::create(&zip_path).unwrap();
        let mut zip_writer = ZipWriter::new(zip_file);
        let opts = SimpleFileOptions::default();

        zip_writer.start_file("item_00001_chat.dat", opts).unwrap();
        zip_writer.write_all(content1).unwrap();

        zip_writer.start_file("item_00002_image.png", opts).unwrap();
        zip_writer.write_all(content2).unwrap();

        let mut inner = zip_writer.finish().unwrap();
        inner.flush().unwrap();
    }

    let files = vec![
        VaultFileItem {
            id: None,
            archive_id: None,
            original_path: orig_file1.clone(),
            rel_arcname: "item_00001_chat.dat".to_string(),
            size_bytes: content1.len() as u64,
            sha256: sha1.clone(),
            mtime: 1731760000.0,
            atime: 1731760000.0,
            hardlink_paths: vec![orig_file1.clone()],
            restored_individually: false,
        },
        VaultFileItem {
            id: None,
            archive_id: None,
            original_path: orig_file2.clone(),
            rel_arcname: "item_00002_image.png".to_string(),
            size_bytes: content2.len() as u64,
            sha256: sha2.clone(),
            mtime: 1731760000.0,
            atime: 1731760000.0,
            hardlink_paths: vec![orig_file2.clone()],
            restored_individually: false,
        },
    ];

    let manifest = VaultManifest::new(
        "vault_test_01".to_string(),
        1,
        "ChatApp".to_string(),
        Utc::now(),
        7,
        target_dir.clone(),
        files.clone(),
        (content1.len() + content2.len()) as u64,
        256,
        "archive_hash_abc".to_string(),
    );
    manifest.write_to_file(&manifest_path).unwrap();

    let archive_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "ChatApp".to_string(),
                zip_path: zip_path.clone(),
                manifest_path: manifest_path.clone(),
                file_count: 2,
                original_bytes: (content1.len() + content2.len()) as u64,
                compressed_bytes: 256,
                created_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::days(7),
                status: VaultStatus::Quarantined,
                status_reason: "Initial quarantine".to_string(),
                resolved_at: None,
            },
            &files,
        )
        .unwrap();

    // Simulate cleanup: delete original files
    std::fs::remove_file(&orig_file1).unwrap();
    std::fs::remove_file(&orig_file2).unwrap();
    assert!(!orig_file1.exists());
    assert!(!orig_file2.exists());

    // Execute full rollback
    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    let report = engine
        .rollback_archive(archive_id, "User requested rollback", false)
        .expect("Rollback must succeed");

    assert_eq!(report.restored_count, 2);
    assert_eq!(report.archive_id, archive_id);
    assert!(orig_file1.exists(), "orig_file1 must be restored");
    assert!(orig_file2.exists(), "orig_file2 must be restored");

    // Verify content and SHA-256
    let rest_sha1 = greenc_vault::manifest::compute_file_sha256(&orig_file1).unwrap();
    let rest_sha2 = greenc_vault::manifest::compute_file_sha256(&orig_file2).unwrap();
    assert_eq!(rest_sha1, sha1);
    assert_eq!(rest_sha2, sha2);

    // Verify archive state updated and zip unlinked
    let updated = db.get_vault_archive(archive_id).unwrap().unwrap();
    assert_eq!(updated.status, VaultStatus::RolledBack);
    assert!(
        !zip_path.exists(),
        "ZIP file must be unlinked after rollback"
    );
}

#[test]
fn test_restoration_collision_non_destructive() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("greenc.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let target_dir = tmp.path().join("app_data");
    std::fs::create_dir_all(&target_dir).unwrap();
    let file_path = target_dir.join("document.txt");

    let archived_content = b"Archived original text";
    let new_app_content = b"New content generated by app after cleanup";

    // Create archive ZIP
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let zip_path = vault_dir.join("vault_collision.zip");
    let manifest_path = vault_dir.join("vault_collision.manifest.json");

    {
        let zip_file = File::create(&zip_path).unwrap();
        let mut zip_writer = ZipWriter::new(zip_file);
        zip_writer
            .start_file("item_00001_document.txt", SimpleFileOptions::default())
            .unwrap();
        zip_writer.write_all(archived_content).unwrap();
        zip_writer.finish().unwrap();
    }

    let files = vec![VaultFileItem {
        id: None,
        archive_id: None,
        original_path: file_path.clone(),
        rel_arcname: "item_00001_document.txt".to_string(),
        size_bytes: archived_content.len() as u64,
        sha256: greenc_vault::manifest::compute_bytes_sha256(archived_content),
        mtime: 1731760000.0,
        atime: 1731760000.0,
        hardlink_paths: vec![file_path.clone()],
        restored_individually: false,
    }];

    let archive_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "DocApp".to_string(),
                zip_path: zip_path.clone(),
                manifest_path,
                file_count: 1,
                original_bytes: archived_content.len() as u64,
                compressed_bytes: 100,
                created_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::days(7),
                status: VaultStatus::Quarantined,
                status_reason: "Quarantined".to_string(),
                resolved_at: None,
            },
            &files,
        )
        .unwrap();

    // Application generated a new non-empty file at the original path
    File::create(&file_path)
        .unwrap()
        .write_all(new_app_content)
        .unwrap();

    // Execute rollback
    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    let report = engine
        .rollback_archive(archive_id, "Collision test", false)
        .unwrap();

    assert_eq!(report.restored_count, 1);
    let restored_file = &report.restored_paths[0];

    // Original file must NOT be overwritten!
    let current_orig_content = std::fs::read(&file_path).unwrap();
    assert_eq!(current_orig_content, new_app_content);

    // Restored file must be non-destructive `.restored_<timestamp>`
    assert_ne!(restored_file, &file_path);
    assert!(restored_file.to_string_lossy().contains(".restored_"));
    let restored_content = std::fs::read(restored_file).unwrap();
    assert_eq!(restored_content, archived_content);
}

#[test]
fn test_granular_single_file_restore() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("greenc.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let target_dir = tmp.path().join("multi_app");
    std::fs::create_dir_all(&target_dir).unwrap();
    let file1 = target_dir.join("f1.bin");
    let file2 = target_dir.join("f2.bin");

    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let zip_path = vault_dir.join("vault_multi.zip");
    let manifest_path = vault_dir.join("vault_multi.manifest.json");

    {
        let zip_file = File::create(&zip_path).unwrap();
        let mut zip_writer = ZipWriter::new(zip_file);
        zip_writer
            .start_file("item_00001_f1.bin", SimpleFileOptions::default())
            .unwrap();
        zip_writer.write_all(b"File 1 content").unwrap();
        zip_writer
            .start_file("item_00002_f2.bin", SimpleFileOptions::default())
            .unwrap();
        zip_writer.write_all(b"File 2 content").unwrap();
        zip_writer.finish().unwrap();
    }

    let files = vec![
        VaultFileItem {
            id: None,
            archive_id: None,
            original_path: file1.clone(),
            rel_arcname: "item_00001_f1.bin".to_string(),
            size_bytes: 14,
            sha256: greenc_vault::manifest::compute_bytes_sha256(b"File 1 content"),
            mtime: 1731760000.0,
            atime: 1731760000.0,
            hardlink_paths: vec![file1.clone()],
            restored_individually: false,
        },
        VaultFileItem {
            id: None,
            archive_id: None,
            original_path: file2.clone(),
            rel_arcname: "item_00002_f2.bin".to_string(),
            size_bytes: 14,
            sha256: greenc_vault::manifest::compute_bytes_sha256(b"File 2 content"),
            mtime: 1731760000.0,
            atime: 1731760000.0,
            hardlink_paths: vec![file2.clone()],
            restored_individually: false,
        },
    ];

    let archive_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "MultiApp".to_string(),
                zip_path: zip_path.clone(),
                manifest_path,
                file_count: 2,
                original_bytes: 28,
                compressed_bytes: 100,
                created_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::days(7),
                status: VaultStatus::Quarantined,
                status_reason: "Quarantined".to_string(),
                resolved_at: None,
            },
            &files,
        )
        .unwrap();

    let db_files = db.get_vault_files(archive_id).unwrap();
    let target_file_id = db_files[0].id.unwrap();

    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    let report = engine
        .restore_files_selective(archive_id, &[target_file_id])
        .unwrap();

    assert_eq!(report.restored_count, 1);
    assert!(file1.exists(), "f1.bin must be restored");
    assert!(!file2.exists(), "f2.bin must NOT be restored");

    // Archive must remain QUARANTINED
    let arch_status = db.get_vault_archive(archive_id).unwrap().unwrap();
    assert_eq!(arch_status.status, VaultStatus::Quarantined);

    // Selected file must be marked restored_individually = true
    let updated_files = db.get_vault_files(archive_id).unwrap();
    assert!(updated_files[0].restored_individually);
    assert!(!updated_files[1].restored_individually);
}

#[test]
fn test_auto_purge_expired_archives() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("greenc.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let zip_path = vault_dir.join("expired.zip");
    let manifest_path = vault_dir.join("expired.manifest.json");

    File::create(&zip_path)
        .unwrap()
        .write_all(b"fake zip")
        .unwrap();
    File::create(&manifest_path)
        .unwrap()
        .write_all(b"{}")
        .unwrap();

    let archive_id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "ExpiredApp".to_string(),
                zip_path: zip_path.clone(),
                manifest_path: manifest_path.clone(),
                file_count: 1,
                original_bytes: 100,
                compressed_bytes: 50,
                created_at: Utc::now() - chrono::Duration::days(10),
                expires_at: Utc::now() - chrono::Duration::days(1), // Expired 1 day ago
                status: VaultStatus::Quarantined,
                status_reason: "Quarantined".to_string(),
                resolved_at: None,
            },
            &[],
        )
        .unwrap();

    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    let mock_probe = MockProcessProbe {
        service_running: true,
    };

    let _ = engine.run_watchdog_cycle(&mock_probe).unwrap();

    let updated = db.get_vault_archive(archive_id).unwrap().unwrap();
    assert_eq!(updated.status, VaultStatus::Purged);
    assert!(!zip_path.exists(), "Expired zip must be purged from disk");
    assert!(
        !manifest_path.exists(),
        "Expired manifest must be purged from disk"
    );
}

/// Builds a one-file archive whose original path is occupied by newer app content.
fn archive_with_occupied_original(
    tmp: &std::path::Path,
    db: &GreenCDb,
) -> (i64, std::path::PathBuf, &'static [u8]) {
    let archived: &'static [u8] = b"Archived original text";
    let target_dir = tmp.join("app_data");
    std::fs::create_dir_all(&target_dir).unwrap();
    let file_path = target_dir.join("document.txt");
    let vault_dir = tmp.join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let zip_path = vault_dir.join("v.zip");
    {
        let mut zw = ZipWriter::new(File::create(&zip_path).unwrap());
        zw.start_file("item_00001_document.txt", SimpleFileOptions::default())
            .unwrap();
        zw.write_all(archived).unwrap();
        zw.finish().unwrap();
    }
    let files = vec![VaultFileItem {
        id: None,
        archive_id: None,
        original_path: file_path.clone(),
        rel_arcname: "item_00001_document.txt".to_string(),
        size_bytes: archived.len() as u64,
        sha256: greenc_vault::manifest::compute_bytes_sha256(archived),
        mtime: 1731760000.0,
        atime: 1731760000.0,
        hardlink_paths: vec![file_path.clone()],
        restored_individually: false,
    }];
    let id = db
        .create_vault_archive(
            &VaultArchive {
                id: None,
                rule_id: 1,
                app_name: "DocApp".to_string(),
                zip_path,
                manifest_path: vault_dir.join("v.manifest.json"),
                file_count: 1,
                original_bytes: archived.len() as u64,
                compressed_bytes: 100,
                created_at: Utc::now(),
                expires_at: Utc::now() + chrono::Duration::days(7),
                status: VaultStatus::Quarantined,
                status_reason: "Quarantined".to_string(),
                resolved_at: None,
            },
            &files,
        )
        .unwrap();
    std::fs::write(&file_path, b"newer app content").unwrap();
    (id, file_path, archived)
}

/// Two restores within the same second must not land on the same `.restored_<ts>` name
/// (the second one used to truncate the first).
#[test]
fn test_repeated_restores_get_unique_names_and_never_overwrite() {
    let tmp = tempdir().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();
    let (archive_id, original, archived) = archive_with_occupied_original(tmp.path(), &db);
    let file_id = db.get_vault_files(archive_id).unwrap()[0].id.unwrap();

    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    let first = engine.restore_files_selective(archive_id, &[file_id]).unwrap();
    // Pre-fill the first restored copy with a marker: a second restore must not clobber it.
    let first_path = first.restored_paths[0].clone();
    std::fs::write(&first_path, b"user edited the restored copy").unwrap();

    let second = engine.rollback_archive(archive_id, "again", false);
    // Either the full rollback skips the already-restored file, or it writes to a NEW name.
    if let Ok(rep) = &second {
        for p in &rep.restored_paths {
            assert_ne!(p, &first_path, "must not reuse the first restored name");
        }
    }
    assert_eq!(
        std::fs::read(&first_path).unwrap(),
        b"user edited the restored copy",
        "first restored copy was overwritten"
    );
    assert_eq!(std::fs::read(&original).unwrap(), b"newer app content");
    let _ = archived;
}

/// A full rollback after a selective restore must not restore the same file a second time.
#[test]
fn test_full_rollback_skips_files_already_restored_individually() {
    let tmp = tempdir().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();
    let (archive_id, _original, _) = archive_with_occupied_original(tmp.path(), &db);
    let file_id = db.get_vault_files(archive_id).unwrap()[0].id.unwrap();

    let engine = RollbackEngine::new(&db, RestoreOptions::default());
    engine.restore_files_selective(archive_id, &[file_id]).unwrap();
    let full = engine.rollback_archive(archive_id, "full", false).unwrap();
    assert_eq!(full.restored_count, 0, "already-restored file restored twice");

    let copies = std::fs::read_dir(tmp.path().join("app_data"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".restored_")
        })
        .count();
    assert_eq!(copies, 1);
}
