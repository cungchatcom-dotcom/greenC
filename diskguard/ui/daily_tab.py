from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QPushButton,
    QSplitter,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from diskguard.models import DailySnapshot
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes
from diskguard.ui.chart_widget import DailyBarChartWidget


class DailyMonitorTab(QWidget):
    """
    Clean Apple/Google-style Overview page:
    - Top: Interactive Daily Bar Chart.
    - Bottom Left: Folders that grew on the selected day & why.
    - Bottom Right: Applications & Windows Services responsible for the growth.
    """

    requestCreateQuota = Signal(str, str, str, list)

    def __init__(self, db: DiskGuardDB, parent: QWidget | None = None):
        super().__init__(parent)
        self.db = db
        self._current_drive = "C:\\"
        self._selected_date = ""

        root_layout = QVBoxLayout(self)
        root_layout.setContentsMargins(0, 0, 0, 0)
        root_layout.setSpacing(12)

        self.chart = DailyBarChartWidget(self)
        self.chart.daySelected.connect(self._on_day_selected)
        root_layout.addWidget(self.chart)

        self.day_banner = QLabel("Chi tiết biến động theo ngày")
        self.day_banner.setStyleSheet(
            "font-size: 13px; font-weight: 600; color: #1D1D1F; padding: 2px 4px;"
        )
        root_layout.addWidget(self.day_banner)

        splitter = QSplitter(Qt.Orientation.Horizontal, self)
        splitter.setHandleWidth(12)

        # Left card: Folders & Reasons
        left_card = QFrame()
        left_card.setObjectName("SurfaceCard")
        left_layout = QVBoxLayout(left_card)
        left_layout.setContentsMargins(14, 12, 14, 12)
        left_title = QLabel("Vị trí tăng dung lượng & Nguyên nhân")
        left_title.setStyleSheet("font-size: 13px; font-weight: 600; color: #1D1D1F;")
        left_layout.addWidget(left_title)

        self.folder_table = QTableWidget(0, 3)
        self.folder_table.setHorizontalHeaderLabels(
            [
                "Thư mục",
                "Mức tăng",
                "Nguyên nhân ghi nhận",
            ]
        )
        self.folder_table.verticalHeader().setVisible(False)
        self.folder_table.setShowGrid(False)
        self.folder_table.setAlternatingRowColors(True)
        self.folder_table.horizontalHeader().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch
        )
        self.folder_table.horizontalHeader().setSectionResizeMode(
            1, QHeaderView.ResizeMode.ResizeToContents
        )
        self.folder_table.horizontalHeader().setSectionResizeMode(
            2, QHeaderView.ResizeMode.Stretch
        )
        left_layout.addWidget(self.folder_table)

        # Right card: Applications & Services
        right_card = QFrame()
        right_card.setObjectName("SurfaceCard")
        right_layout = QVBoxLayout(right_card)
        right_layout.setContentsMargins(14, 12, 14, 12)
        right_title = QLabel("Ứng dụng & Dịch vụ hệ thống (Services)")
        right_title.setStyleSheet("font-size: 13px; font-weight: 600; color: #1D1D1F;")
        right_layout.addWidget(right_title)

        self.actor_table = QTableWidget(0, 4)
        self.actor_table.setHorizontalHeaderLabels(
            [
                "Ứng dụng / Service",
                "Tiến trình",
                "Dung lượng ghi",
                "Giới hạn",
            ]
        )
        self.actor_table.verticalHeader().setVisible(False)
        self.actor_table.setShowGrid(False)
        self.actor_table.setAlternatingRowColors(True)
        self.actor_table.horizontalHeader().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            1, QHeaderView.ResizeMode.ResizeToContents
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            2, QHeaderView.ResizeMode.ResizeToContents
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            3, QHeaderView.ResizeMode.ResizeToContents
        )
        right_layout.addWidget(self.actor_table)

        splitter.addWidget(left_card)
        splitter.addWidget(right_card)
        splitter.setSizes([580, 520])
        root_layout.addWidget(splitter, stretch=1)

    def load_drive_data(
        self, drive: str, snapshots: list[DailySnapshot] | None = None
    ) -> None:
        self._current_drive = drive
        snaps = (
            snapshots
            if snapshots is not None
            else self.db.get_daily_snapshots(drive, limit_days=14)
        )
        selected = self._selected_date
        if snaps and (
            not selected or all(s.snapshot_date != selected for s in snaps)
        ):
            active_days = [s.snapshot_date for s in snaps if s.net_change_bytes > 0]
            selected = active_days[-1] if active_days else snaps[-1].snapshot_date
        self._selected_date = selected
        self.chart.set_snapshots(snaps, selected_date=selected)
        if selected:
            self._populate_day_tables(selected)

    def _on_day_selected(self, date_str: str) -> None:
        self._selected_date = date_str
        self._populate_day_tables(date_str)

    def _populate_day_tables(self, date_str: str) -> None:
        folders = self.db.get_day_folder_breakdown(self._current_drive, date_str)
        actors = self.db.get_day_actor_breakdown(self._current_drive, date_str)
        total_day_delta = sum(int(f["total_delta_bytes"]) for f in folders)

        self.day_banner.setText(
            f"Ngày {date_str} trên ổ {self._current_drive} — "
            f"Tổng mức tăng ghi nhận: +{format_bytes(total_day_delta)} "
            f"({len(folders)} thư mục, {len(actors)} tác nhân)"
        )

        self.folder_table.setRowCount(len(folders))
        for row_idx, f in enumerate(folders):
            p_item = QTableWidgetItem(f["folder_path"])
            delta_item = QTableWidgetItem(f"+{format_bytes(f['total_delta_bytes'])}")
            delta_item.setForeground(QColor("#B45309"))
            detail_item = QTableWidgetItem(f["reason_detail"])

            self.folder_table.setItem(row_idx, 0, p_item)
            self.folder_table.setItem(row_idx, 1, delta_item)
            self.folder_table.setItem(row_idx, 2, detail_item)

        self.actor_table.setRowCount(len(actors))
        for row_idx, a in enumerate(actors):
            proc_name = a["process_name"] or "Unknown.exe"
            svc_name = a["service_name"] or ""
            svc_disp = a["service_display_name"] or proc_name
            label_svc = f"{svc_disp} ({svc_name})" if svc_name else svc_disp
            folders_list = [
                p.strip()
                for p in (a.get("folders") or "").split(",")
                if p.strip()
            ]

            self.actor_table.setItem(row_idx, 0, QTableWidgetItem(label_svc))
            self.actor_table.setItem(row_idx, 1, QTableWidgetItem(proc_name))

            d_item = QTableWidgetItem(f"+{format_bytes(a['total_delta_bytes'])}")
            d_item.setForeground(QColor("#0066CC"))
            self.actor_table.setItem(row_idx, 2, d_item)

            btn = QPushButton("Thiết lập quy tắc")
            btn.setObjectName("SecondaryButton")
            btn.clicked.connect(
                lambda _=False, app_n=svc_disp, pr=proc_name, sv=svc_name, fl=folders_list: self.requestCreateQuota.emit(
                    app_n, pr, sv, fl
                )
            )
            container = QWidget()
            h = QHBoxLayout(container)
            h.setContentsMargins(2, 2, 2, 2)
            h.addWidget(btn)
            self.actor_table.setCellWidget(row_idx, 3, container)
