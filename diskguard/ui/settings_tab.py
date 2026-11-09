from datetime import datetime
from PySide6.QtCore import Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QMessageBox,
    QPushButton,
    QTabWidget,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.simulator import SandboxSimulator
from diskguard.models import QuotaRule, VaultArchive
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes
from diskguard.ui.dialogs import ArchiveDetailsDialog, QuotaRuleDialog


class SettingsTab(QWidget):
    """
    Settings > Rules & Safe Vault:
    Keeps all per-application background rules (Zalo, DingTalk, and dozens of
    auto-discovered apps) tucked neatly inside Settings while they run silently
    in the background, alongside the D-day Backup Vault & Rollback controls.
    """

    settingsDataChanged = Signal()

    def __init__(
        self,
        db: DiskGuardDB,
        enforcer: QuotaEnforcer,
        watchdog: SafetyWatchdog,
        simulator: SandboxSimulator,
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.db = db
        self.enforcer = enforcer
        self.watchdog = watchdog
        self.simulator = simulator

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(10)

        self.sub_tabs = QTabWidget()

        # Sub-tab 1: Settings > Rules (Quy tắc ứng dụng chạy ngầm)
        rules_page = QFrame()
        rules_page.setObjectName("SurfaceCard")
        rules_layout = QVBoxLayout(rules_page)
        rules_layout.setContentsMargins(16, 14, 16, 14)

        rules_header = QHBoxLayout()
        self.rules_info_label = QLabel(
            "Danh sách các quy tắc giới hạn dung lượng đang tự động chạy ngầm cho từng ứng dụng"
        )
        self.rules_info_label.setStyleSheet(
            "font-size: 13px; font-weight: 600; color: #1D1D1F;"
        )
        rules_header.addWidget(self.rules_info_label, stretch=1)

        btn_new_rule = QPushButton("Thêm quy tắc thủ công")
        btn_new_rule.setObjectName("SecondaryButton")
        btn_new_rule.clicked.connect(self.open_new_rule_dialog)
        rules_header.addWidget(btn_new_rule)
        rules_layout.addLayout(rules_header)

        self.rules_table = QTableWidget(0, 7)
        self.rules_table.setHorizontalHeaderLabels(
            [
                "Ứng dụng / Service",
                "Thư mục giám sát ngầm",
                "Dung lượng thực tế",
                "Hạn mức cho phép",
                "Trạng thái",
                "Thời hạn giữ bản nén",
                "Tùy chỉnh",
            ]
        )
        self.rules_table.verticalHeader().setVisible(False)
        self.rules_table.setShowGrid(False)
        self.rules_table.setAlternatingRowColors(True)
        self.rules_table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self.rules_table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.Stretch)
        self.rules_table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        self.rules_table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)
        self.rules_table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeMode.ResizeToContents)
        self.rules_table.horizontalHeader().setSectionResizeMode(5, QHeaderView.ResizeMode.ResizeToContents)
        self.rules_table.horizontalHeader().setSectionResizeMode(6, QHeaderView.ResizeMode.ResizeToContents)
        rules_layout.addWidget(self.rules_table, stretch=1)

        # Sub-tab 2: Settings > Safe Vault (Kho dự phòng D ngày & Rollback)
        vault_page = QFrame()
        vault_page.setObjectName("SurfaceCard")
        vault_layout = QVBoxLayout(vault_page)
        vault_layout.setContentsMargins(16, 14, 16, 14)

        vault_header = QHBoxLayout()
        self.vault_info_label = QLabel(
            "Các bản nén dự phòng (Safe Vault) được giữ an toàn D ngày trước khi tự động xóa hẳn"
        )
        self.vault_info_label.setStyleSheet(
            "font-size: 13px; font-weight: 600; color: #1D1D1F;"
        )
        vault_header.addWidget(self.vault_info_label, stretch=1)

        btn_check_safety = QPushButton("Kiểm tra an toàn & Dọn bản nén quá hạn")
        btn_check_safety.setObjectName("SecondaryButton")
        btn_check_safety.clicked.connect(self._run_safety_check_now)
        vault_header.addWidget(btn_check_safety)
        vault_layout.addLayout(vault_header)

        self.vault_table = QTableWidget(0, 7)
        self.vault_table.setHorizontalHeaderLabels(
            [
                "Mã gói",
                "Ứng dụng",
                "Dung lượng gốc -> Nén",
                "Thời hạn tự xóa (D ngày)",
                "Trạng thái",
                "Ghi chú",
                "Khôi phục (Rollback)",
            ]
        )
        self.vault_table.verticalHeader().setVisible(False)
        self.vault_table.setShowGrid(False)
        self.vault_table.setAlternatingRowColors(True)
        self.vault_table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self.vault_table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        self.vault_table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        self.vault_table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)
        self.vault_table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeMode.ResizeToContents)
        self.vault_table.horizontalHeader().setSectionResizeMode(5, QHeaderView.ResizeMode.Stretch)
        self.vault_table.horizontalHeader().setSectionResizeMode(6, QHeaderView.ResizeMode.ResizeToContents)
        vault_layout.addWidget(self.vault_table, stretch=1)

        self.sub_tabs.addTab(rules_page, "Quy tắc Ứng dụng (Rules chạy ngầm)")
        self.sub_tabs.addTab(vault_page, "Kho Dự phòng & Rollback (Safe Vault)")
        layout.addWidget(self.sub_tabs, stretch=1)

    def refresh_all(self) -> None:
        self.refresh_rules_table()
        self.refresh_vault_table()

    def refresh_rules_table(self) -> None:
        rules = self.db.list_quota_rules()
        self.rules_info_label.setText(
            f"Quy tắc chạy ngầm ({len(rules)} ứng dụng đang được tự động kiểm soát dung lượng)"
        )
        self.rules_table.setRowCount(len(rules))

        for row_idx, rule in enumerate(rules):
            usage = self.enforcer.evaluate_rule_usage(rule)
            phys_b = usage["physical_bytes"]
            pct = usage["usage_pct"]

            self.rules_table.setItem(
                row_idx,
                0,
                QTableWidgetItem(f"{rule.app_name}\n({rule.process_name or rule.service_name})"),
            )
            self.rules_table.setItem(
                row_idx, 1, QTableWidgetItem("\n".join(rule.target_paths[:2]))
            )
            self.rules_table.setItem(row_idx, 2, QTableWidgetItem(format_bytes(phys_b)))
            self.rules_table.setItem(
                row_idx, 3, QTableWidgetItem(format_bytes(rule.max_bytes))
            )

            st_txt = (
                f"Cần thu hồi ({pct:.0f}%)"
                if phys_b > rule.max_bytes
                else f"Đang ổn định ({pct:.0f}%)"
            )
            if not rule.enabled:
                st_txt = "Tạm tắt"
            st_item = QTableWidgetItem(st_txt)
            st_item.setForeground(
                QColor("#B45309") if phys_b > rule.max_bytes else QColor("#15803D")
            )
            self.rules_table.setItem(row_idx, 4, st_item)

            self.rules_table.setItem(
                row_idx,
                5,
                QTableWidgetItem(
                    f"{rule.retention_days} ngày\n(Miễn trừ: {len(rule.protected_files)})"
                ),
            )

            act_w = QWidget()
            h = QHBoxLayout(act_w)
            h.setContentsMargins(2, 2, 2, 2)
            h.setSpacing(4)

            btn_edit = QPushButton("Sửa")
            btn_edit.setObjectName("SecondaryButton")
            btn_edit.clicked.connect(lambda _=False, r=rule: self._edit_rule(r))

            btn_del = QPushButton("Xóa")
            btn_del.setObjectName("SecondaryButton")
            btn_del.clicked.connect(lambda _=False, rid=rule.id: self._delete_rule(rid))

            h.addWidget(btn_edit)
            h.addWidget(btn_del)
            self.rules_table.setCellWidget(row_idx, 6, act_w)

        self.rules_table.resizeRowsToContents()

    def refresh_vault_table(self) -> None:
        archives = self.db.list_vault_archives()
        quarantined = [a for a in archives if a.status == "QUARANTINED"]
        self.vault_info_label.setText(
            f"Kho dự phòng: {len(quarantined)} gói đang trong thời gian chờ an toàn D ngày (Tổng {len(archives)} bản ghi)"
        )
        self.vault_table.setRowCount(len(archives))
        now = datetime.now()

        for row_idx, arch in enumerate(archives):
            self.vault_table.setItem(row_idx, 0, QTableWidgetItem(f"#{arch.id}"))
            self.vault_table.setItem(row_idx, 1, QTableWidgetItem(arch.app_name))
            self.vault_table.setItem(
                row_idx,
                2,
                QTableWidgetItem(
                    f"{format_bytes(arch.original_bytes)} -> {format_bytes(arch.compressed_bytes)} ({arch.file_count} files)"
                ),
            )

            expires_dt = datetime.fromisoformat(arch.expires_at)
            rem = expires_dt - now
            if arch.status == "QUARANTINED":
                time_txt = f"Còn {max(0, rem.days)} ngày {max(0, rem.seconds // 3600)} giờ"
            else:
                time_txt = arch.resolved_at[:16] if arch.resolved_at else arch.created_at[:16]
            self.vault_table.setItem(row_idx, 3, QTableWidgetItem(time_txt))

            status_map = {
                "QUARANTINED": "Đang giữ dự phòng",
                "ROLLED_BACK": "Đã khôi phục (Rollback)",
                "PURGED": "Đã xóa an toàn",
            }
            st_item = QTableWidgetItem(status_map.get(arch.status, arch.status))
            st_item.setForeground(
                QColor("#0066CC")
                if arch.status == "ROLLED_BACK"
                else (QColor("#B45309") if arch.status == "QUARANTINED" else QColor("#15803D"))
            )
            self.vault_table.setItem(row_idx, 4, st_item)
            self.vault_table.setItem(row_idx, 5, QTableWidgetItem(arch.status_reason))

            act_w = QWidget()
            h = QHBoxLayout(act_w)
            h.setContentsMargins(2, 2, 2, 2)
            h.setSpacing(4)

            btn_view = QPushButton("Chi tiết")
            btn_view.setObjectName("SecondaryButton")
            btn_view.clicked.connect(lambda _=False, a=arch: self._view_archive(a))
            h.addWidget(btn_view)

            if arch.status == "QUARANTINED":
                btn_rb = QPushButton("Khôi phục ngay")
                btn_rb.setObjectName("PrimaryButton")
                btn_rb.clicked.connect(
                    lambda _=False, aid=arch.id: self._manual_rollback(aid)
                )
                h.addWidget(btn_rb)

                btn_purge = QPushButton("Xóa hẳn")
                btn_purge.setObjectName("SecondaryButton")
                btn_purge.clicked.connect(
                    lambda _=False, aid=arch.id: self._manual_purge(aid)
                )
                h.addWidget(btn_purge)

            self.vault_table.setCellWidget(row_idx, 6, act_w)

        self.vault_table.resizeRowsToContents()

    def open_new_rule_dialog(
        self,
        app_name: str = "",
        process_name: str = "",
        service_name: str = "",
        paths: list[str] | None = None,
    ) -> None:
        self.sub_tabs.setCurrentIndex(0)
        dlg = QuotaRuleDialog(
            self,
            prefill_app_name=app_name,
            prefill_process_name=process_name,
            prefill_service_name=service_name,
            prefill_paths=paths,
        )
        if dlg.exec():
            self.db.save_quota_rule(dlg.get_rule())
            self.refresh_rules_table()
            self.settingsDataChanged.emit()

    def _edit_rule(self, rule: QuotaRule) -> None:
        dlg = QuotaRuleDialog(self, rule=rule)
        if dlg.exec():
            self.db.save_quota_rule(dlg.get_rule())
            self.refresh_rules_table()
            self.settingsDataChanged.emit()

    def _delete_rule(self, rule_id: int | None) -> None:
        if rule_id is None:
            return
        self.db.delete_quota_rule(rule_id)
        self.refresh_rules_table()
        self.settingsDataChanged.emit()

    def _view_archive(self, arch: VaultArchive) -> None:
        files = self.db.get_vault_files(arch.id or 0)
        dlg = ArchiveDetailsDialog(arch, files, self)
        dlg.exec()

    def _manual_rollback(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        res = self.watchdog.rollback_archive(
            archive_id,
            reason="Khôi phục thủ công từ Cài đặt",
            auto_triggered=False,
        )
        self.refresh_all()
        self.settingsDataChanged.emit()
        QMessageBox.information(
            self,
            "Đã khôi phục dữ liệu",
            f"Đã giải nén trả nguyên vẹn {res['restored_count']} đường dẫn về vị trí gốc.",
        )

    def _manual_purge(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        self.watchdog.purge_archive_now(archive_id, "Người dùng xóa vĩnh viễn gói dự phòng")
        self.refresh_all()
        self.settingsDataChanged.emit()

    def _run_safety_check_now(self) -> None:
        res = self.watchdog.run_safety_cycle()
        self.refresh_all()
        self.settingsDataChanged.emit()
        QMessageBox.information(
            self,
            "Kiểm tra an toàn hoàn tất",
            f"Số gói tự động Rollback: {len(res['auto_rolled_back'])}\n"
            f"Số gói quá hạn D ngày đã xóa: {len(res['purged'])}",
        )
