import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path

from diskguard.core.win_locks import is_file_locked, compute_physical_usage
from diskguard.core.attribution import classify_write_reason, AttributionEngine
from diskguard.core.app_profiles import discover_zalo_profile_from_config, discover_dingtalk_profile_from_dirs


def test_win32_is_file_locked(tmp_path: Path):
    target = tmp_path / "active.db"
    target.write_bytes(b"sqlite data")

    assert is_file_locked(target) is False

    # Open with Win32 exclusive lock (dwShareMode=0) like SQLite WAL/DB or Zalo lockfile
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    GENERIC_READ = 0x80000000
    GENERIC_WRITE = 0x40000000
    OPEN_EXISTING = 3
    h = kernel32.CreateFileW(
        str(target),
        GENERIC_READ | GENERIC_WRITE,
        0,  # No share
        None,
        OPEN_EXISTING,
        0,
        None,
    )
    assert h != wintypes.HANDLE(-1).value
    try:
        assert is_file_locked(target) is True
    finally:
        kernel32.CloseHandle(h)

    assert is_file_locked(target) is False


def test_ntfs_hardlink_physical_usage_deduplication(tmp_path: Path):
    f1 = tmp_path / "fileNoise" / "hash_abc"
    f2 = tmp_path / "resource" / "chat_1" / "fileNoise" / "msg_1"
    f1.parent.mkdir(parents=True)
    f2.parent.mkdir(parents=True)

    payload = b"X" * (2 * 1024 * 1024)  # 2 MB
    f1.write_bytes(payload)
    os.link(f1, f2)  # Create NTFS hardlink

    phys_bytes, naive_bytes, inode_map = compute_physical_usage([tmp_path])
    assert naive_bytes == 4 * 1024 * 1024
    assert phys_bytes == 2 * 1024 * 1024
    assert len(inode_map) == 1
    only_group = next(iter(inode_map.values()))
    assert len(only_group) == 2


def test_classify_write_reason_and_attribution():
    code, detail = classify_write_reason(r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\EAppFiles\download\app1.amr")
    assert code == "EAPP_BUNDLE"
    assert "DingTalk" in detail or "Mini-App" in detail

    code2, detail2 = classify_write_reason(r"F:\Zalo Data\media\1988\ZaloDownloads\resource\123\fileNoise\456")
    assert code2 == "MEDIA_CHAT"

    code3, _ = classify_write_reason(r"C:\Users\duong\AppData\Local\DingTalk\dumps\crash.dmp")
    assert code3 == "CRASH_DUMP"

    code4, _ = classify_write_reason(r"C:\ProgramData\MyService\logs\service.log")
    assert code4 == "LOG_GROWTH"

    engine = AttributionEngine()
    actor = engine.attribute_path(r"F:\Zalo Data\media\1988\ZaloDownloads\fileNoise\abc")
    assert actor["process_name"] == "Zalo.exe"

    dt_actor = engine.attribute_path(r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\resource_cache\03\img.jpg")
    assert dt_actor["process_name"] == "DingTalk.exe"


def test_zalo_and_dingtalk_smart_profile_discovery(tmp_path: Path):
    fake_roaming = tmp_path / "Roaming" / "ZaloData"
    fake_roaming.mkdir(parents=True)
    fake_f_zalo = tmp_path / "F_Drive" / "Zalo Data"
    (fake_f_zalo / "media").mkdir(parents=True)
    (fake_f_zalo / "message" / "Database").mkdir(parents=True)

    cfg = {
        "resource": {"location": {"path": str(tmp_path / "F_Drive")}},
        "db_path": {"location": str(fake_f_zalo / "message")},
    }
    cfg_file = fake_roaming / "config.json"
    cfg_file.write_text(json.dumps(cfg), encoding="utf-8")

    profile = discover_zalo_profile_from_config(cfg_file, local_appdata=tmp_path / "Local")
    assert profile is not None
    assert any("media" in p for p in profile.reclaimable_paths)
    assert any("Database" in p or "*.db" in p for p in profile.exclude_patterns)

    # DingTalk profile discovery
    dt_roaming = tmp_path / "Roaming" / "DingTalk"
    (dt_roaming / "40012285383_v2" / "EAppFiles" / "download").mkdir(parents=True)
    (dt_roaming / "40012285383_v2" / "resource_cache").mkdir(parents=True)
    dt_local = tmp_path / "Local" / "DingTalk"
    (dt_local / "dumps").mkdir(parents=True)

    dt_profile = discover_dingtalk_profile_from_dirs(dt_roaming, dt_local)
    assert dt_profile is not None
    assert len(dt_profile.reclaimable_paths) >= 3
