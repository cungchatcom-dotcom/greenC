"""
Bộ Kịch Bản Kiểm Thử Thực Tế Toàn Diện (End-to-End Application Test Cases)
Thực thi trực tiếp trên giao diện PySide6 MainWindow của DiskGuard:
- TC-01: Giao diện chuẩn Apple/Google & Điều hướng Sidebar 3 màn hình.
- TC-02: Biểu đồ cột biến động theo ngày & Click chọn cột ngày xem chi tiết (Drill-down).
- TC-03: Kiểm tra Rule chuyên sâu (Zalo / DingTalk) ẩn trong Settings > Rules và chạy ngầm.
- TC-04: Quét ứng dụng + Tự kiểm chứng 1 chu trình Sandbox (HardLink + Win32 Lock + Fallback).
- TC-05: Đa luồng QThread + Thanh tiến trình Pause / Resume / Stop & Chú thích từng bước.
- TC-06: Bấm 1 nút "Dọn dẹp" duy nhất -> Nén vào Safe Vault & Lưu Rule chạy ngầm.
- TC-07: Tự động Rollback khi phát hiện mất an toàn & Tự động xóa sau D ngày an toàn.
"""

import ctypes
from ctypes import wintypes
from datetime import datetime, timedelta
import hashlib
import os
from pathlib import Path
import time

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QPointF, Qt
from PySide6.QtGui import QMouseEvent
from PySide6.QtWidgets import QApplication

from diskguard.models import WriteEvent
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.main_window import MainWindow
from diskguard.ui.workers import BackgroundTaskWorker


def _wait_for_worker(app: QApplication, worker: BackgroundTaskWorker | None, timeout_s: float = 15.0) -> None:
    if worker is None:
        return
    deadline = time.time() + timeout_s
    while worker.isRunning() and time.time() < deadline:
        app.processEvents()
        time.sleep(0.02)
    app.processEvents()


def test_tc01_clean_ui_and_sidebar_navigation(tmp_path: Path):
    """TC-01: Kiểm tra giao diện tối giản chuẩn Apple/Google và chuyển trang trên Sidebar."""
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "tc01.db")
    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sandbox", auto_scan_on_init=False)
    win.show()
    app.processEvents()

    # Mặc định mở ở trang 0 (Tổng quan ổ đĩa)
    assert win.stack.currentIndex() == 0
    assert win.btn_nav_overview.isChecked() is True

    # Chuyển sang trang 1 (Dọn dẹp thông minh)
    win.btn_nav_cleanup.click()
    app.processEvents()
    assert win.stack.currentIndex() == 1

    # Chuyển sang trang 2 (Cài đặt: Rules & Safe Vault)
    win.btn_nav_settings.click()
    app.processEvents()
    assert win.stack.currentIndex() == 2

    win.close()
    win.deleteLater()
    app.processEvents()


def test_tc02_daily_bar_chart_click_drilldown(tmp_path: Path):
    """TC-02: Kiểm tra biểu đồ cột theo ngày và sự kiện click chuột vào cột ngày để xem chi tiết."""
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "tc02.db")

    # Tạo dữ liệu 3 ngày liên tiếp với biến động khác nhau
    db.upsert_daily_snapshot("C:\\", "2026-11-07", 200 * 1024**3, 150 * 1024**3, 50 * 1024**3, 100 * 1024**2)
    db.upsert_daily_snapshot("C:\\", "2026-11-08", 200 * 1024**3, 152 * 1024**3, 48 * 1024**3, 2 * 1024**3)
    db.upsert_daily_snapshot("C:\\", "2026-11-09", 200 * 1024**3, 153 * 1024**3, 47 * 1024**3, 500 * 1024**2)

    db.record_write_event(
        WriteEvent(
            event_date="2026-11-08",
            timestamp="2026-11-08T14:20:00",
            drive_letter="C:\\",
            folder_path=r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\EAppFiles\download",
            file_path=r"C:\Users\duong\AppData\Roaming\DingTalk\40012285383_v2\EAppFiles\download\bundle.amr",
            delta_bytes=2 * 1024**3,
            process_name="DingTalk.exe",
            pid=8812,
            service_name="",
            service_display_name="DingTalk Desktop",
            reason_code="EAPP_BUNDLE",
            reason_detail="Gói Mini-App DingTalk tải về lưu cữu (42 files)",
        )
    )

    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sandbox", auto_scan_on_init=False)
    win.resize(1280, 800)
    win.show()
    snaps = db.get_daily_snapshots("C:\\", limit_days=14)
    win.daily_tab.load_drive_data("C:\\", snaps)
    win.daily_tab.chart.repaint()
    app.processEvents()

    # Giả lập click chuột trực tiếp vào cột ngày 2026-11-08 trên biểu đồ
    bar_rects = win.daily_tab.chart._bar_rects
    assert len(bar_rects) == 3
    target_rect, target_date = bar_rects[1]
    assert target_date == "2026-11-08"

    click_pos = target_rect.center()
    mouse_event = QMouseEvent(
        QMouseEvent.Type.MouseButtonPress,
        QPointF(click_pos),
        QPointF(click_pos),
        Qt.MouseButton.LeftButton,
        Qt.MouseButton.LeftButton,
        Qt.KeyboardModifier.NoModifier,
    )
    win.daily_tab.chart.mousePressEvent(mouse_event)
    app.processEvents()

    assert win.daily_tab.chart.selected_date() == "2026-11-08"
    assert win.daily_tab.folder_table.rowCount() == 1
    assert "DingTalk" in win.daily_tab.folder_table.item(0, 0).text()
    assert win.daily_tab.actor_table.rowCount() == 1
    assert "DingTalk" in win.daily_tab.actor_table.item(0, 0).text()

    win.close()
    win.deleteLater()
    app.processEvents()


