from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QPushButton,
    QSpinBox,
    QTableWidget,
    QTableWidgetItem,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import DEFAULT_PROTECTED_PATTERNS
from diskguard.models import FileManifestItem, QuotaRule, VaultArchive
from diskguard.ui import format_bytes


class QuotaRuleDialog(QDialog):
    """Dialog to create or edit a Soft Rolling Quota Rule for an application/service."""

    def __init__(
        self,
        parent: QWidget | None = None,
        rule: QuotaRule | None = None,
        prefill_app_name: str = "",
        prefill_process_name: str = "",
        prefill_service_name: str = "",
        prefill_paths: list[str] | None = None,
        prefill_max_gb: float = 2.0,
        prefill_excludes: list[str] | None = None,
    ):
        super().__init__(parent)
        self.setWindowTitle(
            "Thiết lập Giới hạn Dung lượng Xoay vòng (Soft Rolling Quota)"
        )
        self.resize(620, 480)
        self._rule = rule

        layout = QVBoxLayout(self)
        form = QFormLayout()

        self.app_name_edit = QLineEdit(
            rule.app_name if rule else (prefill_app_name or "Ứng dụng giám sát")
        )
        self.process_edit = QLineEdit(
            rule.process_name if rule else prefill_process_name
        )
        self.service_edit = QLineEdit(
            rule.service_name if rule else prefill_service_name
        )

        paths_init = (
            "\n".join(rule.target_paths)
            if rule
            else "\n".join(prefill_paths or [])
        )
        self.paths_edit = QTextEdit()
        self.paths_edit.setPlainText(paths_init)
        self.paths_edit.setPlaceholderText("Mỗi dòng 1 đường dẫn thư mục cần giới hạn dung lượng...")
        self.paths_edit.setMaximumHeight(95)

        browse_btn = QPushButton("📁 Thêm thư mục...")
        browse_btn.clicked.connect(self._on_browse_folder)

        self.max_gb_spin = QDoubleSpinBox()
        self.max_gb_spin.setRange(0.001, 10000.0)
        self.max_gb_spin.setDecimals(3)
        self.max_gb_spin.setSuffix(" GB")
        if rule:
            self.max_gb_spin.setValue(rule.max_bytes / (1024**3))
        else:
            self.max_gb_spin.setValue(prefill_max_gb)

        self.watermark_spin = QSpinBox()
        self.watermark_spin.setRange(50, 95)
        self.watermark_spin.setSuffix(" % trần Quota (Mức thu hồi về)")
        self.watermark_spin.setValue(
            int((rule.watermark_ratio if rule else 0.85) * 100)
        )

        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(1, 365)
        self.retention_spin.setSuffix(" ngày (Giữ bản .zip trước khi tự xóa)")
        self.retention_spin.setValue(rule.retention_days if rule else 7)

        excludes_init = (
            ", ".join(rule.exclude_patterns)
            if rule
            else ", ".join(prefill_excludes or DEFAULT_PROTECTED_PATTERNS[:10])
        )
        self.excludes_edit = QLineEdit(excludes_init)

        self.enabled_chk = QCheckBox("Kích hoạt giám sát và tự động nén xoay vòng")
        self.enabled_chk.setChecked(rule.enabled if rule else True)

        form.addRow("Tên Ứng dụng / Nhóm:", self.app_name_edit)
        form.addRow("Tên Tiến trình (.exe):", self.process_edit)
        form.addRow("Tên Windows Service (nếu có):", self.service_edit)

        path_box = QVBoxLayout()
        path_box.addWidget(self.paths_edit)
        path_box.addWidget(browse_btn, alignment=Qt.AlignmentFlag.AlignLeft)
        form.addRow("Thư mục áp dụng Quota:", path_box)

        form.addRow("Dung lượng tối đa cho phép:", self.max_gb_spin)
        form.addRow("Ngưỡng thu hồi an toàn:", self.watermark_spin)
        form.addRow("Số ngày D giữ bản Zip dự phòng:", self.retention_spin)
        form.addRow("Mẫu file bảo vệ (không nén):", self.excludes_edit)
        form.addRow("", self.enabled_chk)

        layout.addLayout(form)

        note = QLabel(
            "💡 Khi tổng dung lượng vật lý vượt mức tối đa, DiskGuard chỉ chọn các file CŨ NHẤT hiện KHÔNG BỊ KHÓA (unlocked) "
            "để nén vào Safe Vault (.zip). Nếu trong D ngày phát hiện ứng dụng báo lỗi thiếu file hoặc Service dừng bất thường, "
            "hệ thống sẽ TỰ ĐỘNG ROLLBACK giải nén trả lại ngay lập tức (kèm cả liên kết NTFS HardLink)."
        )
        note.setWordWrap(True)
        note.setStyleSheet("color: #93C5FD; padding: 6px;")
        layout.addWidget(note)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

    def _on_browse_folder(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, "Chọn thư mục của ứng dụng")
        if folder:
            cur = self.paths_edit.toPlainText().strip()
            self.paths_edit.setPlainText(f"{cur}\n{folder}".strip())

    def get_rule(self) -> QuotaRule:
        paths = [
            line.strip()
            for line in self.paths_edit.toPlainText().splitlines()
            if line.strip()
        ]
        excludes = [
            x.strip()
            for x in self.excludes_edit.text().split(",")
            if x.strip()
        ]
        return QuotaRule(
            id=self._rule.id if self._rule else None,
            app_name=self.app_name_edit.text().strip() or "App Quota Rule",
            process_name=self.process_edit.text().strip(),
            service_name=self.service_edit.text().strip(),
            target_paths=paths,
            max_bytes=int(self.max_gb_spin.value() * (1024**3)),
            watermark_ratio=self.watermark_spin.value() / 100.0,
            retention_days=self.retention_spin.value(),
            min_file_age_seconds=self._rule.min_file_age_seconds if self._rule else 60,
            exclude_patterns=excludes,
            protected_files=list(self._rule.protected_files) if self._rule else [],
            enabled=self.enabled_chk.isChecked(),
            created_at=self._rule.created_at if self._rule else "",
        )


