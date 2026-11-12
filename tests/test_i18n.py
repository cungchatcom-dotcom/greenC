import json
import os
import sys
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from greenc import i18n
from greenc.core.settings import AppSettings
from greenc.storage.database import GreenCDB

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import i18n as i18n_tool  # noqa: E402  (scripts/i18n.py)


@pytest.fixture(autouse=True)
def _reset_language():
    yield
    i18n.set_language(i18n.SOURCE_LANGUAGE)


def test_fifty_unique_languages():
    assert len(i18n.LANGUAGES) == 50
    assert len(set(i18n.LANGUAGE_CODES)) == 50
    assert i18n.LANGUAGE_CODES[:2] == ["vi", "en"]


def test_source_language_is_identity_and_formats():
    assert i18n.current_language() == "vi"
    assert i18n.tr("Dọn dẹp") == "Dọn dẹp"
    assert i18n.tr("Dọn dẹp {size}", size="4,0 GB") == "Dọn dẹp 4,0 GB"


def test_translation_lookup_and_fallbacks(monkeypatch, tmp_path):
    (tmp_path / "en.json").write_text(
        json.dumps({"Dọn dẹp {size}": "Clean up {size}", "Hỏng {x}": "Broken {y}"}), encoding="utf-8"
    )
    monkeypatch.setattr(i18n, "LOCALES_DIR", tmp_path)
    assert i18n.set_language("en") == "en"
    assert i18n.tr("Dọn dẹp {size}", size="4.0 GB") == "Clean up 4.0 GB"
    assert i18n.tr("Chưa dịch") == "Chưa dịch"  # missing key -> source text
    assert i18n.tr("Hỏng {x}", x=1) == "Hỏng 1"  # bad placeholder in translation -> source text
    assert i18n.decimal_point() == "."
    assert i18n.set_language("xx") == "vi"


@pytest.mark.parametrize(
    ("qt_name", "expected"),
    [("vi_VN", "vi"), ("en_US", "en"), ("pt_BR", "pt_BR"), ("pt_PT", "pt_PT"), ("zh_CN", "zh_CN"),
     ("zh_TW", "zh_TW"), ("zh_Hant_HK", "zh_TW"), ("nn_NO", "nb"), ("de_AT", "de"), ("zu_ZA", "en")],
)
def test_match_system_language(qt_name, expected):
    assert i18n.match_system_language(qt_name) == expected


def test_rtl_languages():
    assert all(i18n.is_rtl(code) for code in ("ar", "he", "fa", "ur"))
    assert not i18n.is_rtl("vi")


def test_language_setting_roundtrip(tmp_path):
    settings = AppSettings(GreenCDB(tmp_path / "l.db"))
    assert settings.language is None
    settings.set_language("ja")
    assert AppSettings(GreenCDB(tmp_path / "l.db")).language == "ja"
    with pytest.raises(ValueError):
        settings.set_language("klingon")


def test_settings_tab_language_picker(tmp_path):
    from PySide6.QtWidgets import QApplication

    from greenc.ui.main_window import MainWindow

    QApplication.instance() or QApplication([])
    db = GreenCDB(tmp_path / "w.db")
    settings = AppSettings(db)
    win = MainWindow(db=db, vault_dir=tmp_path / "v", sandbox_dir=tmp_path / "s", auto_scan_on_init=False, settings=settings)
    win._bg_timer.stop()
    tab = win.settings_tab
    assert tab.language_combo.count() == 50
    assert tab.language_combo.currentData() == "vi"
    assert tab.btn_restart.isHidden()
    tab.language_combo.setCurrentIndex(tab.language_combo.findData("fr"))
    assert settings.language == "fr"
    assert not tab.btn_restart.isHidden()
    tab.language_combo.setCurrentIndex(tab.language_combo.findData("vi"))
    assert tab.btn_restart.isHidden()
    win.close()


def test_restart_command_adds_flag_once(monkeypatch):
    from greenc.ui import restart

    monkeypatch.setattr(sys, "argv", ["main.py", "--restarted"])
    program, args = restart.restart_command()
    assert program == sys.executable
    assert args[-1] == "--restarted" and args.count("--restarted") == 1
    assert args[0].endswith("main.py")


def test_catalog_is_up_to_date_with_code():
    messages = i18n_tool.load_messages()
    assert list(i18n_tool.extract_strings()) == messages, "run: uv run python scripts/i18n.py extract"


@pytest.mark.parametrize("code", [c for c in i18n.LANGUAGE_CODES if c != i18n.SOURCE_LANGUAGE])
def test_every_locale_is_complete(code):
    messages = i18n_tool.load_messages()
    catalog = json.loads((i18n.LOCALES_DIR / f"{code}.json").read_text(encoding="utf-8"))
    report = i18n_tool.check_locale(messages, catalog)
    assert report == {"missing": [], "extra": [], "placeholders": []}, {k: v[:5] for k, v in report.items()}
