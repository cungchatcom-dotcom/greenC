from dataclasses import dataclass, field
import os
from pathlib import Path
import subprocess

from PySide6.QtCore import QSize, Qt, QTimer
from PySide6.QtGui import QColor, QGuiApplication
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QSizePolicy,
    QStackedWidget,
    QTableWidget,
    QTableWidgetItem,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from diskguard.ui.icons import icon


def icon_button(name: str, tooltip: str, checkable: bool = False) -> QToolButton:
    btn = QToolButton()
    btn.setObjectName("IconButton")
    btn.setIcon(icon(name))
    btn.setIconSize(QSize(16, 16))
    btn.setToolTip(tooltip)
    btn.setCheckable(checkable)
    btn.setAutoRaise(True)
    btn.setCursor(Qt.CursorShape.PointingHandCursor)
    btn.setFixedSize(26, 26)
    return btn


def _label(text: str, name: str) -> QLabel:
    lbl = QLabel(text)
    lbl.setObjectName(name)
    return lbl


def page_title(text: str) -> QLabel:
    return _label(text, "PageTitle")


def card_title(text: str) -> QLabel:
    return _label(text, "CardTitle")


def muted_label(text: str, wrap: bool = True) -> QLabel:
    lbl = _label(text, "Muted")
    lbl.setWordWrap(wrap)
    return lbl


def make_card() -> QFrame:
    card = QFrame()
    card.setObjectName("Card")
    return card


def make_table(headers: list[str], stretch_col: int, right_cols: tuple[int, ...] = ()) -> QTableWidget:
    table = QTableWidget(0, len(headers))
    table.setHorizontalHeaderLabels(headers)
    table.verticalHeader().setVisible(False)
    table.setShowGrid(False)
    table.setFocusPolicy(Qt.FocusPolicy.NoFocus)
    table.setSelectionMode(QTableWidget.SelectionMode.NoSelection)
    table.setEditTriggers(QTableWidget.EditTrigger.NoEditTriggers)
    table.setVerticalScrollMode(QTableWidget.ScrollMode.ScrollPerPixel)
    header = table.horizontalHeader()
    header.setHighlightSections(False)
    for col in range(len(headers)):
        mode = QHeaderView.ResizeMode.Stretch if col == stretch_col else QHeaderView.ResizeMode.ResizeToContents
        header.setSectionResizeMode(col, mode)
        if col in right_cols:
            item = table.horizontalHeaderItem(col)
            item.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
    table.verticalHeader().setDefaultSectionSize(36)
    return table


def fit_table_height(table: QTableWidget, max_rows: int = 12) -> None:
    rows = min(table.rowCount(), max_rows)
    header_h = table.horizontalHeader().height() if table.horizontalHeader().isVisible() else 0
    height = header_h + sum(table.rowHeight(r) for r in range(rows)) + 4
    table.setFixedHeight(max(height, header_h + 8))


def open_in_explorer(path: str) -> bool:
    p = Path(path)
    try:
        if p.is_file():
            subprocess.Popen(["explorer", "/select,", str(p)])
            return True
        target = p if p.is_dir() else p.parent
        if target.is_dir():
            os.startfile(str(target))
            return True
    except OSError:
        pass
    return False


class Toast(QLabel):
    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self.setObjectName("Toast")
        self.hide()

    @staticmethod
    def show_message(anchor: QWidget, text: str, ms: int = 1200) -> None:
        win = anchor.window()
        toast = win.findChild(Toast) or Toast(win)
        toast.setText(text)
        toast.adjustSize()
        toast.move(max(8, win.width() - toast.width() - 24), max(8, win.height() - toast.height() - 24))
        toast.raise_()
        toast.show()
        QTimer.singleShot(ms, toast.hide)


