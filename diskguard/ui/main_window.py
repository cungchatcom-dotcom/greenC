import ctypes
from pathlib import Path
import shutil

from PySide6.QtCore import QTimer
from PySide6.QtGui import QAction
from PySide6.QtWidgets import (
    QApplication,
    QComboBox,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMenu,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QStyle,
    QSystemTrayIcon,
    QTabWidget,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import (
    DEFAULT_DB_PATH,
    DEFAULT_VAULT_DIR,
    SANDBOX_DEMO_DIR,
    ensure_app_dirs,
)
from diskguard.core.disk_tracker import DiskTracker
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.simulator import SandboxSimulator
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes
from diskguard.ui.daily_tab import DailyMonitorTab
from diskguard.ui.quota_tab import QuotaManagerTab
from diskguard.ui.vault_tab import SafeVaultTab


class MainWindow(QMainWindow):
    """
    Main PySide6 window for DiskGuard:
    - Drive selector + real-time disk usage progress bar.
    - 1-click real drive & Zalo/DingTalk scanner + 1-click Sandbox Demo.
    - 3 tabs: Daily Monitor & Drill-down, Application Quota Rules, Safe Vault & Rollback.
    - System Tray integration and background monitoring timer.
    """

    def __init__(
        self,
        db: DiskGuardDB | None = None,
        vault_dir: Path | str | None = None,
        sandbox_dir: Path | str | None = None,
        auto_scan_on_init: bool = True,
    ):
        super().__init__()
        ensure_app_dirs()

        self.db = db or DiskGuardDB(DEFAULT_DB_PATH)
        self.vault_dir = Path(vault_dir) if vault_dir else DEFAULT_VAULT_DIR
        self.sandbox_dir = Path(sandbox_dir) if sandbox_dir else SANDBOX_DEMO_DIR

        self.tracker = DiskTracker(self.db)
        self.enforcer = QuotaEnforcer(self.db, self.vault_dir)
        self.watchdog = SafetyWatchdog(self.db)
        self.simulator = SandboxSimulator(self.db, self.sandbox_dir, self.vault_dir)

        self.setWindowTitle(
            "DiskGuard — Theo Dõi Biến Động Ổ Cứng, Truy Vết Tác Nhân (Zalo/DingTalk) & Quản Lý Quota An Toàn"
        )
        self.resize(1340, 820)
        self._apply_dark_theme()

        central = QWidget(self)
        self.setCentralWidget(central)
        main_layout = QVBoxLayout(central)
        main_layout.setContentsMargins(12, 12, 12, 12)
        main_layout.setSpacing(10)

        # Top control bar
        header_bar = QHBoxLayout()
        header_bar.addWidget(QLabel("<b>Chọn Ổ Đĩa:</b>"))

        self.drive_combo = QComboBox()
        drives = self.tracker.list_available_drives()
        self.drive_combo.addItems(drives)
        self.drive_combo.currentTextChanged.connect(self._on_drive_changed)
        header_bar.addWidget(self.drive_combo)

        self.drive_usage_bar = QProgressBar()
        self.drive_usage_bar.setMinimumWidth(260)
        header_bar.addWidget(self.drive_usage_bar)

        self.drive_stats_label = QLabel("")
        header_bar.addWidget(self.drive_stats_label, stretch=1)

        is_admin = self._is_running_as_admin()
        admin_badge = QLabel(
            "🛡️ Quyền: Administrator" if is_admin else "ℹ️ Quyền: User Standard"
        )
        admin_badge.setStyleSheet(
            "color: #4ADE80; font-weight: bold;"
            if is_admin
            else "color: #FBBF24; font-weight: bold;"
        )
        header_bar.addWidget(admin_badge)

        btn_scan = QPushButton("🔍 Quét Ổ Đĩa & Zalo/DingTalk")
        btn_scan.setStyleSheet(
            "background-color: #0284C7; color: white; font-weight: bold; padding: 6px 12px;"
        )
        btn_scan.clicked.connect(self.scan_current_drive)
        header_bar.addWidget(btn_scan)

        btn_demo = QPushButton("🧪 Chạy Mô Phỏng Thực Tế (Sandbox Demo)")
        btn_demo.setToolTip(
            "Tạo kịch bản Zalo HardLink Media vượt trần Quota -> Tự nén vào Safe Vault -> Sẵn sàng test Auto-Rollback"
        )
        btn_demo.setStyleSheet(
            "background-color: #7C3AED; color: white; font-weight: bold; padding: 6px 12px;"
        )
        btn_demo.clicked.connect(lambda: self.run_sandbox_demo(silent=False))
        header_bar.addWidget(btn_demo)

        main_layout.addLayout(header_bar)

        # Tabs
        self.tabs = QTabWidget()
        self.daily_tab = DailyMonitorTab(self.db, self)
        self.quota_tab = QuotaManagerTab(self.db, self.enforcer, self)
        self.vault_tab = SafeVaultTab(self.db, self.watchdog, self.simulator, self)

        self.tabs.addTab(
            self.daily_tab,
            "📊 1. Biểu Đồ Ngày & Truy Vết Nguyên Nhân / Ứng Dụng",
        )
        self.tabs.addTab(
            self.quota_tab,
            "⚖️ 2. Giới Hạn Quota Xoay Vòng (Zalo / DingTalk / App)",
        )
        self.tabs.addTab(
            self.vault_tab,
            "📦 3. Kho Nén Tạm (Safe Vault) & Tự Động Rollback (D Ngày)",
        )

        self.daily_tab.requestCreateQuota.connect(self._on_request_create_quota)
        self.quota_tab.archiveCreated.connect(self.vault_tab.refresh_vault_table)
        self.vault_tab.vaultChanged.connect(self.quota_tab.refresh_rules_table)

        main_layout.addWidget(self.tabs, stretch=1)

        self._setup_system_tray()

        # Background periodic timer (every 30s) for Quota & Safety Watchdog
        self._bg_timer = QTimer(self)
        self._bg_timer.timeout.connect(self._run_background_cycle)
        self._bg_timer.start(30_000)

        self._update_drive_header(self.drive_combo.currentText())
        self.vault_tab.refresh_vault_table()

        if auto_scan_on_init:
            self.scan_current_drive()
        else:
            self.quota_tab.refresh_rules_table()

    def closeEvent(self, event) -> None:  # noqa: N802
        self._bg_timer.stop()
        if hasattr(self, "tray_icon") and self.tray_icon is not None:
            self.tray_icon.hide()
        super().closeEvent(event)

    @staticmethod
    def _is_running_as_admin() -> bool:
        try:
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        except Exception:
            return False

    def _apply_dark_theme(self) -> None:
        self.setStyleSheet(
            """
            QMainWindow, QWidget {
                background-color: #0F172A;
                color: #E2E8F0;
                font-family: 'Segoe UI';
                font-size: 12px;
            }
            QGroupBox {
                border: 1px solid #334155;
                border-radius: 6px;
                margin-top: 8px;
                padding-top: 10px;
                font-weight: bold;
            }
            QGroupBox::title {
                subcontrol-origin: margin;
                left: 10px;
                padding: 0 4px;
                color: #38BDF8;
            }
            QTableWidget {
                background-color: #1E293B;
                alternate-background-color: #0F172A;
                gridline-color: #334155;
                border: 1px solid #334155;
                border-radius: 4px;
            }
            QHeaderView::section {
                background-color: #0F172A;
                color: #93C5FD;
                padding: 6px;
                border: 1px solid #334155;
                font-weight: bold;
            }
            QTabWidget::pane {
                border: 1px solid #334155;
                border-radius: 4px;
            }
            QTabBar::tab {
                background-color: #1E293B;
                color: #94A3B8;
                padding: 8px 16px;
                margin-right: 4px;
                border-top-left-radius: 6px;
                border-top-right-radius: 6px;
                font-weight: bold;
            }
            QTabBar::tab:selected {
                background-color: #0284C7;
                color: #FFFFFF;
            }
            QLineEdit, QTextEdit, QSpinBox, QDoubleSpinBox, QComboBox {
                background-color: #1E293B;
                border: 1px solid #475569;
                border-radius: 4px;
                padding: 4px;
                color: #F8FAFC;
            }
            QPushButton {
                background-color: #334155;
                color: #F8FAFC;
                border: 1px solid #475569;
                border-radius: 4px;
                padding: 5px 10px;
            }
            QPushButton:hover {
                background-color: #475569;
            }
            """
        )

    def _setup_system_tray(self) -> None:
        self.tray_icon = QSystemTrayIcon(self)
        icon = self.style().standardIcon(QStyle.StandardPixmap.SP_DriveHDIcon)
        self.setWindowIcon(icon)
        self.tray_icon.setIcon(icon)
        self.tray_icon.setToolTip("DiskGuard — Giám sát ổ đĩa & Quota chạy nền")

        menu = QMenu(self)
        act_show = QAction("Mở cửa sổ DiskGuard", self)
        act_show.triggered.connect(self.showNormal)
        act_scan = QAction("Quét & Kiểm tra Quota ngay", self)
        act_scan.triggered.connect(self._run_background_cycle)
        act_quit = QAction("Thoát hoàn toàn", self)
        act_quit.triggered.connect(QApplication.instance().quit)

        menu.addAction(act_show)
        menu.addAction(act_scan)
        menu.addSeparator()
        menu.addAction(act_quit)
        self.tray_icon.setContextMenu(menu)
        self.tray_icon.show()

    def _update_drive_header(self, drive: str) -> None:
        if not drive:
            return
        try:
            u = shutil.disk_usage(drive)
            pct = int(u.used / max(1, u.total) * 100)
            self.drive_usage_bar.setValue(pct)
            self.drive_stats_label.setText(
                f"Đã dùng: <b>{format_bytes(u.used)}</b> / {format_bytes(u.total)} "
                f"(Còn trống: <b style='color:#4ADE80;'>{format_bytes(u.free)}</b>) "
                f"| Vault: <code>{self.vault_dir}</code>"
            )
        except OSError:
            pass

    def _on_drive_changed(self, drive: str) -> None:
        if not drive:
            return
        self._update_drive_header(drive)
        snaps = self.db.get_daily_snapshots(drive, limit_days=14)
        if not snaps:
            self.scan_current_drive()
        else:
            self.daily_tab.load_drive_data(drive, snaps)

    def scan_current_drive(self) -> None:
        drive = self.drive_combo.currentText() or "C:\\"
        self._update_drive_header(drive)
        snaps = self.tracker.scan_drive_history_by_day(drive, days_back=14)
        self.daily_tab.load_drive_data(drive, snaps)
        self.quota_tab.refresh_profiles_and_rules()
        self.vault_tab.refresh_vault_table()

    def _on_request_create_quota(
        self, app_name: str, process_name: str, service_name: str, paths: list[str]
    ) -> None:
        self.tabs.setCurrentWidget(self.quota_tab)
        self.quota_tab.open_new_rule_dialog(
            app_name=app_name,
            process_name=process_name,
            service_name=service_name,
            paths=paths,
        )

    def run_sandbox_demo(self, silent: bool = False) -> dict:
        res = self.simulator.setup_and_trigger_quota_demo()
        self.daily_tab.load_drive_data(self.drive_combo.currentText() or "C:\\")
        self.quota_tab.refresh_rules_table()
        self.vault_tab.refresh_vault_table()
        if not silent:
            arch = res["archive"]
            self.tabs.setCurrentWidget(self.vault_tab)
            if arch:
                QMessageBox.information(
                    self,
                    "🧪 Mô Phỏng Sandbox Thành Công",
                    f"Đã tạo môi trường giả lập Zalo HardLink Media tại:\n{res['demo_dir']}\n\n"
                    f"1. Đã tạo 4 cặp file Media HardLink (~9.8 MB vật lý) vượt trần Quota 6 MB.\n"
                    f"2. Hệ thống tự động bỏ qua file 'Media.db' và chỉ nén các cặp file cũ nhất vào Safe Vault (#{arch.id}).\n"
                    f"3. Tại Tab 3 (Safe Vault), bạn có thể bấm nút [🧪 Giả lập Lỗi -> Auto-Rollback] hoặc [⏪ Rollback Ngay] để kiểm chứng!",
                )
        return res

    def simulate_auto_rollback_for_archive(
        self, archive_id: int, silent: bool = False
    ) -> dict:
        res = self.simulator.simulate_unsafe_file_access_and_auto_rollback(archive_id)
        self.quota_tab.refresh_rules_table()
        self.vault_tab.refresh_vault_table()
        if not silent and archive_id in res["auto_rolled_back"]:
            QMessageBox.warning(
                self,
                "⚡ Tự Động Rollback Thành Công",
                f"Đã tự động khôi phục gói #{archive_id} và đưa vào danh sách miễn trừ!",
            )
        return res

    def _run_background_cycle(self) -> None:
        created = self.enforcer.enforce_all_rules()
        safety = self.watchdog.run_safety_cycle()
        if created or safety["auto_rolled_back"] or safety["purged"]:
            self.quota_tab.refresh_rules_table()
            self.vault_tab.refresh_vault_table()
            if safety["auto_rolled_back"] and self.tray_icon.isVisible():
                self.tray_icon.showMessage(
                    "DiskGuard — Tự động Rollback Khẩn cấp",
                    f"Đã tự động giải nén khôi phục {len(safety['auto_rolled_back'])} gói do phát hiện ứng dụng cần lại file!",
                    QSystemTrayIcon.MessageIcon.Warning,
                    5000,
                )
