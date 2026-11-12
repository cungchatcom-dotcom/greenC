import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from greenc.core.smart_analyzer import AppAnalysisProposal
from greenc.ui.dialogs import QuotaRuleDialog
from greenc.ui.smart_cleanup_tab import SmartCleanupTab
from greenc.ui.widgets import PathCell

BANNED = ("(", "thông minh", "1-click", "Sandbox", "Rollback", "Verified")


def _app():
    return QApplication.instance() or QApplication([])


def _prop(name, reclaim):
    return AppAnalysisProposal(
        app_name=name, process_name=f"{name}.exe", service_name="", is_running=False,
        storage_summary="Đang nghỉ", target_paths=[rf"C:\Users\me\AppData\Roaming\{name}"],
        total_physical_bytes=reclaim * 2, reclaimable_bytes=reclaim, recommended_max_bytes=reclaim,
        probe_passed=True, probe_strategy="Đạt", probe_detail="Đã thử nén và khôi phục, dữ liệu khớp.",
    )


def test_cleanup_tab_copy_and_path_cells():
    _app()
    tab = SmartCleanupTab()
    tab.set_retention_days(14)
    assert "14 ngày" in tab.description.text()
    tab.set_proposals([_prop("Zalo", 3 * 1024**3), _prop("Teams", 1024**3)])
    assert tab.table.rowCount() == 2
    assert isinstance(tab.table.cellWidget(0, 2), PathCell)
    assert tab.btn_clean_now.isEnabled() and tab.btn_clean_now.text() == "Dọn dẹp 4,0 GB"
    for text in (tab.description.text(), tab.proposal_summary_label.text(), tab.btn_scan_apps.text()):
        assert not any(b in text for b in BANNED), text


def test_rule_dialog_uses_default_retention():
    _app()
    dlg = QuotaRuleDialog(default_retention_days=21)
    assert dlg.retention_spin.value() == 21
    assert dlg.windowTitle() == "Quy tắc dung lượng"