class PathCell(QWidget):
    """Elided path + copy + open-in-Explorer buttons; full path in the tooltip."""

    def __init__(self, path: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.path = path
        lay = QHBoxLayout(self)
        lay.setContentsMargins(8, 0, 4, 0)
        lay.setSpacing(2)
        self.label = QLabel()
        self.label.setObjectName("PathText")
        self.label.setToolTip(path)
        self.label.setMinimumWidth(40)
        self.label.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        self.copy_button = icon_button("copy", "Sao chép đường dẫn")
        self.copy_button.clicked.connect(self.copy_to_clipboard)
        self.open_button = icon_button("folder", "Mở trong Explorer")
        self.open_button.clicked.connect(lambda: open_in_explorer(self.path))
        lay.addWidget(self.label, 1)
        lay.addWidget(self.copy_button)
        lay.addWidget(self.open_button)
        self._update_elide()

    def copy_to_clipboard(self) -> None:
        QGuiApplication.clipboard().setText(self.path)
        Toast.show_message(self, "Đã sao chép đường dẫn")

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        self._update_elide()

    def _update_elide(self) -> None:
        width = max(40, self.label.width())
        self.label.setText(self.label.fontMetrics().elidedText(self.path, Qt.TextElideMode.ElideMiddle, width))


@dataclass
class PathValue:
    path: str


@dataclass
class TagValue:
    text: str
    warning: bool = False
    tooltip: str = ""


@dataclass
class TableRow:
    simple: tuple[str, str, str]  # name, summary, size
    detail: list = field(default_factory=list)  # str | PathValue | TagValue | QWidget
    sort_key: int = 0


def _tag_widget(value: TagValue) -> QWidget:
    wrap = QWidget()
    lay = QHBoxLayout(wrap)
    lay.setContentsMargins(8, 0, 8, 0)
    tag = QLabel(value.text)
    tag.setObjectName("TagWarning" if value.warning else "Tag")
    if value.tooltip:
        tag.setToolTip(value.tooltip)
    lay.addWidget(tag)
    lay.addStretch(1)
    return wrap


def set_cell(table: QTableWidget, row: int, col: int, value, right: bool = False) -> None:
    if isinstance(value, PathValue):
        item = QTableWidgetItem(value.path)  # keeps text for sorting/tests; hidden behind the widget
        item.setForeground(QColor(0, 0, 0, 0))
        table.setItem(row, col, item)
        table.setCellWidget(row, col, PathCell(value.path))
    elif isinstance(value, TagValue):
        table.setItem(row, col, QTableWidgetItem(""))
        table.setCellWidget(row, col, _tag_widget(value))
    elif isinstance(value, QWidget):
        table.setCellWidget(row, col, value)
    else:
        item = QTableWidgetItem(str(value))
        if right:
            item.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
        table.setItem(row, col, item)


class ExpandableTableCard(QFrame):
    """Card with a compact list (default) and a detailed table behind an eye toggle."""

    def __init__(self, title: str, detail_headers: list[str], right_detail_cols: tuple[int, ...] = (), parent=None):
        super().__init__(parent)
        self.setObjectName("Card")
        self._right_cols = right_detail_cols
        lay = QVBoxLayout(self)
        lay.setContentsMargins(14, 10, 14, 10)
        lay.setSpacing(6)

        head = QHBoxLayout()
        self.title_label = card_title(title)
        head.addWidget(self.title_label)
        head.addStretch(1)
        self.header_actions = QHBoxLayout()
        self.header_actions.setSpacing(6)
        head.addLayout(self.header_actions)
        self.eye_button = icon_button("eye", "Xem chi tiết", checkable=True)
        self.eye_button.toggled.connect(self.set_expanded)
        head.addWidget(self.eye_button)
        lay.addLayout(head)

        self.empty_label = muted_label("")
        lay.addWidget(self.empty_label)

        self.stack = QStackedWidget()
        self.simple_table = make_table(["", "", ""], stretch_col=1, right_cols=(2,))
        self.simple_table.horizontalHeader().setVisible(False)
        self.detail_table = make_table(detail_headers, stretch_col=1, right_cols=right_detail_cols)
        self.stack.addWidget(self.simple_table)
        self.stack.addWidget(self.detail_table)
        lay.addWidget(self.stack)

    def set_empty_text(self, text: str) -> None:
        self.empty_label.setText(text)

    def set_rows(self, rows: list[TableRow]) -> None:
        empty = not rows
        self.empty_label.setHidden(not empty)
        self.stack.setHidden(empty)
        self.eye_button.setHidden(empty)
        self.simple_table.setRowCount(len(rows))
        self.detail_table.setRowCount(len(rows))
        for r, row in enumerate(rows):
            name, summary, size = row.simple
            set_cell(self.simple_table, r, 0, name)
            item = QTableWidgetItem(summary)
            item.setForeground(QColor("#616161"))
            self.simple_table.setItem(r, 1, item)
            set_cell(self.simple_table, r, 2, size, right=True)
            for c, value in enumerate(row.detail):
                set_cell(self.detail_table, r, c, value, right=c in self._right_cols)
        fit_table_height(self.simple_table)
        fit_table_height(self.detail_table)

    def is_expanded(self) -> bool:
        return self.stack.currentIndex() == 1

    def set_expanded(self, on: bool) -> None:
        self.stack.setCurrentIndex(1 if on else 0)
        if self.eye_button.isChecked() != on:
            self.eye_button.blockSignals(True)
            self.eye_button.setChecked(on)
            self.eye_button.blockSignals(False)
        self.eye_button.setToolTip("Ẩn chi tiết" if on else "Xem chi tiết")
