from PySide6.QtCore import QRectF, Qt, Signal
from PySide6.QtGui import QColor, QFont, QMouseEvent, QPainter, QPen
from PySide6.QtWidgets import QWidget

from diskguard.models import DailySnapshot
from diskguard.ui import format_bytes


class DailyBarChartWidget(QWidget):
    """
    Interactive bar chart showing daily disk usage and daily growth (+MB/GB).
    Clicking any bar emits `daySelected(snapshot_date)` to drill down into
    that day's folders, reasons, and responsible applications/services.
    """

    daySelected = Signal(str)

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setMinimumHeight(235)
        self.setMouseTracking(True)
        self._snapshots: list[DailySnapshot] = []
        self._selected_date: str = ""
        self._hover_index: int = -1
        self._bar_rects: list[tuple[QRectF, str]] = []

    def set_snapshots(
        self, snapshots: list[DailySnapshot], selected_date: str | None = None
    ) -> None:
        self._snapshots = list(snapshots)
        if selected_date:
            self._selected_date = selected_date
        elif self._snapshots and not self._selected_date:
            self._selected_date = self._snapshots[-1].snapshot_date
        elif self._snapshots and all(
            s.snapshot_date != self._selected_date for s in self._snapshots
        ):
            self._selected_date = self._snapshots[-1].snapshot_date
        self.update()

    def selected_date(self) -> str:
        return self._selected_date

    def paintEvent(self, event) -> None:  # noqa: N802
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        rect = self.rect()
        painter.fillRect(rect, QColor("#151922"))

        if not self._snapshots:
            painter.setPen(QColor("#94A3B8"))
            painter.setFont(QFont("Segoe UI", 10))
            painter.drawText(
                rect,
                Qt.AlignmentFlag.AlignCenter,
                "Chưa có dữ liệu ngày — hãy bấm [🔍 Quét Ổ Đĩa & Zalo/DingTalk] để nạp biểu đồ.",
            )
            return

        left_pad = 68.0
        right_pad = 24.0
        top_pad = 36.0
        bottom_pad = 50.0

        chart_w = max(100.0, rect.width() - left_pad - right_pad)
        chart_h = max(80.0, rect.height() - top_pad - bottom_pad)

        # Title
        painter.setPen(QColor("#E2E8F0"))
        title_font = QFont("Segoe UI", 10, QFont.Weight.Bold)
        painter.setFont(title_font)
        painter.drawText(
            int(left_pad),
            22,
            "Biểu đồ Mức Sử Dụng & Biến Động Ổ Đĩa Theo Ngày (Click vào cột ngày để xem chi tiết bên dưới)",
        )

        # Compute scale: show bars proportional to daily delta or used_bytes
        max_delta = max(max(0, s.net_change_bytes) for s in self._snapshots) or 1
        max_used = max(s.used_bytes for s in self._snapshots) or 1
        min_used = min(s.used_bytes for s in self._snapshots)
        used_span = max(max_used - min_used, int(max_used * 0.15), 1)
        base_used = max(0, min_used - int(used_span * 0.25))

        # Horizontal grid lines
        grid_pen = QPen(QColor("#262D3D"), 1, Qt.PenStyle.DashLine)
        label_font = QFont("Segoe UI", 8)
        painter.setFont(label_font)
        for i in range(4):
            ratio = i / 3.0
            y = top_pad + chart_h * (1.0 - ratio)
            painter.setPen(grid_pen)
            painter.drawLine(int(left_pad), int(y), int(left_pad + chart_w), int(y))
            val_bytes = base_used + ratio * (max_used - base_used)
            painter.setPen(QColor("#94A3B8"))
            painter.drawText(
                QRectF(4, y - 10, left_pad - 10, 20),
                Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter,
                format_bytes(val_bytes),
            )

        n = len(self._snapshots)
        slot_w = chart_w / max(1, n)
        bar_w = min(48.0, max(16.0, slot_w * 0.62))

        self._bar_rects.clear()

        for idx, snap in enumerate(self._snapshots):
            cx = left_pad + idx * slot_w + slot_w / 2.0
            norm_h = (snap.used_bytes - base_used) / max(1, (max_used - base_used))
            # Also emphasize days with large positive delta
            delta_boost = (
                (max(0, snap.net_change_bytes) / max_delta) * 0.35
                if max_delta > 0
                else 0.0
            )
            height_ratio = min(1.0, max(0.14, norm_h * 0.65 + delta_boost))
            bar_h = chart_h * height_ratio
            x = cx - bar_w / 2.0
            y = top_pad + chart_h - bar_h

            bar_rect = QRectF(x, y, bar_w, bar_h)
            self._bar_rects.append((bar_rect, snap.snapshot_date))

            is_selected = snap.snapshot_date == self._selected_date
            is_hovered = idx == self._hover_index
            is_spike = snap.net_change_bytes >= 200 * 1024 * 1024  # >= 200 MB growth

            if is_selected:
                fill_color = QColor("#38BDF8") if not is_spike else QColor("#F97316")
                border_pen = QPen(QColor("#F8FAFC"), 2.2)
            elif is_hovered:
                fill_color = QColor("#60A5FA") if not is_spike else QColor("#FB923C")
                border_pen = QPen(QColor("#E2E8F0"), 1.5)
            else:
                fill_color = QColor("#2563EB") if not is_spike else QColor("#EA580C")
                border_pen = QPen(Qt.PenStyle.NoPen)

            painter.setPen(border_pen)
            painter.setBrush(fill_color)
            painter.drawRoundedRect(bar_rect, 4.0, 4.0)

            # Top label: +MB/GB net change for the day
            if snap.net_change_bytes > 0:
                delta_txt = f"+{format_bytes(snap.net_change_bytes)}"
                painter.setPen(
                    QColor("#FDBA74") if is_spike else QColor("#93C5FD")
                )
            elif snap.net_change_bytes < 0:
                delta_txt = format_bytes(snap.net_change_bytes)
                painter.setPen(QColor("#4ADE80"))
            else:
                delta_txt = "0 B"
                painter.setPen(QColor("#64748B"))

            small_font = QFont("Segoe UI", 7, QFont.Weight.Bold if is_selected else QFont.Weight.Normal)
            painter.setFont(small_font)
            painter.drawText(
                QRectF(cx - slot_w / 2.0, y - 18.0, slot_w, 16.0),
                Qt.AlignmentFlag.AlignCenter,
                delta_txt,
            )

            # Bottom X-axis date label (MM-DD)
            short_date = snap.snapshot_date[5:]  # MM-DD
            painter.setPen(
                QColor("#F8FAFC") if is_selected else QColor("#94A3B8")
            )
            painter.setFont(
                QFont(
                    "Segoe UI",
                    8,
                    QFont.Weight.Bold if is_selected else QFont.Weight.Normal,
                )
            )
            painter.drawText(
                QRectF(cx - slot_w / 2.0, top_pad + chart_h + 6.0, slot_w, 20.0),
                Qt.AlignmentFlag.AlignCenter,
                short_date,
            )

            if is_selected:
                painter.setPen(QColor("#38BDF8"))
                painter.drawText(
                    QRectF(cx - slot_w / 2.0, top_pad + chart_h + 22.0, slot_w, 18.0),
                    Qt.AlignmentFlag.AlignCenter,
                    "▲ Đang xem",
                )

    def mouseMoveEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        pos = event.position()
        hovered = -1
        for idx, (r, _) in enumerate(self._bar_rects):
            hit_rect = QRectF(r.left() - 4, 30, r.width() + 8, self.height() - 40)
            if hit_rect.contains(pos):
                hovered = idx
                break
        if hovered != self._hover_index:
            self._hover_index = hovered
            self.setCursor(
                Qt.CursorShape.PointingHandCursor
                if hovered >= 0
                else Qt.CursorShape.ArrowCursor
            )
            self.update()

    def mousePressEvent(self, event: QMouseEvent) -> None:  # noqa: N802
        if event.button() != Qt.MouseButton.LeftButton:
            return
        pos = event.position()
        for r, date_str in self._bar_rects:
            hit_rect = QRectF(r.left() - 6, 30, r.width() + 12, self.height() - 40)
            if hit_rect.contains(pos):
                self._selected_date = date_str
                self.update()
                self.daySelected.emit(date_str)
                break
