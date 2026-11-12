from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import QCheckBox, QHBoxLayout, QPushButton, QVBoxLayout, QWidget

from greenc.core.smart_analyzer import AppAnalysisProposal
from greenc.ui import format_size
from greenc.ui.widgets import (
    PathValue,
    TagValue,
    fit_widget_columns,
    card_title,
    make_card,
    make_table,
    muted_label,
    page_title,
    set_cell,
)


class SmartCleanupTab(QWidget):
    """Dọn dẹp: analysis results with checkboxes and one primary action."""

    scanAppsRequested = Signal()
    executeCleanupRequested = Signal(list)  # list[AppAnalysisProposal]

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._proposals: list[AppAnalysisProposal] = []
        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        head = QHBoxLayout()
        head.addWidget(page_title("Dọn dẹp"))
        head.addStretch(1)
        self.btn_scan_apps = QPushButton("Phân tích")
        self.btn_scan_apps.clicked.connect(self.scanAppsRequested.emit)
        self.btn_clean_now = QPushButton("Dọn dẹp")
        self.btn_clean_now.setObjectName("PrimaryButton")
        self.btn_clean_now.setEnabled(False)
        self.btn_clean_now.clicked.connect(self._on_clean_clicked)
        head.addWidget(self.btn_scan_apps)
        head.addWidget(self.btn_clean_now)
        lay.addLayout(head)

        self.description = muted_label("")
        lay.addWidget(self.description)
        self.set_retention_days(7)

        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        self.proposal_summary_label = card_title("Chưa có kết quả phân tích")
        cl.addWidget(self.proposal_summary_label)
        self.table = make_table(
            ["", "Ứng dụng", "Vị trí", "Đang dùng", "Giải phóng", "Kiểm tra"], stretch_col=2, right_cols=(3, 4)
        )
        cl.addWidget(self.table)
        lay.addWidget(card, 1)

    def set_retention_days(self, days: int) -> None:
        self.description.setText(
            f"Nén file cũ không dùng tới vào Kho dự phòng. Sau {days} ngày gói nén tự xóa; "
            "trong thời gian đó có thể khôi phục bất cứ lúc nào."
        )

    def set_proposals(self, proposals: list[AppAnalysisProposal]) -> None:
        self._proposals = list(proposals)
        self.table.setRowCount(len(self._proposals))
        for row, prop in enumerate(self._proposals):
            chk = QCheckBox()
            chk.setChecked(prop.selected)
            chk.toggled.connect(lambda checked, idx=row: self._on_row_toggled(idx, checked))
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(10, 0, 4, 0)
            h.setAlignment(Qt.AlignmentFlag.AlignCenter)
            h.addWidget(chk)
            self.table.setCellWidget(row, 0, wrap)
            set_cell(self.table, row, 1, prop.app_name)
            set_cell(self.table, row, 2, PathValue(prop.target_paths[0] if prop.target_paths else ""))
            set_cell(self.table, row, 3, format_size(prop.total_physical_bytes), right=True)
            set_cell(self.table, row, 4, format_size(prop.reclaimable_bytes), right=True)
            set_cell(self.table, row, 5, TagValue(prop.probe_strategy, warning=not prop.probe_passed, tooltip=prop.probe_detail))
        fit_widget_columns(self.table)
        self._refresh_summary()

    def _refresh_summary(self) -> None:
        total = sum(p.reclaimable_bytes for p in self._proposals if p.selected)
        if not self._proposals:
            self.proposal_summary_label.setText("Không có ứng dụng nào cần dọn")
        else:
            self.proposal_summary_label.setText(
                f"{len(self._proposals)} ứng dụng · có thể giải phóng {format_size(total)}"
            )
        self.btn_clean_now.setEnabled(total > 0)
        self.btn_clean_now.setText(f"Dọn dẹp {format_size(total)}" if total > 0 else "Dọn dẹp")

    def _on_row_toggled(self, row: int, checked: bool) -> None:
        if 0 <= row < len(self._proposals):
            self._proposals[row].selected = checked
            self._refresh_summary()

    def _on_clean_clicked(self) -> None:
        selected = [p for p in self._proposals if p.selected]
        if selected:
            self.executeCleanupRequested.emit(selected)
