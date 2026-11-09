"""
Kiểm Thử Hiệu Năng, Độ Phản Hồi Giao Diện & Rò Rỉ Tài Nguyên (Application Performance & Stress Suite):
- PERF-01: Tốc độ quét & khử trùng lặp NTFS HardLink trên tập dữ liệu lớn (2,000 đường dẫn file).
- PERF-02: Tốc độ nén xoay vòng (QuotaEnforcer) & giải nén khôi phục nguyên tử (Rollback + SHA-256) hàng loạt.
- PERF-03: Độ trễ khung hình giao diện (UI Frame Latency P95 < 16ms / >= 60 FPS) khi luồng nền QThread chạy tải nặng.
- PERF-04: Độ trễ phản hồi tín hiệu Tạm dừng (Pause) / Tiếp tục (Resume) / Dừng (Stop) (< 50ms).
- PERF-05: Kiểm tra rò rỉ Windows File Handle (Zero Handle Leak) và ổn định bộ nhớ RAM (tracemalloc) qua nhiều chu kỳ.
"""

import os
from pathlib import Path
import time
import tracemalloc

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import psutil
from PySide6.QtWidgets import QApplication

from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.win_locks import compute_physical_usage, is_file_locked
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.main_window import MainWindow


def test_perf01_high_volume_hardlink_deduplication_throughput(tmp_path: Path):
    """
    PERF-01: Tạo 2,000 đường dẫn file (gồm 600 cặp NTFS HardLink = 1,200 nhánh + 800 file đơn = 1,400 inode vật lý).
    Đo thời gian quét và khử trùng lặp Inode: yêu cầu hoàn tất < 2.0 giây và chính xác 100%.
    """
    root_a = tmp_path / "fileNoise"
    root_b = tmp_path / "resource"
    root_single = tmp_path / "cache"
    root_a.mkdir()
    root_b.mkdir()
    root_single.mkdir()

    payload_1k = b"Z" * 1024
    for i in range(600):
        fa = root_a / f"hl_{i:04d}.bin"
        fb = root_b / f"msg_{i:04d}.bin"
        fa.write_bytes(payload_1k)
        os.link(fa, fb)

    for j in range(800):
        fs = root_single / f"single_{j:04d}.bin"
        fs.write_bytes(payload_1k)

    t0 = time.perf_counter()
    phys_bytes, naive_bytes, inode_map = compute_physical_usage([tmp_path])
    elapsed = time.perf_counter() - t0

    assert naive_bytes == 2000 * 1024
    assert phys_bytes == 1400 * 1024
    assert len(inode_map) == 1400
    assert elapsed < 2.0, f"Deduplication scan took too long: {elapsed:.3f}s"


def test_perf02_bulk_quota_zip_and_rollback_throughput(tmp_path: Path):
    """
    PERF-02: Tạo 30 file cũ (~6 MB tổng dung lượng) vượt ngưỡng Quota.
    Đo tốc độ nén vào Safe Vault (kèm kiểm tra khóa Win32 + SHA-256 + testzip)
    và tốc độ Rollback khôi phục toàn bộ file: tổng thời gian < 3.5 giây.
    """
    db = DiskGuardDB(tmp_path / "perf02.db")
    app_dir = tmp_path / "BulkApp"
    app_dir.mkdir()
    now = time.time()

    block_200k = (b"BULK_LOG_LINE_0123456789ABCDEF\n" * 6400)  # ~200 KB compressible
    for i in range(30):
        fp = app_dir / f"old_chunk_{i:03d}.dat"
        fp.write_bytes(block_200k)
        os.utime(fp, (now - 3600 - i, now - 3600 - i))

    rule = QuotaRule(
        id=None,
        app_name="Bulk Throughput App",
        process_name="bulk.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=1 * 1024 * 1024,  # 1 MB cap -> forces ~5 MB (25+ files) to be zipped
        watermark_ratio=0.80,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    watchdog = SafetyWatchdog(db)

    t_zip_start = time.perf_counter()
    arch = enforcer.enforce_rule(rule, now_ts=now)
    t_zip = time.perf_counter() - t_zip_start

    assert arch is not None
    assert arch.file_count >= 25

    t_rb_start = time.perf_counter()
    rb_res = watchdog.rollback_archive(arch.id, reason="Perf benchmark rollback")
    t_rb = time.perf_counter() - t_rb_start

    assert rb_res["restored_count"] == arch.file_count
    assert (t_zip + t_rb) < 3.5, f"Bulk Zip ({t_zip:.2f}s) + Rollback ({t_rb:.2f}s) exceeded SLA"


def test_perf03_ui_frame_latency_under_heavy_background_qthread_load(tmp_path: Path):
    """
    PERF-03: Đo độ trễ phản hồi của giao diện chính (UI Main Thread Event Loop Latency)
    trong lúc BackgroundTaskWorker (QThread) đang chạy tác vụ nặng và bắn tín hiệu liên tục.
    Yêu cầu: P95 UI event-loop frame latency < 16.0 ms (đảm bảo tốc độ khung hình >= 60 FPS, không bị 'quay quay').
    """
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "perf03.db")
    win = MainWindow(
        db=db,
        vault_dir=tmp_path / "vault",
        sandbox_dir=tmp_path / "sandbox",
        auto_scan_on_init=False,
    )
    win.show()
    app.processEvents()

    def _heavy_bg_job(ctrl):
        import hashlib

        payload = b"DISKGUARD_BENCHMARK_BLOCK_" * 10_000  # ~260 KB block
        # Simulate heavy I/O + SHA-256 hashing with 60 progress updates
        for step in range(1, 61):
            if not ctrl.report(
                int(step * 100 / 60),
                f"Đang xử lý bước {step}/60",
                f"Đang tính toán băm dữ liệu khối #{step * 100}...",
            ):
                return "STOPPED"
            _ = hashlib.sha256(payload).hexdigest()
            time.sleep(0.004)
        return "DONE"

    win._launch_background_worker(
        "Đang chạy bài kiểm tra tải nặng...",
        _heavy_bg_job,
        lambda res, stopped: None,
    )
    worker = win._active_worker
    assert worker is not None

    frame_latencies_ms: list[float] = []
    while worker.isRunning():
        t_frame0 = time.perf_counter()
        # Process Qt signals & UI updates on main thread while QThread works
        app.processEvents()
        dt_ms = (time.perf_counter() - t_frame0) * 1000.0
        frame_latencies_ms.append(dt_ms)
        time.sleep(0.004)

    assert len(frame_latencies_ms) >= 10
    sorted_lat = sorted(frame_latencies_ms)
    p95_idx = min(len(sorted_lat) - 1, int(len(sorted_lat) * 0.95))
    p95_ms = sorted_lat[p95_idx]

    assert p95_ms < 16.0, f"UI P95 frame latency too high: {p95_ms:.2f} ms (target < 16ms)"

    win.close()
    win.deleteLater()
    app.processEvents()


