from datetime import datetime
from PySide6.QtCore import Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QMessageBox,
    QPushButton,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.simulator import SandboxSimulator
from diskguard.models import VaultArchive
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes
from diskguard.ui.dialogs import ArchiveDetailsDialog


class SafeVaultTab(QWidget):
    """
    Tab 3:
    - Lists all `.zip` archives in Safe Vault (`QUARANTINED`, `ROLLED_BACK`, `PURGED`).
    - Displays remaining time before D-day auto-deletion.
    - Provides 1-click Manual Rollback, 1-click Simulate Unsafe Access (Auto-Rollback test),
      manifest inspection, and immediate permanent deletion.
    """

    vaultChanged = Signal()

    def __init__(
        self,
        db: DiskGuardDB,
        watchdog: SafetyWatchdog,
        simulator: SandboxSimulator,
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.db = db
        self.watchdog = watchdog
        self.simulator = simulator

        layout = QVBoxLayout(self)
        layout.setContentsMargins(8, 8, 8, 8)
        layout.setSpacing(10)

        top_bar = QHBoxLayout()
        self.summary_label = QLabel("📦 Kho Lưu Trữ Nén Tạm (Safe Vault) & Trung Tâm Rollback")
        self.summary_label.setStyleSheet("font-size: 13px; font-weight: bold; color: #38BDF8;")
        top_bar.addWidget(self.summary_label, stretch=1)

        btn_check_safety = QPushButton("🛡️ Chạy Kiểm Tra An Toàn & Xóa Hết Hạn D Ngày")
        btn_check_safety.setStyleSheet(
            "background-color: #0284C7; color: white; font-weight: bold; padding: 6px 12px;"
        )
        btn_check_safety.clicked.connect(self.run_safety_check_now)
        top_bar.addWidget(btn_check_safety)
        layout.addLayout(top_bar)

        self.table = QTableWidget(0, 8)
        self.table.setHorizontalHeaderLabels(
            [
                "Mã Gói",
                "Ứng dụng",
                "Số File",
                "Dung lượng Gốc -> Nén",
                "Ngày Tạo & Đếm Ngược Xóa (D ngày)",
                "Trạng Thái",
                "Lý do / Ghi chú An toàn",
                "Thao tác (Rollback / Xem / Xóa)",
            ]
        )
        self.table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(5, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(6, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(7, QHeaderView.ResizeMode.ResizeToContents)
        layout.addWidget(self.table, stretch=1)

    def refresh_vault_table(self) -> None:
        archives = self.db.list_vault_archives()
        quarantined = [a for a in archives if a.status == "QUARANTINED"]
        rolled_back = [a for a in archives if a.status == "ROLLED_BACK"]
        purged = [a for a in archives if a.status == "PURGED"]
        total_saved = sum(a.original_bytes for a in quarantined + purged)

        self.summary_label.setText(
            f"📦 Safe Vault: {len(quarantined)} gói đang theo dõi D ngày | "
            f"{len(rolled_back)} gói đã Rollback khôi phục | "
            f"{len(purged)} gói đã xóa an toàn — "
            f"Tổng dung lượng đã giải phóng: {format_bytes(total_saved)}"
        )

        self.table.setRowCount(len(archives))
        now = datetime.now()

        for row_idx, arch in enumerate(archives):
            self.table.setItem(row_idx, 0, QTableWidgetItem(f"#{arch.id}"))
            self.table.setItem(row_idx, 1, QTableWidgetItem(arch.app_name))
            self.table.setItem(row_idx, 2, QTableWidgetItem(f"{arch.file_count} files"))

            size_txt = (
                f"{format_bytes(arch.original_bytes)} -> {format_bytes(arch.compressed_bytes)}"
            )
            self.table.setItem(row_idx, 3, QTableWidgetItem(size_txt))

            expires_dt = datetime.fromisoformat(arch.expires_at)
            rem = expires_dt - now
            if arch.status == "QUARANTINED":
                rem_days = max(0, rem.days)
                rem_hours = max(0, rem.seconds // 3600)
                time_txt = f"Tạo: {arch.created_at[:16]}\nCòn {rem_days} ngày {rem_hours}h sẽ tự xóa"
            else:
                time_txt = f"Tạo: {arch.created_at[:16]}\nXử lý: {arch.resolved_at[:16]}"
            self.table.setItem(row_idx, 4, QTableWidgetItem(time_txt))

            st_item = QTableWidgetItem(arch.status)
            if arch.status == "QUARANTINED":
                st_item.setForeground(QColor("#FBBF24"))
            elif arch.status == "ROLLED_BACK":
                st_item.setForeground(QColor("#38BDF8"))
            else:
                st_item.setForeground(QColor("#4ADE80"))
            self.table.setItem(row_idx, 5, st_item)

            self.table.setItem(row_idx, 6, QTableWidgetItem(arch.status_reason))

            actions_widget = QWidget()
            h = QHBoxLayout(actions_widget)
            h.setContentsMargins(2, 2, 2, 2)
            h.setSpacing(4)

            btn_view = QPushButton("🔍 File")
            btn_view.setToolTip("Xem danh sách file gốc & liên kết NTFS HardLink trong gói .zip")
            btn_view.clicked.connect(lambda _=False, a=arch: self._view_archive(a))
            h.addWidget(btn_view)

            if arch.status == "QUARANTINED":
                btn_rb = QPushButton("⏪ Rollback Ngay")
                btn_rb.setStyleSheet(
                    "background-color: #0284C7; color: white; font-weight: bold;"
                )
                btn_rb.clicked.connect(
                    lambda _=False, aid=arch.id: self._manual_rollback(aid)
                )
                h.addWidget(btn_rb)

                btn_sim = QPushButton("🧪 Giả lập Lỗi -> Auto-Rollback")
                btn_sim.setToolTip(
                    "Giả lập ứng dụng cố đọc lại file trong gói nén này (báo lỗi FileNotFoundError) để kiểm chứng Tự động Rollback"
                )
                btn_sim.setStyleSheet("background-color: #7C3AED; color: white;")
                btn_sim.clicked.connect(
                    lambda _=False, aid=arch.id: self._simulate_auto_rollback(aid)
                )
                h.addWidget(btn_sim)

                btn_purge = QPushButton("🗑️ Xóa hẳn")
                btn_purge.clicked.connect(
                    lambda _=False, aid=arch.id: self._manual_purge(aid)
                )
                h.addWidget(btn_purge)

            self.table.setCellWidget(row_idx, 7, actions_widget)

        self.table.resizeRowsToContents()

    def _view_archive(self, arch: VaultArchive) -> None:
        files = self.db.get_vault_files(arch.id or 0)
        dlg = ArchiveDetailsDialog(arch, files, self)
        dlg.exec()

    def _manual_rollback(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        res = self.watchdog.rollback_archive(
            archive_id,
            reason="Người dùng bấm Rollback khôi phục thủ công",
            auto_triggered=False,
        )
        self.refresh_vault_table()
        self.vaultChanged.emit()
        QMessageBox.information(
            self,
            "Đã Rollback Khôi Phục Thành Công",
            f"Đã giải nén trả nguyên vẹn {res['restored_count']} đường dẫn (bao gồm cả HardLink) về vị trí gốc "
            f"và thêm vào danh sách miễn trừ vĩnh viễn!",
        )

    def _simulate_auto_rollback(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        res = self.simulator.simulate_unsafe_file_access_and_auto_rollback(archive_id)
        self.refresh_vault_table()
        self.vaultChanged.emit()
        reason = res["reasons"].get(archive_id, "Phát hiện lỗi thiếu file")
        QMessageBox.warning(
            self,
            "⚡ Tự Động Rollback Đã Kích Hoạt!",
            f"Safety Watchdog vừa phát hiện dấu hiệu mất an toàn:\n\n"
            f"• Nguyên nhân: {reason}\n\n"
            f"Hệ thống đã TỰ ĐỘNG giải nén khôi phục toàn bộ file về đường dẫn gốc, "
            f"tái tạo NTFS HardLink và khóa bảo vệ file này không bao giờ bị nén lại!",
        )

    def _manual_purge(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        self.watchdog.purge_archive_now(
            archive_id, "Người dùng xác nhận an toàn — xóa vĩnh viễn gói .zip"
        )
        self.refresh_vault_table()
        self.vaultChanged.emit()

    def run_safety_check_now(self) -> None:
        res = self.watchdog.run_safety_cycle()
        self.refresh_vault_table()
        self.vaultChanged.emit()
        QMessageBox.information(
            self,
            "Kết quả Kiểm tra An toàn",
            f"• Số gói tự động Rollback do phát hiện lỗi: {len(res['auto_rolled_back'])}\n"
            f"• Số gói đã xóa vĩnh viễn do đủ D ngày an toàn: {len(res['purged'])}",
        )
