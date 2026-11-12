from pathlib import Path

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QSpinBox,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from greenc.config import DEFAULT_PROTECTED_PATTERNS, DEFAULT_RETENTION_DAYS
from greenc.core.relocation.base import RelocationCandidate
from greenc.core.relocation.service import RelocationService
from greenc.models import FileManifestItem, QuotaRule, VaultArchive
from greenc.ui import format_size
from greenc.ui.theme import apply_theme
from greenc.ui.widgets import PathValue, make_table, muted_label, page_title, set_cell


class QuotaRuleDialog(QDialog):
    """Create or edit a background size rule."""

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
        default_retention_days: int = DEFAULT_RETENTION_DAYS,
    ):
        super().__init__(parent)
        self.setWindowTitle("Quy tắc dung lượng")
        apply_theme(self)
        self.resize(580, 440)
        self._rule = rule
        layout = QVBoxLayout(self)
        layout.setContentsMargins(20, 18, 20, 14)
        form = QFormLayout()
        form.setVerticalSpacing(10)

        self.app_name_edit = QLineEdit(rule.app_name if rule else (prefill_app_name or ""))
        self.process_edit = QLineEdit(rule.process_name if rule else prefill_process_name)
        self.service_edit = QLineEdit(rule.service_name if rule else prefill_service_name)
        self.paths_edit = QTextEdit()
        self.paths_edit.setPlainText("\n".join(rule.target_paths) if rule else "\n".join(prefill_paths or []))
        self.paths_edit.setPlaceholderText("Mỗi dòng một thư mục")
        self.paths_edit.setMaximumHeight(90)
        browse_btn = QPushButton("Chọn thư mục…")
        browse_btn.clicked.connect(self._on_browse_folder)

        self.max_gb_spin = QDoubleSpinBox()
        self.max_gb_spin.setRange(0.001, 10000.0)
        self.max_gb_spin.setDecimals(2)
        self.max_gb_spin.setSuffix(" GB")
        self.max_gb_spin.setValue(rule.max_bytes / (1024**3) if rule else prefill_max_gb)
        self.watermark_spin = QSpinBox()
        self.watermark_spin.setRange(50, 95)
        self.watermark_spin.setSuffix(" %")
        self.watermark_spin.setValue(int((rule.watermark_ratio if rule else 0.85) * 100))
        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(1, 365)
        self.retention_spin.setSuffix(" ngày")
        self.retention_spin.setValue(rule.retention_days if rule else default_retention_days)
        self.excludes_edit = QLineEdit(
            ", ".join(rule.exclude_patterns) if rule else ", ".join(prefill_excludes or DEFAULT_PROTECTED_PATTERNS[:10])
        )
        self.enabled_chk = QCheckBox("Chạy nền")
        self.enabled_chk.setChecked(rule.enabled if rule else True)

        form.addRow("Tên ứng dụng", self.app_name_edit)
        form.addRow("Tiến trình", self.process_edit)
        form.addRow("Dịch vụ Windows", self.service_edit)
        paths_box = QVBoxLayout()
        paths_box.addWidget(self.paths_edit)
        paths_box.addWidget(browse_btn, alignment=Qt.AlignmentFlag.AlignLeft)
        form.addRow("Thư mục", paths_box)
        form.addRow("Giới hạn", self.max_gb_spin)
        form.addRow("Dọn về mức", self.watermark_spin)
        form.addRow("Giữ bản dự phòng", self.retention_spin)
        form.addRow("Không dọn các file", self.excludes_edit)
        form.addRow("", self.enabled_chk)
        layout.addLayout(form)
        layout.addWidget(muted_label(
            "Khi vượt giới hạn, các file cũ nhất không bị khóa được nén vào Kho dự phòng. "
            "Nếu ứng dụng cần lại file trong thời gian giữ, file được khôi phục tự động."
        ))
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Save | QDialogButtonBox.StandardButton.Cancel)
        buttons.button(QDialogButtonBox.StandardButton.Save).setText("Lưu")
        buttons.button(QDialogButtonBox.StandardButton.Save).setObjectName("PrimaryButton")
        buttons.button(QDialogButtonBox.StandardButton.Cancel).setText("Hủy")
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

    def _on_browse_folder(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, "Chọn thư mục")
        if folder:
            cur = self.paths_edit.toPlainText().strip()
            self.paths_edit.setPlainText(f"{cur}\n{folder}".strip())

    def get_rule(self) -> QuotaRule:
        paths = [ln.strip() for ln in self.paths_edit.toPlainText().splitlines() if ln.strip()]
        excludes = [x.strip() for x in self.excludes_edit.text().split(",") if x.strip()]
        return QuotaRule(
            id=self._rule.id if self._rule else None,
            app_name=self.app_name_edit.text().strip() or "Ứng dụng",
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
    """Files inside one Vault archive."""

    def __init__(self, archive: VaultArchive, files: list[FileManifestItem], parent: QWidget | None = None):
        super().__init__(parent)
        self.setWindowTitle(f"Gói dự phòng · {archive.app_name}")
        apply_theme(self)
        self.resize(860, 460)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(20, 18, 20, 14)
        layout.addWidget(muted_label(
            f"{len(files)} tệp · {format_size(archive.original_bytes)} → {format_size(archive.compressed_bytes)} · "
            f"{archive.status_reason}"
        ))
        table = make_table(["Đường dẫn gốc", "Kích thước", "Liên kết", "SHA-256"], stretch_col=0, right_cols=(1,))
        table.setRowCount(len(files))
        for row, item in enumerate(files):
            links = len(item.hardlink_paths) if item.hardlink_paths else 1
            set_cell(table, row, 0, PathValue(item.original_path))
            set_cell(table, row, 1, format_size(item.size_bytes), right=True)
            set_cell(table, row, 2, f"{links} đường dẫn")
            set_cell(table, row, 3, item.sha256[:16] + "…")
        layout.addWidget(table)
        close = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        close.button(QDialogButtonBox.StandardButton.Close).setText("Đóng")
        close.rejected.connect(self.reject)
        layout.addWidget(close)


class GuidedDialog(QDialog):
    """Step-by-step instructions for apps that move their own data (Zalo)."""

    def __init__(self, cand: RelocationCandidate, target_root: Path, service: RelocationService, parent=None):
        super().__init__(parent)
        self.cand = cand
        self.service = service
        self.record = None
        self.setWindowTitle(f"Chuyển dữ liệu {cand.name}")
        apply_theme(self)
        self.setMinimumWidth(520)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(22, 20, 22, 16)
        lay.setSpacing(10)
        lay.addWidget(page_title(f"Chuyển dữ liệu {cand.name}"))
        lay.addWidget(muted_label(
            f"{cand.name} tự chuyển dữ liệu của mình, Green C:\\ chỉ hướng dẫn và kiểm tra lại. "
            f"Hiện đang dùng {format_size(cand.size_bytes)} trên ổ C:."
        ))
        steps = cand.strategy.steps(target_root)
        self.steps_label = QLabel(
            "<ol style='margin-left:-20px'>" + "".join(f"<li>{s}</li>" for s in steps) + "</ol>"
        )
        self.steps_label.setTextFormat(Qt.TextFormat.RichText)
        self.steps_label.setWordWrap(True)
        lay.addWidget(self.steps_label)
        self.result_label = muted_label("")
        lay.addWidget(self.result_label)
        row = QHBoxLayout()
        self.btn_open = QPushButton(f"Mở {cand.name}")
        self.btn_open.clicked.connect(self._open)
        self.btn_verify = QPushButton("Kiểm tra lại")
        self.btn_verify.setObjectName("PrimaryButton")
        self.btn_verify.clicked.connect(self._verify)
        close = QPushButton("Đóng")
        close.clicked.connect(self.reject)
        row.addWidget(self.btn_open)
        row.addStretch(1)
        row.addWidget(close)
        row.addWidget(self.btn_verify)
        lay.addLayout(row)

    def _open(self) -> None:
        if not self.cand.strategy.open_app():
            self.result_label.setText(f"Không tìm thấy {self.cand.name} trên máy. Hãy mở bằng tay.")

    def _verify(self) -> None:
        rec = self.service.complete_guided(self.cand)
        if rec is None:
            current = self.cand.strategy.data_dir()
            self.result_label.setText(f"Chưa thấy thay đổi. Dữ liệu {self.cand.name} vẫn ở {current}.")
            return
        self.record = rec
        self.result_label.setText(f"Đã chuyển xong. Dữ liệu mới ở {rec.target_path}.")
        self.btn_verify.setEnabled(False)
