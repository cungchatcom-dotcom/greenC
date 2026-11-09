import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import time

from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.win_locks import compute_physical_usage
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB


def test_quota_enforcer_skips_locked_and_protected_and_zips_hardlink_groups(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "diskguard.db")
    vault_dir = tmp_path / "vault"
    app_dir = tmp_path / "ZaloMedia"
    (app_dir / "fileNoise").mkdir(parents=True)
    (app_dir / "resource" / "chat_99" / "fileNoise").mkdir(parents=True)

    now = time.time()

    # 1) Active locked DB/config file (should NEVER be touched)
    db_file = app_dir / "Media.db"
    db_file.write_bytes(b"D" * (512 * 1024))
    os.utime(db_file, (now - 86400 * 10, now - 86400 * 10))

    # 2) Locked log file held open with Win32 exclusive lock (should NEVER be touched)
    active_log = app_dir / "active_stream.bin"
    active_log.write_bytes(b"L" * (1024 * 1024))  # 1 MB
    os.utime(active_log, (now - 86400 * 9, now - 86400 * 9))

    # 3) Oldest HardLink pair (2 MB physical, 4 MB naive) -> should be zipped & both links removed!
    hl_a = app_dir / "fileNoise" / "md5_oldest"
    hl_b = app_dir / "resource" / "chat_99" / "fileNoise" / "msg_oldest"
    hl_a.write_bytes(b"O" * (2 * 1024 * 1024))
    os.utime(hl_a, (now - 86400 * 5, now - 86400 * 5))
    os.link(hl_a, hl_b)

    # 4) Newer unlocked file (1.5 MB physical)
    newer_media = app_dir / "resource" / "chat_99" / "video_recent.mp4"
    newer_media.write_bytes(b"N" * int(1.5 * 1024 * 1024))
    os.utime(newer_media, (now - 3600, now - 3600))

    # Physical total = 0.5 MB (db) + 1.0 MB (locked) + 2.0 MB (hardlink pair) + 1.5 MB (newer) = 5.0 MB
    phys_before, naive_before, _ = compute_physical_usage([app_dir])
    assert phys_before == 5 * 1024 * 1024
    assert naive_before == 7 * 1024 * 1024  # Because hardlink is counted twice naively

    # Set Quota max_bytes = 4.0 MB, watermark = 0.80 -> target = 3.2 MB (need to free >= 1.8 MB)
    rule = QuotaRule(
        id=None,
        app_name="Zalo Test",
        process_name="Zalo.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=4 * 1024 * 1024,
        watermark_ratio=0.80,
        retention_days=7,
        min_file_age_seconds=60,
        exclude_patterns=["*.db"],
        protected_files=[],
        enabled=True,
    )
    rule.id = db.save_quota_rule(rule)

    # Hold exclusive lock on active_log during enforcement
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    h = kernel32.CreateFileW(str(active_log), 0x80000000 | 0x40000000, 0, None, 3, 0, None)
    assert h != wintypes.HANDLE(-1).value
    try:
        enforcer = QuotaEnforcer(db, vault_dir)
        archive = enforcer.enforce_rule(rule, now_ts=now)
    finally:
        kernel32.CloseHandle(h)

    assert archive is not None
    assert Path(archive.zip_path).exists()
    assert Path(archive.manifest_path).exists()

    # Verify locked file & .db file & newer file remain untouched
    assert db_file.exists()
    assert active_log.exists()
    assert newer_media.exists()

    # Verify BOTH hardlink paths of the oldest file were removed so physical space dropped by 2 MB!
    assert not hl_a.exists()
    assert not hl_b.exists()

    phys_after, _, _ = compute_physical_usage([app_dir])
    assert phys_after == 3 * 1024 * 1024  # 5 MB - 2 MB = 3 MB <= 3.2 MB target!
