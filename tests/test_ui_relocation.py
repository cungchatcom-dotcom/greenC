import json
import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from greenc.core.relocation.base import RelocationCandidate
from greenc.core.relocation.junction import JunctionStrategy
from greenc.core.relocation.service import RelocationService
from greenc.core.relocation.zalo import ZaloGuidedStrategy
from greenc.core.settings import AppSettings
from greenc.models import RELOCATION_DONE, RelocationRecord
from greenc.storage.database import GreenCDB
from greenc.ui.dialogs import GuidedDialog
from greenc.ui.relocation_page import RelocationPage
from greenc.ui.widgets import PathCell


def _app():
    return QApplication.instance() or QApplication([])


def test_page_lists_candidates_and_emits_actions():
    _app()
    page = RelocationPage()
    page.set_target(Path("D:\\Green C"), 600 * 1024**3)
    tg = RelocationCandidate("telegram", "Telegram Desktop", r"C:\TG", 9 * 1024**3, JunctionStrategy([]), "Chuyển thư mục và tạo liên kết")
    adv = RelocationCandidate("dir-x", "X", r"C:\X", 1024**3, JunctionStrategy([]), "Nâng cao", advanced=True)
    zalo = RelocationCandidate("zalo", "Zalo", r"C:\Z", 30 * 1024**3, ZaloGuidedStrategy(), "Dùng tùy chọn của Zalo")
    page.set_candidates([zalo, tg, adv])
    assert page.table.rowCount() == 2 and page.advanced_table.rowCount() == 1
    assert page.advanced_table.isHidden()
    page.advanced_toggle.setChecked(True)
    assert not page.advanced_table.isHidden()
    got = []
    page.relocateRequested.connect(got.append)
    page.guideRequested.connect(got.append)
    page.action_buttons[0].click()  # Zalo → guide
    page.action_buttons[1].click()  # Telegram → relocate
    assert got == [zalo, tg]
    assert page.action_buttons[0].text() == "Hướng dẫn"


def test_records_and_disabled_banner():
    _app()
    page = RelocationPage()
    rec = RelocationRecord(5, "telegram", "Telegram Desktop", "junction", r"C:\TG", r"D:\Green C\Apps\telegram", 1, RELOCATION_DONE)
    page.set_records([rec])
    assert isinstance(page.done_table.cellWidget(0, 1), PathCell)
    undone = []
    page.undoRequested.connect(undone.append)
    page.undo_buttons[0].click()
    assert undone == [rec]
    page.set_disabled_reason("Chọn ổ dữ liệu khác ổ C:")
    assert not page.banner.isHidden()


def test_guided_dialog_verifies_zalo(tmp_path):
    _app()
    cfg = tmp_path / "config.json"
    cfg.write_text(json.dumps({"db_path": {"location": "C:\\ZaloPC\\Zalo Data\\message"}}), encoding="utf-8")
    db = GreenCDB(tmp_path / "g.db")
    svc = RelocationService(db, AppSettings(db))
    cand = RelocationCandidate("zalo", "Zalo", "C:\\ZaloPC\\Zalo Data", 10, ZaloGuidedStrategy(config_path=cfg, system_root="C:\\"), "how")
    dlg = GuidedDialog(cand, Path("D:\\Green C"), svc)
    assert "D:\\Green C\\Zalo" in dlg.steps_label.text()
    dlg.btn_verify.click()
    assert dlg.record is None and "vẫn ở" in dlg.result_label.text()
    cfg.write_text(json.dumps({"db_path": {"location": "D:\\Green C\\Zalo\\Zalo Data\\message"}}), encoding="utf-8")
    dlg.btn_verify.click()
    assert dlg.record is not None and dlg.record.status == RELOCATION_DONE


def test_main_window_shows_relocation_page(tmp_path):
    _app()
    from greenc.ui.main_window import MainWindow

    db = GreenCDB(tmp_path / "w.db")
    settings = AppSettings(db)
    win = MainWindow(db=db, vault_dir=tmp_path / "v", sandbox_dir=tmp_path / "s", auto_scan_on_init=False, settings=settings)
    win._bg_timer.stop()
    assert isinstance(win.relocation_page, RelocationPage)
    assert not win.relocation_page.banner.isHidden()  # no data root chosen yet
    settings.set_data_root(tmp_path / "Green C")
    win._refresh_relocation_target()
    win.close()
