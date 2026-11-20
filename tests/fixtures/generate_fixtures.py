#!/usr/bin/env python3
"""
Synthetic Fixtures Generator for GreenC v2 E2E Tests.
Generates deterministic test fixtures matching specifications in PROJECT.md and spec.md:
1. Legacy SQLite v1/v2 databases (greenc_v1.db, diskguard_v1.db)
2. Mock directory trees (Zalo, DingTalk, dev caches, downloads, corrupt data)
3. Corrupt and adversarial archives (broken CRC, tampered manifest, truncated stream)
4. Sample locales and edge cases
5. Crash recovery state (.greenc-moving)
"""

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import struct
import zipfile

FIXTURES_ROOT = Path(__file__).parent.resolve()


def generate_legacy_db():
    db_dir = FIXTURES_ROOT / "legacy_db"
    db_dir.mkdir(parents=True, exist_ok=True)

    # 1. greenc_v1.db (Schema v1 from Python GreenC 2.0)
    v1_path = db_dir / "greenc_v1.db"
    if v1_path.exists():
        v1_path.unlink()

    conn = sqlite3.connect(str(v1_path))
    conn.execute("PRAGMA journal_mode=WAL;")
    conn.executescript("""
        CREATE TABLE daily_snapshots (
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

        CREATE TABLE write_events (
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

        CREATE TABLE quota_rules (
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

        CREATE TABLE vault_archives (
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

        CREATE TABLE vault_files (
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

        CREATE TABLE app_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
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

        CREATE INDEX idx_snapshots_drive_date ON daily_snapshots(drive_letter, snapshot_date);
        CREATE INDEX idx_events_drive_date ON write_events(drive_letter, event_date);
        CREATE INDEX idx_vault_files_path ON vault_files(original_path);
    """)

    # Seed data
    conn.execute("""
        INSERT INTO daily_snapshots (drive_letter, snapshot_date, total_bytes, used_bytes, free_bytes, net_change_bytes, updated_at)
        VALUES ('C:\\', '2026-11-15', 512000000000, 420000000000, 92000000000, 1500000000, '2026-11-15T23:59:59Z')
    """)
    conn.execute("""
        INSERT INTO app_settings (key, value) VALUES
        ('data_root', 'D:\\GreenC\\'),
        ('language', 'vi'),
        ('theme_mode', 'system'),
        ('retention_days_default', '7'),
        ('low_disk_shield_gb', '10')
    """)
    conn.execute("""
        INSERT INTO quota_rules (app_name, process_name, target_paths_json, max_bytes, watermark_ratio, retention_days, min_file_age_seconds, created_at)
        VALUES ('Zalo', 'Zalo.exe', '["C:\\\\Users\\\\Test\\\\AppData\\\\Roaming\\\\ZaloData"]', 5368709120, 0.85, 7, 60, '2026-11-01T10:00:00Z')
    """)
    conn.execute("""
        INSERT INTO relocations (app_id, app_name, strategy, source_path, target_path, size_bytes, status, process_names, created_at, detail)
        VALUES ('telegram', 'Telegram Desktop', 'junction', 'C:\\\\Users\\\\Test\\\\AppData\\\\Roaming\\\\Telegram Desktop', 'D:\\\\GreenC\\\\Apps\\\\Telegram Desktop', 2147483648, 'done', 'Telegram.exe', '2026-11-10T12:00:00Z', 'Active junction')
    """)
    conn.commit()
    conn.close()

    # 2. diskguard_v1.db (DiskGuard v0.1 legacy schema)
    dg_path = db_dir / "diskguard_v1.db"
    if dg_path.exists():
        dg_path.unlink()

    conn_dg = sqlite3.connect(str(dg_path))
    conn_dg.executescript("""
        CREATE TABLE settings (
            key TEXT PRIMARY KEY,
            val TEXT NOT NULL
        );
        CREATE TABLE history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts TEXT NOT NULL,
            used_gb REAL NOT NULL,
            free_gb REAL NOT NULL
        );
        CREATE TABLE rules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            limit_mb INTEGER NOT NULL
        );
    """)
    conn_dg.execute("INSERT INTO settings VALUES ('target_dir', 'D:\\DiskGuard\\')")
    conn_dg.execute("INSERT INTO rules VALUES (1, 'Zalo', 'C:\\Users\\Test\\AppData\\Local\\Zalo', 3000)")
    conn_dg.commit()
    conn_dg.close()


