"""
Kiểm Thử Hộp Trắng Chuyên Sâu (White-Box Internal & Structural Testing Suite):
- WB-01: Branch & Path Coverage cho xung đột tên file khi Rollback (0-byte vs non-empty file mới).
- WB-02: Fault Injection — Phát hiện giả mạo dữ liệu nén (SHA-256 mismatch) & xử lý ngoại lệ thiếu gói Zip.
- WB-03: N-Way NTFS HardLink Group Partial Protection — 1 nhánh trong nhóm 3 HardLink nằm trong danh sách bảo vệ.
- WB-04: SmartAppAnalyzer Fallback Branch — Nhánh tự động rút lui về thư mục con Cache/Temp khi thư mục gốc gặp lỗi probe.
- WB-05: Multi-Threaded SQLite WAL Concurrency — 8 luồng đọc/ghi đồng thời không xảy ra Deadlock hay Database Locked.
- WB-06: DiskTracker Deep Path Summarization — Kiểm thử hàm rút gọn đường dẫn sâu (_summarize_folder_path).
"""

from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import time
import zipfile
import pytest

from greenc.core.disk_tracker import DiskTracker
from greenc.core.quota_enforcer import QuotaEnforcer
from greenc.core.safety_watchdog import SafetyWatchdog
from greenc.core.smart_analyzer import SmartAppAnalyzer
from greenc.models import QuotaRule, WriteEvent
from greenc.storage.database import GreenCDB


