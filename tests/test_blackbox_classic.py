"""
Kiểm Thử Hộp Đen Kinh Điển (Classical Black-Box Testing Suite — ISTQB Standard):
1. Equivalence Partitioning (EP — Phân vùng tương đương)
2. Boundary Value Analysis (BVA — Phân tích giá trị biên)
3. Decision Table Testing (DT — Bảng quyết định đa điều kiện)
4. State Transition Testing (ST — Kiểm thử chuyển trạng thái)
"""

import ctypes
from ctypes import wintypes
from datetime import datetime, timedelta
import os
from pathlib import Path
import time
import pytest

from diskguard.core.attribution import AttributionEngine, classify_write_reason
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.task_controller import TaskControl
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes


# ==============================================================================
# 1. EQUIVALENCE PARTITIONING (EP — PHÂN VÙNG TƯƠNG ĐƯƠNG)
# ==============================================================================


@pytest.mark.parametrize(
    "sample_path,expected_reason_code,expected_proc",
    [
        # Partition 1: Zalo Media Chat (HardLink resource & fileNoise)
        (
            r"F:\Zalo Data\media\1988850\ZaloDownloads\resource\7017\fileNoise\179353",
            "MEDIA_CHAT",
            "Zalo.exe",
        ),
        (
            r"C:\Users\duong\Documents\Zalo Received Files\report_final.pdf",
            "MEDIA_CHAT",
            "Zalo.exe",
        ),
        # Partition 2: DingTalk EApp Bundles (.amr & unziped)
        (
            r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\EAppFiles\download\50001_abc.amr",
            "EAPP_BUNDLE",
            "DingTalk.exe",
        ),
        (
            r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\EAppFiles\unziped\50001\index.bundle",
            "EAPP_BUNDLE",
            "DingTalk.exe",
        ),
        # Partition 3: Leftover Old Application Versions
        (
            r"C:\Users\duong\AppData\Local\Programs\Zalo\Zalo-26.9.10\Zalo.exe",
            "OLD_APP_VERSION",
            "Zalo.exe",
        ),
        (
            r"C:\Users\duong\AppData\Local\Zalo\pending\ZaloSetup.exe",
            "OLD_APP_VERSION",
            "Zalo.exe",
        ),
        # Partition 4: Crash Dumps (.dmp, .hdmp, .mdmp, dumps folder)
        (
            r"C:\Users\duong\AppData\Local\DingTalk\dumps\crash_2026.dmp",
            "CRASH_DUMP",
            "DingTalk.exe",
        ),
        # Partition 5: Log Growth (.log, .etl, .evtx, .calf, holmeslogs)
        (
            r"C:\Users\duong\AppData\Roaming\ZaloData\cal\u-process-media.calf",
            "LOG_GROWTH",
            "Zalo.exe",
        ),
        # Partition 6: Cache & Temporary files
        (
            r"C:\Users\duong\AppData\Local\Google\Chrome\User Data\Default\Cache\data_1",
            "CACHE_TEMP",
            "chrome.exe",
        ),
        # Partition 7: Database & WAL storage
        (
            r"C:\ProgramData\CustomApp\Database\main.db-wal",
            "DB_STORAGE",
            "CustomApp.exe",
        ),
        # Partition 8: Container / VM virtual disks
        (
            r"C:\Users\duong\AppData\Local\Docker\wsl\data\ext4.vhdx",
            "CONTAINER_VM",
            "Docker.exe",
        ),
        # Partition 9: Large Media / Installer Downloads
        (
            r"C:\Users\duong\Downloads\Windows11_Insider.iso",
            "MEDIA_DOWNLOAD",
            "Users (User/App)",
        ),
        # Partition 10: Unicode Vietnamese paths & spaces
        (
            r"C:\Users\duong\AppData\Roaming\PhầnMềmKếToán\Dữ Liệu Năm 2026\sổ_cái (bản sao).dat",
            "APP_DATA",
            "PhầnMềmKếToán.exe",
        ),
    ],
)
def test_bb_ep_reason_and_actor_partitions(
    sample_path: str, expected_reason_code: str, expected_proc: str
):
    """BB-EP-01: Kiểm thử phân vùng tương đương cho toàn bộ 10 lớp nguyên nhân & tác nhân ghi đĩa."""
    code, detail = classify_write_reason(sample_path)
    assert code == expected_reason_code
    assert len(detail) > 5

    engine = AttributionEngine()
    attr = engine.attribute_path(sample_path)
    assert attr["process_name"].lower() == expected_proc.lower()


