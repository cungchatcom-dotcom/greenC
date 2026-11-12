import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from greenc.core.drives import DriveInfo
from greenc.core.settings import AppSettings
from greenc.storage.database import GreenCDB
from greenc.ui.data_location import FirstRunDialog, run_first_run

GB = 1024**3
DRIVES = [
    DriveInfo("C:\\", "Windows", "NTFS", 237 * GB, 18 * GB, True, True),
    DriveInfo("D:\\", "Data", "NTFS", 931 * GB, 612 * GB, False, True),
    DriveInfo("E:\\", "USB", "FAT32", 64 * GB, 60 * GB, False, False, "Ổ di động hoặc ổ mạng"),
]


def _app():
    return QApplication.instance() or QApplication([])


def test_dialog_preselects_recommended_and_disables_ineligible():
    _app()
    dlg = FirstRunDialog(DRIVES)
    loc = dlg.location
    assert loc.selected_drive().root == "D:\\"
    assert loc.data_root() == Path("D:\\Green C")
    assert "D:\\Green C" in loc.path_label.text()
    assert not loc.options[2].isEnabled()
    assert loc.retention_spin.value() == 7
    loc.options[0].radio.setChecked(True)
    assert loc.data_root() == Path("C:\\Green C")


def test_run_first_run_accept_saves_choice(tmp_path):
    _app()
    settings = AppSettings(GreenCDB(tmp_path / "f.db"))
    root = run_first_run(settings, DRIVES, exec_fn=lambda dlg: (dlg.location.retention_spin.setValue(14), True)[1])
    assert root == Path("D:\\Green C")
    assert settings.data_root == Path("D:\\Green C")
    assert settings.retention_days_default == 14


def test_run_first_run_later_uses_recommended(tmp_path):
    _app()
    settings = AppSettings(GreenCDB(tmp_path / "f.db"))
    assert run_first_run(settings, DRIVES, exec_fn=lambda dlg: False) == Path("D:\\Green C")
    assert settings.data_root == Path("D:\\Green C")


def test_current_root_is_preselected():
    _app()
    dlg = FirstRunDialog(DRIVES, current_root=Path("C:\\Green C"), allow_later=False)
    assert dlg.location.selected_drive().root == "C:\\"
    assert dlg.btn_later.text() == "Hủy"