def test_wb01_rollback_branch_coverage_on_existing_files(tmp_path: Path):
    """
    WB-01: Kiểm tra cả 2 nhánh rẽ trong SafetyWatchdog.rollback_archive khi tại đường dẫn gốc
    đã xuất hiện file mới trùng tên:
    - Nhánh 1: File mới có kích thước > 0 byte -> Phải giữ nguyên file mới và khôi phục bản cũ ra `.restored_<timestamp>`.
    - Nhánh 2: File mới có kích thước == 0 byte (placeholder trống) -> Ghi đè khôi phục trực tiếp bản gốc.
    """
    db = GreenCDB(tmp_path / "wb01.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    f1 = app_dir / "conflict_nonempty.dat"
    f2 = app_dir / "conflict_zero.dat"
    f1.write_bytes(b"ORIGINAL_DATA_1_" * 20_000)
    f2.write_bytes(b"ORIGINAL_DATA_2_" * 20_000)
    os.utime(f1, (now - 3600, now - 3600))
    os.utime(f2, (now - 3600, now - 3600))

    rule = QuotaRule(
        id=None,
        app_name="Conflict Branch App",
        process_name="app.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=100_000,
        watermark_ratio=0.10,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    arch = enforcer.enforce_rule(rule, now_ts=now)
    assert arch is not None
    assert not f1.exists() and not f2.exists()

    # Trước khi Rollback, ứng dụng tự tạo ra f1 mới (có dữ liệu mới) và f2 mới (0 byte rỗng)
    f1.write_bytes(b"NEW_USER_DATA_MUST_NOT_BE_LOST")
    f2.write_bytes(b"")

    watchdog = SafetyWatchdog(db)
    res = watchdog.rollback_archive(arch.id, reason="Test collision branches", auto_triggered=False)
    assert res["restored_count"] == 2

    # Kiểm tra Nhánh 1: f1 vẫn giữ dữ liệu mới, bản cũ được khôi phục ra file .restored_*
    assert f1.read_bytes() == b"NEW_USER_DATA_MUST_NOT_BE_LOST"
    restored_variants = list(app_dir.glob("conflict_nonempty.dat.restored_*"))
    assert len(restored_variants) == 1
    assert restored_variants[0].read_bytes() == b"ORIGINAL_DATA_1_" * 20_000

    # Kiểm tra Nhánh 2: f2 (0 byte) được ghi đè khôi phục trực tiếp dữ liệu gốc
    assert f2.read_bytes() == b"ORIGINAL_DATA_2_" * 20_000


def test_wb02_fault_injection_sha256_tamper_and_missing_archive_detection(tmp_path: Path):
    """
    WB-02: Fault Injection — Giả lập gói .zip trong Vault bị sửa đổi nội dung bên trong
    làm lệch mã băm SHA-256. Hệ thống phải phát hiện và tung RuntimeError('SHA-256 mismatch').
    """
    db = GreenCDB(tmp_path / "wb02.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    f = app_dir / "critical.bin"
    f.write_bytes(b"AUTHENTIC_CONTENT_" * 20_000)
    os.utime(f, (now - 3600, now - 3600))

    rule = QuotaRule(
        id=None,
        app_name="Tamper Check App",
        process_name="tamper.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=50_000,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    arch = enforcer.enforce_rule(rule, now_ts=now)
    assert arch is not None

    # Giả lập kẻ tấn công hoặc lỗi đĩa làm thay đổi 1 byte nội dung file bên trong gói .zip
    vf = db.get_vault_files(arch.id)[0]
    with zipfile.ZipFile(arch.zip_path, mode="w", compression=zipfile.ZIP_DEFLATED) as zf_tamper:
        zf_tamper.writestr(vf.rel_arcname, b"CORRUPTED_CONTENT_" * 20_000)

    watchdog = SafetyWatchdog(db)
    with pytest.raises(RuntimeError, match="SHA-256 mismatch"):
        watchdog.rollback_archive(arch.id, reason="Test tamper detection")

    # Kiểm tra nhánh ngoại lệ khi gói .zip bị xóa mất khỏi đĩa
    Path(arch.zip_path).unlink(missing_ok=True)
    with pytest.raises(FileNotFoundError):
        watchdog.rollback_archive(arch.id, reason="Test missing zip")


def test_wb03_three_way_hardlink_group_atomic_skip_when_one_sibling_protected(tmp_path: Path):
    """
    WB-03: Kiểm tra cấu trúc Inode Group khi 1 file vật lý có 3 nhánh HardLink (nlink=3):
    Nếu chỉ 1 trong 3 nhánh nằm trong `protected_files`, cả 3 nhánh của cùng Inode đó
    phải được bảo vệ nguyên vẹn (không được xóa 2 nhánh kia).
    """
    db = GreenCDB(tmp_path / "wb03.db")
    app_dir = tmp_path / "app"
    (app_dir / "dirA").mkdir(parents=True)
    (app_dir / "dirB").mkdir(parents=True)
    (app_dir / "dirC").mkdir(parents=True)
    now = time.time()

    p_a = app_dir / "dirA" / "link1.bin"
    p_b = app_dir / "dirB" / "link2.bin"
    p_c = app_dir / "dirC" / "link3_protected.bin"
    p_a.write_bytes(b"SHARED_INODE_DATA_" * 30_000)
    os.utime(p_a, (now - 86400 * 5, now - 86400 * 5))
    os.link(p_a, p_b)
    os.link(p_a, p_c)

    # Chỉ đưa nhánh thứ 3 (p_c) vào protected_files
    rule = QuotaRule(
        id=None,
        app_name="3-Way HardLink App",
        process_name="hl3.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=100_000,
        retention_days=7,
        min_file_age_seconds=60,
        protected_files=[str(p_c)],
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    arch = enforcer.enforce_rule(rule, now_ts=now)

    # Toàn bộ nhóm 3 nhánh phải được bỏ qua an toàn
    assert arch is None
    assert p_a.exists() and p_b.exists() and p_c.exists()


def test_wb04_smart_analyzer_fallback_to_cache_subdirs_on_probe_failure(tmp_path: Path):
    """
    WB-04: Kiểm tra nhánh `except Exception:` trong SmartAppAnalyzer._run_single_cycle_sandbox_probe:
    Khi mẫu canary ở thư mục gốc gặp lỗi trong lúc thử nghiệm, hệ thống tự động tìm các thư mục con
    an toàn (`Cache`, `Temp`, `logs`, `dumps`) và chuyển sang 'Phương án dự phòng (Chỉ dọn vùng Cache/Temp)'.
    """
    db = GreenCDB(tmp_path / "wb04.db")
    app_dir = tmp_path / "SensitiveApp"
    (app_dir / "Cache").mkdir(parents=True)
    (app_dir / "logs").mkdir(parents=True)

    # Tạo 1 đường dẫn canary giả (không tồn tại trên đĩa) để kích hoạt nhánh except trong _run_single_cycle_sandbox_probe
    ghost_canary = app_dir / "ghost_locked.bin"
    analyzer = SmartAppAnalyzer(db=db, vault_dir=tmp_path / "vault", probe_dir=tmp_path / "probe")

    ok, strategy, detail, effective_dirs = analyzer._run_single_cycle_sandbox_probe(
        app_name="SensitiveApp",
        target_dirs=[str(app_dir)],
        unlocked_groups=[(time.time() - 3600, 1024 * 1024, [ghost_canary])],
        locked_count=5,
    )
    assert ok is True
    assert strategy == "Chỉ dọn bộ đệm"
    assert any("Cache" in d or "logs" in d for d in effective_dirs)


def test_wb05_sqlite_wal_multithreaded_concurrency_stress(tmp_path: Path):
    """
    WB-05: Chạy đồng thời 8 luồng (threads) thực hiện 160 thao tác đọc/ghi song song
    vào SQLite WAL database để kiểm chứng khóa RLock và tính toàn vẹn giao dịch.
    """
    db = GreenCDB(tmp_path / "concurrency.db")
    errors: list[Exception] = []

    def _worker_task(thread_id: int) -> int:
        try:
            for i in range(20):
                day_str = f"2026-11-{(i % 9) + 1:02d}"
                db.upsert_daily_snapshot(
                    "C:\\",
                    day_str,
                    total_bytes=500_000_000,
                    used_bytes=200_000_000 + thread_id * 1000 + i,
                    free_bytes=300_000_000,
                )
                db.record_write_event(
                    WriteEvent(
                        event_date=day_str,
                        timestamp=f"{day_str}T12:00:00",
                        drive_letter="C:\\",
                        folder_path=f"C:\\Thread_{thread_id}",
                        file_path=f"C:\\Thread_{thread_id}\\f_{i}.log",
                        delta_bytes=4096,
                        process_name=f"worker_{thread_id}.exe",
                        pid=1000 + thread_id,
                        service_name="",
                        service_display_name="Worker",
                        reason_code="LOG_GROWTH",
                        reason_detail="Concurrent write",
                    )
                )
                _ = db.get_daily_snapshots("C:\\", limit_days=14)
                _ = db.get_day_folder_breakdown("C:\\", day_str)
            return 20
        except Exception as exc:
            errors.append(exc)
            return 0

    with ThreadPoolExecutor(max_workers=8) as pool:
        counts = list(pool.map(_worker_task, range(8)))

    assert errors == []
    assert sum(counts) == 160
    snaps = db.get_daily_snapshots("C:\\", limit_days=14)
    assert len(snaps) == 9


def test_wb06_disk_tracker_deep_folder_summarization():
    """WB-06: Kiểm tra các nhánh rút gọn đường dẫn sâu trong DiskTracker._summarize_folder_path."""
    root = Path(r"C:\Users\duong\AppData\Roaming\DingTalk")
    deep_rc = Path(r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\resource_cache\0a\sub")
    summarized = DiskTracker._summarize_folder_path(deep_rc, root)
    assert summarized.endswith("resource_cache")

    zalo_root = Path(r"F:\Zalo Data")
    deep_zalo = Path(r"F:\Zalo Data\media\1988850\ZaloDownloads\resource\7017210\fileNoise")
    summarized_zalo = DiskTracker._summarize_folder_path(deep_zalo, zalo_root)
    assert summarized_zalo.endswith(r"ZaloDownloads\resource")

