import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import QApplication

from diskguard.ui import format_size
from diskguard.ui.icons import icon
from diskguard.ui.widgets import ExpandableTableCard, PathCell, PathValue, TableRow, TagValue


def _app():
    return QApplication.instance() or QApplication([])


def test_format_size_vietnamese():
    assert format_size(0) == "0 B"
    assert format_size(1536) == "2 KB"
    assert format_size(int(18.4 * 1024**3)) == "18,4 GB"
    assert format_size(5 * 1024**2) == "5,0 MB"


def test_icons_render():
    _app()
    for name in ("eye", "copy", "folder", "check", "warning", "drive"):
        assert not icon(name).isNull()


def test_path_cell_copies_full_path():
    _app()
    path = r"C:\Users\duong\AppData\Roaming\Telegram Desktop\tdata\very\long\folder"
    cell = PathCell(path)
    cell.resize(120, 28)
    cell.show()
    assert cell.label.toolTip() == path
    cell.copy_button.click()
    assert QGuiApplication.clipboard().text() == path
    assert cell.open_button.toolTip() == "Mở trong Explorer"


def test_expandable_card_toggles_between_simple_and_detail():
    _app()
    card = ExpandableTableCard("Có thể giải phóng", ["Ứng dụng", "Vị trí", "Dung lượng", "Giải phóng", "Đề xuất"], right_detail_cols=(2, 3))
    card.set_rows([
        TableRow(("Zalo", "Chuyển sang D:", "32,1 GB"),
                 ["Zalo", PathValue(r"C:\Zalo"), "34,0 GB", "32,1 GB", TagValue("Chuyển sang D:")], 32),
        TableRow(("DingTalk", "Dọn file cũ", "1,9 GB"),
                 ["DingTalk", PathValue(r"C:\DT"), "3,3 GB", "1,9 GB", TagValue("Đang chạy", warning=True)], 2),
    ])
    assert not card.is_expanded()
    assert card.simple_table.rowCount() == 2
    assert card.simple_table.item(0, 0).text() == "Zalo"
    card.eye_button.click()
    assert card.is_expanded()
    assert card.detail_table.rowCount() == 2
    assert isinstance(card.detail_table.cellWidget(0, 1), PathCell)
    card.eye_button.click()
    assert not card.is_expanded()


def test_empty_card_shows_hint():
    _app()
    card = ExpandableTableCard("X", ["A"])
    card.set_empty_text("Chưa phân tích.")
    card.set_rows([])
    assert card.empty_label.text() == "Chưa phân tích."
    assert not card.empty_label.isHidden()