def generate_mock_tree():
    tree_dir = FIXTURES_ROOT / "mock_tree"
    tree_dir.mkdir(parents=True, exist_ok=True)

    # 1. Zalo Profile
    zalo_dir = tree_dir / "zalo_profile"
    zalo_dir.mkdir(parents=True, exist_ok=True)
    with open(zalo_dir / "config.json", "w", encoding="utf-8") as f:
        json.dump({"active_version": "2.1.0", "storage_drive": "C:"}, f, indent=2)

    (zalo_dir / "Zalo-2.0.0").mkdir(exist_ok=True)
    with open(zalo_dir / "Zalo-2.0.0" / "old_app.dll", "wb") as f:
        f.write(b"MOCK_OLD_DLL" * 1024)

    (zalo_dir / "Zalo-2.1.0").mkdir(exist_ok=True)
    with open(zalo_dir / "Zalo-2.1.0" / "active_app.dll", "wb") as f:
        f.write(b"MOCK_ACTIVE_DLL" * 1024)

    media_dir = zalo_dir / "fileNoise"
    media_dir.mkdir(exist_ok=True)
    sample_content = b"TEST_SHARED_MEDIA_CONTENT_FOR_HARDLINK_TEST" * 50
    with open(media_dir / "media_orig.dat", "wb") as f:
        f.write(sample_content)

    # Create hardlink if on Windows
    try:
        os.link(media_dir / "media_orig.dat", media_dir / "media_link1.dat")
    except Exception:
        # Fallback copy if filesystem doesn't support hardlinks in test environment
        with open(media_dir / "media_link1.dat", "wb") as f:
            f.write(sample_content)

    # 2. DingTalk Cache
    ding_dir = tree_dir / "dingtalk_profile" / "v2"
    ding_dir.mkdir(parents=True, exist_ok=True)
    (ding_dir / "EAppFiles").mkdir(exist_ok=True)
    with open(ding_dir / "EAppFiles" / "cached_miniapp.pkg", "wb") as f:
        f.write(b"MOCK_MINIAPP" * 2048)

    (ding_dir / "resource_cache").mkdir(exist_ok=True)
    with open(ding_dir / "resource_cache" / "thumb_01.jpg", "wb") as f:
        f.write(b"\xFF\xD8\xFF\xE0" + b"JPEG_MOCK_PAYLOAD" * 512)

    (ding_dir / "dumps").mkdir(exist_ok=True)
    with open(ding_dir / "dumps" / "crash_20261101.dmp", "wb") as f:
        f.write(b"MDMP" + b"\x00" * 4096)

    # 3. Dev Caches (npm, pip, cargo)
    dev_dir = tree_dir / "dev_caches"
    (dev_dir / "npm" / "_cacache" / "content-v2" / "sha512").mkdir(parents=True, exist_ok=True)
    with open(dev_dir / "npm" / "_cacache" / "content-v2" / "sha512" / "package.tgz", "wb") as f:
        f.write(b"\x1F\x8B\x08" + b"GZIP_MOCK_TARBALL" * 1024)

    (dev_dir / "pip" / "wheels").mkdir(parents=True, exist_ok=True)
    with open(dev_dir / "pip" / "wheels" / "sample-1.0.0-py3-none-any.whl", "wb") as f:
        f.write(b"PK\x03\x04" + b"WHEEL_MOCK" * 512)

    (dev_dir / "cargo" / "registry" / "cache").mkdir(parents=True, exist_ok=True)
    with open(dev_dir / "cargo" / "registry" / "cache" / "serde-1.0.crate", "wb") as f:
        f.write(b"\x1F\x8B\x08" + b"CRATE_MOCK" * 256)

    # 4. Downloads folder
    dl_dir = tree_dir / "downloads"
    dl_dir.mkdir(parents=True, exist_ok=True)
    with open(dl_dir / "setup_test.exe", "wb") as f:
        f.write(b"MZ" + b"\x00" * 8192)
    with open(dl_dir / "dataset.csv", "w", encoding="utf-8") as f:
        f.write("id,name,value\n" * 1000)

    # 5. Corrupt / boundary files
    corrupt_dir = tree_dir / "corrupt_data"
    corrupt_dir.mkdir(parents=True, exist_ok=True)
    # Zero byte
    with open(corrupt_dir / "zero_byte.tmp", "wb") as f:
        pass
    # Special unicode name
    with open(corrupt_dir / "tệp_tiếng_việt_🚀_#123.dat", "w", encoding="utf-8") as f:
        f.write("Unicode content")


