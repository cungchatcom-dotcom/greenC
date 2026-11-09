from PySide6.QtCore import Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (
    QGroupBox,
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

from diskguard.core.app_profiles import AppProfile, discover_installed_heavy_apps
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes
from diskguard.ui.dialogs import QuotaRuleDialog


class QuotaManagerTab(QWidget):
    """
    Tab 2:
    - Smart Preset bar for detected heavy apps on the user's machine (Zalo Cross-Drive & DingTalk).
    - Table of active Soft Rolling Quota Rules showing real NTFS physical usage vs naive usage,
      max allowed cap, watermark target, D-day retention, protected files count, and 1-click actions.
    """

    archiveCreated = Signal()

    def __init__(
        self,
        db: DiskGuardDB,
        enforcer: QuotaEnforcer,
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.db = db
        self.enforcer = enforcer
        self._profiles: list[AppProfile] = []

        layout = QVBoxLayout(self)
        layout.setContentsMargins(8, 8, 8, 8)
        layout.setSpacing(10)

        # Smart Presets banner for Zalo & DingTalk
        preset_group = QGroupBox(
            "🚀 Nhận diện Thông minh Ứng dụng Phình To Nhanh trên Máy (Zalo & DingTalk Smart Profiles)"
        )
        preset_layout = QHBoxLayout(preset_group)
        self.preset_info_label = QLabel("Đang kiểm tra cấu hình Zalo & DingTalk...")
        self.preset_info_label.setWordWrap(True)
        preset_layout.addWidget(self.preset_info_label, stretch=1)

        self.btn_add_zalo = QPushButton("⚡ Tạo Quota cho Zalo PC")
        self.btn_add_zalo.setStyleSheet(
            "background-color: #0284C7; color: white; font-weight: bold; padding: 6px 12px;"
        )
        self.btn_add_zalo.clicked.connect(lambda: self._apply_smart_profile("zalo_pc"))

        self.btn_add_dingtalk = QPushButton("⚡ Tạo Quota cho DingTalk")
        self.btn_add_dingtalk.setStyleSheet(
            "background-color: #2563EB; color: white; font-weight: bold; padding: 6px 12px;"
        )
        self.btn_add_dingtalk.clicked.connect(
            lambda: self._apply_smart_profile("dingtalk_pc")
        )

        self.btn_add_custom = QPushButton("➕ Tạo Rule Quota Mới")
        self.btn_add_custom.setStyleSheet(
            "background-color: #16A34A; color: white; font-weight: bold; padding: 6px 12px;"
        )
        self.btn_add_custom.clicked.connect(self.open_new_rule_dialog)

        self.btn_enforce_all = QPushButton("🧹 Kiểm tra & Thu hồi Quota Ngay")
        self.btn_enforce_all.setStyleSheet(
            "background-color: #D97706; color: white; font-weight: bold; padding: 6px 12px;"
        )
        self.btn_enforce_all.clicked.connect(self.run_all_quotas_now)

        preset_layout.addWidget(self.btn_add_zalo)
        preset_layout.addWidget(self.btn_add_dingtalk)
        preset_layout.addWidget(self.btn_add_custom)
        preset_layout.addWidget(self.btn_enforce_all)
        layout.addWidget(preset_group)

        # Quota Rules Table
        self.table = QTableWidget(0, 8)
        self.table.setHorizontalHeaderLabels(
            [
                "ID",
                "Ứng dụng / Service",
                "Thư mục Giám sát",
                "Dung lượng Vật lý Thực (Đã khử HardLink)",
                "Trần Tối đa (Max Quota)",
                "Trạng thái & % Sử dụng",
                "Giữ Zip (D ngày) / Miễn trừ",
                "Thao tác",
            ]
        )
        self.table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(4, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(5, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(6, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(7, QHeaderView.ResizeMode.ResizeToContents)
        layout.addWidget(self.table, stretch=1)

    def refresh_profiles_and_rules(self) -> None:
        self._profiles = discover_installed_heavy_apps()
        if self._profiles:
            parts = []
            for p in self._profiles:
                hl_saved = max(0, p.naive_bytes - p.physical_bytes)
                hl_note = (
                    f" [HardLink trùng: {format_bytes(hl_saved)}]"
                    if hl_saved > 1024 * 1024
                    else ""
                )
                parts.append(
                    f"• <b>{p.app_name}</b>: Thực tế <b>{format_bytes(p.physical_bytes)}</b>{hl_note}"
                )
            self.preset_info_label.setText(
                "Đã phát hiện trên máy:<br>" + "<br>".join(parts)
            )
        else:
            self.preset_info_label.setText(
                "Bạn có thể tạo Rule Quota cho bất kỳ ứng dụng hoặc Windows Service nào."
            )
        self.refresh_rules_table()

    def refresh_rules_table(self) -> None:
        rules = self.db.list_quota_rules()
        self.table.setRowCount(len(rules))

        for row_idx, rule in enumerate(rules):
            usage = self.enforcer.evaluate_rule_usage(rule)
            phys_b = usage["physical_bytes"]
            naive_b = usage["naive_bytes"]
            pct = usage["usage_pct"]

            self.table.setItem(row_idx, 0, QTableWidgetItem(str(rule.id)))
            app_label = f"{rule.app_name}\n({rule.process_name or rule.service_name})"
            self.table.setItem(row_idx, 1, QTableWidgetItem(app_label))
            self.table.setItem(
                row_idx, 2, QTableWidgetItem("\n".join(rule.target_paths))
            )

            hl_diff = max(0, naive_b - phys_b)
            phys_txt = format_bytes(phys_b)
            if hl_diff > 1024 * 1024:
                phys_txt += f"\n(Cộng thường: {format_bytes(naive_b)})"
            self.table.setItem(row_idx, 3, QTableWidgetItem(phys_txt))
            self.table.setItem(
                row_idx,
                4,
                QTableWidgetItem(
                    f"{format_bytes(rule.max_bytes)}\n(Mục tiêu dọn: {int(rule.watermark_ratio*100)}%)"
                ),
            )

            status_txt = (
                f"VƯỢT NGƯỠNG ({pct:.1f}%)"
                if phys_b > rule.max_bytes
                else f"An toàn ({pct:.1f}%)"
            )
            if not rule.enabled:
                status_txt = f"[Đang Tắt] ({pct:.1f}%)"
            st_item = QTableWidgetItem(status_txt)
            st_item.setForeground(
                QColor("#F87171")
                if phys_b > rule.max_bytes
                else QColor("#4ADE80")
            )
            self.table.setItem(row_idx, 5, st_item)

            prot_info = (
                f"D = {rule.retention_days} ngày\n"
                f"Bảo vệ: {len(rule.protected_files)} file"
            )
            self.table.setItem(row_idx, 6, QTableWidgetItem(prot_info))

            actions_widget = QWidget()
            h = QHBoxLayout(actions_widget)
            h.setContentsMargins(2, 2, 2, 2)
            h.setSpacing(4)

            btn_run = QPushButton("🧹 Thu hồi")
            btn_run.setToolTip("Nén các file cũ nhất không bị lock vào Safe Vault ngay")
            btn_run.clicked.connect(
                lambda _=False, r=rule: self._enforce_single_rule(r)
            )

            btn_edit = QPushButton("⚙️ Sửa")
            btn_edit.clicked.connect(lambda _=False, r=rule: self._edit_rule(r))

            btn_del = QPushButton("🗑️")
            btn_del.clicked.connect(
                lambda _=False, rid=rule.id: self._delete_rule(rid)
            )

            h.addWidget(btn_run)
            h.addWidget(btn_edit)
            h.addWidget(btn_del)
            self.table.setCellWidget(row_idx, 7, actions_widget)

        self.table.resizeRowsToContents()

    def _apply_smart_profile(self, app_id: str) -> None:
        prof = next((p for p in self._profiles if p.app_id == app_id), None)
        if not prof:
            QMessageBox.information(
                self,
                "Thông báo",
                "Không tìm thấy thư mục mặc định của ứng dụng này trên máy. Bạn có thể chọn thư mục thủ công.",
            )
            self.open_new_rule_dialog()
            return

        # Suggest a default max quota around current physical usage (or recommended_max_gb)
        cur_gb = prof.physical_bytes / (1024**3)
        suggested_gb = round(max(0.5, cur_gb * 0.90), 2) if cur_gb > 0.2 else prof.recommended_max_gb

        dlg = QuotaRuleDialog(
            self,
            prefill_app_name=prof.app_name,
            prefill_process_name=prof.process_name,
            prefill_service_name=prof.service_name,
            prefill_paths=prof.reclaimable_paths,
            prefill_max_gb=suggested_gb,
            prefill_excludes=prof.exclude_patterns,
        )
        if dlg.exec():
            new_rule = dlg.get_rule()
            self.db.save_quota_rule(new_rule)
            self.refresh_rules_table()

    def open_new_rule_dialog(
        self,
        app_name: str = "",
        process_name: str = "",
        service_name: str = "",
        paths: list[str] | None = None,
    ) -> None:
        dlg = QuotaRuleDialog(
            self,
            prefill_app_name=app_name,
            prefill_process_name=process_name,
            prefill_service_name=service_name,
            prefill_paths=paths,
        )
        if dlg.exec():
            rule = dlg.get_rule()
            self.db.save_quota_rule(rule)
            self.refresh_rules_table()

    def _edit_rule(self, rule: QuotaRule) -> None:
        dlg = QuotaRuleDialog(self, rule=rule)
        if dlg.exec():
            updated = dlg.get_rule()
            self.db.save_quota_rule(updated)
            self.refresh_rules_table()

    def _delete_rule(self, rule_id: int | None) -> None:
        if rule_id is None:
            return
        self.db.delete_quota_rule(rule_id)
        self.refresh_rules_table()

    def _enforce_single_rule(self, rule: QuotaRule) -> None:
        arch = self.enforcer.enforce_rule(rule)
        self.refresh_rules_table()
        if arch:
            self.archiveCreated.emit()
            QMessageBox.information(
                self,
                "Đã Nén Thu Hồi Thành Công",
                f"Đã nén {arch.file_count} file cũ nhất (unlocked) của '{rule.app_name}' vào Safe Vault:\n"
                f"• Dung lượng gốc thu hồi: {format_bytes(arch.original_bytes)}\n"
                f"• Kích thước gói .zip: {format_bytes(arch.compressed_bytes)}\n"
                f"• Hạn tự xóa an toàn: {arch.expires_at[:10]}",
            )
        else:
            QMessageBox.information(
                self,
                "Dung lượng trong ngưỡng an toàn",
                f"Ứng dụng '{rule.app_name}' hiện chưa vượt trần Quota hoặc mọi file đều đang mở/được bảo vệ.",
            )

    def run_all_quotas_now(self) -> None:
        created = self.enforcer.enforce_all_rules()
        self.refresh_rules_table()
        if created:
            self.archiveCreated.emit()
            total_freed = sum(a.original_bytes for a in created)
            QMessageBox.information(
                self,
                "Thu hồi Quota Hoàn tất",
                f"Đã tạo {len(created)} gói .zip trong Safe Vault, thu hồi tổng cộng {format_bytes(total_freed)}!",
            )
        else:
            QMessageBox.information(
                self,
                "Kiểm tra Quota",
                "Tất cả các Rule hiện đều nằm trong ngưỡng cho phép.",
            )
