from pathlib import Path

from PySide6.QtCore import Signal
from PySide6.QtWidgets import (
    QCheckBox,
    QFrame,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from greenc.core.relocation.base import RelocationCandidate
from greenc.i18n import N_, tr
from greenc.models import RelocationRecord
from greenc.ui import format_size
from greenc.ui.widgets import (
    PathValue,
    TagValue,
    card_title,
    fit_table_height,
    fit_widget_columns,
    make_card,
    make_table,
    muted_label,
    page_title,
    set_cell,
)

STRATEGY_TEXT = {
    "known_folder": N_("Vị trí của Windows"),
    "tool_config": N_("Cấu hình công cụ"),
    "guided": N_("Tùy chọn của ứng dụng"),
    "junction": N_("Liên kết thư mục"),
}


def _name_cell(name: str, how: str) -> QWidget:
    w = QWidget()
    v = QVBoxLayout(w)
    v.setContentsMargins(8, 4, 8, 4)
    v.setSpacing(0)
    title = QLabel(name)
    title.setStyleSheet("font-weight: 600;")
    v.addWidget(title)
    v.addWidget(muted_label(tr(how), wrap=False))
    return w


class RelocationPage(QWidget):
    relocateRequested = Signal(object)  # RelocationCandidate
    guideRequested = Signal(object)  # RelocationCandidate (guided strategy)
    undoRequested = Signal(object)  # RelocationRecord
    refreshRequested = Signal()

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.action_buttons: list[QPushButton] = []
        self.undo_buttons: list[QPushButton] = []
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        scroll = QScrollArea()
        scroll.setObjectName("PageScroll")
        scroll.setWidgetResizable(True)
        body = QWidget()
        body.setObjectName("PageBody")
        scroll.setWidget(body)
        outer.addWidget(scroll)
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        head = QHBoxLayout()
        head.addWidget(page_title(tr("Di chuyển dữ liệu")))
        head.addStretch(1)
        self.target_label = muted_label("", wrap=False)
        head.addWidget(self.target_label)
        self.btn_refresh = QPushButton(tr("Phân tích"))
        self.btn_refresh.clicked.connect(self.refreshRequested.emit)
        head.addWidget(self.btn_refresh)
        lay.addLayout(head)

        self.banner = QFrame()
        self.banner.setObjectName("Banner")
        bl = QHBoxLayout(self.banner)
        bl.setContentsMargins(12, 8, 12, 8)
        self.banner_label = QLabel("")
        self.banner_label.setWordWrap(True)
        bl.addWidget(self.banner_label)
        self.banner.hide()
        lay.addWidget(self.banner)

        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        cl.addWidget(card_title(tr("Có thể chuyển")))
        self.empty_label = muted_label(tr("Bấm Phân tích để tìm dữ liệu ứng dụng đang nằm trên ổ C:."))
        cl.addWidget(self.empty_label)
        self.table = make_table([tr("Ứng dụng"), tr("Vị trí"), tr("Dung lượng"), ""], stretch_col=1, right_cols=(2,))
        self.table.verticalHeader().setDefaultSectionSize(48)
        self.table.hide()
        cl.addWidget(self.table)
        self.advanced_toggle = QCheckBox(tr("Hiện thư mục nâng cao"))
        self.advanced_toggle.toggled.connect(lambda on: self.advanced_table.setHidden(not on))
        self.advanced_toggle.hide()
        cl.addWidget(self.advanced_toggle)
        self.advanced_table = make_table([tr("Thư mục"), tr("Vị trí"), tr("Dung lượng"), ""], stretch_col=1, right_cols=(2,))
        self.advanced_table.verticalHeader().setDefaultSectionSize(48)
        self.advanced_table.hide()
        cl.addWidget(self.advanced_table)
        lay.addWidget(card)

        done = make_card()
        dl = QVBoxLayout(done)
        dl.setContentsMargins(14, 10, 14, 10)
        dl.addWidget(card_title(tr("Đã chuyển")))
        self.done_empty = muted_label(tr("Chưa chuyển mục nào."))
        dl.addWidget(self.done_empty)
        self.done_table = make_table([tr("Ứng dụng"), tr("Vị trí mới"), tr("Cách chuyển"), tr("Trạng thái"), ""], stretch_col=1)
        self.done_table.hide()
        dl.addWidget(self.done_table)
        lay.addWidget(done)
        lay.addStretch(1)

    def set_target(self, root: Path | None, free_bytes: int | None) -> None:
        if root is None:
            self.target_label.setText("")
        else:
            target = f"{root}\\"
            if free_bytes is not None:
                text = tr("Đích: {target} · Còn trống {free}", target=target, free=format_size(free_bytes))
            else:
                text = tr("Đích: {target}", target=target)
            self.target_label.setText(text)

    def set_disabled_reason(self, text: str | None) -> None:
        self.banner_label.setText(text or "")
        self.banner.setHidden(not text)
        for btn in self.action_buttons:
            btn.setEnabled(not text)

    def _fill(self, table, cands: list[RelocationCandidate]) -> None:
        table.setRowCount(len(cands))
        for row, cand in enumerate(cands):
            table.setCellWidget(row, 0, _name_cell(cand.name, cand.how))
            set_cell(table, row, 1, PathValue(cand.source_path))
            set_cell(table, row, 2, format_size(cand.size_bytes), right=True)
            guided = cand.strategy.kind == "guided"
            btn = QPushButton(tr("Hướng dẫn") if guided else tr("Chuyển"))
            btn.setObjectName("PrimaryButton")
            btn.setEnabled(self.banner.isHidden())
            signal = self.guideRequested if guided else self.relocateRequested
            btn.clicked.connect(lambda _=False, c=cand, s=signal: s.emit(c))
            self.action_buttons.append(btn)
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 8, 2)
            h.addWidget(btn)
            table.setCellWidget(row, 3, wrap)
        fit_widget_columns(table)
        fit_table_height(table, max_rows=10)

    def set_candidates(self, cands: list[RelocationCandidate]) -> None:
        self.action_buttons = []
        main = [c for c in cands if not c.advanced]
        adv = [c for c in cands if c.advanced]
        self._fill(self.table, main)
        self._fill(self.advanced_table, adv)
        self.empty_label.setText(tr("Không có dữ liệu ứng dụng lớn nào trên ổ C:."))
        self.empty_label.setHidden(bool(cands))
        self.table.setHidden(not main)
        self.advanced_toggle.setHidden(not adv)
        self.advanced_table.setHidden(not (adv and self.advanced_toggle.isChecked()))

    def set_records(self, records: list[RelocationRecord]) -> None:
        self.undo_buttons = []
        self.done_table.setRowCount(len(records))
        for row, rec in enumerate(records):
            set_cell(self.done_table, row, 0, rec.app_name)
            set_cell(self.done_table, row, 1, PathValue(rec.target_path))
            set_cell(self.done_table, row, 2, tr(STRATEGY_TEXT.get(rec.strategy, rec.strategy)))
            set_cell(self.done_table, row, 3, TagValue(tr(rec.detail) if rec.detail else tr("Hoạt động bình thường"), warning=bool(rec.detail)))
            btn = QPushButton(tr("Hoàn tác"))
            btn.setEnabled(rec.strategy != "guided")
            if rec.strategy == "guided":
                btn.setToolTip(tr("Đổi lại vị trí lưu trong Cài đặt của ứng dụng"))
            btn.clicked.connect(lambda _=False, r=rec: self.undoRequested.emit(r))
            self.undo_buttons.append(btn)
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 8, 2)
            h.addWidget(btn)
            self.done_table.setCellWidget(row, 4, wrap)
        fit_widget_columns(self.done_table)
        fit_table_height(self.done_table, max_rows=10)
        self.done_table.setHidden(not records)
        self.done_empty.setHidden(bool(records))
