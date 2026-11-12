import shutil

from PySide6.QtCore import Signal
from PySide6.QtWidgets import (
    QComboBox,
    QHBoxLayout,
    QProgressBar,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.daily_tab import DailyMonitorTab
from diskguard.ui.theme import repolish
from diskguard.ui.widgets import (
    ExpandableTableCard,
    PathValue,
    TableRow,
    TagValue,
    card_title,
    make_card,
    muted_label,
    page_title,
)


class OverviewPage(QWidget):
    analyzeRequested = Signal()
    cleanupRequested = Signal()
    refreshHistoryRequested = Signal()
    driveChanged = Signal(str)

    def __init__(self, db: DiskGuardDB, drives: list[str], parent: QWidget | None = None):
        super().__init__(parent)
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
        head.addWidget(page_title("Tổng quan"))
        head.addStretch(1)
        self.btn_analyze = QPushButton("Phân tích")
        self.btn_analyze.clicked.connect(self.analyzeRequested.emit)
        self.btn_clean = QPushButton("Dọn dẹp")
        self.btn_clean.setObjectName("PrimaryButton")
        self.btn_clean.clicked.connect(self.cleanupRequested.emit)
        head.addWidget(self.btn_analyze)
        head.addWidget(self.btn_clean)
        lay.addLayout(head)

        drive_card = make_card()
        dl = QVBoxLayout(drive_card)
        dl.setContentsMargins(14, 12, 14, 12)
        dl.setSpacing(8)
        top = QHBoxLayout()
        top.addWidget(card_title("Ổ đĩa"))
        self.drive_combo = QComboBox()
        self.drive_combo.addItems(drives)
        self.drive_combo.setMinimumWidth(80)
        self.drive_combo.currentTextChanged.connect(self._on_drive_changed)
        top.addWidget(self.drive_combo)
        top.addStretch(1)
        self.drive_stats_label = muted_label("", wrap=False)
        top.addWidget(self.drive_stats_label)
        dl.addLayout(top)
        self.drive_usage_bar = QProgressBar()
        self.drive_usage_bar.setRange(0, 100)
        self.drive_usage_bar.setTextVisible(False)
        dl.addWidget(self.drive_usage_bar)
        lay.addWidget(drive_card)

        self.freeable_card = ExpandableTableCard(
            "Có thể giải phóng", ["Ứng dụng", "Vị trí", "Dung lượng", "Giải phóng", "Đề xuất"], right_detail_cols=(2, 3)
        )
        self.freeable_card.set_empty_text(
            "Chưa phân tích. Bấm Phân tích để tìm dữ liệu có thể dọn hoặc chuyển sang ổ khác."
        )
        self.freeable_card.set_rows([])
        lay.addWidget(self.freeable_card)

        hist = make_card()
        hl = QVBoxLayout(hist)
        hl.setContentsMargins(14, 12, 14, 12)
        hh = QHBoxLayout()
        hh.addWidget(card_title("Biến động theo ngày"))
        hh.addStretch(1)
        self.btn_refresh_history = QPushButton("Làm mới")
        self.btn_refresh_history.clicked.connect(self.refreshHistoryRequested.emit)
        hh.addWidget(self.btn_refresh_history)
        hl.addLayout(hh)
        self.daily_tab = DailyMonitorTab(db, hist)
        hl.addWidget(self.daily_tab)
        lay.addWidget(hist)
        lay.addStretch(1)

        self.update_drive_usage(self.drive_combo.currentText())

    def _on_drive_changed(self, drive: str) -> None:
        self.update_drive_usage(drive)
        self.driveChanged.emit(drive)

    def update_drive_usage(self, drive: str) -> None:
        if not drive:
            return
        try:
            u = shutil.disk_usage(drive)
        except OSError:
            return
        pct = int(u.used * 100 / max(1, u.total))
        self.drive_usage_bar.setValue(pct)
        self.drive_usage_bar.setObjectName("DangerBar" if pct >= 90 else "")
        repolish(self.drive_usage_bar)
        self.drive_stats_label.setText(f"Còn trống {format_size(u.free)} / {format_size(u.total)}")

    def set_freeable(self, proposals, candidates, target_label: str) -> None:
        rows: list[TableRow] = []
        for p in proposals:
            rows.append(TableRow(
                (p.app_name, "Dọn file cũ", format_size(p.reclaimable_bytes)),
                [p.app_name, PathValue(p.target_paths[0] if p.target_paths else ""),
                 format_size(p.total_physical_bytes), format_size(p.reclaimable_bytes), TagValue("Dọn file cũ")],
                p.reclaimable_bytes,
            ))
        for c in candidates:
            if c.advanced:
                continue
            action = f"Chuyển sang {target_label}"
            rows.append(TableRow(
                (c.name, action, format_size(c.size_bytes)),
                [c.name, PathValue(c.source_path), format_size(c.size_bytes), format_size(c.size_bytes), TagValue(action)],
                c.size_bytes,
            ))
        rows.sort(key=lambda r: r.sort_key, reverse=True)
        self.freeable_card.set_rows(rows)
