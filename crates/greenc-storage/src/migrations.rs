use greenc_core::error::{GreenCError, Result};
use rusqlite::Connection;
use tracing::info;

pub const CURRENT_SCHEMA_VERSION: u32 = 6;

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

CREATE TABLE IF NOT EXISTS savings_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    event_date TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL DEFAULT '',
    app_name TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'other',
    bytes_freed INTEGER NOT NULL,
    drive_letter TEXT NOT NULL DEFAULT 'C',
    reverted INTEGER NOT NULL DEFAULT 0
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

CREATE INDEX IF NOT EXISTS idx_savings_events_date
    ON savings_events(event_date);
CREATE INDEX IF NOT EXISTS idx_savings_events_app
    ON savings_events(app_name);
CREATE INDEX IF NOT EXISTS idx_savings_events_ref
    ON savings_events(source_kind, source_ref);
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

        // 6. Schema v6: Idempotent backfill of `savings_events` from legacy tables
        if current_version < 6 {
            backfill_savings_events(&tx)?;
        }
    }

    tx.execute(
        &format!("PRAGMA user_version = {};", CURRENT_SCHEMA_VERSION),
        [],
    )
    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    tx.commit()
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

    info!("Database successfully migrated to Schema v{}", CURRENT_SCHEMA_VERSION);
    Ok(CURRENT_SCHEMA_VERSION)
}

fn extract_iso_date(ts: &str) -> String {
    if ts.len() >= 10 {
        ts[..10].to_string()
    } else {
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    }
}

pub fn infer_app_from_path_str(path: &str, fallback: &str) -> (String, String) {
    let lower = path.replace('/', "\\").to_lowercase();
    if lower.contains("\\google\\chrome\\") || lower.contains("google-chrome") {
        return ("Google Chrome".to_string(), "browser".to_string());
    }
    if lower.contains("\\microsoft\\edge\\") || lower.contains("microsoft edge") {
        return ("Microsoft Edge".to_string(), "browser".to_string());
    }
    if lower.contains("bravesoftware") || lower.contains("brave-browser") {
        return ("Brave Browser".to_string(), "browser".to_string());
    }
    if lower.contains("coccoc") {
        return ("Cốc Cốc".to_string(), "browser".to_string());
    }
    if lower.contains("firefox") || lower.contains("mozilla") {
        return ("Mozilla Firefox".to_string(), "browser".to_string());
    }
    if lower.contains("\\discord\\") || lower.ends_with("\\discord") {
        return ("Discord".to_string(), "chat".to_string());
    }
    if lower.contains("\\slack\\") || lower.ends_with("\\slack") {
        return ("Slack".to_string(), "chat".to_string());
    }
    if lower.contains("telegram") {
        return ("Telegram Desktop".to_string(), "chat".to_string());
    }
    if lower.contains("zalo") {
        return ("Zalo PC".to_string(), "chat".to_string());
    }
    if lower.contains("msteams") || lower.contains("\\microsoft\\teams\\") {
        return ("Microsoft Teams".to_string(), "chat".to_string());
    }
    if lower.contains("whatsapp") {
        return ("WhatsApp".to_string(), "chat".to_string());
    }
    if lower.contains("wechat") {
        return ("WeChat".to_string(), "chat".to_string());
    }
    if lower.contains("viber") {
        return ("Viber".to_string(), "chat".to_string());
    }
    if lower.contains("\\line\\") {
        return ("LINE".to_string(), "chat".to_string());
    }
    if lower.contains("dingtalk") {
        return ("DingTalk".to_string(), "chat".to_string());
    }
    if lower.contains("npm-cache") || lower.contains("\\.npm\\") {
        return ("npm Cache".to_string(), "dev".to_string());
    }
    if lower.contains("\\pip\\") {
        return ("pip Cache".to_string(), "dev".to_string());
    }
    if lower.contains("\\.cargo\\") {
        return ("Cargo Rust".to_string(), "dev".to_string());
    }
    if lower.contains("\\.gradle\\") {
        return ("Gradle".to_string(), "dev".to_string());
    }
    if lower.contains("\\uv\\") {
        return ("uv Python".to_string(), "dev".to_string());
    }
    if lower.contains("\\pnpm\\") {
        return ("pnpm Store".to_string(), "dev".to_string());
    }
    if lower.contains("\\temp\\") || lower.ends_with("\\temp") {
        return ("Windows Temp".to_string(), "system".to_string());
    }
    if lower.contains("crashdumps") || lower.contains("\\wer\\") || lower.contains("minidump") {
        return ("Crash Dumps".to_string(), "diagnostic".to_string());
    }
    if lower.contains("softwaredistribution") || lower.contains("windows.old") {
        return ("Windows Update".to_string(), "system".to_string());
    }
    if lower.contains("\\logs\\") || lower.contains("\\panther\\") {
        return ("System Logs".to_string(), "logs".to_string());
    }
    (fallback.to_string(), "other".to_string())
}