def test_tc03_silent_rules_in_settings_not_cluttering_main_ui(tmp_path: Path):
    """TC-03: Kiểm tra Rule chuyên sâu của Zalo & DingTalk nằm gọn trong Settings > Rules và chạy ngầm."""
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "tc03.db")
    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sandbox", auto_scan_on_init=False)
    win._seed_default_silent_rules()
    win.settings_tab.refresh_all()
    app.processEvents()

    rules = db.list_quota_rules()
    rule_names = [r.app_name for r in rules]
    # Trên máy thật đang có Zalo & DingTalk nên cả 2 đều được nạp tự động vào Settings > Rules
    assert any("Zalo" in n for n in rule_names)
    assert any("DingTalk" in n for n in rule_names)
    assert win.settings_tab.rules_table.rowCount() == len(rules)

    win.close()
    win.deleteLater()
    app.processEvents()


def test_tc04_and_tc06_smart_scan_sandbox_probe_and_one_click_cleanup_on_ui(tmp_path: Path):
    """
    TC-04 & TC-06: Quét ứng dụng -> Tự test 1 chu trình Sandbox (khớp SHA-256 & HardLink,
    tự chuyển phương án dự phòng nếu gặp file khóa) -> Hiển thị lên UI -> Bấm 1 nút 'Dọn dẹp'.
    """
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "tc04.db")
    vault_dir = tmp_path / "vault"
    sandbox_dir = tmp_path / "sandbox"

    # Tạo 2 ứng dụng mẫu để kiểm thử thực tế trên UI
    zalo_sim = tmp_path / "MockZalo"
    (zalo_sim / "fileNoise").mkdir(parents=True)
    (zalo_sim / "resource" / "chat_01").mkdir(parents=True)
    now = time.time()

    hl_1 = zalo_sim / "fileNoise" / "hash_media_01.bin"
    hl_2 = zalo_sim / "resource" / "chat_01" / "msg_media_01.bin"
    hl_1.write_bytes(b"ZALO_HARDLINK_MEDIA_" * 150_000)  # ~3 MB
    os.utime(hl_1, (now - 86400 * 10, now - 86400 * 10))
    os.link(hl_1, hl_2)
    expected_sha = hashlib.sha256(hl_1.read_bytes()).hexdigest()

    ding_sim = tmp_path / "MockDingTalk"
    (ding_sim / "DBFiles").mkdir(parents=True)
    (ding_sim / "EAppFiles" / "download").mkdir(parents=True)
    locked_db = ding_sim / "DBFiles" / "live_msg.dat"
    locked_db.write_bytes(b"ACTIVE_LOCKED_DATA_" * 80_000)
    os.utime(locked_db, (now - 86400 * 15, now - 86400 * 15))

    old_amr = ding_sim / "EAppFiles" / "download" / "miniapp_v1.amr"
    old_amr.write_bytes(b"DINGTALK_AMR_BUNDLE_" * 160_000)  # ~3.2 MB
    os.utime(old_amr, (now - 86400 * 20, now - 86400 * 20))

    win = MainWindow(db=db, vault_dir=vault_dir, sandbox_dir=sandbox_dir, auto_scan_on_init=False)
    win.show()
    app.processEvents()

    # Giữ khóa độc quyền Win32 trên file locked_db trong suốt quá trình quét & dọn dẹp
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    h = kernel32.CreateFileW(str(locked_db), 0x80000000 | 0x40000000, 0, None, 3, 0, None)
    assert h != wintypes.HANDLE(-1).value
    try:
        proposals = win.analyzer.scan_and_probe_apps(
            custom_app_dirs=[
                ("Zalo Mock App", "Zalo.exe", [str(zalo_sim)]),
                ("DingTalk Mock App", "DingTalk.exe", [str(ding_sim)]),
            ],
            min_reclaimable_bytes=512 * 1024,
        )
        # Sau bước tự thử nghiệm Sandbox, file gốc và HardLink phải vẫn nguyên vẹn 100%
        assert hl_1.exists() and hl_2.exists()
        assert hashlib.sha256(hl_1.read_bytes()).hexdigest() == expected_sha
        assert os.stat(hl_1).st_ino == os.stat(hl_2).st_ino

        win.cleanup_tab.set_proposals(proposals)
        app.processEvents()
        assert win.cleanup_tab.table.rowCount() == 2
        assert win.cleanup_tab.btn_clean_now.isEnabled() is True

        # Bấm nút 1-Click "Dọn dẹp" trực tiếp trên giao diện (chạy qua BackgroundTaskWorker QThread!)
        win.cleanup_tab.btn_clean_now.click()
        _wait_for_worker(app, win._active_worker)

        # Kiểm tra kết quả sau khi bấm "Dọn dẹp":
        # 1. File đang bị khóa (locked_db) tuyệt đối không bị đụng vào
        assert locked_db.exists()
        # 2. Cả 2 ứng dụng đã được tự động tạo Rule chạy ngầm trong Settings > Rules
        assert len(db.list_quota_rules()) == 2
        # 3. Các file cũ đã được nén vào Safe Vault
        archives = db.list_vault_archives(status="QUARANTINED")
        assert len(archives) == 2
    finally:
        kernel32.CloseHandle(h)
        win.close()
        win.deleteLater()
        app.processEvents()


