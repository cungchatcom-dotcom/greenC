use crate::migrations::run_migrations;
use chrono::{DateTime, NaiveDate, Utc};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::*;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct GreenCDb {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl GreenCDb {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

        // Configure WAL & Safety Pragmas
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -8000;",
        )
        .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

        // Execute idempotent Schema v2 migrations
        run_migrations(&mut conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn with_conn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> Result<R>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| GreenCError::Database(format!("Mutex poisoned: {}", e)))?;
        f(&conn)
    }

    pub fn with_conn_mut<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R>,
    {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| GreenCError::Database(format!("Mutex poisoned: {}", e)))?;
        f(&mut conn)
    }

    // ==========================================
    // Settings CRUD
    // ==========================================

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            let res = conn
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1;",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(res)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
                params![key, value],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    // ==========================================
    // Snapshots & Write Events CRUD
    // ==========================================

    pub fn upsert_daily_snapshot(
        &self,
        drive: &str,
        date: &str,
        total: u64,
        used: u64,
        free: u64,
        net_change: Option<i64>,
    ) -> Result<DailySnapshot> {
        let now = Utc::now().to_rfc3339();
        let net = net_change.unwrap_or(0);
        let parsed_date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|e| GreenCError::ConfigError(format!("Invalid date format: {}", e)))?;

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO daily_snapshots (
                    drive_letter, snapshot_date, total_bytes, used_bytes, free_bytes, net_change_bytes, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(drive_letter, snapshot_date) DO UPDATE SET
                    total_bytes = excluded.total_bytes,
                    used_bytes = excluded.used_bytes,
                    free_bytes = excluded.free_bytes,
                    net_change_bytes = excluded.net_change_bytes,
                    updated_at = excluded.updated_at;",
                params![drive, date, total as i64, used as i64, free as i64, net, now],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let snap = conn.query_row(
                "SELECT id, drive_letter, snapshot_date, total_bytes, used_bytes, free_bytes, net_change_bytes, reclaimed_bytes, updated_at
                 FROM daily_snapshots WHERE drive_letter = ?1 AND snapshot_date = ?2;",
                params![drive, date],
                |r| {
                    let date_str: String = r.get(2)?;
                    let updated_str: String = r.get(8)?;
                    Ok(DailySnapshot {
                        id: Some(r.get(0)?),
                        drive_letter: r.get(1)?,
                        snapshot_date: NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").unwrap_or(parsed_date),
                        total_bytes: r.get::<_, i64>(3)? as u64,
                        used_bytes: r.get::<_, i64>(4)? as u64,
                        free_bytes: r.get::<_, i64>(5)? as u64,
                        net_change_bytes: r.get(6)?,
                        reclaimed_bytes: r.get::<_, i64>(7)? as u64,
                        updated_at: DateTime::parse_from_rfc3339(&updated_str).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                    })
                },
            ).map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            Ok(snap)
        })
    }

    pub fn get_daily_snapshots(&self, drive: &str, limit_days: u32) -> Result<Vec<DailySnapshot>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, drive_letter, snapshot_date, total_bytes, used_bytes, free_bytes, net_change_bytes, reclaimed_bytes, updated_at
                     FROM daily_snapshots WHERE drive_letter = ?1
                     ORDER BY snapshot_date ASC LIMIT ?2;",
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let rows = stmt
                .query_map(params![drive, limit_days], |r| {
                    let date_str: String = r.get(2)?;
                    let updated_str: String = r.get(8)?;
                    Ok(DailySnapshot {
                        id: Some(r.get(0)?),
                        drive_letter: r.get(1)?,
                        snapshot_date: NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").unwrap_or_default(),
                        total_bytes: r.get::<_, i64>(3)? as u64,
                        used_bytes: r.get::<_, i64>(4)? as u64,
                        free_bytes: r.get::<_, i64>(5)? as u64,
                        net_change_bytes: r.get(6)?,
                        reclaimed_bytes: r.get::<_, i64>(7)? as u64,
                        updated_at: DateTime::parse_from_rfc3339(&updated_str).map(|d| d.with_timezone(&Utc)).unwrap_or_else(|_| Utc::now()),
                    })
                })
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    pub fn record_reclaimed_bytes(&self, drive: &str, date: &str, bytes: u64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE daily_snapshots
                 SET reclaimed_bytes = reclaimed_bytes + ?1, updated_at = ?2
                 WHERE drive_letter = ?3 AND snapshot_date = ?4;",
                params![bytes as i64, now, drive, date],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    pub fn record_write_event(&self, event: &WriteEvent) -> Result<i64> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO write_events (
                    event_date, timestamp, drive_letter, folder_path, file_path, delta_bytes,
                    process_name, pid, service_name, service_display_name, reason_code, reason_detail
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12);",
                params![
                    event.event_date,
                    event.timestamp.to_rfc3339(),
                    event.drive_letter,
                    event.folder_path.to_string_lossy().to_string(),
                    event.file_path.to_string_lossy().to_string(),
                    event.delta_bytes,
                    event.process_name,
                    event.pid,
                    event.service_name,
                    event.service_display_name,
                    event.reason_code.as_str(),
                    event.reason_detail
                ],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn record_write_events_batch(&self, events: &[WriteEvent]) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO write_events (
                            event_date, timestamp, drive_letter, folder_path, file_path, delta_bytes,
                            process_name, pid, service_name, service_display_name, reason_code, reason_detail
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12);",
                    )
                    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

                for event in events {
                    stmt.execute(params![
                        event.event_date,
                        event.timestamp.to_rfc3339(),
                        event.drive_letter,
                        event.folder_path.to_string_lossy().to_string(),
                        event.file_path.to_string_lossy().to_string(),
                        event.delta_bytes,
                        event.process_name,
                        event.pid,
                        event.service_name,
                        event.service_display_name,
                        event.reason_code.as_str(),
                        event.reason_detail
                    ])
                    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                }
            }
            tx.commit().map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    pub fn get_day_folder_breakdown(
        &self,
        drive: &str,
        date: &str,
    ) -> Result<Vec<FolderGrowthRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT folder_path, reason_code, SUM(delta_bytes) as total_delta, COUNT(*) as file_count
                     FROM write_events
                     WHERE drive_letter = ?1 AND event_date = ?2
                     GROUP BY folder_path, reason_code
                     ORDER BY total_delta DESC;",
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let rows = stmt
                .query_map(params![drive, date], |r| {
                    let folder: String = r.get(0)?;
                    Ok(FolderGrowthRow {
                        folder_path: PathBuf::from(folder),
                        reason_code: r.get(1)?,
                        total_delta_bytes: r.get(2)?,
                        file_count: r.get::<_, i64>(3)? as u64,
                    })
                })
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    pub fn get_day_actor_breakdown(&self, drive: &str, date: &str) -> Result<Vec<ActorWriteRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT process_name, service_name, SUM(delta_bytes) as total_delta, COUNT(*) as event_count
                     FROM write_events
                     WHERE drive_letter = ?1 AND event_date = ?2
                     GROUP BY process_name, service_name
                     ORDER BY total_delta DESC;",
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let rows = stmt
                .query_map(params![drive, date], |r| {
                    Ok(ActorWriteRow {
                        process_name: r.get(0)?,
                        service_name: r.get(1)?,
                        total_delta_bytes: r.get(2)?,
                        event_count: r.get::<_, i64>(3)? as u64,
                    })
                })
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    // ==========================================
    // Quota Rules CRUD
    // ==========================================

    pub fn save_quota_rule(&self, rule: &QuotaRule) -> Result<i64> {
        let targets_json = serde_json::to_string(&rule.target_paths)?;
        let exclude_json = serde_json::to_string(&rule.exclude_patterns)?;
        let protected_json = serde_json::to_string(&rule.protected_files)?;

        self.with_conn(|conn| {
            if let Some(id) = rule.id {
                conn.execute(
                    "UPDATE quota_rules SET
                        app_name = ?1, process_name = ?2, service_name = ?3,
                        target_paths_json = ?4, max_bytes = ?5, watermark_ratio = ?6,
                        retention_days = ?7, min_file_age_seconds = ?8,
                        exclude_patterns_json = ?9, protected_files_json = ?10,
                        enabled = ?11
                     WHERE id = ?12;",
                    params![
                        rule.app_name,
                        rule.process_name,
                        rule.service_name,
                        targets_json,
                        rule.max_bytes as i64,
                        rule.watermark_ratio,
                        rule.retention_days,
                        rule.min_file_age_seconds as i64,
                        exclude_json,
                        protected_json,
                        if rule.enabled { 1 } else { 0 },
                        id
                    ],
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                Ok(id)
            } else {
                conn.execute(
                    "INSERT INTO quota_rules (
                        app_name, process_name, service_name, target_paths_json, max_bytes,
                        watermark_ratio, retention_days, min_file_age_seconds,
                        exclude_patterns_json, protected_files_json, enabled, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12);",
                    params![
                        rule.app_name,
                        rule.process_name,
                        rule.service_name,
                        targets_json,
                        rule.max_bytes as i64,
                        rule.watermark_ratio,
                        rule.retention_days,
                        rule.min_file_age_seconds as i64,
                        exclude_json,
                        protected_json,
                        if rule.enabled { 1 } else { 0 },
                        rule.created_at.to_rfc3339()
                    ],
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                Ok(conn.last_insert_rowid())
            }
        })
    }

    pub fn list_quota_rules(&self, enabled_only: bool) -> Result<Vec<QuotaRule>> {
        self.with_conn(|conn| {
            let sql = if enabled_only {
                "SELECT id, app_name, process_name, service_name, target_paths_json, max_bytes,
                        watermark_ratio, retention_days, min_file_age_seconds,
                        exclude_patterns_json, protected_files_json, enabled, created_at
                 FROM quota_rules WHERE enabled = 1 ORDER BY id ASC;"
            } else {
                "SELECT id, app_name, process_name, service_name, target_paths_json, max_bytes,
                        watermark_ratio, retention_days, min_file_age_seconds,
                        exclude_patterns_json, protected_files_json, enabled, created_at
                 FROM quota_rules ORDER BY id ASC;"
            };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let rows = stmt
                .query_map([], |r| {
                    let targets_str: String = r.get(4)?;
                    let exclude_str: String = r.get(9)?;
                    let protected_str: String = r.get(10)?;
                    let created_str: String = r.get(12)?;

                    Ok(QuotaRule {
                        id: Some(r.get(0)?),
                        app_name: r.get(1)?,
                        process_name: r.get(2)?,
                        service_name: r.get(3)?,
                        target_paths: serde_json::from_str(&targets_str).unwrap_or_default(),
                        max_bytes: r.get::<_, i64>(5)? as u64,
                        watermark_ratio: r.get(6)?,
                        retention_days: r.get(7)?,
                        min_file_age_seconds: r.get::<_, i64>(8)? as u64,
                        exclude_patterns: serde_json::from_str(&exclude_str).unwrap_or_default(),
                        protected_files: serde_json::from_str(&protected_str).unwrap_or_default(),
                        enabled: r.get::<_, i64>(11)? == 1,
                        created_at: DateTime::parse_from_rfc3339(&created_str)
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    })
                })
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    pub fn get_quota_rule(&self, rule_id: i64) -> Result<Option<QuotaRule>> {
        self.with_conn(|conn| {
            let res = conn
                .query_row(
                    "SELECT id, app_name, process_name, service_name, target_paths_json, max_bytes,
                            watermark_ratio, retention_days, min_file_age_seconds,
                            exclude_patterns_json, protected_files_json, enabled, created_at
                     FROM quota_rules WHERE id = ?1;",
                    params![rule_id],
                    |r| {
                        let targets_str: String = r.get(4)?;
                        let exclude_str: String = r.get(9)?;
                        let protected_str: String = r.get(10)?;
                        let created_str: String = r.get(12)?;

                        Ok(QuotaRule {
                            id: Some(r.get(0)?),
                            app_name: r.get(1)?,
                            process_name: r.get(2)?,
                            service_name: r.get(3)?,
                            target_paths: serde_json::from_str(&targets_str).unwrap_or_default(),
                            max_bytes: r.get::<_, i64>(5)? as u64,
                            watermark_ratio: r.get(6)?,
                            retention_days: r.get(7)?,
                            min_file_age_seconds: r.get::<_, i64>(8)? as u64,
                            exclude_patterns: serde_json::from_str(&exclude_str)
                                .unwrap_or_default(),
                            protected_files: serde_json::from_str(&protected_str)
                                .unwrap_or_default(),
                            enabled: r.get::<_, i64>(11)? == 1,
                            created_at: DateTime::parse_from_rfc3339(&created_str)
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(|_| Utc::now()),
                        })
                    },
                )
                .optional()
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(res)
        })
    }

    pub fn delete_quota_rule(&self, rule_id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM quota_rules WHERE id = ?1;", params![rule_id])
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    pub fn add_protected_files(&self, rule_id: i64, paths: &[String]) -> Result<()> {
        self.with_conn(|conn| {
            let current_json: String = conn
                .query_row(
                    "SELECT protected_files_json FROM quota_rules WHERE id = ?1;",
                    params![rule_id],
                    |r| r.get(0),
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut current: Vec<String> = serde_json::from_str(&current_json).unwrap_or_default();
            for p in paths {
                if !current.contains(p) {
                    current.push(p.clone());
                }
            }

            let new_json = serde_json::to_string(&current)?;
            conn.execute(
                "UPDATE quota_rules SET protected_files_json = ?1 WHERE id = ?2;",
                params![new_json, rule_id],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    // ==========================================
    // Vault Archives & Files CRUD
    // ==========================================

    pub fn create_vault_archive(
        &self,
        arch: &VaultArchive,
        files: &[VaultFileItem],
    ) -> Result<i64> {
        self.with_conn_mut(|conn| {
            let tx = conn
                .transaction()
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let arch_id = {
                tx.execute(
                    "INSERT INTO vault_archives (
                        rule_id, app_name, zip_path, manifest_path, file_count,
                        original_bytes, compressed_bytes, created_at, expires_at,
                        status, status_reason, resolved_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12);",
                    params![
                        arch.rule_id,
                        arch.app_name,
                        arch.zip_path.to_string_lossy().to_string(),
                        arch.manifest_path.to_string_lossy().to_string(),
                        arch.file_count as i64,
                        arch.original_bytes as i64,
                        arch.compressed_bytes as i64,
                        arch.created_at.to_rfc3339(),
                        arch.expires_at.to_rfc3339(),
                        arch.status.as_str(),
                        arch.status_reason,
                        arch.resolved_at.map(|d| d.to_rfc3339()).unwrap_or_default()
                    ],
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                tx.last_insert_rowid()
            };

            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO vault_files (
                            archive_id, original_path, rel_arcname, size_bytes,
                            sha256, mtime, atime, hardlink_paths_json, restored_individually
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9);",
                    )
                    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

                for f in files {
                    let hardlinks_json = serde_json::to_string(&f.hardlink_paths)?;
                    stmt.execute(params![
                        arch_id,
                        f.original_path.to_string_lossy().to_string(),
                        f.rel_arcname,
                        f.size_bytes as i64,
                        f.sha256,
                        f.mtime,
                        f.atime,
                        hardlinks_json,
                        if f.restored_individually { 1 } else { 0 }
                    ])
                    .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                }
            }

            tx.commit()
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(arch_id)
        })
    }

    pub fn list_vault_archives(&self, status: Option<&str>) -> Result<Vec<VaultArchive>> {
        self.with_conn(|conn| {
            let (sql, param) = match status {
                Some(st) => (
                    "SELECT id, rule_id, app_name, zip_path, manifest_path, file_count,
                            original_bytes, compressed_bytes, created_at, expires_at,
                            status, status_reason, resolved_at
                     FROM vault_archives WHERE status = ?1 ORDER BY id DESC;",
                    Some(st),
                ),
                None => (
                    "SELECT id, rule_id, app_name, zip_path, manifest_path, file_count,
                            original_bytes, compressed_bytes, created_at, expires_at,
                            status, status_reason, resolved_at
                     FROM vault_archives ORDER BY id DESC;",
                    None,
                ),
            };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let mapper = |r: &rusqlite::Row| {
                let zip_str: String = r.get(3)?;
                let man_str: String = r.get(4)?;
                let created_str: String = r.get(8)?;
                let expires_str: String = r.get(9)?;
                let status_str: String = r.get(10)?;
                let resolved_str: String = r.get(12)?;

                Ok(VaultArchive {
                    id: Some(r.get(0)?),
                    rule_id: r.get(1)?,
                    app_name: r.get(2)?,
                    zip_path: PathBuf::from(zip_str),
                    manifest_path: PathBuf::from(man_str),
                    file_count: r.get::<_, i64>(5)? as u64,
                    original_bytes: r.get::<_, i64>(6)? as u64,
                    compressed_bytes: r.get::<_, i64>(7)? as u64,
                    created_at: DateTime::parse_from_rfc3339(&created_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    expires_at: DateTime::parse_from_rfc3339(&expires_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    status: VaultStatus::from_str_loose(&status_str),
                    status_reason: r.get(11)?,
                    resolved_at: DateTime::parse_from_rfc3339(&resolved_str)
                        .map(|d| d.with_timezone(&Utc))
                        .ok(),
                })
            };

            let rows = if let Some(p) = param {
                stmt.query_map(params![p], mapper)
            } else {
                stmt.query_map([], mapper)
            }
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    pub fn get_vault_archive(&self, archive_id: i64) -> Result<Option<VaultArchive>> {
        self.with_conn(|conn| {
            let res = conn
                .query_row(
                    "SELECT id, rule_id, app_name, zip_path, manifest_path, file_count,
                            original_bytes, compressed_bytes, created_at, expires_at,
                            status, status_reason, resolved_at
                     FROM vault_archives WHERE id = ?1;",
                    params![archive_id],
                    |r| {
                        let zip_str: String = r.get(3)?;
                        let man_str: String = r.get(4)?;
                        let created_str: String = r.get(8)?;
                        let expires_str: String = r.get(9)?;
                        let status_str: String = r.get(10)?;
                        let resolved_str: String = r.get(12)?;

                        Ok(VaultArchive {
                            id: Some(r.get(0)?),
                            rule_id: r.get(1)?,
                            app_name: r.get(2)?,
                            zip_path: PathBuf::from(zip_str),
                            manifest_path: PathBuf::from(man_str),
                            file_count: r.get::<_, i64>(5)? as u64,
                            original_bytes: r.get::<_, i64>(6)? as u64,
                            compressed_bytes: r.get::<_, i64>(7)? as u64,
                            created_at: DateTime::parse_from_rfc3339(&created_str)
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(|_| Utc::now()),
                            expires_at: DateTime::parse_from_rfc3339(&expires_str)
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(|_| Utc::now()),
                            status: VaultStatus::from_str_loose(&status_str),
                            status_reason: r.get(11)?,
                            resolved_at: DateTime::parse_from_rfc3339(&resolved_str)
                                .map(|d| d.with_timezone(&Utc))
                                .ok(),
                        })
                    },
                )
                .optional()
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(res)
        })
    }

    pub fn get_vault_files(&self, archive_id: i64) -> Result<Vec<VaultFileItem>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, archive_id, original_path, rel_arcname, size_bytes,
                            sha256, mtime, atime, hardlink_paths_json, restored_individually
                     FROM vault_files WHERE archive_id = ?1 ORDER BY id ASC;",
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let rows = stmt
                .query_map(params![archive_id], |r| {
                    let orig_str: String = r.get(2)?;
                    let hardlinks_str: String = r.get(8)?;
                    Ok(VaultFileItem {
                        id: Some(r.get(0)?),
                        archive_id: Some(r.get(1)?),
                        original_path: PathBuf::from(orig_str),
                        rel_arcname: r.get(3)?,
                        size_bytes: r.get::<_, i64>(4)? as u64,
                        sha256: r.get(5)?,
                        mtime: r.get(6)?,
                        atime: r.get(7)?,
                        hardlink_paths: serde_json::from_str(&hardlinks_str).unwrap_or_default(),
                        restored_individually: r.get::<_, i64>(9)? == 1,
                    })
                })
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    pub fn update_vault_status(&self, archive_id: i64, status: &str, reason: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE vault_archives
                 SET status = ?1, status_reason = ?2, resolved_at = ?3
                 WHERE id = ?4;",
                params![status, reason, now, archive_id],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    pub fn update_vault_paths(
        &self,
        archive_id: i64,
        new_zip: &Path,
        new_manifest: &Path,
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE vault_archives
                 SET zip_path = ?1, manifest_path = ?2
                 WHERE id = ?3;",
                params![
                    new_zip.to_string_lossy().to_string(),
                    new_manifest.to_string_lossy().to_string(),
                    archive_id
                ],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    pub fn mark_file_restored(&self, file_id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE vault_files SET restored_individually = 1 WHERE id = ?1;",
                params![file_id],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    // ==========================================
    // Relocations CRUD
    // ==========================================

    pub fn save_relocation(&self, rec: &RelocationRecord) -> Result<i64> {
        let procs_str = rec.process_names.join(",");
        self.with_conn(|conn| {
            if let Some(id) = rec.id {
                conn.execute(
                    "UPDATE relocations SET
                        app_id = ?1, app_name = ?2, strategy = ?3,
                        source_path = ?4, target_path = ?5, size_bytes = ?6,
                        status = ?7, health_status = ?8, process_names = ?9, detail = ?10
                     WHERE id = ?11;",
                    params![
                        rec.app_id,
                        rec.app_name,
                        rec.strategy.as_str(),
                        rec.source_path.to_string_lossy().to_string(),
                        rec.target_path.to_string_lossy().to_string(),
                        rec.size_bytes as i64,
                        rec.status.as_str(),
                        rec.health_status.as_str(),
                        procs_str,
                        rec.detail.clone().unwrap_or_default(),
                        id
                    ],
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                Ok(id)
            } else {
                conn.execute(
                    "INSERT INTO relocations (
                        app_id, app_name, strategy, source_path, target_path,
                        size_bytes, status, health_status, process_names, created_at, detail
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11);",
                    params![
                        rec.app_id,
                        rec.app_name,
                        rec.strategy.as_str(),
                        rec.source_path.to_string_lossy().to_string(),
                        rec.target_path.to_string_lossy().to_string(),
                        rec.size_bytes as i64,
                        rec.status.as_str(),
                        rec.health_status.as_str(),
                        procs_str,
                        rec.created_at.to_rfc3339(),
                        rec.detail.clone().unwrap_or_default()
                    ],
                )
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
                Ok(conn.last_insert_rowid())
            }
        })
    }

    pub fn list_relocations(&self, status: Option<&str>) -> Result<Vec<RelocationRecord>> {
        self.with_conn(|conn| {
            let (sql, param) = match status {
                Some(st) => (
                    "SELECT id, app_id, app_name, strategy, source_path, target_path,
                            size_bytes, status, health_status, process_names, created_at, detail
                     FROM relocations WHERE status = ?1 ORDER BY id DESC;",
                    Some(st),
                ),
                None => (
                    "SELECT id, app_id, app_name, strategy, source_path, target_path,
                            size_bytes, status, health_status, process_names, created_at, detail
                     FROM relocations ORDER BY id DESC;",
                    None,
                ),
            };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let mapper = |r: &rusqlite::Row| {
                let strat_str: String = r.get(3)?;
                let src_str: String = r.get(4)?;
                let tgt_str: String = r.get(5)?;
                let status_str: String = r.get(7)?;
                let health_str: String = r.get(8)?;
                let procs_str: String = r.get(9)?;
                let created_str: String = r.get(10)?;
                let detail_str: String = r.get(11)?;

                Ok(RelocationRecord {
                    id: Some(r.get(0)?),
                    app_id: r.get(1)?,
                    app_name: r.get(2)?,
                    strategy: RelocationStrategyKind::from_str_loose(&strat_str),
                    source_path: PathBuf::from(src_str),
                    target_path: PathBuf::from(tgt_str),
                    size_bytes: r.get::<_, i64>(6)? as u64,
                    status: RelocationStatus::from_str_loose(&status_str),
                    health_status: RelocationHealthStatus::from_str_loose(&health_str),
                    process_names: procs_str
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                    created_at: DateTime::parse_from_rfc3339(&created_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    detail: if detail_str.is_empty() {
                        None
                    } else {
                        Some(detail_str)
                    },
                })
            };

            let rows = if let Some(p) = param {
                stmt.query_map(params![p], mapper)
            } else {
                stmt.query_map([], mapper)
            }
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }

    pub fn update_relocation_health(&self, rec_id: i64, health: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE relocations SET health_status = ?1 WHERE id = ?2;",
                params![health, rec_id],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }

    // ==========================================
    // Compaction Records CRUD
    // ==========================================

    pub fn save_compaction_record(&self, rec: &CompactionRecord) -> Result<i64> {
        let tgt = rec.target_path.to_string_lossy().to_string();
        let now = rec.updated_at.to_rfc3339();

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO compaction_records (
                    target_path, kind, algorithm, logical_bytes,
                    physical_before_bytes, physical_after_bytes, file_count, status, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(target_path) DO UPDATE SET
                    kind = excluded.kind,
                    algorithm = excluded.algorithm,
                    logical_bytes = excluded.logical_bytes,
                    physical_before_bytes = excluded.physical_before_bytes,
                    physical_after_bytes = excluded.physical_after_bytes,
                    file_count = excluded.file_count,
                    status = excluded.status,
                    updated_at = excluded.updated_at;",
                params![
                    tgt,
                    rec.kind.as_str(),
                    rec.algorithm,
                    rec.logical_bytes as i64,
                    rec.physical_before_bytes as i64,
                    rec.physical_after_bytes as i64,
                    rec.file_count as i64,
                    rec.status,
                    now
                ],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            Ok(conn.last_insert_rowid())
        })
    }

    pub fn get_compaction_record(&self, target_path: &Path) -> Result<Option<CompactionRecord>> {
        let tgt = target_path.to_string_lossy().to_string();
        self.with_conn(|conn| {
            let res = conn
                .query_row(
                    "SELECT id, target_path, kind, algorithm, logical_bytes,
                            physical_before_bytes, physical_after_bytes, file_count, status, updated_at
                     FROM compaction_records WHERE target_path = ?1;",
                    params![tgt],
                    |r| {
                        let path_str: String = r.get(1)?;
                        let kind_str: String = r.get(2)?;
                        let updated_str: String = r.get(9)?;

                        Ok(CompactionRecord {
                            id: Some(r.get(0)?),
                            target_path: PathBuf::from(path_str),
                            kind: CompactionKind::from_str_loose(&kind_str),
                            algorithm: r.get(3)?,
                            logical_bytes: r.get::<_, i64>(4)? as u64,
                            physical_before_bytes: r.get::<_, i64>(5)? as u64,
                            physical_after_bytes: r.get::<_, i64>(6)? as u64,
                            file_count: r.get::<_, i64>(7)? as u64,
                            status: r.get(8)?,
                            updated_at: DateTime::parse_from_rfc3339(&updated_str)
                                .map(|d| d.with_timezone(&Utc))
                                .unwrap_or_else(|_| Utc::now()),
                        })
                    },
                )
                .optional()
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(res)
        })
    }

    pub fn list_compaction_records(&self, status: Option<&str>) -> Result<Vec<CompactionRecord>> {
        self.with_conn(|conn| {
            let (sql, param) = match status {
                Some(st) => (
                    "SELECT id, target_path, kind, algorithm, logical_bytes,
                            physical_before_bytes, physical_after_bytes, file_count, status, updated_at
                     FROM compaction_records WHERE status = ?1 ORDER BY id DESC;",
                    Some(st),
                ),
                None => (
                    "SELECT id, target_path, kind, algorithm, logical_bytes,
                            physical_before_bytes, physical_after_bytes, file_count, status, updated_at
                     FROM compaction_records ORDER BY id DESC;",
                    None,
                ),
            };

            let mut stmt = conn.prepare(sql).map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let mapper = |r: &rusqlite::Row| {
                let path_str: String = r.get(1)?;
                let kind_str: String = r.get(2)?;
                let updated_str: String = r.get(9)?;

                Ok(CompactionRecord {
                    id: Some(r.get(0)?),
                    target_path: PathBuf::from(path_str),
                    kind: CompactionKind::from_str_loose(&kind_str),
                    algorithm: r.get(3)?,
                    logical_bytes: r.get::<_, i64>(4)? as u64,
                    physical_before_bytes: r.get::<_, i64>(5)? as u64,
                    physical_after_bytes: r.get::<_, i64>(6)? as u64,
                    file_count: r.get::<_, i64>(7)? as u64,
                    status: r.get(8)?,
                    updated_at: DateTime::parse_from_rfc3339(&updated_str)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                })
            };

            let rows = if let Some(p) = param {
                stmt.query_map(params![p], mapper)
            } else {
                stmt.query_map([], mapper)
            }
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;

            let mut result = Vec::new();
            for item in rows {
                result.push(item.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(result)
        })
    }
}

// ==========================================
// Drive Quotas CRUD
// ==========================================

impl GreenCDb {
    /// Inserts or updates a per-drive quota. One quota per `drive_root` (case-insensitive).
    pub fn save_drive_quota(&self, quota: &DriveQuota) -> Result<i64> {
        let backup = quota
            .backup_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let root = quota.drive_root.clone();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO drive_quotas (
                    drive_root, limit_bytes, warn_ratio, clean_ratio, target_ratio,
                    auto_clean, auto_backup, backup_dir, enabled, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(drive_root) DO UPDATE SET
                    limit_bytes = excluded.limit_bytes,
                    warn_ratio = excluded.warn_ratio,
                    clean_ratio = excluded.clean_ratio,
                    target_ratio = excluded.target_ratio,
                    auto_clean = excluded.auto_clean,
                    auto_backup = excluded.auto_backup,
                    backup_dir = excluded.backup_dir,
                    enabled = excluded.enabled;",
                params![
                    root,
                    quota.limit_bytes as i64,
                    quota.warn_ratio,
                    quota.clean_ratio,
                    quota.target_ratio,
                    quota.auto_clean as i64,
                    quota.auto_backup as i64,
                    backup,
                    quota.enabled as i64,
                    quota.created_at.to_rfc3339()
                ],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            conn.query_row(
                "SELECT id FROM drive_quotas WHERE drive_root = ?1;",
                params![root],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))
        })
    }

    pub fn list_drive_quotas(&self, enabled_only: bool) -> Result<Vec<DriveQuota>> {
        self.with_conn(|conn| {
            let sql = format!(
                "SELECT id, drive_root, limit_bytes, warn_ratio, clean_ratio, target_ratio,
                        auto_clean, auto_backup, backup_dir, enabled, created_at
                 FROM drive_quotas {} ORDER BY drive_root ASC;",
                if enabled_only { "WHERE enabled = 1" } else { "" }
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let rows = stmt
                .query_map([], |r| {
                    let backup: String = r.get(8)?;
                    let created: String = r.get(10)?;
                    Ok(DriveQuota {
                        id: Some(r.get(0)?),
                        drive_root: r.get(1)?,
                        limit_bytes: r.get::<_, i64>(2)? as u64,
                        warn_ratio: r.get(3)?,
                        clean_ratio: r.get(4)?,
                        target_ratio: r.get(5)?,
                        auto_clean: r.get::<_, i64>(6)? == 1,
                        auto_backup: r.get::<_, i64>(7)? == 1,
                        backup_dir: if backup.is_empty() {
                            None
                        } else {
                            Some(PathBuf::from(backup))
                        },
                        enabled: r.get::<_, i64>(9)? == 1,
                        created_at: DateTime::parse_from_rfc3339(&created)
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    })
                })
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| GreenCError::Sqlite(e.to_string()))?);
            }
            Ok(out)
        })
    }

    pub fn delete_drive_quota(&self, id: i64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM drive_quotas WHERE id = ?1;", params![id])
                .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod drive_quota_db_tests {
    use super::*;

    #[test]
    fn drive_quota_roundtrip_and_upsert() {
        let dir = tempfile::tempdir().unwrap();
        let db = GreenCDb::open(dir.path().join("q.db")).unwrap();
        let mut q = DriveQuota::new("c:\\");
        q.limit_bytes = 123;
        q.auto_clean = true;
        q.backup_dir = Some(PathBuf::from("D:/bk"));
        let id = db.save_drive_quota(&q).unwrap();

        // Saving the same drive again updates instead of duplicating.
        q.limit_bytes = 456;
        q.auto_clean = false;
        assert_eq!(db.save_drive_quota(&q).unwrap(), id);

        let all = db.list_drive_quotas(false).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].drive_root, "c:\\");
        assert_eq!(all[0].limit_bytes, 456);
        assert!(!all[0].auto_clean);
        assert_eq!(all[0].backup_dir, Some(PathBuf::from("D:/bk")));

        db.delete_drive_quota(id).unwrap();
        assert!(db.list_drive_quotas(false).unwrap().is_empty());
    }
}