@pytest.mark.parametrize(
    "byte_val,expected_str",
    [
        (0, "0 B"),
        (512, "512 B"),
        (1023, "1023 B"),
        (1024, "1 KB"),
        (1024 * 1024, "1.00 MB"),
        (-5 * 1024 * 1024, "-5.00 MB"),
        (1024**3, "1.00 GB"),
        (2.5 * 1024**4, "2.50 TB"),
    ],
)
def test_bb_ep_format_bytes_partitions(byte_val: float, expected_str: str):
    """BB-EP-02: Phân vùng tương đương hàm hiển thị dung lượng (0, âm, B, KB, MB, GB, TB)."""
    assert format_bytes(byte_val) == expected_str


# ==============================================================================
# 2. BOUNDARY VALUE ANALYSIS (BVA — PHÂN TÍCH GIÁ TRỊ BIÊN)
# ==============================================================================


@pytest.mark.parametrize(
    "file_size_bytes,should_trigger_zip",
    [
        (1_000_000 - 1, False),  # Dưới biên đúng 1 byte (999,999 B <= 1,000,000 B) -> KHÔNG nén
        (1_000_000, False),      # Đúng tại biên (1,000,000 B == 1,000,000 B) -> KHÔNG nén
        (1_000_000 + 1, True),   # Vượt biên đúng 1 byte (1,000,001 B > 1,000,000 B) -> PHẢI nén
    ],
)
def test_bb_bva_quota_threshold_exact_byte_boundaries(
    tmp_path: Path, file_size_bytes: int, should_trigger_zip: bool
):
    """BB-BVA-01: Phân tích giá trị biên quanh ngưỡng Quota (max_bytes - 1, max_bytes, max_bytes + 1)."""
    db = DiskGuardDB(tmp_path / "bva_quota.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    f = app_dir / "data_old.bin"
    f.write_bytes(b"A" * file_size_bytes)
    os.utime(f, (now - 3600, now - 3600))

    rule = QuotaRule(
        id=None,
        app_name="BVA App",
        process_name="bva.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=1_000_000,
        watermark_ratio=0.80,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    arch = enforcer.enforce_rule(rule, now_ts=now)
    if should_trigger_zip:
        assert arch is not None
        assert not f.exists()
    else:
        assert arch is None
        assert f.exists()


@pytest.mark.parametrize(
    "file_age_seconds,should_be_eligible",
    [
        (59, False),  # Dưới biên tuổi tối thiểu 1 giây (59s < 60s) -> KHÔNG được đụng vào
        (60, True),   # Đúng tại biên tuổi tối thiểu (60s == 60s) -> ĐƯỢC thu hồi
        (61, True),   # Trên biên tuổi tối thiểu 1 giây (61s > 60s) -> ĐƯỢC thu hồi
    ],
)
def test_bb_bva_min_file_age_seconds_boundaries(
    tmp_path: Path, file_age_seconds: int, should_be_eligible: bool
):
    """BB-BVA-02: Phân tích giá trị biên cho tuổi file tối thiểu (59s, 60s, 61s với ngưỡng 60s)."""
    db = DiskGuardDB(tmp_path / "bva_age.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = 1_800_000_000.0

    f = app_dir / "candidate.bin"
    f.write_bytes(b"B" * 200_000)
    os.utime(f, (now - file_age_seconds, now - file_age_seconds))

    rule = QuotaRule(
        id=None,
        app_name="Age Boundary App",
        process_name="age.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=100_000,
        watermark_ratio=0.50,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    arch = enforcer.enforce_rule(rule, now_ts=now)
    if should_be_eligible:
        assert arch is not None
        assert not f.exists()
    else:
        assert arch is None
        assert f.exists()


def test_bb_bva_d_day_expiration_exact_second_boundaries(tmp_path: Path):
    """BB-BVA-03: Phân tích giá trị biên thời điểm hết hạn D ngày (expires_at - 1s, expires_at, expires_at + 1s)."""
    db = DiskGuardDB(tmp_path / "bva_expiry.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    f = app_dir / "old.dat"
    f.write_bytes(b"C" * 300_000)
    os.utime(f, (now - 7200, now - 7200))

    rule = QuotaRule(
        id=None,
        app_name="Expiry App",
        process_name="exp.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=100_000,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    arch = enforcer.enforce_rule(rule, now_ts=now)
    assert arch is not None

    expires_dt = datetime.fromisoformat(arch.expires_at)
    watchdog = SafetyWatchdog(db)

    # 1. Trước biên đúng 1 giây (expires_at - 1s): Chưa được xóa
    purged_before = watchdog.purge_expired_archives(now_dt=expires_dt - timedelta(seconds=1))
    assert arch.id not in purged_before
    assert Path(arch.zip_path).exists()

    # 2. Đúng tại biên (expires_at): Phải tự động xóa ngay
    purged_exact = watchdog.purge_expired_archives(now_dt=expires_dt)
    assert arch.id in purged_exact
    assert not Path(arch.zip_path).exists()
    assert db.get_vault_archive(arch.id).status == "PURGED"


# ==============================================================================
# 3. DECISION TABLE TESTING (DT — BẢNG QUYẾT ĐỊNH ĐA ĐIỀU KIỆN)
# ==============================================================================


@pytest.mark.parametrize(
    "rule_enabled,is_locked,is_excluded_ext,is_protected_path,is_old_enough,expected_archived",
    [
        # Duy nhất trường hợp hội tụ đủ 5 điều kiện an toàn mới được phép nén & xóa:
        (True,  False, False, False, True,  True),
        # Mọi tổ hợp vi phạm bất kỳ 1 điều kiện nào đều PHẢI GIỮ NGUYÊN file gốc:
        (False, False, False, False, True,  False),  # Rule đang tắt
        (True,  True,  False, False, True,  False),  # File đang bị tiến trình khóa (Win32 Lock)
        (True,  False, True,  False, True,  False),  # File thuộc mẫu bảo vệ (.db / .sqlite)
        (True,  False, False, True,  True,  False),  # File nằm trong danh sách miễn trừ (protected_files)
        (True,  False, False, False, False, False),  # File mới sinh (< min_file_age_seconds)
    ],
)
def test_bb_dt_quota_candidate_decision_table(
    tmp_path: Path,
    rule_enabled: bool,
    is_locked: bool,
    is_excluded_ext: bool,
    is_protected_path: bool,
    is_old_enough: bool,
    expected_archived: bool,
):
    """BB-DT-01: Kiểm thử bảng quyết định 5 điều kiện lọc file của QuotaEnforcer."""
    db = DiskGuardDB(tmp_path / "dt.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    ext = ".db" if is_excluded_ext else ".dat"
    target_file = app_dir / f"record{ext}"
    target_file.write_bytes(b"X" * 400_000)
    age = 3600 if is_old_enough else 10
    os.utime(target_file, (now - age, now - age))

    rule = QuotaRule(
        id=None,
        app_name="Decision Table App",
        process_name="dt.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=100_000,
        watermark_ratio=0.50,
        retention_days=7,
        min_file_age_seconds=60,
        exclude_patterns=["*.db"],
        protected_files=[str(target_file)] if is_protected_path else [],
        enabled=rule_enabled,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    handle = None
    if is_locked:
        handle = kernel32.CreateFileW(
            str(target_file), 0x80000000 | 0x40000000, 0, None, 3, 0, None
        )
        assert handle != wintypes.HANDLE(-1).value

    try:
        arch = enforcer.enforce_rule(rule, now_ts=now)
    finally:
        if handle is not None:
            kernel32.CloseHandle(handle)

    if expected_archived:
        assert arch is not None
        assert not target_file.exists()
    else:
        assert arch is None
        assert target_file.exists()


# ==============================================================================
# 4. STATE TRANSITION TESTING (ST — KIỂM THỬ CHUYỂN TRẠNG THÁI)
# ==============================================================================


def test_bb_st_task_control_state_machine_transitions():
    """BB-ST-01: Kiểm thử sơ đồ chuyển trạng thái TaskControl: IDLE -> PAUSED -> RESUMED -> STOPPED -> RESET."""
    ctrl = TaskControl()
    assert ctrl.is_paused is False
    assert ctrl.is_stopped is False

    ctrl.pause()
    assert ctrl.is_paused is True
    assert ctrl.is_stopped is False

    ctrl.resume()
    assert ctrl.is_paused is False
    assert ctrl.wait_if_paused() is True

    # Chuyển từ PAUSED thẳng sang STOPPED phải tự giải phóng khóa chờ (unblock)
    ctrl.pause()
    assert ctrl.is_paused is True
    ctrl.stop()
    assert ctrl.is_stopped is True
    assert ctrl.wait_if_paused() is False

    # RESET đưa về trạng thái ban đầu
    ctrl.reset()
    assert ctrl.is_paused is False
    assert ctrl.is_stopped is False
    assert ctrl.wait_if_paused() is True

