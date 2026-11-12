import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication
from greenc.core.smart_analyzer import AppAnalysisProposal
from greenc.storage.database import GreenCDB
from greenc.ui.main_window import MainWindow


def test_main_window_initializes_and_runs_sandbox_flow(tmp_path: Path):
    app = QApplication.instance() or QApplication([])
    db = GreenCDB(tmp_path / "ui_test.db")
    vault_dir = tmp_path / "vault"
    sandbox_dir = tmp_path / "sandbox"

    win = MainWindow(
        db=db,
        vault_dir=vault_dir,
        sandbox_dir=sandbox_dir,
        auto_scan_on_init=False,
    )
    assert win.windowTitle() == "Green C:\\"

    # 0 Tổng quan, 1 Dọn dẹp, 2 Di chuyển dữ liệu, 3 Kho dự phòng, 4 Cài đặt
    assert win.stack.count() == 5

    # Populate Smart Cleanup proposals & check 1-click "Dọn dẹp" button state
    sample_prop = AppAnalysisProposal(
        app_name="SampleApp",
        process_name="SampleApp.exe",
        service_name="",
        is_running=True,
        storage_summary="Đang chạy • Các file cũ đều rảnh",
        target_paths=[str(tmp_path)],
        total_physical_bytes=100 * 1024 * 1024,
        reclaimable_bytes=40 * 1024 * 1024,
        recommended_max_bytes=60 * 1024 * 1024,
        probe_passed=True,
        probe_strategy="Đạt",
        probe_detail="Đã chạy thử 1 vòng Nén -> Khôi phục khớp SHA-256 100%.",
    )
    win.cleanup_tab.set_proposals([sample_prop])
    assert win.cleanup_tab.btn_clean_now.isEnabled() is True
    assert "Dọn dẹp" in win.cleanup_tab.btn_clean_now.text()

    # Verify TaskProgressCard updates step title & detail caption cleanly
    win.progress_card.update_progress(
        45,
        "Bước 2/4: Đang phân tích DingTalk",
        "Đang kiểm tra khóa file Win32 và chạy thử nghiệm Sandbox...",
    )
    assert win.progress_card.progress_bar.value() == 45
    assert "DingTalk" in win.progress_card.title_label.text()

    # Trigger Sandbox Demo & Auto-Rollback flow
    res = win.run_sandbox_demo(silent=True)
    assert res["archive"] is not None
    assert len(db.list_quota_rules()) >= 1
    assert len(db.list_vault_archives(status="QUARANTINED")) >= 1

    arch_id = res["archive"].id
    rb_res = win.simulate_auto_rollback_for_archive(arch_id, silent=True)
    assert arch_id in rb_res["auto_rolled_back"]
    updated_arch = db.get_vault_archive(arch_id)
    assert updated_arch is not None
    assert updated_arch.status == "ROLLED_BACK"

    win.close()
    win.deleteLater()
    app.processEvents()
