from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QCheckBox,
    QFrame,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QPushButton,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from diskguard.core.smart_analyzer import AppAnalysisProposal
from diskguard.ui import format_bytes


class SmartCleanupTab(QWidget):
    """
    Apple/Google-style Smart Analysis & 1-Click Cleanup view:
    1. User clicks 'Quét và Phân tích Ứng dụng' -> scans installed apps on the PC,
       analyzes how they run & store data, and tests a 1-cycle Sandbox probe per app.
    2. Displays clear, verified proposals.
    3. User clicks the single primary button 'Dọn dẹp' -> cleans up old unlocked files
       into the D-day backup vault and silently registers long-term rules in Settings > Rules.
    """

    scanAppsRequested = Signal()
    executeCleanupRequested = Signal(list)  # list[AppAnalysisProposal]

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._proposals: list[AppAnalysisProposal] = []

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(12)

        # Top Action Hero Card
        hero_card = QFrame()
        hero_card.setObjectName("SurfaceCard")
        hero_layout = QHBoxLayout(hero_card)
        hero_layout.setContentsMargins(20, 16, 20, 16)

        text_col = QVBoxLayout()
        self.hero_title = QLabel("Phân tích Thông minh & Tự động Kiểm chứng Ứng dụng")
        self.hero_title.setStyleSheet(
            "font-size: 15px; font-weight: 600; color: #1D1D1F;"
        )
        self.hero_subtitle = QLabel(
            "Hệ thống quét các ứng dụng đang cài đặt trên máy, phân tích cách ứng dụng lưu trữ dữ liệu, "
            "tự động chạy thử 1 chu trình nén & khôi phục trong vùng cách ly (Sandbox) để đảm bảo an toàn tuyệt đối, "
            "sau đó tự động áp dụng quy tắc chạy ngầm lâu dài."
        )
        self.hero_subtitle.setStyleSheet("font-size: 12px; color: #6E6E73;")
        self.hero_subtitle.setWordWrap(True)
        text_col.addWidget(self.hero_title)
        text_col.addWidget(self.hero_subtitle)
        hero_layout.addLayout(text_col, stretch=1)

        btn_col = QHBoxLayout()
        btn_col.setSpacing(10)

        self.btn_scan_apps = QPushButton("Phân tích các ứng dụng trên máy")
        self.btn_scan_apps.setObjectName("SecondaryButton")
        self.btn_scan_apps.setMinimumHeight(36)
        self.btn_scan_apps.clicked.connect(self.scanAppsRequested.emit)

        self.btn_clean_now = QPushButton("Dọn dẹp")
        self.btn_clean_now.setObjectName("PrimaryButton")
        self.btn_clean_now.setMinimumHeight(36)
        self.btn_clean_now.setMinimumWidth(140)
        self.btn_clean_now.setEnabled(False)
        self.btn_clean_now.clicked.connect(self._on_clean_clicked)

        btn_col.addWidget(self.btn_scan_apps)
        btn_col.addWidget(self.btn_clean_now)
        hero_layout.addLayout(btn_col)

        layout.addWidget(hero_card)

        # Proposals Table Card
        table_card = QFrame()
        table_card.setObjectName("SurfaceCard")
        table_layout = QVBoxLayout(table_card)
        table_layout.setContentsMargins(16, 14, 16, 14)

        self.proposal_summary_label = QLabel(
            "Bấm 'Phân tích các ứng dụng trên máy' để hệ thống quét và kiểm thử phương án tối ưu."
        )
        self.proposal_summary_label.setStyleSheet(
            "font-size: 13px; font-weight: 600; color: #1D1D1F;"
        )
        table_layout.addWidget(self.proposal_summary_label)

        self.table = QTableWidget(0, 6)
        self.table.setHorizontalHeaderLabels(
            [
                "Chọn",
                "Ứng dụng",
                "Trạng thái & Cấu trúc lưu trữ",
                "Kết quả thử nghiệm Sandbox",
                "Đang chiếm",
                "Có thể dọn an toàn",
            ]
        )
        self.table.verticalHeader().setVisible(False)
        self.table.setShowGrid(False)
        self.table.setAlternatingRowColors(True)
        self.table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(5, QHeaderView.ResizeMode.ResizeToContents)
        table_layout.addWidget(self.table, stretch=1)

        layout.addWidget(table_card, stretch=1)

    def set_proposals(self, proposals: list[AppAnalysisProposal]) -> None:
        self._proposals = list(proposals)
        total_reclaim = sum(p.reclaimable_bytes for p in self._proposals if p.selected)

        if not self._proposals:
            self.proposal_summary_label.setText(
                "Không phát hiện ứng dụng nào vượt ngưỡng cần dọn dẹp."
            )
            self.btn_clean_now.setEnabled(False)
            self.btn_clean_now.setText("Dọn dẹp")
            self.table.setRowCount(0)
            return

        self.proposal_summary_label.setText(
            f"Phương án đề xuất: Có thể thu hồi an toàn {format_bytes(total_reclaim)} từ {len(self._proposals)} ứng dụng "
            f"(Tất cả đã vượt qua bài kiểm thử chu trình Sandbox 1 vòng)"
        )
        self.btn_clean_now.setEnabled(True)
        self.btn_clean_now.setText(f"Dọn dẹp ({format_bytes(total_reclaim)})")

        self.table.setRowCount(len(self._proposals))
        for row_idx, prop in enumerate(self._proposals):
            chk = QCheckBox()
            chk.setChecked(prop.selected)
            chk.toggled.connect(
                lambda checked, idx=row_idx: self._on_row_toggled(idx, checked)
            )
            chk_wrap = QWidget()
            h = QHBoxLayout(chk_wrap)
            h.setContentsMargins(8, 2, 8, 2)
            h.setAlignment(Qt.AlignmentFlag.AlignCenter)
            h.addWidget(chk)
            self.table.setCellWidget(row_idx, 0, chk_wrap)

            app_item = QTableWidgetItem(f"{prop.app_name}\n({prop.process_name})")
            self.table.setItem(row_idx, 1, app_item)

            storage_txt = f"{prop.storage_summary}\nThư mục: {prop.target_paths[0] if prop.target_paths else ''}"
            self.table.setItem(row_idx, 2, QTableWidgetItem(storage_txt))

            probe_item = QTableWidgetItem(f"{prop.probe_strategy}\n{prop.probe_detail}")
            probe_item.setForeground(QColor("#15803D"))
            self.table.setItem(row_idx, 3, probe_item)

            self.table.setItem(
                row_idx, 4, QTableWidgetItem(format_bytes(prop.total_physical_bytes))
            )

            reclaim_item = QTableWidgetItem(format_bytes(prop.reclaimable_bytes))
            reclaim_item.setForeground(QColor("#0066CC"))
            self.table.setItem(row_idx, 5, reclaim_item)

        self.table.resizeRowsToContents()

    def _on_row_toggled(self, row_idx: int, checked: bool) -> None:
        if 0 <= row_idx < len(self._proposals):
            self._proposals[row_idx].selected = checked
            total_reclaim = sum(
                p.reclaimable_bytes for p in self._proposals if p.selected
            )
            self.btn_clean_now.setEnabled(total_reclaim > 0)
            self.btn_clean_now.setText(
                f"Dọn dẹp ({format_bytes(total_reclaim)})"
                if total_reclaim > 0
                else "Dọn dẹp"
            )

    def _on_clean_clicked(self) -> None:
        selected = [p for p in self._proposals if p.selected]
        if selected:
            self.executeCleanupRequested.emit(selected)

