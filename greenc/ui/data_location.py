from pathlib import Path
from typing import Callable

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QButtonGroup,
    QDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QProgressBar,
    QPushButton,
    QRadioButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from greenc.config import APP_NAME, DEFAULT_RETENTION_DAYS, MAX_RETENTION_DAYS, MIN_RETENTION_DAYS
from greenc.core.drives import DriveInfo, data_root_for, drive_of, recommend_drive
from greenc.core.settings import AppSettings
from greenc.i18n import N_, tr
from greenc.ui import format_size
from greenc.ui.theme import apply_theme, repolish
from greenc.ui.widgets import muted_label, page_title


class DriveOption(QFrame):
    def __init__(self, drive: DriveInfo, recommended: bool, parent: QWidget | None = None):
        super().__init__(parent)
        self.drive = drive
        self.setObjectName("DriveOption")
        self.setProperty("selected", False)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(12, 10, 12, 10)
        lay.setSpacing(10)
        self.radio = QRadioButton()
        lay.addWidget(self.radio, 0, Qt.AlignmentFlag.AlignTop)

        col = QVBoxLayout()
        col.setSpacing(4)
        top = QHBoxLayout()
        name = QLabel(tr(
            "<b>{letter}</b>&nbsp;&nbsp;{label}", letter=drive.letter, label=drive.label or tr("Ổ đĩa cục bộ")
        ))
        top.addWidget(name)
        top.addStretch(1)
        tag_text, tag_name = "", "Tag"
        if not drive.eligible:
            tag_text, tag_name = tr("Không dùng được"), "TagNeutral"
        elif recommended:
            tag_text = tr("Đề xuất")
        elif drive.is_system:
            tag_text, tag_name = tr("Ổ hệ thống"), "TagNeutral"
        if tag_text:
            tag = QLabel(tag_text)
            tag.setObjectName(tag_name)
            top.addWidget(tag)
        col.addLayout(top)

        bar = QProgressBar()
        bar.setRange(0, 100)
        pct = int(drive.used * 100 / max(1, drive.total))
        bar.setValue(pct)
        bar.setTextVisible(False)
        if pct >= 90:
            bar.setObjectName("DangerBar")
        col.addWidget(bar)
        detail = (
            tr("Còn trống {free} / {total}", free=format_size(drive.free), total=format_size(drive.total))
            if drive.eligible
            else tr(drive.reason)
        )
        col.addWidget(muted_label(detail))
        lay.addLayout(col, 1)
        self.setEnabled(drive.eligible)
        self.radio.toggled.connect(self._on_toggled)

    def mousePressEvent(self, event) -> None:  # noqa: N802
        if self.isEnabled():
            self.radio.setChecked(True)
        super().mousePressEvent(event)

    def _on_toggled(self, on: bool) -> None:
        self.setProperty("selected", on)
        repolish(self)