fn friendly_profile_label(profile_id: &str) -> (&'static str, &'static str) {
    match profile_id {
        "windows_temp" => ("Windows Temp", "system"),
        "browser_cache" => ("Browser Cache", "browser"),
        "chat_media" => ("Chat & Social", "chat"),
        "crash_dumps" => ("Crash Dumps", "diagnostic"),
        "package_managers" => ("Dev Cache", "dev"),
        "app_logs" => ("System Logs", "logs"),
        "old_versions" => ("Old Versions", "system"),
        "orphan_radar" => ("Orphan Data", "other"),
        _ => ("Other", "other"),
    }
}

fn backfill_savings_events(conn: &Connection) -> Result<()> {
    let existing: i64 = conn
        .query_row("SELECT COUNT(*) FROM savings_events;", [], |r| r.get(0))
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    if existing > 0 {
        return Ok(());
    }

    // 1. Vault archives (exclude ROLLED_BACK)
    let mut vault_rows: Vec<(i64, String, String, i64, String)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, app_name, zip_path, original_bytes, created_at
                 FROM vault_archives
                 WHERE UPPER(status) != 'ROLLED_BACK' AND original_bytes > 0;",
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        while let Some(r) = rows.next().map_err(|e| GreenCError::Sqlite(e.to_string()))? {
            vault_rows.push((
                r.get(0).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(1).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(2).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(3).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(4).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
            ));
        }
    }

    for (aid, app_name, zip_path, original_bytes, created_at) in vault_rows {
        let date = extract_iso_date(&created_at);
        let source_ref = std::path::Path::new(&zip_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| aid.to_string());

        if app_name.eq_ignore_ascii_case("QuickCleanup") {
            let mut per_app: std::collections::BTreeMap<(String, String), i64> =
                std::collections::BTreeMap::new();
            let mut fstmt = conn
                .prepare("SELECT original_path, size_bytes FROM vault_files WHERE archive_id = ?1;")
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let mut frows = fstmt
                .query([aid])
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let mut file_sum = 0i64;
            while let Some(fr) = frows.next().map_err(|e| GreenCError::Sqlite(e.to_string()))? {
                let p: String = fr.get(0).map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                let sz: i64 = fr.get(1).map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                if sz > 0 {
                    let key = infer_app_from_path_str(&p, "QuickCleanup");
                    *per_app.entry(key).or_insert(0) += sz;
                    file_sum += sz;
                }
            }
            if !per_app.is_empty() && file_sum > 0 {
                for ((inferred_app, cat), bytes) in per_app {
                    conn.execute(
                        "INSERT INTO savings_events
                            (timestamp, event_date, source_kind, source_ref, app_name, category, bytes_freed, drive_letter, reverted)
                         VALUES (?1, ?2, 'cleanup_vault', ?3, ?4, ?5, ?6, 'C', 0);",
                        rusqlite::params![created_at, date, source_ref, inferred_app, cat, bytes],
                    )
                    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                }
                continue;
            }
        }

        conn.execute(
            "INSERT INTO savings_events
                (timestamp, event_date, source_kind, source_ref, app_name, category, bytes_freed, drive_letter, reverted)
             VALUES (?1, ?2, 'cleanup_vault', ?3, ?4, 'cleanup', ?5, 'C', 0);",
            rusqlite::params![created_at, date, source_ref, app_name, original_bytes],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    }

    // 2. Direct delete log
    let mut direct_rows: Vec<(i64, String, String, String, String, i64)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp, drive_root, profile_id, unit_path, logical_bytes
                 FROM direct_delete_log
                 WHERE logical_bytes > 0;",
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        while let Some(r) = rows.next().map_err(|e| GreenCError::Sqlite(e.to_string()))? {
            direct_rows.push((
                r.get(0).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(1).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(2).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(3).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(4).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(5).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
            ));
        }
    }
    for (did, ts, drive_root, profile_id, unit_path, bytes) in direct_rows {
        let date = extract_iso_date(&ts);
        let (fallback, fallback_cat) = friendly_profile_label(&profile_id);
        let (app_name, cat) = infer_app_from_path_str(&unit_path, fallback);
        let category = if cat == "other" {
            fallback_cat.to_string()
        } else {
            cat
        };
        let drv = drive_root
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_else(|| "C".to_string());
        conn.execute(
            "INSERT INTO savings_events
                (timestamp, event_date, source_kind, source_ref, app_name, category, bytes_freed, drive_letter, reverted)
             VALUES (?1, ?2, 'cleanup_direct', ?3, ?4, ?5, ?6, ?7, 0);",
            rusqlite::params![ts, date, did.to_string(), app_name, category, bytes, drv],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    }

    // 3. Relocations (status = 'done')
    let mut reloc_rows: Vec<(i64, String, i64, String)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, app_name, size_bytes, created_at
                 FROM relocations
                 WHERE LOWER(status) = 'done' AND size_bytes > 0;",
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        while let Some(r) = rows.next().map_err(|e| GreenCError::Sqlite(e.to_string()))? {
            reloc_rows.push((
                r.get(0).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(1).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(2).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(3).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
            ));
        }
    }
    for (rid, app_name, bytes, created_at) in reloc_rows {
        let date = extract_iso_date(&created_at);
        conn.execute(
            "INSERT INTO savings_events
                (timestamp, event_date, source_kind, source_ref, app_name, category, bytes_freed, drive_letter, reverted)
             VALUES (?1, ?2, 'relocate', ?3, ?4, 'relocate', ?5, 'C', 0);",
            rusqlite::params![created_at, date, rid.to_string(), app_name, bytes],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    }

    // 4. Compaction records
    let mut comp_rows: Vec<(i64, String, i64, i64, String)> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, target_path, physical_before_bytes, physical_after_bytes, updated_at
                 FROM compaction_records
                 WHERE physical_before_bytes > physical_after_bytes;",
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
        while let Some(r) = rows.next().map_err(|e| GreenCError::Sqlite(e.to_string()))? {
            comp_rows.push((
                r.get(0).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(1).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(2).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(3).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
                r.get(4).map_err(|e| GreenCError::Sqlite(e.to_string()))?,
            ));
        }
    }
    for (cid, target_path, before_b, after_b, updated_at) in comp_rows {
        let freed = before_b.saturating_sub(after_b);
        if freed <= 0 {
            continue;
        }
        let date = extract_iso_date(&updated_at);
        let fallback = std::path::Path::new(&target_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Compact");
        let (app_name, _) = infer_app_from_path_str(&target_path, fallback);
        conn.execute(
            "INSERT INTO savings_events
                (timestamp, event_date, source_kind, source_ref, app_name, category, bytes_freed, drive_letter, reverted)
             VALUES (?1, ?2, 'compact', ?3, ?4, 'compact', ?5, 'C', 0);",
            rusqlite::params![updated_at, date, cid.to_string(), app_name, freed],
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
    }

    Ok(())
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
