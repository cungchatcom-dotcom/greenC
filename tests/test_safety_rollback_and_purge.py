from datetime import datetime, timedelta
import hashlib
import os
from pathlib import Path
import time

from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB


def _sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def test_auto_rollback_on_missing_file_error_and_hardlink_restore(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "diskguard.db")
    vault_dir = tmp_path / "vault"
    app_dir = tmp_path / "ZaloMedia"
    (app_dir / "fileNoise").mkdir(parents=True)
    (app_dir / "resource" / "chat_1" / "fileNoise").mkdir(parents=True)

    now = time.time()
    hl_primary = app_dir / "fileNoise" / "0010aa51cfe34b97"
    hl_sibling = app_dir / "resource" / "chat_1" / "fileNoise" / "1793536_chat_1"
    content = b"ZALO_IMPORTANT_MEDIA_PAYLOAD_" * 40_000  # ~1.16 MB
    hl_primary.write_bytes(content)
    orig_mtime = now - 86400 * 4
    os.utime(hl_primary, (orig_mtime, orig_mtime))
    os.link(hl_primary, hl_sibling)
    expected_hash = _sha256(hl_primary)

    keep_file = app_dir / "recent.dat"
    keep_file.write_bytes(b"R" * (1024 * 1024))
    os.utime(keep_file, (now - 120, now - 120))

    rule = QuotaRule(
        id=None,
        app_name="Zalo Media Test",
        process_name="Zalo.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=int(1.5 * 1024 * 1024),
        watermark_ratio=0.80,
        retention_days=7,
        min_file_age_seconds=60,
        exclude_patterns=["*.log"],
        protected_files=[],
        enabled=True,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, vault_dir)
    archive = enforcer.enforce_rule(rule, now_ts=now)
    assert archive is not None
    assert not hl_primary.exists()
    assert not hl_sibling.exists()

    # Simulate application logging a missing-file access error referencing the archived file!
    err_log = app_dir / "error.log"
    err_log.write_text(
        f"[2026-11-09 14:00:00] ERROR: FileNotFoundError: Cannot open {hl_sibling.name} (ENOENT)\n",
        encoding="utf-8",
    )

    watchdog = SafetyWatchdog(db)
    cycle_result = watchdog.run_safety_cycle()
    assert archive.id in cycle_result["auto_rolled_back"]

    # Both hardlink paths must be restored with identical SHA-256 and shared NTFS inode!
    assert hl_primary.exists()
    assert hl_sibling.exists()
    assert _sha256(hl_primary) == expected_hash
    assert _sha256(hl_sibling) == expected_hash
    assert os.stat(hl_primary).st_ino == os.stat(hl_sibling).st_ino

    # Protected list must now contain the restored paths so they are NEVER zipped again
    updated_rule = db.get_quota_rule(rule.id)
    assert updated_rule is not None
    assert str(hl_primary) in updated_rule.protected_files

    second_try = enforcer.enforce_rule(updated_rule, now_ts=now)
    assert hl_primary.exists() and hl_sibling.exists()


def test_auto_purge_after_d_days_when_safe(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "diskguard.db")
    vault_dir = tmp_path / "vault"
    app_dir = tmp_path / "DingTalkApp"
    app_dir.mkdir(parents=True)

    now = time.time()
    old_bundle = app_dir / "bundle_old.amr"
    old_bundle.write_bytes(b"A" * (2 * 1024 * 1024))
    os.utime(old_bundle, (now - 86400 * 10, now - 86400 * 10))

    rule = QuotaRule(
        id=None,
        app_name="DingTalk Bundles",
        process_name="DingTalk.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=1 * 1024 * 1024,
        watermark_ratio=0.80,
        retention_days=5,
        min_file_age_seconds=60,
        exclude_patterns=[],
        protected_files=[],
        enabled=True,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, vault_dir)
    archive = enforcer.enforce_rule(rule, now_ts=now)
    assert archive is not None
    assert Path(archive.zip_path).exists()

    watchdog = SafetyWatchdog(db)
    # After 3 days (< 5 days retention): should NOT be purged yet
    res_day3 = watchdog.run_safety_cycle(now_dt=datetime.now() + timedelta(days=3))
    assert archive.id not in res_day3["purged"]
    assert Path(archive.zip_path).exists()

    # After 6 days (> 5 days retention) with no errors: should be automatically purged!
    res_day6 = watchdog.run_safety_cycle(now_dt=datetime.now() + timedelta(days=6))
    assert archive.id in res_day6["purged"]
    assert not Path(archive.zip_path).exists()
    arch_loaded = db.get_vault_archive(archive.id)
    assert arch_loaded is not None
    assert arch_loaded.status == "PURGED"

