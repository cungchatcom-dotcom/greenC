from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFileDialog,
    QFormLayout,
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
    """Clean dialog to create or edit a background Quota Rule in Settings > Rules."""

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
        self.setWindowTitle("Thiết lập Quy tắc Dung lượng Ứng dụng")
        self.resize(580, 440)
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
        self.paths_edit.setPlaceholderText("Mỗi dòng một đường dẫn thư mục...")
        self.paths_edit.setMaximumHeight(90)

        browse_btn = QPushButton("Chọn thư mục...")
        browse_btn.setObjectName("SecondaryButton")
        browse_btn.clicked.connect(self._on_browse_folder)

        self.max_gb_spin = QDoubleSpinBox()
        self.max_gb_spin.setRange(0.001, 10000.0)
        self.max_gb_spin.setDecimals(2)
        self.max_gb_spin.setSuffix(" GB")
        if rule:
            self.max_gb_spin.setValue(rule.max_bytes / (1024**3))
        else:
            self.max_gb_spin.setValue(prefill_max_gb)

        self.watermark_spin = QSpinBox()
        self.watermark_spin.setRange(50, 95)
        self.watermark_spin.setSuffix(" %")
        self.watermark_spin.setValue(
            int((rule.watermark_ratio if rule else 0.85) * 100)
        )

        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(1, 365)
        self.retention_spin.setSuffix(" ngày")
        self.retention_spin.setValue(rule.retention_days if rule else 7)

        excludes_init = (
            ", ".join(rule.exclude_patterns)
            if rule
            else ", ".join(prefill_excludes or DEFAULT_PROTECTED_PATTERNS[:10])
        )
        self.excludes_edit = QLineEdit(excludes_init)

        self.enabled_chk = QCheckBox("Tự động chạy ngầm quy tắc này")
        self.enabled_chk.setChecked(rule.enabled if rule else True)

        form.addRow("Tên ứng dụng:", self.app_name_edit)
        form.addRow("Tiến trình (.exe):", self.process_edit)
        form.addRow("Windows Service:", self.service_edit)

        path_box = QVBoxLayout()
        path_box.addWidget(self.paths_edit)
        path_box.addWidget(browse_btn, alignment=Qt.AlignmentFlag.AlignLeft)
        form.addRow("Thư mục giám sát:", path_box)

        form.addRow("Dung lượng tối đa:", self.max_gb_spin)
        form.addRow("Mức thu hồi mục tiêu:", self.watermark_spin)
        form.addRow("Giữ bản nén dự phòng:", self.retention_spin)
        form.addRow("Mẫu file miễn trừ:", self.excludes_edit)
        form.addRow("", self.enabled_chk)

        layout.addLayout(form)

        note = QLabel(
            "Khi vượt ngưỡng cho phép, hệ thống sẽ tự động nén các file cũ nhất không bị khóa vào Kho dự phòng "
            "và tự động khôi phục (Rollback) nếu phát hiện ứng dụng cần dùng lại file trong D ngày."
        )
        note.setWordWrap(True)
        note.setStyleSheet("color: #6E6E73; padding: 4px 0;")
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
            app_name=self.app_name_edit.text().strip() or "App Rule",
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
    """Clean dialog inspecting manifest files inside a Safe Vault archive."""

    def __init__(
        self,
        archive: VaultArchive,
        files: list[FileManifestItem],
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.setWindowTitle(f"Chi tiết gói dự phòng #{archive.id} — {archive.app_name}")
        self.resize(820, 440)

        layout = QVBoxLayout(self)
        summary = QLabel(
            f"<b>Ứng dụng:</b> {archive.app_name} &nbsp;|&nbsp; "
            f"<b>Đường dẫn gói nén:</b> {archive.zip_path}<br>"
            f"<b>Dung lượng gốc:</b> {format_bytes(archive.original_bytes)} &rarr; "
            f"<b>Sau nén:</b> {format_bytes(archive.compressed_bytes)} &nbsp;|&nbsp; "
            f"<b>Trạng thái:</b> {archive.status_reason}"
        )
        summary.setWordWrap(True)
        layout.addWidget(summary)

        table = QTableWidget(len(files), 4)
        table.setHorizontalHeaderLabels(
            [
                "Đường dẫn gốc",
                "Kích thước",
                "Liên kết HardLink",
                "Mã kiểm tra SHA-256",
            ]
        )
        table.verticalHeader().setVisible(False)
        table.setShowGrid(False)
        table.setAlternatingRowColors(True)
        table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.ResizeToContents)
        table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.ResizeToContents)
        table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.ResizeToContents)

        for row_idx, item in enumerate(files):
            hl_count = len(item.hardlink_paths) if item.hardlink_paths else 1
            hl_text = f"{hl_count} đường dẫn" if hl_count > 1 else "1 đường dẫn"
            table.setItem(row_idx, 0, QTableWidgetItem(item.original_path))
            table.setItem(row_idx, 1, QTableWidgetItem(format_bytes(item.size_bytes)))
            table.setItem(row_idx, 2, QTableWidgetItem(hl_text))
            table.setItem(row_idx, 3, QTableWidgetItem(item.sha256[:16] + "..."))

        layout.addWidget(table)

        close_btn = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        close_btn.rejected.connect(self.reject)
        layout.addWidget(close_btn)