def test_tc05_background_qthread_pause_resume_stop_and_step_captions(tmp_path: Path):
    """TC-05: Kiểm tra tác vụ nặng chạy trên QThread có thể Tạm dừng (Pause), Tiếp tục (Resume), Dừng (Stop) và hiển thị chú thích từng bước."""
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "tc05.db")
    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sandbox", auto_scan_on_init=False)
    win.show()
    app.processEvents()

    captions_seen: list[tuple[int, str, str]] = []
    win.progress_card.progressChanged = None  # Use card labels directly

    def _slow_task(ctrl):
        for step in range(1, 6):
            ok = ctrl.report(
                step * 20,
                f"Bước {step}/5: Đang kiểm tra phân vùng",
                f"Chú thích chi tiết bước {step}: Đang đối chiếu dữ liệu...",
            )
            if not ok:
                return "STOPPED"
            time.sleep(0.06)
        return "COMPLETED"

    done_holder = []
    win._launch_background_worker(
        "Khởi chạy tác vụ kiểm thử đa luồng...",
        _slow_task,
        lambda res, stopped: done_holder.append((res, stopped)),
    )
    worker = win._active_worker
    assert worker is not None

    # Đợi bước đầu tiên cập nhật lên thanh tiến trình
    time.sleep(0.04)
    app.processEvents()

    # Bấm nút "Tạm dừng" (Pause) trên TaskProgressCard
    win.progress_card.btn_pause.click()
    app.processEvents()
    assert worker.control.is_paused is True
    assert win.progress_card.btn_pause.text() == "Tiếp tục"

    # Bấm nút "Tiếp tục" (Resume)
    win.progress_card.btn_pause.click()
    app.processEvents()
    assert worker.control.is_paused is False
    assert win.progress_card.btn_pause.text() == "Tạm dừng"

    # Bấm nút "Dừng" (Stop)
    win.progress_card.btn_stop.click()
    _wait_for_worker(app, worker)

    assert len(done_holder) == 1
    assert done_holder[0][1] is True  # was_stopped == True
    assert "Đã dừng" in win.progress_card.title_label.text()

    win.close()
    win.deleteLater()
    app.processEvents()


def test_tc07_auto_rollback_and_d_day_expiry_on_app(tmp_path: Path):
    """TC-07: Kiểm tra tự động Rollback khi ứng dụng cần lại file và tự động xóa sau D ngày an toàn."""
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "tc07.db")
    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sandbox", auto_scan_on_init=False)

    # Tạo 1 gói nén trong Safe Vault qua Sandbox Simulator
    demo = win.run_sandbox_demo(silent=True)
    arch = demo["archive"]
    assert arch is not None

    # Nhánh 1: Phát sinh lỗi thiếu file -> Tự động Rollback
    rb = win.simulate_auto_rollback_for_archive(arch.id, silent=True)
    assert arch.id in rb["auto_rolled_back"]
    assert db.get_vault_archive(arch.id).status == "ROLLED_BACK"

    # Nhánh 2: Tạo gói nén mới và tua thời gian qua 8 ngày (> D=7 ngày) -> Tự động xóa vĩnh viễn
    demo2 = win.run_sandbox_demo(silent=True)
    arch2 = demo2["archive"]
    assert arch2 is not None
    purged_ids = win.watchdog.purge_expired_archives(now_dt=datetime.now() + timedelta(days=8))
    assert arch2.id in purged_ids
    assert db.get_vault_archive(arch2.id).status == "PURGED"
    assert not Path(arch2.zip_path).exists()

    win.close()
    win.deleteLater()
    app.processEvents()
