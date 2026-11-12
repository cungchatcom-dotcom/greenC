import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from greenc.config import APP_NAME
from greenc.core.settings import AppSettings
from greenc.storage.database import GreenCDB
from greenc.ui.main_window import MainWindow


def _win(tmp_path: Path):
    app = QApplication.instance() or QApplication([])
    db = GreenCDB(tmp_path / "ui.db")
    settings = AppSettings(db)
    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sb",
                     auto_scan_on_init=False, settings=settings)
    win._bg_timer.stop()
    return app, win, settings


def test_brand_author_and_navigation(tmp_path: Path):
    app, win, _ = _win(tmp_path)
    assert win.windowTitle() == APP_NAME
    assert "Green" in win.brand_label.text() and "C:\\" in win.brand_label.text()
    assert "Cùng Chat" in win.author_label.text()
    assert 'href="https://cungchat.com"' in win.author_label.text()
    assert win.author_label.openExternalLinks()
    assert win.stack.count() == 5
    for btn, idx in ((win.btn_nav_overview, 0), (win.btn_nav_cleanup, 1), (win.btn_nav_relocation, 2),
                     (win.btn_nav_vault, 3), (win.btn_nav_settings, 4)):
        btn.click()
        app.processEvents()
        assert win.stack.currentIndex() == idx
    win.close()


def test_overview_freeable_card_has_eye_toggle(tmp_path: Path):
    app, win, _ = _win(tmp_path)
    card = win.overview_page.freeable_card
    assert not card.empty_label.isHidden()
    from greenc.core.smart_analyzer import AppAnalysisProposal

    prop = AppAnalysisProposal("Zalo", "Zalo.exe", "", False, "", [r"C:\Zalo"], 2 * 1024**3,
                               1024**3, 1024**3, True, "Đạt", "")
    win.overview_page.set_freeable([prop], [], "D:")
    assert card.simple_table.rowCount() == 1
    card.eye_button.click()
    assert card.is_expanded() and card.detail_table.rowCount() == 1
    win.close()


def test_settings_retention_persists_and_feeds_cleanup(tmp_path: Path):
    app, win, settings = _win(tmp_path)
    win.settings_tab.retention_spin.setValue(30)
    app.processEvents()
    assert settings.retention_days_default == 30
    assert "30 ngày" in win.cleanup_tab.description.text()
    win.close()


def test_vault_page_lists_archives(tmp_path: Path):
    app, win, _ = _win(tmp_path)
    win.run_sandbox_demo(silent=True)
    win.refresh_data_views()
    assert win.vault_page.vault_table.rowCount() >= 1
    win.close()
