from datetime import datetime
from pathlib import Path
from typing import Callable

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QHBoxLayout, QMessageBox, QPushButton, QVBoxLayout, QWidget

from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.models import VaultArchive
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.dialogs import ArchiveDetailsDialog
from diskguard.ui.widgets import PathValue, TagValue, make_card, make_table, muted_label, page_title, set_cell

STATUS_TEXT = {"QUARANTINED": "Đang giữ", "ROLLED_BACK": "Đã khôi phục", "PURGED": "Đã xóa"}


class VaultPage(QWidget):
    dataChanged = Signal()

    def __init__(self, db: DiskGuardDB, watchdog: SafetyWatchdog, vault_dir_provider: Callable[[], Path], parent=None):
        super().__init__(parent)
        self.db = db
        self.watchdog = watchdog
        self._vault_dir = vault_dir_provider
        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)
        head = QHBoxLayout()
        head.addWidget(page_title("Kho dự phòng"))
        head.addStretch(1)
        self.btn_check = QPushButton("Kiểm tra ngay")
        self.btn_check.clicked.connect(self._check_now)
        head.addWidget(self.btn_check)
        lay.addLayout(head)
        self.info_label = muted_label("")
        lay.addWidget(self.info_label)
        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        self.vault_table = make_table(
            ["Ứng dụng", "Tạo lúc", "Còn lại", "Gốc → Nén", "Trạng thái", "Vị trí", ""], stretch_col=5
        )
        cl.addWidget(self.vault_table)
        lay.addWidget(card, 1)

    def refresh(self) -> None:
        archives = self.db.list_vault_archives()
        live = [a for a in archives if a.status == "QUARANTINED"]
        self.info_label.setText(
            f"{self._vault_dir()} · {len(live)} gói đang giữ · "
            f"{format_size(sum(a.compressed_bytes for a in live))}"
        )
        now = datetime.now()
        self.vault_table.setRowCount(len(archives))
        for row, arch in enumerate(archives):
            created = datetime.fromisoformat(arch.created_at)
            remaining = "—"
            if arch.status == "QUARANTINED":
                delta = datetime.fromisoformat(arch.expires_at) - now
                remaining = f"{max(0, delta.days)} ngày {max(0, delta.seconds // 3600)} giờ"
            set_cell(self.vault_table, row, 0, arch.app_name)
            set_cell(self.vault_table, row, 1, created.strftime("%d/%m %H:%M"))
            set_cell(self.vault_table, row, 2, remaining)
            set_cell(self.vault_table, row, 3, f"{format_size(arch.original_bytes)} → {format_size(arch.compressed_bytes)}")
            set_cell(self.vault_table, row, 4, TagValue(STATUS_TEXT.get(arch.status, arch.status), tooltip=arch.status_reason))
            set_cell(self.vault_table, row, 5, PathValue(arch.zip_path))
            self.vault_table.setCellWidget(row, 6, self._actions(arch))

    def _actions(self, arch: VaultArchive) -> QWidget:
        wrap = QWidget()
        h = QHBoxLayout(wrap)
        h.setContentsMargins(4, 2, 4, 2)
        h.setSpacing(6)
        btn_view = QPushButton("Chi tiết")
        btn_view.clicked.connect(lambda _=False, a=arch: self._view(a))
        h.addWidget(btn_view)
        if arch.status == "QUARANTINED":
            btn_restore = QPushButton("Khôi phục")
            btn_restore.clicked.connect(lambda _=False, aid=arch.id: self._rollback(aid))
            btn_purge = QPushButton("Xóa ngay")
            btn_purge.clicked.connect(lambda _=False, aid=arch.id: self._purge(aid))
            h.addWidget(btn_restore)
            h.addWidget(btn_purge)
        return wrap

    def _view(self, arch: VaultArchive) -> None:
        ArchiveDetailsDialog(arch, self.db.get_vault_files(arch.id or 0), self).exec()

    def _rollback(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        res = self.watchdog.rollback_archive(archive_id, reason="Khôi phục thủ công", auto_triggered=False)
        self.refresh()
        self.dataChanged.emit()
        QMessageBox.information(self, "Đã khôi phục", f"Đã khôi phục {res['restored_count']} tệp về vị trí cũ.")

    def _purge(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        answer = QMessageBox.question(self, "Xóa gói dự phòng", "Xóa vĩnh viễn gói này? Không thể hoàn tác.")
        if answer != QMessageBox.StandardButton.Yes:
            return
        self.watchdog.purge_archive_now(archive_id, "Người dùng xóa")
        self.refresh()
        self.dataChanged.emit()

    def _check_now(self) -> None:
        res = self.watchdog.run_safety_cycle()
        self.refresh()
        self.dataChanged.emit()
        QMessageBox.information(
            self, "Đã kiểm tra",
            f"Tự khôi phục: {len(res['auto_rolled_back'])} gói. Đã xóa gói hết hạn: {len(res['purged'])}.",
        )
