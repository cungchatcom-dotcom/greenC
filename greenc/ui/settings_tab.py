from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QComboBox,
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from greenc.config import (
    APP_NAME,
    APP_VERSION,
    AUTHOR,
    MAX_RETENTION_DAYS,
    MIN_RETENTION_DAYS,
    WEBSITE_LABEL,
    WEBSITE_URL,
)
from greenc.i18n import LANGUAGES, N_, current_language, load_catalog, tr
from greenc.core.quota_enforcer import QuotaEnforcer
from greenc.core.safety_watchdog import SafetyWatchdog
from greenc.core.settings import AppSettings
from greenc.core.simulator import SandboxSimulator
from greenc.models import QuotaRule
from greenc.storage.database import GreenCDB
from greenc.ui import format_size
from greenc.ui.dialogs import QuotaRuleDialog
from greenc.ui.restart import restart_application
from greenc.ui.widgets import (
    PathCell,
    PathValue,
    TagValue,
    fit_widget_columns,
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
        db: GreenCDB,
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
        lay.addWidget(page_title(tr("Cài đặt")))

        # Language
        lang = make_card()
        lgl = QVBoxLayout(lang)
        lgl.setContentsMargins(14, 12, 14, 12)
        lgl.addWidget(card_title(tr("Ngôn ngữ")))
        lrow = QHBoxLayout()
        self.language_combo = QComboBox()
        self.language_combo.setMinimumWidth(260)
        for code, native, english in LANGUAGES:
            label = native if native == english else f"{native} — {english}"
            self.language_combo.addItem(label, code)
        self.language_combo.setCurrentIndex(max(0, self.language_combo.findData(current_language())))
        self.language_combo.currentIndexChanged.connect(self._on_language_changed)
        lrow.addWidget(self.language_combo)
        self.btn_restart = QPushButton(tr("Khởi động lại"))
        self.btn_restart.setObjectName("PrimaryButton")
        self.btn_restart.setVisible(False)
        self.btn_restart.clicked.connect(restart_application)
        lrow.addWidget(self.btn_restart)
        lrow.addStretch(1)
        lgl.addLayout(lrow)
        self.language_hint = muted_label("")
        self.language_hint.setVisible(False)
        lgl.addWidget(self.language_hint)
        lay.addWidget(lang)

        # Data location
        loc = make_card()
        ll = QVBoxLayout(loc)
        ll.setContentsMargins(14, 12, 14, 12)
        ll.addWidget(card_title(tr("Vị trí lưu dữ liệu")))
        row = QHBoxLayout()
        self._root_holder = QHBoxLayout()
        row.addLayout(self._root_holder, 1)
        self.btn_change_root = QPushButton(tr("Đổi…"))
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
        rl.addWidget(card_title(tr("Thời gian giữ bản dự phòng")))
        rrow = QHBoxLayout()
        rrow.addWidget(QLabel(tr("Giữ bản dự phòng trong")))
        self.retention_spin = QSpinBox()
        self.retention_spin.setLayoutDirection(Qt.LayoutDirection.LeftToRight)  # numbers stay LTR in RTL UIs
        self.retention_spin.setRange(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
        self.retention_spin.setValue(self.settings.retention_days_default)
        self.retention_spin.setFixedWidth(70)
        self.retention_spin.valueChanged.connect(self._on_retention_changed)
        rrow.addWidget(self.retention_spin)
        rrow.addWidget(QLabel(tr("ngày trước khi xóa vĩnh viễn")))
        rrow.addStretch(1)
        rl.addLayout(rrow)
        rl.addWidget(muted_label(tr("Áp dụng cho quy tắc mới và lần dọn dẹp tiếp theo.")))
        lay.addWidget(ret)

        # Rules
        rules = make_card()
        rul = QVBoxLayout(rules)
        rul.setContentsMargins(14, 12, 14, 12)
        rhead = QHBoxLayout()
        rhead.addWidget(card_title(tr("Quy tắc chạy nền")))
        rhead.addStretch(1)
        btn_new = QPushButton(tr("Thêm quy tắc"))
        btn_new.clicked.connect(lambda: self.open_new_rule_dialog())
        rhead.addWidget(btn_new)
        rul.addLayout(rhead)
        self.rules_info_label = muted_label("")
        rul.addWidget(self.rules_info_label)
        self.rules_table = make_table(
            [tr("Ứng dụng"), tr("Thư mục"), tr("Đang dùng"), tr("Giới hạn"), tr("Trạng thái"), tr("Giữ"), ""], stretch_col=1, right_cols=(2, 3)
        )
        self.rules_table.setMinimumHeight(240)
        rul.addWidget(self.rules_table)
        lay.addWidget(rules)

        # About
        about = make_card()
        al = QVBoxLayout(about)
        al.setContentsMargins(14, 12, 14, 12)
        al.addWidget(card_title(tr("Giới thiệu")))
        self.about_label = QLabel(
            f"{APP_NAME} {APP_VERSION}<br>"
            + tr("Tác giả: {author}", author=f"<b>{AUTHOR}</b>")
            + f' · <a href="{WEBSITE_URL}">{WEBSITE_LABEL}</a>'
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
            self._root_holder.addWidget(QLabel(tr("Chưa chọn")))
            self.data_root_hint.setText(
                tr("Kho dự phòng đang ở thư mục tạm trên ổ C:. Chọn một ổ khác để giải phóng ổ C:.")
            )
        else:
            self._root_holder.addWidget(PathCell(str(root)))
            self.data_root_hint.setText(tr("Kho dự phòng và dữ liệu ứng dụng được chuyển vào thư mục này."))

    def _on_language_changed(self, _index: int) -> None:
        code = self.language_combo.currentData()
        self.settings.set_language(code)
        pending = code != current_language()
        self.btn_restart.setVisible(pending)
        self.language_hint.setVisible(pending)
        if pending:
            # Shown in the newly chosen language so the user can read it.
            catalog = load_catalog(code)
            hint = N_("Ngôn ngữ mới được áp dụng sau khi khởi động lại.")
            restart = N_("Khởi động lại")
            self.language_hint.setText(catalog.get(hint, hint))
            self.btn_restart.setText(catalog.get(restart, restart))

    def _on_retention_changed(self, days: int) -> None:
        self.settings.set_retention_days_default(days)
        self.settingsDataChanged.emit()

    def refresh_rules_table(self) -> None:
        rules = self.db.list_quota_rules()
        self.rules_info_label.setText(tr("{count} quy tắc · kiểm tra mỗi {seconds} giây", count=len(rules), seconds=45))
        self.rules_table.setRowCount(len(rules))
        for row, rule in enumerate(rules):
            usage = self.enforcer.evaluate_rule_usage(rule)
            phys, pct = usage["physical_bytes"], usage["usage_pct"]
            if not rule.enabled:
                status = TagValue(tr("Tạm tắt"))
            elif phys > rule.max_bytes:
                status = TagValue(tr("Vượt mức · {pct:.0f}%", pct=pct), warning=True)
            else:
                status = TagValue(tr("Ổn định · {pct:.0f}%", pct=pct))
            set_cell(self.rules_table, row, 0, tr(rule.app_name))
            set_cell(self.rules_table, row, 1, PathValue(rule.target_paths[0] if rule.target_paths else ""))
            set_cell(self.rules_table, row, 2, format_size(phys), right=True)
            set_cell(self.rules_table, row, 3, format_size(rule.max_bytes), right=True)
            set_cell(self.rules_table, row, 4, status)
            set_cell(self.rules_table, row, 5, tr("{days} ngày", days=rule.retention_days))
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 4, 2)
            h.setSpacing(6)
            btn_edit = QPushButton(tr("Sửa"))
            btn_edit.clicked.connect(lambda _=False, r=rule: self._edit_rule(r))
            btn_del = QPushButton(tr("Xóa"))
            btn_del.clicked.connect(lambda _=False, r=rule: self._delete_rule(r))
            h.addWidget(btn_edit)
            h.addWidget(btn_del)
            self.rules_table.setCellWidget(row, 6, wrap)
        fit_widget_columns(self.rules_table)

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
        answer = QMessageBox.question(
            self, tr("Xóa quy tắc"), tr("Xóa quy tắc của {app}?", app=rule.app_name)
        )
        if answer != QMessageBox.StandardButton.Yes:
            return
        self.db.delete_quota_rule(rule.id)
        self.refresh_rules_table()
        self.settingsDataChanged.emit()