def test_perf04_pause_and_stop_signal_reaction_latency():
    """
    PERF-04: Đo thời gian phản ứng từ khi người dùng bấm Pause / Resume / Stop
    tới khi trạng thái TaskControl hiệu lực: yêu cầu < 20ms.
    """
    from diskguard.core.task_controller import TaskControl

    ctrl = TaskControl()
    t0 = time.perf_counter()
    ctrl.pause()
    assert ctrl.is_paused is True
    ctrl.resume()
    assert ctrl.is_paused is False
    ctrl.stop()
    assert ctrl.is_stopped is True
    elapsed_ms = (time.perf_counter() - t0) * 1000.0
    assert elapsed_ms < 20.0


def test_perf05_zero_windows_handle_leak_and_memory_stability(tmp_path: Path):
    """
    PERF-05: Chạy lặp 15 chu kỳ kiểm tra khóa Win32 CreateFileW, Nén Zip và Rollback.
    Đo số lượng Windows OS Handle (`psutil.Process().num_handles()`) và bộ nhớ (`tracemalloc`)
    để chứng minh không rò rỉ File Handle hay rò rỉ bộ nhớ.
    """
    proc = psutil.Process(os.getpid())
    db = DiskGuardDB(tmp_path / "perf05.db")
    app_dir = tmp_path / "LeakCheckApp"
    app_dir.mkdir()
    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    watchdog = SafetyWatchdog(db)
    now = time.time()

    for i in range(10):
        f = app_dir / f"item_{i}.bin"
        f.write_bytes(b"M" * 64_000)
        os.utime(f, (now - 3600, now - 3600))

    rule = QuotaRule(
        id=None,
        app_name="Leak Check App",
        process_name="leak.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=200_000,
        watermark_ratio=0.50,
        retention_days=7,
        min_file_age_seconds=60,
    )
    rule.id = db.save_quota_rule(rule)

    # Warm-up 1 cycle
    for f in app_dir.iterdir():
        _ = is_file_locked(f)
    arch0 = enforcer.enforce_rule(rule, now_ts=now)
    if arch0:
        watchdog.rollback_archive(arch0.id, "warmup")
        # Clear protected_files so subsequent cycles can re-archive
        rule.protected_files = []
        db.save_quota_rule(rule)

    tracemalloc.start()
    handles_before = proc.num_handles()
    mem_before, _ = tracemalloc.get_traced_memory()

    for cycle in range(15):
        for f in app_dir.iterdir():
            assert is_file_locked(f) is False
        arch = enforcer.enforce_rule(rule, now_ts=now)
        assert arch is not None
        watchdog.rollback_archive(arch.id, f"cycle_{cycle}")
        rule.protected_files = []
        db.save_quota_rule(rule)

    mem_after, _ = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    handles_after = proc.num_handles()

    handle_delta = handles_after - handles_before
    mem_growth_kb = (mem_after - mem_before) / 1024.0

    # Cho phép dao động nhỏ của trình quản lý bộ nhớ/SQLite, nhưng không được rò rỉ hàng chục handle mỗi vòng
    assert handle_delta <= 10, f"Windows Handle leak detected: +{handle_delta} handles after 15 cycles"
    assert mem_growth_kb < 2048.0, f"Memory grew excessively: +{mem_growth_kb:.1f} KB"