class ArchiveDetailsDialog(QDialog):
    """Shows the manifest files and NTFS HardLink siblings inside a Vault .zip archive."""

    def __init__(
        self,
        archive: VaultArchive,
        files: list[FileManifestItem],
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.setWindowTitle(f"Chi tiết Gói Nén Safe Vault #{archive.id} — {archive.app_name}")
        self.resize(860, 480)

        layout = QVBoxLayout(self)
        summary = QLabel(
            f"<b>Ứng dụng:</b> {archive.app_name} &nbsp;|&nbsp; "
            f"<b>File Zip:</b> {archive.zip_path}<br>"
            f"<b>Dung lượng gốc:</b> {format_bytes(archive.original_bytes)} &rarr; "
            f"<b>Sau nén:</b> {format_bytes(archive.compressed_bytes)} &nbsp;|&nbsp; "
            f"<b>Trạng thái:</b> {archive.status} ({archive.status_reason})"
        )
        summary.setWordWrap(True)
        layout.addWidget(summary)

        table = QTableWidget(len(files), 4)
        table.setHorizontalHeaderLabels(
            [
                "Đường dẫn gốc (Primary Path)",
                "Kích thước",
                "Số nhánh HardLink (Zalo/NTFS)",
                "SHA-256",
            ]
        )
        table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)

        for row_idx, item in enumerate(files):
            hl_count = len(item.hardlink_paths) if item.hardlink_paths else 1
            hl_text = (
                f"{hl_count} liên kết ({', '.join(item.hardlink_paths)})"
                if hl_count > 1
                else "1 (File đơn)"
            )
            table.setItem(row_idx, 0, QTableWidgetItem(item.original_path))
            table.setItem(row_idx, 1, QTableWidgetItem(format_bytes(item.size_bytes)))
            table.setItem(row_idx, 2, QTableWidgetItem(hl_text))
            table.setItem(row_idx, 3, QTableWidgetItem(item.sha256[:16] + "..."))

        layout.addWidget(table)

        close_btn = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        close_btn.rejected.connect(self.reject)
        layout.addWidget(close_btn)