class DataLocationWidget(QWidget):
    changed = Signal()

    def __init__(
        self,
        drives: list[DriveInfo],
        current_root: Path | None = None,
        retention_days: int = DEFAULT_RETENTION_DAYS,
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.drives = list(drives)
        recommended = recommend_drive(self.drives)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(8)

        self.group = QButtonGroup(self)
        self.group.setExclusive(True)
        self.options: list[DriveOption] = []
        for idx, drive in enumerate(self.drives):
            opt = DriveOption(drive, recommended is not None and drive.root == recommended.root)
            self.group.addButton(opt.radio, idx)
            opt.radio.toggled.connect(lambda _on: self._refresh_path())
            self.options.append(opt)
            lay.addWidget(opt)

        row = QHBoxLayout()
        row.setSpacing(8)
        row.addWidget(QLabel(tr("Giữ bản dự phòng trong")))
        self.retention_spin = QSpinBox()
        self.retention_spin.setLayoutDirection(Qt.LayoutDirection.LeftToRight)  # numbers stay LTR in RTL UIs
        self.retention_spin.setRange(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
        self.retention_spin.setValue(retention_days)
        self.retention_spin.setFixedWidth(70)
        self.retention_spin.valueChanged.connect(lambda _v: self.changed.emit())
        row.addWidget(self.retention_spin)
        row.addWidget(QLabel(tr("ngày trước khi xóa vĩnh viễn")))
        row.addStretch(1)
        lay.addSpacing(6)
        lay.addLayout(row)

        self.path_label = muted_label("")
        lay.addWidget(self.path_label)

        preselect = None
        if current_root is not None:
            cur_drive = drive_of(current_root)
            preselect = next((o for o in self.options if o.drive.root == cur_drive and o.drive.eligible), None)
        if preselect is None and recommended is not None:
            preselect = next(o for o in self.options if o.drive.root == recommended.root)
        if preselect is not None:
            preselect.radio.setChecked(True)
        self._refresh_path()

    def selected_drive(self) -> DriveInfo | None:
        idx = self.group.checkedId()
        return self.drives[idx] if idx >= 0 else None

    def data_root(self) -> Path | None:
        drive = self.selected_drive()
        return data_root_for(drive.root) if drive else None

    def retention_days(self) -> int:
        return self.retention_spin.value()

    def _refresh_path(self) -> None:
        root = self.data_root()
        self.path_label.setText(tr("Thư mục: {path}", path=f"{root}\\") if root else tr("Chưa chọn ổ đĩa"))
        self.changed.emit()


class FirstRunDialog(QDialog):
    def __init__(
        self,
        drives: list[DriveInfo],
        current_root: Path | None = None,
        retention_days: int = DEFAULT_RETENTION_DAYS,
        parent: QWidget | None = None,
        title: str = N_("Chọn nơi lưu dữ liệu"),
        allow_later: bool = True,
    ):
        super().__init__(parent)
        self.setWindowTitle(APP_NAME)
        apply_theme(self)
        self.setMinimumWidth(540)
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.setSpacing(0)

        body = QWidget()
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 22, 24, 18)
        lay.setSpacing(8)
        lay.addWidget(page_title(tr(title)))
        lay.addWidget(muted_label(tr(
            "{app} chuyển dữ liệu ứng dụng và Kho dự phòng sang ổ này để giải phóng ổ C:. "
            "Có thể đổi lại trong Cài đặt.",
            app=APP_NAME,
        )))
        lay.addSpacing(4)
        self.location = DataLocationWidget(drives, current_root, retention_days)
        lay.addWidget(self.location)
        outer.addWidget(body)

        footer = QFrame()
        footer.setObjectName("DialogFooter")
        f = QHBoxLayout(footer)
        f.setContentsMargins(24, 12, 24, 12)
        f.addStretch(1)
        self.btn_later = QPushButton(tr("Để sau") if allow_later else tr("Hủy"))
        self.btn_later.clicked.connect(self.reject)
        self.btn_ok = QPushButton(tr("Tiếp tục") if allow_later else tr("Lưu"))
        self.btn_ok.setObjectName("PrimaryButton")
        self.btn_ok.setDefault(True)
        self.btn_ok.clicked.connect(self.accept)
        f.addWidget(self.btn_later)
        f.addWidget(self.btn_ok)
        outer.addWidget(footer)

        self.location.changed.connect(self._sync_ok)
        self._sync_ok()

    def _sync_ok(self) -> None:
        self.btn_ok.setEnabled(self.location.data_root() is not None)

    def chosen_root(self) -> Path | None:
        return self.location.data_root()

    def chosen_retention(self) -> int:
        return self.location.retention_days()


def run_first_run(
    settings: AppSettings,
    drives: list[DriveInfo],
    parent: QWidget | None = None,
    exec_fn: Callable[[FirstRunDialog], bool] | None = None,
) -> Path | None:
    """Show the first-run dialog and persist the result. "Để sau" keeps the recommended drive."""
    dlg = FirstRunDialog(drives, None, settings.retention_days_default, parent)
    accepted = bool(exec_fn(dlg)) if exec_fn else dlg.exec() == QDialog.DialogCode.Accepted
    settings.set_retention_days_default(dlg.chosen_retention())
    if accepted:
        root = dlg.chosen_root()
    else:
        rec = recommend_drive(drives)
        root = data_root_for(rec.root) if rec else None
    if root is not None:
        settings.set_data_root(root)
    return root
