use greenc_core::error::GreenCError;
use greenc_core::models::*;
use greenc_storage::db::GreenCDb;
use greenc_storage::legacy_migration::{
    auto_migrate_legacy_db_with_candidates, migrate_legacy_db, move_vault,
};
use greenc_storage::settings::AppSettings;
use greenc_storage::CURRENT_SCHEMA_VERSION;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_clean_schema_v2_initialization() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");
    let db = GreenCDb::open(&db_path).unwrap();

    db.with_conn(|conn| {
        let version: u32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        // Verify all 8 tables exist
        let tables = vec![
            "app_settings",
            "daily_snapshots",
            "write_events",
            "quota_rules",
            "vault_archives",
            "vault_files",
            "relocations",
            "compaction_records",
        ];
        for t in tables {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1;",
                    [t],
                    |r| Ok(r.get::<_, i64>(0)? > 0),
                )
                .unwrap();
            assert!(exists, "Table {} must exist", t);
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn test_idempotent_migration_from_python_v1_schema() {
    let dir = tempdir().unwrap();
    let legacy_path = dir.path().join("legacy.db");

    // 1. Create a mock Python v1 database (user_version = 0, without reclaimed_bytes)
    {
        let conn = Connection::open(&legacy_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE daily_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                drive_letter TEXT NOT NULL,
                snapshot_date TEXT NOT NULL,
                total_bytes INTEGER NOT NULL,
                used_bytes INTEGER NOT NULL,
                free_bytes INTEGER NOT NULL,
                net_change_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE vault_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                archive_id INTEGER NOT NULL,
                original_path TEXT NOT NULL,
                rel_arcname TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                mtime REAL NOT NULL,
                atime REAL NOT NULL,
                hardlink_paths_json TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE relocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_id TEXT NOT NULL,
                app_name TEXT NOT NULL,
                strategy TEXT NOT NULL,
                source_path TEXT NOT NULL,
                target_path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                process_names TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                detail TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO daily_snapshots (drive_letter, snapshot_date, total_bytes, used_bytes, free_bytes, updated_at)
            VALUES ('C:\\', '2026-11-15', 500000000000, 400000000000, 100000000000, '2026-11-15T12:00:00');"
        ).unwrap();
    }

    // 2. Open with GreenCDb and trigger migration
    let db = GreenCDb::open(&legacy_path).unwrap();

    db.with_conn(|conn| {
        // Verify user_version updated to 2
        let version: u32 = conn
            .query_row("PRAGMA user_version;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        // Verify new column reclaimed_bytes was added
        let reclaimed: i64 = conn
            .query_row(
                "SELECT reclaimed_bytes FROM daily_snapshots WHERE drive_letter = 'C:\\';",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reclaimed, 0);

        // Verify compaction_records table created
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='compaction_records';",
                [],
                |r| Ok(r.get::<_, i64>(0)? > 0),
            )
            .unwrap();
        assert!(exists);
        Ok(())
    })
    .unwrap();
}

#[test]
fn test_app_settings_domain_clamping() {
    let dir = tempdir().unwrap();
    let db = GreenCDb::open(dir.path().join("greenc.db")).unwrap();
    let settings = AppSettings::new(&db);

    // Retention days clamped to 1..=90
    assert_eq!(settings.set_retention_days(0).unwrap(), 1);
    assert_eq!(settings.get_retention_days().unwrap(), 1);
    assert_eq!(settings.set_retention_days(120).unwrap(), 90);
    assert_eq!(settings.get_retention_days().unwrap(), 90);

    // Low-disk shield clamped to 2..=100
    assert_eq!(settings.set_low_disk_shield_gb(1).unwrap(), 2);
    assert_eq!(settings.get_low_disk_shield_gb().unwrap(), 2);
    assert_eq!(settings.set_low_disk_shield_gb(250).unwrap(), 100);
    assert_eq!(settings.get_low_disk_shield_gb().unwrap(), 100);

    // Language validation
    assert!(settings.set_language("invalid_lang").is_err());
    assert!(settings.set_language("vi").is_ok());
    assert_eq!(settings.get_language().unwrap(), Some("vi".to_string()));
}

#[test]
fn test_migrate_legacy_db_atomic_rollback_on_failure() {
    let dir = tempdir().unwrap();
    let corrupt_legacy_db = dir.path().join("corrupt_legacy.db");
    let target_db = dir.path().join("target.db");

    // Create an invalid, non-sqlite file as legacy db
    fs::write(&corrupt_legacy_db, b"NOT_A_SQLITE_DATABASE_HEADER").unwrap();

    // Attempt migration, which must fail
    let res = migrate_legacy_db(&corrupt_legacy_db, &target_db);
    assert!(res.is_err(), "Migration must fail on invalid database file");

    // Verify no orphan file was left at target_db
    assert!(
        !target_db.exists(),
        "target_db must not exist on migration failure"
    );

    // Verify temporary file was deleted
    let tmp_db = PathBuf::from(format!("{}.tmp", target_db.display()));
    assert!(
        !tmp_db.exists(),
        "temporary .tmp file must be removed on failure"
    );
}

#[test]
fn test_auto_migrate_legacy_db_cleans_zero_byte_orphan() {
    let dir = tempdir().unwrap();
    let target_db = dir.path().join("greenc.db");

    // Simulate an empty 0-byte orphan file left by a prior crashed migration
    fs::write(&target_db, b"").unwrap();
    assert_eq!(fs::metadata(&target_db).unwrap().len(), 0);

    // auto_migrate_legacy_db_with_candidates should detect 0-byte orphan, delete it, and return Ok(false) if no candidate
    let migrated = auto_migrate_legacy_db_with_candidates(&target_db, &[]).unwrap();
    assert!(!migrated);
    assert!(!target_db.exists(), "0-byte orphan must be cleaned up");
}

#[test]
fn test_move_vault_reserve_check_fails_when_space_tight() {
    let dir = tempdir().unwrap();
    let db = GreenCDb::open(dir.path().join("test.db")).unwrap();

    let old_vault_dir = dir.path().join("old_vault");
    let new_vault_dir = dir.path().join("new_vault");
    fs::create_dir_all(&old_vault_dir).unwrap();

    let zip_file = old_vault_dir.join("arch_001.zip");
    let man_file = old_vault_dir.join("arch_001.manifest.json");

    // Create 10 MiB dummy zip and 1 KiB manifest
    fs::write(&zip_file, vec![0u8; 10 * 1024 * 1024]).unwrap();
    fs::write(&man_file, b"{}").unwrap();

    let arch = VaultArchive {
        id: None,
        rule_id: 1,
        app_name: "TestApp".to_string(),
        zip_path: zip_file,
        manifest_path: man_file,
        file_count: 1,
        original_bytes: 20 * 1024 * 1024,
        compressed_bytes: 10 * 1024 * 1024,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now(),
        status: VaultStatus::Quarantined,
        status_reason: "Test".to_string(),
        resolved_at: None,
    };
    db.create_vault_archive(&arch, &[]).unwrap();

    const MIN_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
    // Target drive has 515 MiB free, but archive requires 10 MiB + 512 MiB = 522 MiB
    let target_free = MIN_RESERVE_BYTES + (3 * 1024 * 1024); // 515 MiB

    let res = move_vault(&db, &new_vault_dir, target_free, None);
    assert!(res.is_err());
    match res.unwrap_err() {
        GreenCError::InsufficientVaultSpace(msg) => {
            assert!(msg.contains("insufficient"));
        }
        other => panic!("Expected InsufficientVaultSpace, got: {:?}", other),
    }
}

#[test]
fn test_move_vault_reserve_check_succeeds_when_space_sufficient() {
    let dir = tempdir().unwrap();
    let db = GreenCDb::open(dir.path().join("test.db")).unwrap();

    let old_vault_dir = dir.path().join("old_vault");
    let new_vault_dir = dir.path().join("new_vault");
    fs::create_dir_all(&old_vault_dir).unwrap();

    let zip_file = old_vault_dir.join("arch_002.zip");
    let man_file = old_vault_dir.join("arch_002.manifest.json");

    fs::write(&zip_file, vec![0u8; 10 * 1024 * 1024]).unwrap();
    fs::write(&man_file, b"{}").unwrap();

    let arch = VaultArchive {
        id: None,
        rule_id: 1,
        app_name: "TestApp".to_string(),
        zip_path: zip_file.clone(),
        manifest_path: man_file.clone(),
        file_count: 1,
        original_bytes: 20 * 1024 * 1024,
        compressed_bytes: 10 * 1024 * 1024,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now(),
        status: VaultStatus::Quarantined,
        status_reason: "Test".to_string(),
        resolved_at: None,
    };
    let arch_id = db.create_vault_archive(&arch, &[]).unwrap();

    const MIN_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
    // Target drive has 600 MiB free (sufficient for 10 MiB + 512 MiB = 522 MiB)
    let target_free = MIN_RESERVE_BYTES + (88 * 1024 * 1024); // 600 MiB

    let summary = move_vault(&db, &new_vault_dir, target_free, None).unwrap();
    assert_eq!(summary.moved_count, 1);
    assert!(summary.errors.is_empty());

    // Verify source files unlinked
    assert!(!zip_file.exists());
    assert!(!man_file.exists());

    // Verify destination files exist
    assert!(new_vault_dir.join("arch_002.zip").exists());
    assert!(new_vault_dir.join("arch_002.manifest.json").exists());

    // Verify DB updated
    let updated = db.get_vault_archive(arch_id).unwrap().unwrap();
    assert_eq!(updated.zip_path, new_vault_dir.join("arch_002.zip"));
}

#[test]
fn test_legacy_migration_unique_index_and_on_conflict_upsert() {
    let dir = tempdir().unwrap();
    let legacy_db_path = dir.path().join("legacy_snapshots.db");

    // 1. Create a legacy table without inline UNIQUE constraint and with a non-unique index
    {
        let conn = Connection::open(&legacy_db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE daily_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                drive_letter TEXT NOT NULL,
                snapshot_date TEXT NOT NULL,
                total_bytes INTEGER NOT NULL,
                used_bytes INTEGER NOT NULL,
                free_bytes INTEGER NOT NULL,
                net_change_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX idx_snapshots_drive_date
                ON daily_snapshots(drive_letter, snapshot_date);
            INSERT INTO daily_snapshots (drive_letter, snapshot_date, total_bytes, used_bytes, free_bytes, updated_at)
            VALUES ('C:\\', '2026-11-15', 500000000000, 400000000000, 100000000000, '2026-11-15T12:00:00');"
        ).unwrap();
    }

    // 2. Open with GreenCDb to trigger migration to Schema v2
    let db = GreenCDb::open(&legacy_db_path).unwrap();

    // 3. Perform an ON CONFLICT upsert on the migrated legacy database
    let snap = db
        .upsert_daily_snapshot(
            "C:\\",
            "2026-11-15",
            550000000000,
            410000000000,
            140000000000,
            Some(10000000000),
        )
        .unwrap();

    assert_eq!(snap.total_bytes, 550000000000);
    assert_eq!(snap.used_bytes, 410000000000);
    assert_eq!(snap.net_change_bytes, 10000000000);
}

#[test]
fn legacy_migration_can_be_disabled_for_sandboxed_runs() {
    use greenc_storage::legacy_migration::{get_legacy_db_candidates, SKIP_LEGACY_MIGRATION_ENV};
    #[cfg(feature = "store")]
    {
        // Microsoft Store (MSIX) builds never auto-migrate legacy or standalone EXE databases.
        let _ = SKIP_LEGACY_MIGRATION_ENV;
        assert!(get_legacy_db_candidates().is_empty());
    }
    #[cfg(not(feature = "store"))]
    {
        // Not set by default in this test process: absolute machine-wide candidates are present.
        if std::env::var(SKIP_LEGACY_MIGRATION_ENV).is_err() {
            assert!(!get_legacy_db_candidates().is_empty());
        }
        std::env::set_var(SKIP_LEGACY_MIGRATION_ENV, "1");
        assert!(get_legacy_db_candidates().is_empty());
        std::env::remove_var(SKIP_LEGACY_MIGRATION_ENV);
    }
}
