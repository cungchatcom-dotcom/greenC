from datetime import datetime
from pathlib import Path
from typing import Callable

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QHBoxLayout, QMessageBox, QPushButton, QVBoxLayout, QWidget

from greenc.core.safety_watchdog import SafetyWatchdog
from greenc.i18n import N_, tr
from greenc.models import VaultArchive
from greenc.storage.database import GreenCDB
from greenc.ui import format_size
from greenc.ui.dialogs import ArchiveDetailsDialog
from greenc.ui.widgets import PathValue, TagValue, fit_widget_columns, make_card, make_table, muted_label, page_title, set_cell

STATUS_TEXT = {"QUARANTINED": N_("Đang giữ"), "ROLLED_BACK": N_("Đã khôi phục"), "PURGED": N_("Đã xóa")}


class VaultPage(QWidget):
    dataChanged = Signal()

    def __init__(self, db: GreenCDB, watchdog: SafetyWatchdog, vault_dir_provider: Callable[[], Path], parent=None):
        super().__init__(parent)
        self.db = db
        self.watchdog = watchdog
        self._vault_dir = vault_dir_provider
        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)
        head = QHBoxLayout()
        head.addWidget(page_title(tr("Kho dự phòng")))
        head.addStretch(1)
        self.btn_check = QPushButton(tr("Kiểm tra ngay"))
        self.btn_check.clicked.connect(self._check_now)
        head.addWidget(self.btn_check)
        lay.addLayout(head)
        self.info_label = muted_label("")
        lay.addWidget(self.info_label)
        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        self.vault_table = make_table(
            [tr("Ứng dụng"), tr("Tạo lúc"), tr("Còn lại"), tr("Gốc → Nén"), tr("Trạng thái"), tr("Vị trí"), ""], stretch_col=5
        )
        cl.addWidget(self.vault_table)
        lay.addWidget(card, 1)

    def refresh(self) -> None:
        archives = self.db.list_vault_archives()
        live = [a for a in archives if a.status == "QUARANTINED"]
        self.info_label.setText(
            tr(
                "{folder} · {count} gói đang giữ · {size}",
                folder=self._vault_dir(), count=len(live),
                size=format_size(sum(a.compressed_bytes for a in live)),
            )
        )
        now = datetime.now()
        self.vault_table.setRowCount(len(archives))
        for row, arch in enumerate(archives):
            created = datetime.fromisoformat(arch.created_at)
            remaining = "—"
            if arch.status == "QUARANTINED":
                delta = datetime.fromisoformat(arch.expires_at) - now
                remaining = tr("{days} ngày {hours} giờ", days=max(0, delta.days), hours=max(0, delta.seconds // 3600))
            set_cell(self.vault_table, row, 0, tr(arch.app_name))
            set_cell(self.vault_table, row, 1, created.strftime("%d/%m %H:%M"))
            set_cell(self.vault_table, row, 2, remaining)
            set_cell(self.vault_table, row, 3, f"{format_size(arch.original_bytes)} → {format_size(arch.compressed_bytes)}")
            set_cell(self.vault_table, row, 4, TagValue(
                tr(STATUS_TEXT.get(arch.status, arch.status)),
                tooltip=tr(arch.status_reason) if arch.status_reason else arch.status_reason,
            ))
            set_cell(self.vault_table, row, 5, PathValue(arch.zip_path))
            self.vault_table.setCellWidget(row, 6, self._actions(arch))
        fit_widget_columns(self.vault_table)

    def _actions(self, arch: VaultArchive) -> QWidget:
        wrap = QWidget()
        h = QHBoxLayout(wrap)
        h.setContentsMargins(4, 2, 4, 2)
        h.setSpacing(6)
        btn_view = QPushButton(tr("Chi tiết"))
        btn_view.clicked.connect(lambda _=False, a=arch: self._view(a))
        h.addWidget(btn_view)
        if arch.status == "QUARANTINED":
            btn_restore = QPushButton(tr("Khôi phục"))
            btn_restore.clicked.connect(lambda _=False, aid=arch.id: self._rollback(aid))
            btn_purge = QPushButton(tr("Xóa ngay"))
            btn_purge.clicked.connect(lambda _=False, aid=arch.id: self._purge(aid))
            h.addWidget(btn_restore)
            h.addWidget(btn_purge)
        return wrap

    def _view(self, arch: VaultArchive) -> None:
        ArchiveDetailsDialog(arch, self.db.get_vault_files(arch.id or 0), self).exec()

    def _rollback(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        res = self.watchdog.rollback_archive(archive_id, reason=N_("Khôi phục thủ công"), auto_triggered=False)
        self.refresh()
        self.dataChanged.emit()
        QMessageBox.information(
            self, tr("Đã khôi phục"),
            tr("Đã khôi phục {count} tệp về vị trí cũ.", count=res["restored_count"]),
        )

    def _purge(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        answer = QMessageBox.question(self, tr("Xóa gói dự phòng"), tr("Xóa vĩnh viễn gói này? Không thể hoàn tác."))
        if answer != QMessageBox.StandardButton.Yes:
            return
        self.watchdog.purge_archive_now(archive_id, N_("Người dùng xóa"))
        self.refresh()
        self.dataChanged.emit()

    def _check_now(self) -> None:
        res = self.watchdog.run_safety_cycle()
        self.refresh()
        self.dataChanged.emit()
        QMessageBox.information(
            self, tr("Đã kiểm tra"),
            tr(
                "Tự khôi phục: {restored} gói. Đã xóa gói hết hạn: {purged}.",
                restored=len(res["auto_rolled_back"]), purged=len(res["purged"]),
            ),
        )
