import re
from datetime import date

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QHBoxLayout, QPushButton, QVBoxLayout, QWidget

from greenc.i18n import tr
from greenc.models import DailySnapshot
from greenc.storage.database import GreenCDB
from greenc.ui import format_size
from greenc.ui.chart_widget import DailyBarChartWidget
from greenc.ui.widgets import PathValue, card_title, fit_widget_columns, make_table, muted_label, set_cell



_REASON_COUNT = re.compile(r"^(?P<reason>.*) \((?P<count>\d+) files\)$")


def localize_reason(stored: str) -> str:
    """write_events.reason_detail is stored as '<Vietnamese label> (<n> files)'."""
    if not stored:
        return stored
    match = _REASON_COUNT.match(stored)
    if not match:
        return tr(stored)
    return tr("{reason} ({count} tệp)", reason=tr(match["reason"]), count=int(match["count"]))


def localize_actor(stored: str) -> str:
    """service_display_name may be a fixed label or 'Ứng dụng <x>' / 'Thư mục <x>' (see core/attribution.py)."""
    if not stored:
        return stored
    if stored.startswith("Ứng dụng "):
        return tr("Ứng dụng {app}", app=stored[len("Ứng dụng "):])
    if stored.startswith("Thư mục "):
        return tr("Thư mục {folder}", folder=stored[len("Thư mục "):])
    return tr(stored)

class DailyMonitorTab(QWidget):
    """Daily growth chart + what grew and who wrote it on the selected day."""

    requestCreateQuota = Signal(str, str, str, list)

    def __init__(self, db: GreenCDB, parent: QWidget | None = None):
        super().__init__(parent)
        self.db = db
        self._current_drive = "C:\\"
        self._selected_date = ""
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(10)

        self.chart = DailyBarChartWidget(self)
        self.chart.daySelected.connect(self._on_day_selected)
        lay.addWidget(self.chart)

        self.day_banner = muted_label(tr("Chọn một cột để xem chi tiết ngày đó."))
        lay.addWidget(self.day_banner)

        cols = QHBoxLayout()
        cols.setSpacing(16)
        left = QVBoxLayout()
        left.addWidget(card_title(tr("Thư mục tăng dung lượng")))
        self.folder_table = make_table([tr("Thư mục"), tr("Tăng"), tr("Nguyên nhân")], stretch_col=0, right_cols=(1,))
        self.folder_table.setMinimumHeight(220)
        left.addWidget(self.folder_table)
        right = QVBoxLayout()
        right.addWidget(card_title(tr("Ứng dụng và dịch vụ")))
        self.actor_table = make_table([tr("Ứng dụng"), tr("Tiến trình"), tr("Đã ghi"), ""], stretch_col=0, right_cols=(2,))
        self.actor_table.setMinimumHeight(220)
        right.addWidget(self.actor_table)
        cols.addLayout(left, 11)
        cols.addLayout(right, 10)
        lay.addLayout(cols)

    def load_drive_data(self, drive: str, snapshots: list[DailySnapshot] | None = None) -> None:
        self._current_drive = drive
        snaps = snapshots if snapshots is not None else self.db.get_daily_snapshots(drive, limit_days=14)
        selected = self._selected_date
        if snaps and (not selected or all(s.snapshot_date != selected for s in snaps)):
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
        total = sum(int(f["total_delta_bytes"]) for f in folders)
        try:
            day_label = date.fromisoformat(date_str).strftime("%d/%m/%Y")
        except ValueError:
            day_label = date_str
        self.day_banner.setText(
            tr(
                "{day} · ổ {drive} · tăng {size} · {folders} thư mục · {apps} ứng dụng",
                day=day_label, drive=self._current_drive[:2], size=format_size(total),
                folders=len(folders), apps=len(actors),
            )
        )

        self.folder_table.setRowCount(len(folders))
        for row, f in enumerate(folders):
            set_cell(self.folder_table, row, 0, PathValue(f["folder_path"]))
            set_cell(self.folder_table, row, 1, f"+{format_size(f['total_delta_bytes'])}", right=True)
            set_cell(self.folder_table, row, 2, localize_reason(f["reason_detail"]))

        self.actor_table.setRowCount(len(actors))
        for row, a in enumerate(actors):
            proc = a["process_name"] or tr("Không rõ")
            svc = a["service_name"] or ""
            display = localize_actor(a["service_display_name"]) or proc
            label = f"{display} · {svc}" if svc else display
            folders_list = [p.strip() for p in (a.get("folders") or "").split(",") if p.strip()]
            set_cell(self.actor_table, row, 0, label)
            set_cell(self.actor_table, row, 1, proc)
            set_cell(self.actor_table, row, 2, f"+{format_size(a['total_delta_bytes'])}", right=True)
            btn = QPushButton(tr("Tạo quy tắc"))
            btn.clicked.connect(
                lambda _=False, n=display, p=proc, s=svc, fl=folders_list: self.requestCreateQuota.emit(n, p, s, fl)
            )
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 4, 2)
            h.addWidget(btn)
            self.actor_table.setCellWidget(row, 3, wrap)
        fit_widget_columns(self.actor_table)
