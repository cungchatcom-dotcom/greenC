use greenc_core::error::{GreenCError, Result};
use rusqlite::Connection;
use tracing::info;

pub const CURRENT_SCHEMA_VERSION: u32 = 5;

pub const SCHEMA_V2_FULL_DDL: &str = "
CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS daily_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    drive_letter TEXT NOT NULL,
    snapshot_date TEXT NOT NULL,
    total_bytes INTEGER NOT NULL,
    used_bytes INTEGER NOT NULL,
    free_bytes INTEGER NOT NULL,
    net_change_bytes INTEGER NOT NULL DEFAULT 0,
    reclaimed_bytes INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    UNIQUE(drive_letter, snapshot_date)
);

CREATE TABLE IF NOT EXISTS write_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_date TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    drive_letter TEXT NOT NULL,
    folder_path TEXT NOT NULL,
    file_path TEXT NOT NULL,
    delta_bytes INTEGER NOT NULL,
    process_name TEXT NOT NULL,
    pid INTEGER NOT NULL DEFAULT 0,
    service_name TEXT NOT NULL DEFAULT '',
    service_display_name TEXT NOT NULL DEFAULT '',
    reason_code TEXT NOT NULL,
    reason_detail TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS quota_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_name TEXT NOT NULL,
    process_name TEXT NOT NULL DEFAULT '',
    service_name TEXT NOT NULL DEFAULT '',
    target_paths_json TEXT NOT NULL,
    max_bytes INTEGER NOT NULL,
    watermark_ratio REAL NOT NULL DEFAULT 0.85,
    retention_days INTEGER NOT NULL DEFAULT 7,
    min_file_age_seconds INTEGER NOT NULL DEFAULT 60,
    exclude_patterns_json TEXT NOT NULL DEFAULT '[]',
    protected_files_json TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS drive_quotas (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    drive_root TEXT NOT NULL UNIQUE COLLATE NOCASE,
    limit_bytes INTEGER NOT NULL DEFAULT 0,
    warn_ratio REAL NOT NULL DEFAULT 0.80,
    clean_ratio REAL NOT NULL DEFAULT 0.90,
    target_ratio REAL NOT NULL DEFAULT 0.85,
    auto_clean INTEGER NOT NULL DEFAULT 0,
    auto_backup INTEGER NOT NULL DEFAULT 0,
    backup_dir TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    regenerable_cache_mode TEXT NOT NULL DEFAULT 'vault',
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS guard_cooldowns (
    drive_root TEXT PRIMARY KEY COLLATE NOCASE,
    consecutive_ineffective INTEGER NOT NULL DEFAULT 0,
    cooldown_until TEXT NOT NULL DEFAULT '',
    last_run_at TEXT NOT NULL DEFAULT '',
    last_reason TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS direct_delete_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    drive_root TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    unit_kind TEXT NOT NULL,
    unit_path TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    logical_bytes INTEGER NOT NULL,
    reason TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS vault_archives (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER NOT NULL,
    app_name TEXT NOT NULL,
    zip_path TEXT NOT NULL,
    manifest_path TEXT NOT NULL,
    file_count INTEGER NOT NULL,
    original_bytes INTEGER NOT NULL,
    compressed_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    status TEXT NOT NULL,
    status_reason TEXT NOT NULL,
    resolved_at TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS vault_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    archive_id INTEGER NOT NULL,
    original_path TEXT NOT NULL,
    rel_arcname TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    mtime REAL NOT NULL,
    atime REAL NOT NULL,
    hardlink_paths_json TEXT NOT NULL DEFAULT '[]',
    restored_individually INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(archive_id) REFERENCES vault_archives(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS relocations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id TEXT NOT NULL,
    app_name TEXT NOT NULL,
    strategy TEXT NOT NULL,
    source_path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    health_status TEXT NOT NULL DEFAULT 'healthy',
    process_names TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS compaction_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_path TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    algorithm TEXT NOT NULL,
    logical_bytes INTEGER NOT NULL,
    physical_before_bytes INTEGER NOT NULL,
    physical_after_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS zalo_retained_originals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    relocation_id INTEGER NOT NULL,
    source_path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    retained_path TEXT NOT NULL UNIQUE,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    file_count INTEGER NOT NULL DEFAULT 0,
    health_verified INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    purge_after TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'retained',
    resolved_at TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT ''
);
";

pub const SCHEMA_V2_INDEXES: &str = "
DROP INDEX IF EXISTS idx_snapshots_drive_date;
CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshots_drive_date
    ON daily_snapshots(drive_letter, snapshot_date);

CREATE INDEX IF NOT EXISTS idx_events_drive_date
    ON write_events(drive_letter, event_date);
CREATE INDEX IF NOT EXISTS idx_events_reason
    ON write_events(reason_code);

CREATE INDEX IF NOT EXISTS idx_vault_archives_status
    ON vault_archives(status);
CREATE INDEX IF NOT EXISTS idx_vault_archives_rule
    ON vault_archives(rule_id);

CREATE INDEX IF NOT EXISTS idx_vault_files_archive
    ON vault_files(archive_id);
CREATE INDEX IF NOT EXISTS idx_vault_files_path
    ON vault_files(original_path);

CREATE INDEX IF NOT EXISTS idx_relocations_app_id
    ON relocations(app_id);
CREATE INDEX IF NOT EXISTS idx_relocations_status
    ON relocations(status);

CREATE UNIQUE INDEX IF NOT EXISTS idx_compaction_target_path
    ON compaction_records(target_path);

CREATE INDEX IF NOT EXISTS idx_zalo_retained_status
    ON zalo_retained_originals(status);
CREATE INDEX IF NOT EXISTS idx_zalo_retained_relocation
    ON zalo_retained_originals(relocation_id);
";

pub fn run_migrations(conn: &mut Connection) -> Result<u32> {
    let current_version: u32 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    if current_version >= CURRENT_SCHEMA_VERSION {
        return Ok(current_version);
    }

    info!(
        "Starting database migration from version {} to {}",
        current_version, CURRENT_SCHEMA_VERSION
    );

    let tx = conn
        .transaction()
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    // Check if legacy tables exist when user_version is 0
    let has_legacy_tables: bool = tx
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='daily_snapshots';",
            [],
            |row| {
                let count: i64 = row.get(0)?;
                Ok(count > 0)
            },
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    if current_version == 0 && !has_legacy_tables {
        info!("Applying clean Schema v2 initialization...");
        tx.execute_batch(SCHEMA_V2_FULL_DDL)
            .map_err(|e| GreenCError::Sqlite(format!("Failed to execute Schema v2 DDL: {}", e)))?;
        tx.execute_batch(SCHEMA_V2_INDEXES).map_err(|e| {
            GreenCError::Sqlite(format!("Failed to execute Schema v2 Indexes: {}", e))
        })?;
    } else {
        info!("Applying incremental upgrade to Schema v2...");
        // 1. Ensure all tables exist
        tx.execute_batch(SCHEMA_V2_FULL_DDL)
            .map_err(|e| GreenCError::Sqlite(format!("Failed to ensure tables: {}", e)))?;

        // 2. Upgrade daily_snapshots with reclaimed_bytes
        if !column_exists(&tx, "daily_snapshots", "reclaimed_bytes")? {
            tx.execute(
                "ALTER TABLE daily_snapshots ADD COLUMN reclaimed_bytes INTEGER NOT NULL DEFAULT 0;",
                [],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        }

        // 3. Upgrade vault_files with restored_individually
        if !column_exists(&tx, "vault_files", "restored_individually")? {
            tx.execute(
                "ALTER TABLE vault_files ADD COLUMN restored_individually INTEGER NOT NULL DEFAULT 0;",
                [],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        }

        // 4. Upgrade relocations with health_status
        if !column_exists(&tx, "relocations", "health_status")? {
            tx.execute(
                "ALTER TABLE relocations ADD COLUMN health_status TEXT NOT NULL DEFAULT 'healthy';",
                [],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        }

        // 4b. Upgrade drive_quotas with regenerable_cache_mode (Schema v4)
        if !column_exists(&tx, "drive_quotas", "regenerable_cache_mode")? {
            tx.execute(
                "ALTER TABLE drive_quotas ADD COLUMN regenerable_cache_mode TEXT NOT NULL DEFAULT 'vault';",
                [],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        }

        // 5. Ensure unique index on daily_snapshots by dropping any legacy non-unique index first
        let _ = tx.execute("DROP INDEX IF EXISTS idx_snapshots_drive_date;", []);

        // Deduplicate any historical duplicate snapshots before creating UNIQUE index
        let _ = tx.execute(
            "DELETE FROM daily_snapshots
             WHERE id NOT IN (
                 SELECT MAX(id) FROM daily_snapshots GROUP BY drive_letter, snapshot_date
             );",
            [],
        );

        // Ensure all Schema v2 indexes exist
        tx.execute_batch(SCHEMA_V2_INDEXES)
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    }

    tx.execute(
        &format!("PRAGMA user_version = {};", CURRENT_SCHEMA_VERSION),
        [],
    )
    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    tx.commit()
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    info!("Database successfully migrated to Schema v2");
    Ok(CURRENT_SCHEMA_VERSION)
}

pub fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({});", table))
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    while let Some(row) = rows
        .next()
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?
    {
        let col_name: String = row.get(1).map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        if col_name.eq_ignore_ascii_case(column) {
            return Ok(true);
        }
    }
    Ok(false)
}