def generate_corrupt_vault():
    vault_dir = FIXTURES_ROOT / "corrupt_vault"
    vault_dir.mkdir(parents=True, exist_ok=True)

    # 1. Truncated ZIP
    with open(vault_dir / "truncated.zip", "wb") as f:
        f.write(b"PK\x03\x04\x14\x00\x00\x00\x08\x00IncompleteStreamHeader")

    # 2. Broken CRC ZIP
    broken_crc_path = vault_dir / "broken_crc.zip"
    with zipfile.ZipFile(broken_crc_path, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("test_file.txt", "Original valid content before tampering")

    # Read binary, corrupt payload without updating CRC in header
    with open(broken_crc_path, "rb") as f:
        data = bytearray(f.read())
    # Modify data byte near middle
    if len(data) > 35:
        data[34] = (data[34] + 1) % 256
    with open(broken_crc_path, "wb") as f:
        f.write(data)

    # 3. Tampered Manifest ZIP
    valid_zip = vault_dir / "tampered_manifest.zip"
    test_content = b"Deterministic payload for checksum verification"
    real_sha256 = hashlib.sha256(test_content).hexdigest()
    fake_sha256 = "0" * 64

    with zipfile.ZipFile(valid_zip, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("item_00000_sample.bin", test_content)

    manifest_data = {
        "archive_id": "vault_20261116_test",
        "created_at": "2026-11-16T12:00:00Z",
        "files": [
            {
                "rel_arcname": "item_00000_sample.bin",
                "original_path": "C:\\Mock\\sample.bin",
                "size_bytes": len(test_content),
                "sha256": fake_sha256,  # Tampered hash!
                "mtime": 1731758400.0,
                "hardlink_paths": []
            }
        ]
    }
    with open(vault_dir / "tampered_manifest.manifest.json", "w", encoding="utf-8") as f:
        json.dump(manifest_data, f, indent=2)


def generate_locales():
    loc_dir = FIXTURES_ROOT / "locales"
    loc_dir.mkdir(parents=True, exist_ok=True)

    # Sample valid catalog
    valid_cat = {
        "app_name": "GreenC",
        "scan_title": "Quét ổ đĩa: {drive}",
        "days_left": "Còn khoảng {days} ngày",
        "size_reclaimed": "Đã giải phóng {size}",
        "status_quarantined": "Đang theo dõi"
    }
    with open(loc_dir / "test_catalog.json", "w", encoding="utf-8") as f:
        json.dump(valid_cat, f, indent=2, ensure_ascii=False)

    # RTL locale sample
    rtl_cat = {
        "app_name": "GreenC",
        "scan_title": "مسح محرك الأقراص: {drive}",
        "days_left": "متبقي {days} يوم تقريبًا",
        "is_rtl": True
    }
    with open(loc_dir / "rtl_sample.json", "w", encoding="utf-8") as f:
        json.dump(rtl_cat, f, indent=2, ensure_ascii=False)

    # Corrupt token locale
    corrupt_cat = {
        "app_name": "GreenC",
        "scan_title": "Quét ổ đĩa: {unclosed_token",
        "corrupt_bracket": "Lỗi {}{}{"
    }
    with open(loc_dir / "corrupted_token.json", "w", encoding="utf-8") as f:
        json.dump(corrupt_cat, f, indent=2, ensure_ascii=False)


def generate_moving_state():
    mov_dir = FIXTURES_ROOT / "moving_state"
    mov_dir.mkdir(parents=True, exist_ok=True)

    src_dir = mov_dir / "app_source"
    src_dir.mkdir(exist_ok=True)
    with open(src_dir / "data.db", "wb") as f:
        f.write(b"SOURCE_DATA" * 512)

    moving_dir = mov_dir / "app_source.greenc-moving"
    moving_dir.mkdir(exist_ok=True)
    with open(moving_dir / "data.db", "wb") as f:
        f.write(b"MOVING_STAGE_DATA" * 512)

    tgt_dir = mov_dir / "target_incomplete"
    tgt_dir.mkdir(exist_ok=True)
    with open(tgt_dir / "data.db.part", "wb") as f:
        f.write(b"INCOMPLETE_COPY" * 128)


if __name__ == "__main__":
    print(f"Generating synthetic test fixtures in: {FIXTURES_ROOT}")
    generate_legacy_db()
    generate_mock_tree()
    generate_corrupt_vault()
    generate_locales()
    generate_moving_state()
    print("Fixtures generation completed successfully.")
