from contextlib import contextmanager
import json
import sqlite3
import threading
from datetime import datetime
from pathlib import Path
from typing import Iterator

from diskguard.models import (
    DailySnapshot,
    FileManifestItem,
    QuotaRule,
    VaultArchive,
    WriteEvent,
)


class DiskGuardDB:
    """Thread-safe SQLite storage layer for DiskGuard with deterministic handle cleanup."""

    def __init__(self, db_path: Path | str):
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = threading.RLock()
        self._init_schema()

    @contextmanager
    def _connect(self) -> Iterator[sqlite3.Connection]:
        conn = sqlite3.connect(str(self.db_path), timeout=30.0, check_same_thread=False)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA synchronous=NORMAL;")
        conn.execute("PRAGMA foreign_keys=ON;")
        try:
            with conn:
                yield conn
        finally:
            conn.close()

    def _init_schema(self) -> None:
        with self._lock, self._connect() as conn:
            conn.execute("PRAGMA journal_mode=WAL;")
            conn.executescript(
                """
                CREATE TABLE IF NOT EXISTS daily_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    drive_letter TEXT NOT NULL,
                    snapshot_date TEXT NOT NULL,
                    total_bytes INTEGER NOT NULL,
                    used_bytes INTEGER NOT NULL,
                    free_bytes INTEGER NOT NULL,
                    net_change_bytes INTEGER NOT NULL DEFAULT 0,
                    updated_at TEXT NOT NULL,
                    UNIQUE(drive_letter, snapshot_date)
                );

                CREATE INDEX IF NOT EXISTS idx_snapshots_drive_date
                    ON daily_snapshots(drive_letter, snapshot_date);

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

                CREATE INDEX IF NOT EXISTS idx_events_drive_date
                    ON write_events(drive_letter, event_date);

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
                    FOREIGN KEY(archive_id) REFERENCES vault_archives(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_vault_files_path
                    ON vault_files(original_path);
                """
            )

    @staticmethod
    def _normalize_drive(drive: str) -> str:
        d = drive.strip().upper()
        if len(d) == 1:
            return f"{d}:\\"
        if len(d) == 2 and d.endswith(":"):
            return f"{d}\\"
        return d

    def upsert_daily_snapshot(
        self,
        drive: str,
        snapshot_date: str,
        total_bytes: int,
        used_bytes: int,
        free_bytes: int,
        net_change_bytes: int | None = None,
    ) -> DailySnapshot:
        drive_norm = self._normalize_drive(drive)
        now_iso = datetime.now().isoformat(timespec="seconds")
        with self._lock, self._connect() as conn:
            if net_change_bytes is None:
                prev_row = conn.execute(
                    """
                    SELECT used_bytes FROM daily_snapshots
                    WHERE drive_letter = ? AND snapshot_date < ?
                    ORDER BY snapshot_date DESC LIMIT 1
                    """,
                    (drive_norm, snapshot_date),
                ).fetchone()
                if prev_row is not None:
                    net_change_bytes = used_bytes - int(prev_row["used_bytes"])
                else:
                    ev_sum = conn.execute(
                        """
                        SELECT COALESCE(SUM(delta_bytes), 0) AS s
                        FROM write_events
                        WHERE drive_letter = ? AND event_date = ?
                        """,
                        (drive_norm, snapshot_date),
                    ).fetchone()
                    net_change_bytes = int(ev_sum["s"]) if ev_sum else 0

            conn.execute(
                """
                INSERT INTO daily_snapshots (
                    drive_letter, snapshot_date, total_bytes, used_bytes,
                    free_bytes, net_change_bytes, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(drive_letter, snapshot_date) DO UPDATE SET
                    total_bytes = excluded.total_bytes,
                    used_bytes = excluded.used_bytes,
                    free_bytes = excluded.free_bytes,
                    net_change_bytes = excluded.net_change_bytes,
                    updated_at = excluded.updated_at
                """,
                (
                    drive_norm,
                    snapshot_date,
                    total_bytes,
                    used_bytes,
                    free_bytes,
                    net_change_bytes,
                    now_iso,
                ),
            )
            row = conn.execute(
                """
                SELECT * FROM daily_snapshots
                WHERE drive_letter = ? AND snapshot_date = ?
                """,
                (drive_norm, snapshot_date),
            ).fetchone()
            return DailySnapshot(
                id=row["id"],
                drive_letter=row["drive_letter"],
                snapshot_date=row["snapshot_date"],
                total_bytes=row["total_bytes"],
                used_bytes=row["used_bytes"],
                free_bytes=row["free_bytes"],
                net_change_bytes=row["net_change_bytes"],
                updated_at=row["updated_at"],
            )

    def get_daily_snapshots(self, drive: str, limit_days: int = 14) -> list[DailySnapshot]:
        drive_norm = self._normalize_drive(drive)
        with self._lock, self._connect() as conn:
            rows = conn.execute(
                """
                SELECT * FROM (
                    SELECT * FROM daily_snapshots
                    WHERE drive_letter = ?
                    ORDER BY snapshot_date DESC
                    LIMIT ?
                ) ORDER BY snapshot_date ASC
                """,
                (drive_norm, limit_days),
            ).fetchall()
            return [
                DailySnapshot(
                    id=r["id"],
                    drive_letter=r["drive_letter"],
                    snapshot_date=r["snapshot_date"],
                    total_bytes=r["total_bytes"],
                    used_bytes=r["used_bytes"],
                    free_bytes=r["free_bytes"],
                    net_change_bytes=r["net_change_bytes"],
                    updated_at=r["updated_at"],
                )
                for r in rows
            ]

    def record_write_event(self, event: WriteEvent) -> int:
        drive_norm = self._normalize_drive(event.drive_letter)
        with self._lock, self._connect() as conn:
            cur = conn.execute(
                """
                INSERT INTO write_events (
                    event_date, timestamp, drive_letter, folder_path, file_path,
                    delta_bytes, process_name, pid, service_name,
                    service_display_name, reason_code, reason_detail
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    event.event_date,
                    event.timestamp,
                    drive_norm,
                    event.folder_path,
                    event.file_path,
                    event.delta_bytes,
                    event.process_name,
                    event.pid,
                    event.service_name,
                    event.service_display_name,
                    event.reason_code,
                    event.reason_detail,
                ),
            )
            return int(cur.lastrowid)

    def clear_write_events_for_drive(self, drive: str) -> None:
        drive_norm = self._normalize_drive(drive)
        with self._lock, self._connect() as conn:
            conn.execute("DELETE FROM write_events WHERE drive_letter = ?", (drive_norm,))

    def get_day_folder_breakdown(self, drive: str, event_date: str) -> list[dict]:
        drive_norm = self._normalize_drive(drive)
        with self._lock, self._connect() as conn:
            rows = conn.execute(
                """
                SELECT
                    folder_path,
                    SUM(delta_bytes) AS total_delta_bytes,
                    COUNT(*) AS event_count,
                    reason_code,
                    reason_detail,
                    process_name,
                    service_name,
                    service_display_name,
                    MAX(timestamp) AS latest_timestamp,
                    MAX(file_path) AS sample_file
                FROM write_events
                WHERE drive_letter = ? AND event_date = ?
                GROUP BY folder_path, reason_code
                ORDER BY total_delta_bytes DESC
                """,
                (drive_norm, event_date),
            ).fetchall()
            return [dict(r) for r in rows]

    def get_day_actor_breakdown(self, drive: str, event_date: str) -> list[dict]:
        drive_norm = self._normalize_drive(drive)
        with self._lock, self._connect() as conn:
            rows = conn.execute(
                """
                SELECT
                    process_name,
                    MAX(pid) AS pid,
                    service_name,
                    service_display_name,
                    SUM(delta_bytes) AS total_delta_bytes,
                    COUNT(*) AS file_count,
                    GROUP_CONCAT(DISTINCT folder_path) AS folders,
                    MAX(reason_detail) AS primary_reason
                FROM write_events
                WHERE drive_letter = ? AND event_date = ?
                GROUP BY process_name, service_name
                ORDER BY total_delta_bytes DESC
                """,
                (drive_norm, event_date),
            ).fetchall()
            return [dict(r) for r in rows]

    def save_quota_rule(self, rule: QuotaRule) -> int:
        now_iso = rule.created_at or datetime.now().isoformat(timespec="seconds")
        with self._lock, self._connect() as conn:
            if rule.id is None:
                cur = conn.execute(
                    """
                    INSERT INTO quota_rules (
                        app_name, process_name, service_name, target_paths_json,
                        max_bytes, watermark_ratio, retention_days, min_file_age_seconds,
                        exclude_patterns_json, protected_files_json, enabled, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        rule.app_name,
                        rule.process_name,
                        rule.service_name,
                        json.dumps(rule.target_paths, ensure_ascii=False),
                        rule.max_bytes,
                        rule.watermark_ratio,
                        rule.retention_days,
                        rule.min_file_age_seconds,
                        json.dumps(rule.exclude_patterns, ensure_ascii=False),
                        json.dumps(rule.protected_files, ensure_ascii=False),
                        1 if rule.enabled else 0,
                        now_iso,
                    ),
                )
                return int(cur.lastrowid)
            else:
                conn.execute(
                    """
                    UPDATE quota_rules SET
                        app_name = ?,
                        process_name = ?,
                        service_name = ?,
                        target_paths_json = ?,
                        max_bytes = ?,
                        watermark_ratio = ?,
                        retention_days = ?,
                        min_file_age_seconds = ?,
                        exclude_patterns_json = ?,
                        protected_files_json = ?,
                        enabled = ?
                    WHERE id = ?
                    """,
                    (
                        rule.app_name,
                        rule.process_name,
                        rule.service_name,
                        json.dumps(rule.target_paths, ensure_ascii=False),
                        rule.max_bytes,
                        rule.watermark_ratio,
                        rule.retention_days,
                        rule.min_file_age_seconds,
                        json.dumps(rule.exclude_patterns, ensure_ascii=False),
                        json.dumps(rule.protected_files, ensure_ascii=False),
                        1 if rule.enabled else 0,
                        rule.id,
                    ),
                )
                return int(rule.id)

    def _row_to_rule(self, r: sqlite3.Row) -> QuotaRule:
        return QuotaRule(
            id=r["id"],
            app_name=r["app_name"],
            process_name=r["process_name"],
            service_name=r["service_name"],
            target_paths=json.loads(r["target_paths_json"]),
            max_bytes=int(r["max_bytes"]),
            watermark_ratio=float(r["watermark_ratio"]),
            retention_days=int(r["retention_days"]),
            min_file_age_seconds=int(r["min_file_age_seconds"]),
            exclude_patterns=json.loads(r["exclude_patterns_json"]),
            protected_files=json.loads(r["protected_files_json"]),
            enabled=bool(r["enabled"]),
            created_at=r["created_at"],
        )

    def list_quota_rules(self, enabled_only: bool = False) -> list[QuotaRule]:
        with self._lock, self._connect() as conn:
            if enabled_only:
                rows = conn.execute(
                    "SELECT * FROM quota_rules WHERE enabled = 1 ORDER BY id ASC"
                ).fetchall()
            else:
                rows = conn.execute("SELECT * FROM quota_rules ORDER BY id ASC").fetchall()
            return [self._row_to_rule(r) for r in rows]

    def get_quota_rule(self, rule_id: int) -> QuotaRule | None:
        with self._lock, self._connect() as conn:
            row = conn.execute("SELECT * FROM quota_rules WHERE id = ?", (rule_id,)).fetchone()
            return self._row_to_rule(row) if row else None

    def add_protected_files(self, rule_id: int, paths: list[str]) -> None:
        rule = self.get_quota_rule(rule_id)
        if not rule:
            return
        existing = set(rule.protected_files)
        for p in paths:
            existing.add(str(Path(p)))
        rule.protected_files = sorted(existing)
        self.save_quota_rule(rule)

    def delete_quota_rule(self, rule_id: int) -> None:
        with self._lock, self._connect() as conn:
            conn.execute("DELETE FROM quota_rules WHERE id = ?", (rule_id,))

    def create_vault_archive(
        self, archive: VaultArchive, files: list[FileManifestItem]
    ) -> int:
        with self._lock, self._connect() as conn:
            cur = conn.execute(
                """
                INSERT INTO vault_archives (
                    rule_id, app_name, zip_path, manifest_path, file_count,
                    original_bytes, compressed_bytes, created_at, expires_at,
                    status, status_reason, resolved_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    archive.rule_id,
                    archive.app_name,
                    archive.zip_path,
                    archive.manifest_path,
                    archive.file_count,
                    archive.original_bytes,
                    archive.compressed_bytes,
                    archive.created_at,
                    archive.expires_at,
                    archive.status,
                    archive.status_reason,
                    archive.resolved_at,
                ),
            )
            archive_id = int(cur.lastrowid)
            for f in files:
                conn.execute(
                    """
                    INSERT INTO vault_files (
                        archive_id, original_path, rel_arcname, size_bytes,
                        sha256, mtime, atime, hardlink_paths_json
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        archive_id,
                        f.original_path,
                        f.rel_arcname,
                        f.size_bytes,
                        f.sha256,
                        f.mtime,
                        f.atime,
                        json.dumps(f.hardlink_paths, ensure_ascii=False),
                    ),
                )
            return archive_id

    def _row_to_archive(self, r: sqlite3.Row) -> VaultArchive:
        return VaultArchive(
            id=r["id"],
            rule_id=r["rule_id"],
            app_name=r["app_name"],
            zip_path=r["zip_path"],
            manifest_path=r["manifest_path"],
            file_count=int(r["file_count"]),
            original_bytes=int(r["original_bytes"]),
            compressed_bytes=int(r["compressed_bytes"]),
            created_at=r["created_at"],
            expires_at=r["expires_at"],
            status=r["status"],
            status_reason=r["status_reason"],
            resolved_at=r["resolved_at"],
        )

    def list_vault_archives(self, status: str | None = None) -> list[VaultArchive]:
        with self._lock, self._connect() as conn:
            if status:
                rows = conn.execute(
                    "SELECT * FROM vault_archives WHERE status = ? ORDER BY id DESC",
                    (status,),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM vault_archives ORDER BY id DESC"
                ).fetchall()
            return [self._row_to_archive(r) for r in rows]

    def get_vault_archive(self, archive_id: int) -> VaultArchive | None:
        with self._lock, self._connect() as conn:
            row = conn.execute(
                "SELECT * FROM vault_archives WHERE id = ?", (archive_id,)
            ).fetchone()
            return self._row_to_archive(row) if row else None

    def get_vault_files(self, archive_id: int) -> list[FileManifestItem]:
        with self._lock, self._connect() as conn:
            rows = conn.execute(
                "SELECT * FROM vault_files WHERE archive_id = ? ORDER BY id ASC",
                (archive_id,),
            ).fetchall()
            return [
                FileManifestItem(
                    id=r["id"],
                    archive_id=r["archive_id"],
                    original_path=r["original_path"],
                    rel_arcname=r["rel_arcname"],
                    size_bytes=int(r["size_bytes"]),
                    sha256=r["sha256"],
                    mtime=float(r["mtime"]),
                    atime=float(r["atime"]),
                    hardlink_paths=json.loads(r["hardlink_paths_json"]),
                )
                for r in rows
            ]

    def update_vault_status(
        self, archive_id: int, status: str, reason: str
    ) -> None:
        now_iso = datetime.now().isoformat(timespec="seconds")
        with self._lock, self._connect() as conn:
            conn.execute(
                """
                UPDATE vault_archives
                SET status = ?, status_reason = ?, resolved_at = ?
                WHERE id = ?
                """,
                (status, reason, now_iso, archive_id),
            )

