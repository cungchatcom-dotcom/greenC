from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QGroupBox,
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
    Tab 1:
    - Top: Interactive Daily Bar Chart (`DailyBarChartWidget`).
    - Bottom Left: Which folders grew on the selected day & Why (Reason classification).
    - Bottom Right: Which Application (.exe) or Windows Service caused the growth,
      with a 1-click button to set a Soft Rolling Quota for that application.
    """

    requestCreateQuota = Signal(str, str, str, list)  # app_name, process_name, service_name, paths

    def __init__(self, db: DiskGuardDB, parent: QWidget | None = None):
        super().__init__(parent)
        self.db = db
        self._current_drive = "C:\\"
        self._selected_date = ""

        root_layout = QVBoxLayout(self)
        root_layout.setContentsMargins(8, 8, 8, 8)
        root_layout.setSpacing(10)

        self.chart = DailyBarChartWidget(self)
        self.chart.daySelected.connect(self._on_day_selected)
        root_layout.addWidget(self.chart)

        self.day_banner = QLabel("📅 Chi tiết biến động ngày: (Chọn một cột ngày trên biểu đồ)")
        self.day_banner.setStyleSheet(
            "font-size: 13px; font-weight: bold; color: #38BDF8; padding: 4px;"
        )
        root_layout.addWidget(self.day_banner)

        splitter = QSplitter(Qt.Orientation.Horizontal, self)

        # Left group: Folders & Reasons
        left_group = QGroupBox("1. Chỗ nào đang làm đầy ổ cứng? Nguyên nhân vì sao?")
        left_layout = QVBoxLayout(left_group)
        self.folder_table = QTableWidget(0, 4)
        self.folder_table.setHorizontalHeaderLabels(
            [
                "Thư mục phát sinh dung lượng",
                "Mức tăng trong ngày",
                "Mã Nguyên nhân",
                "Giải thích Nguyên nhân & Số file",
            ]
        )
        self.folder_table.horizontalHeader().setSectionResizeMode(
            0, QHeaderView.ResizeMode.Stretch
        )
        self.folder_table.horizontalHeader().setSectionResizeMode(
            1, QHeaderView.ResizeMode.ResizeToContents
        )
        self.folder_table.horizontalHeader().setSectionResizeMode(
            2, QHeaderView.ResizeMode.ResizeToContents
        )
        self.folder_table.horizontalHeader().setSectionResizeMode(
            3, QHeaderView.ResizeMode.Stretch
        )
        left_layout.addWidget(self.folder_table)

        # Right group: Applications & Windows Services
        right_group = QGroupBox("2. Do Ứng dụng hay Windows Service nào? (Bấm Đặt Quota để khống chế)")
        right_layout = QVBoxLayout(right_group)
        self.actor_table = QTableWidget(0, 5)
        self.actor_table.setHorizontalHeaderLabels(
            [
                "Tiến trình (.exe)",
                "Ứng dụng / Windows Service",
                "Dung lượng đã ghi",
                "Nguyên nhân chính",
                "Khống chế Quota",
            ]
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            0, QHeaderView.ResizeMode.ResizeToContents
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            1, QHeaderView.ResizeMode.Stretch
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            2, QHeaderView.ResizeMode.ResizeToContents
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            3, QHeaderView.ResizeMode.Stretch
        )
        self.actor_table.horizontalHeader().setSectionResizeMode(
            4, QHeaderView.ResizeMode.ResizeToContents
        )
        right_layout.addWidget(self.actor_table)

        splitter.addWidget(left_group)
        splitter.addWidget(right_group)
        splitter.setSizes([540, 560])
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
        # Pick the most recent day with events or the last snapshot day
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
            f"📅 Chi tiết biến động ổ {self._current_drive} vào ngày {date_str} — "
            f"Tổng dung lượng phát sinh ghi nhận: +{format_bytes(total_day_delta)} "
            f"({len(folders)} nhóm thư mục / {len(actors)} ứng dụng & service)"
        )

        # Populate folder breakdown table
        self.folder_table.setRowCount(len(folders))
        for row_idx, f in enumerate(folders):
            p_item = QTableWidgetItem(f["folder_path"])
            delta_item = QTableWidgetItem(f"+{format_bytes(f['total_delta_bytes'])}")
            delta_item.setForeground(QColor("#FDBA74"))
            code_item = QTableWidgetItem(f["reason_code"])
            code_item.setForeground(QColor("#38BDF8"))
            detail_item = QTableWidgetItem(f["reason_detail"])

            self.folder_table.setItem(row_idx, 0, p_item)
            self.folder_table.setItem(row_idx, 1, delta_item)
            self.folder_table.setItem(row_idx, 2, code_item)
            self.folder_table.setItem(row_idx, 3, detail_item)

        # Populate actor (application / Windows Service) table
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

            self.actor_table.setItem(row_idx, 0, QTableWidgetItem(proc_name))
            self.actor_table.setItem(row_idx, 1, QTableWidgetItem(label_svc))

            d_item = QTableWidgetItem(f"+{format_bytes(a['total_delta_bytes'])}")
            d_item.setForeground(QColor("#F87171"))
            self.actor_table.setItem(row_idx, 2, d_item)
            self.actor_table.setItem(
                row_idx, 3, QTableWidgetItem(a.get("primary_reason") or "")
            )

            btn = QPushButton("⚡ Đặt Quota")
            btn.setStyleSheet(
                "background-color: #0284C7; color: white; font-weight: bold; padding: 4px 8px; border-radius: 4px;"
            )
            btn.clicked.connect(
                lambda _=False, app_n=svc_disp, pr=proc_name, sv=svc_name, fl=folders_list: self.requestCreateQuota.emit(
                    app_n, pr, sv, fl
                )
            )
            container = QWidget()
            h = QHBoxLayout(container)
            h.setContentsMargins(2, 2, 2, 2)
            h.addWidget(btn)
            self.actor_table.setCellWidget(row_idx, 4, container)
