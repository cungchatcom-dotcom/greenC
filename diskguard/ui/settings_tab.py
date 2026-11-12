from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import (
    APP_NAME,
    APP_VERSION,
    AUTHOR,
    MAX_RETENTION_DAYS,
    MIN_RETENTION_DAYS,
    WEBSITE_LABEL,
    WEBSITE_URL,
)
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.settings import AppSettings
from diskguard.core.simulator import SandboxSimulator
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.dialogs import QuotaRuleDialog
from diskguard.ui.widgets import (
    PathCell,
    PathValue,
    TagValue,
    card_title,
    make_card,
    make_table,
    muted_label,
    page_title,
    set_cell,
)


class SettingsTab(QWidget):
    settingsDataChanged = Signal()
    dataRootChangeRequested = Signal()

    def __init__(
        self,
        db: DiskGuardDB,
        enforcer: QuotaEnforcer,
        watchdog: SafetyWatchdog,
        simulator: SandboxSimulator,
        parent: QWidget | None = None,
        settings: AppSettings | None = None,
    ):
        super().__init__(parent)
        self.db = db
        self.enforcer = enforcer
        self.watchdog = watchdog
        self.simulator = simulator
        self.settings = settings or AppSettings(db)

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        scroll = QScrollArea()
        scroll.setObjectName("PageScroll")
        scroll.setWidgetResizable(True)
        body = QWidget()
        body.setObjectName("PageBody")
        scroll.setWidget(body)
        outer.addWidget(scroll)
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)
        lay.addWidget(page_title("Cài đặt"))

        # Data location
        loc = make_card()
        ll = QVBoxLayout(loc)
        ll.setContentsMargins(14, 12, 14, 12)
        ll.addWidget(card_title("Vị trí lưu dữ liệu"))
        row = QHBoxLayout()
        self._root_holder = QHBoxLayout()
        row.addLayout(self._root_holder, 1)
        self.btn_change_root = QPushButton("Đổi…")
        self.btn_change_root.clicked.connect(self.dataRootChangeRequested.emit)
        row.addWidget(self.btn_change_root)
        ll.addLayout(row)
        self.data_root_hint = muted_label("")
        ll.addWidget(self.data_root_hint)
        lay.addWidget(loc)

        # Retention
        ret = make_card()
        rl = QVBoxLayout(ret)
        rl.setContentsMargins(14, 12, 14, 12)
        rl.addWidget(card_title("Thời gian giữ bản dự phòng"))
        rrow = QHBoxLayout()
        rrow.addWidget(QLabel("Giữ bản dự phòng trong"))
        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
        self.retention_spin.setValue(self.settings.retention_days_default)
        self.retention_spin.setFixedWidth(70)
        self.retention_spin.valueChanged.connect(self._on_retention_changed)
        rrow.addWidget(self.retention_spin)
        rrow.addWidget(QLabel("ngày trước khi xóa vĩnh viễn"))
        rrow.addStretch(1)
        rl.addLayout(rrow)
        rl.addWidget(muted_label("Áp dụng cho quy tắc mới và lần dọn dẹp tiếp theo."))
        lay.addWidget(ret)

        # Rules
        rules = make_card()
        rul = QVBoxLayout(rules)
        rul.setContentsMargins(14, 12, 14, 12)
        rhead = QHBoxLayout()
        rhead.addWidget(card_title("Quy tắc chạy nền"))
        rhead.addStretch(1)
        btn_new = QPushButton("Thêm quy tắc")
        btn_new.clicked.connect(lambda: self.open_new_rule_dialog())
        rhead.addWidget(btn_new)
        rul.addLayout(rhead)
        self.rules_info_label = muted_label("")
        rul.addWidget(self.rules_info_label)
        self.rules_table = make_table(
            ["Ứng dụng", "Thư mục", "Đang dùng", "Giới hạn", "Trạng thái", "Giữ", ""], stretch_col=1, right_cols=(2, 3)
        )
        self.rules_table.setMinimumHeight(240)
        rul.addWidget(self.rules_table)
        lay.addWidget(rules)

        # About
        about = make_card()
        al = QVBoxLayout(about)
        al.setContentsMargins(14, 12, 14, 12)
        al.addWidget(card_title("Giới thiệu"))
        self.about_label = QLabel(
            f"{APP_NAME} {APP_VERSION}<br>Tác giả: <b>{AUTHOR}</b> · "
            f'<a href="{WEBSITE_URL}">{WEBSITE_LABEL}</a>'
        )
        self.about_label.setTextFormat(Qt.TextFormat.RichText)
        self.about_label.setOpenExternalLinks(True)
        al.addWidget(self.about_label)
        lay.addWidget(about)
        lay.addStretch(1)

        self.refresh_data_root()

    def refresh_all(self) -> None:
        self.refresh_rules_table()
        self.refresh_data_root()

    def refresh_data_root(self) -> None:
        while self._root_holder.count():
            w = self._root_holder.takeAt(0).widget()
            if w is not None:
                w.deleteLater()
        root = self.settings.data_root
        if root is None:
            self._root_holder.addWidget(QLabel("Chưa chọn"))
            self.data_root_hint.setText(
                "Kho dự phòng đang ở thư mục tạm trên ổ C:. Chọn một ổ khác để giải phóng ổ C:."
            )
        else:
            self._root_holder.addWidget(PathCell(str(root)))
            self.data_root_hint.setText("Kho dự phòng và dữ liệu ứng dụng được chuyển vào thư mục này.")

    def _on_retention_changed(self, days: int) -> None:
        self.settings.set_retention_days_default(days)
        self.settingsDataChanged.emit()

    def refresh_rules_table(self) -> None:
        rules = self.db.list_quota_rules()
        self.rules_info_label.setText(f"{len(rules)} quy tắc · kiểm tra mỗi 45 giây")
        self.rules_table.setRowCount(len(rules))
        for row, rule in enumerate(rules):
            usage = self.enforcer.evaluate_rule_usage(rule)
            phys, pct = usage["physical_bytes"], usage["usage_pct"]
            if not rule.enabled:
                status = TagValue("Tạm tắt")
            elif phys > rule.max_bytes:
                status = TagValue(f"Vượt mức · {pct:.0f}%", warning=True)
            else:
                status = TagValue(f"Ổn định · {pct:.0f}%")
            set_cell(self.rules_table, row, 0, rule.app_name)
            set_cell(self.rules_table, row, 1, PathValue(rule.target_paths[0] if rule.target_paths else ""))
            set_cell(self.rules_table, row, 2, format_size(phys), right=True)
            set_cell(self.rules_table, row, 3, format_size(rule.max_bytes), right=True)
            set_cell(self.rules_table, row, 4, status)
            set_cell(self.rules_table, row, 5, f"{rule.retention_days} ngày")
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 4, 2)
            h.setSpacing(6)
            btn_edit = QPushButton("Sửa")
            btn_edit.clicked.connect(lambda _=False, r=rule: self._edit_rule(r))
            btn_del = QPushButton("Xóa")
            btn_del.clicked.connect(lambda _=False, r=rule: self._delete_rule(r))
            h.addWidget(btn_edit)
            h.addWidget(btn_del)
            self.rules_table.setCellWidget(row, 6, wrap)

    def open_new_rule_dialog(
        self, app_name: str = "", process_name: str = "", service_name: str = "", paths: list[str] | None = None
    ) -> None:
        dlg = QuotaRuleDialog(
            self, prefill_app_name=app_name, prefill_process_name=process_name,
            prefill_service_name=service_name, prefill_paths=paths,
            default_retention_days=self.settings.retention_days_default,
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

    def _delete_rule(self, rule: QuotaRule) -> None:
        if rule.id is None:
            return
        answer = QMessageBox.question(self, "Xóa quy tắc", f"Xóa quy tắc của {rule.app_name}?")
        if answer != QMessageBox.StandardButton.Yes:
            return
        self.db.delete_quota_rule(rule.id)
        self.refresh_rules_table()
        self.settingsDataChanged.emit()
