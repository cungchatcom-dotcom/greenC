from pathlib import Path
import shutil

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QAction
from PySide6.QtWidgets import (
    QApplication,
    QButtonGroup,
    QComboBox,
    QFrame,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMenu,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QStackedWidget,
    QStyle,
    QSystemTrayIcon,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import (
    DEFAULT_DB_PATH,
    DEFAULT_VAULT_DIR,
    SANDBOX_DEMO_DIR,
    ensure_app_dirs,
)
from diskguard.core.app_profiles import discover_installed_heavy_apps
from diskguard.core.disk_tracker import DiskTracker
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.simulator import SandboxSimulator
from diskguard.core.smart_analyzer import AppAnalysisProposal, SmartAppAnalyzer
from diskguard.core.task_controller import TaskControl
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_bytes
from diskguard.ui.daily_tab import DailyMonitorTab
from diskguard.ui.settings_tab import SettingsTab
from diskguard.ui.smart_cleanup_tab import SmartCleanupTab
from diskguard.ui.workers import BackgroundTaskWorker, TaskProgressCard


class MainWindow(QMainWindow):
    """
    Apple macOS / Google Material inspired minimalist desktop interface for DiskGuard:
    - Left Sidebar navigation:
        1. Tổng quan ổ đĩa (Daily bar chart & drill-down)
        2. Dọn dẹp Thông minh (Scan apps -> Auto-test Sandbox -> 1-Click "Dọn dẹp")
        3. Cài đặt (Settings > Rules chạy ngầm & Kho dự phòng Safe Vault)
    - Non-blocking QThread execution with Progress Bar, Pause/Resume/Stop, and
      live step-by-step caption explaining what the engine is doing.
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
        self.analyzer = SmartAppAnalyzer(
            self.db, self.vault_dir, self.sandbox_dir / "canary_probes"
        )
        self._active_worker: BackgroundTaskWorker | None = None
        # Silent 45s cycle runs on its own QThread (never on the GUI thread) and yields to
        # user-started tasks: they ask it to stop, then start as soon as it has finished.
        self._silent_worker: BackgroundTaskWorker | None = None
        self._pending_launch: tuple | None = None

        self.setWindowTitle("DiskGuard")
        self.resize(1280, 800)
        self._apply_clean_apple_google_theme()

        central = QWidget(self)
        self.setCentralWidget(central)
        root_h = QHBoxLayout(central)
        root_h.setContentsMargins(0, 0, 0, 0)
        root_h.setSpacing(0)

        # Left Navigation Sidebar (macOS System Settings style)
        sidebar = QFrame()
        sidebar.setObjectName("Sidebar")
        sidebar.setFixedWidth(220)
        sb_layout = QVBoxLayout(sidebar)
        sb_layout.setContentsMargins(14, 20, 14, 16)
        sb_layout.setSpacing(6)

        brand_label = QLabel("DiskGuard")
        brand_label.setStyleSheet(
            "font-size: 18px; font-weight: 700; color: #1D1D1F; padding: 0 8px 4px 8px;"
        )
        brand_sub = QLabel("Quản lý dung lượng thông minh")
        brand_sub.setStyleSheet(
            "font-size: 11px; color: #6E6E73; padding: 0 8px 14px 8px;"
        )
        sb_layout.addWidget(brand_label)
        sb_layout.addWidget(brand_sub)

        self.nav_group = QButtonGroup(self)
        self.nav_group.setExclusive(True)

        self.btn_nav_overview = self._create_nav_button("Tổng quan ổ đĩa", 0, checked=True)
        self.btn_nav_cleanup = self._create_nav_button("Dọn dẹp thông minh", 1)
        self.btn_nav_settings = self._create_nav_button("Cài đặt (Rules & Vault)", 2)

        sb_layout.addWidget(self.btn_nav_overview)
        sb_layout.addWidget(self.btn_nav_cleanup)
        sb_layout.addWidget(self.btn_nav_settings)
        sb_layout.addStretch(1)

        self.bg_status_label = QLabel("Chế độ bảo vệ ngầm: Đang bật")
        self.bg_status_label.setStyleSheet(
            "font-size: 11px; color: #15803D; padding: 6px 8px;"
        )
        sb_layout.addWidget(self.bg_status_label)

        root_h.addWidget(sidebar)

        # Right Main Content Area
        content_wrap = QWidget()
        content_layout = QVBoxLayout(content_wrap)
        content_layout.setContentsMargins(20, 16, 20, 16)
        content_layout.setSpacing(12)

        # Top Storage Header Card
        header_card = QFrame()
        header_card.setObjectName("SurfaceCard")
        header_layout = QHBoxLayout(header_card)
        header_layout.setContentsMargins(16, 12, 16, 12)
        header_layout.setSpacing(12)

        drive_lbl = QLabel("Ổ đĩa:")
        drive_lbl.setStyleSheet("font-weight: 600; color: #1D1D1F;")
        header_layout.addWidget(drive_lbl)

        self.drive_combo = QComboBox()
        self.drive_combo.setMinimumWidth(85)
        drives = self.tracker.list_available_drives()
        self.drive_combo.addItems(drives)
        self.drive_combo.currentTextChanged.connect(self._on_drive_changed)
        header_layout.addWidget(self.drive_combo)

        self.drive_usage_bar = QProgressBar()
        self.drive_usage_bar.setFixedWidth(220)
        self.drive_usage_bar.setFixedHeight(8)
        self.drive_usage_bar.setTextVisible(False)
        header_layout.addWidget(self.drive_usage_bar)

        self.drive_stats_label = QLabel("")
        self.drive_stats_label.setStyleSheet("color: #6E6E73; font-size: 12px;")
        header_layout.addWidget(self.drive_stats_label, stretch=1)

        btn_rescan_drive = QPushButton("Làm mới biểu đồ ngày")
        btn_rescan_drive.setObjectName("SecondaryButton")
        btn_rescan_drive.clicked.connect(self.start_drive_scan_async)
        header_layout.addWidget(btn_rescan_drive)

        content_layout.addWidget(header_card)

        # Stacked Pages
        self.stack = QStackedWidget()
        self.daily_tab = DailyMonitorTab(self.db, self)
        self.cleanup_tab = SmartCleanupTab(self)
        self.settings_tab = SettingsTab(
            self.db, self.enforcer, self.watchdog, self.simulator, self
        )

        self.stack.addWidget(self.daily_tab)
        self.stack.addWidget(self.cleanup_tab)
        self.stack.addWidget(self.settings_tab)
        content_layout.addWidget(self.stack, stretch=1)

        # Persistent Non-Blocking Task Progress Card (Start / Pause / Resume / Stop + Step Caption)
        self.progress_card = TaskProgressCard(self)
        self.progress_card.startRequested.connect(self.start_smart_app_analysis_async)
        content_layout.addWidget(self.progress_card)

        root_h.addWidget(content_wrap, stretch=1)

        # Wire signals
        self.daily_tab.requestCreateQuota.connect(self._on_request_create_quota)
        self.cleanup_tab.scanAppsRequested.connect(self.start_smart_app_analysis_async)
        self.cleanup_tab.executeCleanupRequested.connect(
            self.start_one_click_cleanup_async
        )
        self.settings_tab.settingsDataChanged.connect(self._on_settings_changed)

        self._setup_system_tray()

        # Background periodic timer (every 45s) for silent rules & D-day safety check
        self._bg_timer = QTimer(self)
        self._bg_timer.timeout.connect(self._run_silent_background_cycle)
        self._bg_timer.start(45_000)

        self._update_drive_header(self.drive_combo.currentText())

        if auto_scan_on_init:
            self._seed_default_silent_rules()
            cur_drv = self.drive_combo.currentText() or "C:\\"
            existing = self.db.get_daily_snapshots(cur_drv, limit_days=14)
            if existing:
                self.daily_tab.load_drive_data(cur_drv, existing)
                self.settings_tab.refresh_all()
            else:
                self.start_drive_scan_async()
        else:
            self.settings_tab.refresh_all()

    def _create_nav_button(
        self, text: str, page_index: int, checked: bool = False
    ) -> QPushButton:
        btn = QPushButton(text)
        btn.setObjectName("NavButton")
        btn.setCheckable(True)
        btn.setChecked(checked)
        btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self.nav_group.addButton(btn, page_index)
        btn.clicked.connect(lambda: self.stack.setCurrentIndex(page_index))
        return btn

    def _seed_default_silent_rules(self) -> None:
        """
        Silently registers specialized rules for Zalo, DingTalk, and detected heavy apps
        into Settings > Rules so they run in the background without cluttering the main UI.
        """
        existing_names = {r.app_name for r in self.db.list_quota_rules()}
        for prof in discover_installed_heavy_apps():
            if prof.app_name in existing_names:
                continue
            cur_gb = prof.physical_bytes / (1024**3)
            cap_bytes = (
                int(max(0.5, cur_gb * 0.90) * (1024**3))
                if cur_gb > 0.2
                else int(prof.recommended_max_gb * (1024**3))
            )
            rule = QuotaRule(
                id=None,
                app_name=prof.app_name,
                process_name=prof.process_name,
                service_name=prof.service_name,
                target_paths=prof.reclaimable_paths,
                max_bytes=cap_bytes,
                watermark_ratio=0.88,
                retention_days=7,
                min_file_age_seconds=60,
                exclude_patterns=prof.exclude_patterns,
                protected_files=[],
                enabled=True,
            )
            self.db.save_quota_rule(rule)

    def _apply_clean_apple_google_theme(self) -> None:
        self.setStyleSheet(
            """
            QMainWindow, QWidget {
                background-color: #F5F5F7;
                color: #1D1D1F;
                font-family: 'Segoe UI';
                font-size: 12px;
            }
            QFrame#Sidebar {
                background-color: #EBEBED;
                border-right: 1px solid #D2D2D7;
            }
            QFrame#SurfaceCard, QFrame#TaskProgressCard {
                background-color: #FFFFFF;
                border: 1px solid #E5E5EA;
                border-radius: 10px;
            }
            QPushButton#NavButton {
                background-color: transparent;
                color: #1D1D1F;
                border: none;
                border-radius: 7px;
                padding: 9px 12px;
                text-align: left;
                font-size: 13px;
                font-weight: 500;
            }
            QPushButton#NavButton:hover {
                background-color: #DFDFE4;
            }
            QPushButton#NavButton:checked {
                background-color: #0066CC;
                color: #FFFFFF;
                font-weight: 600;
            }
            QPushButton#PrimaryButton {
                background-color: #0066CC;
                color: #FFFFFF;
                border: none;
                border-radius: 7px;
                padding: 7px 16px;
                font-size: 12px;
                font-weight: 600;
            }
            QPushButton#PrimaryButton:hover {
                background-color: #0055B3;
            }
            QPushButton#PrimaryButton:disabled {
                background-color: #B0C4DE;
                color: #F5F5F7;
            }
            QPushButton#SecondaryButton {
                background-color: #F2F2F7;
                color: #1D1D1F;
                border: 1px solid #D1D1D6;
                border-radius: 7px;
                padding: 6px 12px;
                font-size: 12px;
                font-weight: 500;
            }
            QPushButton#SecondaryButton:hover {
                background-color: #E5E5EA;
            }
            QPushButton#SecondaryButton:disabled {
                color: #AEAEB2;
                background-color: #F9F9FB;
                border-color: #E5E5EA;
            }
            QTableWidget {
                background-color: #FFFFFF;
                alternate-background-color: #F9F9FB;
                border: 1px solid #E5E5EA;
                border-radius: 6px;
                selection-background-color: #E8F0FE;
                selection-color: #1D1D1F;
            }
            QHeaderView::section {
                background-color: #F5F5F7;
                color: #6E6E73;
                padding: 7px 8px;
                border: none;
                border-bottom: 1px solid #E5E5EA;
                font-weight: 600;
                font-size: 11px;
            }
            QProgressBar {
                background-color: #E5E5EA;
                border: none;
                border-radius: 3px;
            }
            QProgressBar::chunk {
                background-color: #0066CC;
                border-radius: 3px;
            }
            QComboBox, QLineEdit, QTextEdit, QSpinBox, QDoubleSpinBox {
                background-color: #FFFFFF;
                border: 1px solid #D1D1D6;
                border-radius: 6px;
                padding: 5px 8px;
                color: #1D1D1F;
            }
            QTabWidget::pane {
                border: none;
                background-color: transparent;
            }
            QTabBar::tab {
                background-color: #E5E5EA;
                color: #6E6E73;
                padding: 7px 14px;
                margin-right: 6px;
                border-radius: 6px;
                font-weight: 600;
            }
            QTabBar::tab:selected {
                background-color: #FFFFFF;
                color: #1D1D1F;
                border: 1px solid #D1D1D6;
            }
            """
        )

    def _setup_system_tray(self) -> None:
        self.tray_icon = QSystemTrayIcon(self)
        icon = self.style().standardIcon(QStyle.StandardPixmap.SP_DriveHDIcon)
        self.setWindowIcon(icon)
        self.tray_icon.setIcon(icon)
        self.tray_icon.setToolTip("DiskGuard — Đang chạy ngầm")

        menu = QMenu(self)
        act_show = QAction("Mở DiskGuard", self)
        act_show.triggered.connect(self.showNormal)
        act_quit = QAction("Thoát", self)
        act_quit.triggered.connect(QApplication.instance().quit)

        menu.addAction(act_show)
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
                f"Đã dùng {format_bytes(u.used)} / {format_bytes(u.total)} "
                f"(Còn trống {format_bytes(u.free)})"
            )
        except OSError:
            pass

    def _on_drive_changed(self, drive: str) -> None:
        if not drive:
            return
        self._update_drive_header(drive)
        snaps = self.db.get_daily_snapshots(drive, limit_days=14)
        if snaps:
            self.daily_tab.load_drive_data(drive, snaps)
        else:
            self.start_drive_scan_async()

    def _launch_background_worker(
        self,
        initial_title: str,
        fn,
        on_done,
    ) -> None:
        if self._active_worker is not None and self._active_worker.isRunning():
            return
        if self._silent_worker is not None:
            self._pending_launch = (initial_title, fn, on_done)
            self._silent_worker.control.stop()
            self.progress_card.update_progress(
                0, initial_title, "Đang chờ tác vụ chạy ngầm dừng an toàn..."
            )
            return
        worker = BackgroundTaskWorker(fn, self)
        self._active_worker = worker
        self.progress_card.attach_worker(worker, initial_title)
        worker.taskFinished.connect(
            lambda res, stopped: on_done(res, stopped)
        )
        worker.start()

    def start_drive_scan_async(self) -> None:
        drive = self.drive_combo.currentText() or "C:\\"
        self._update_drive_header(drive)

        def _work(ctrl: TaskControl):
            return self.tracker.scan_drive_history_by_day(
                drive, days_back=14, task_control=ctrl
            )

        def _done(snaps, stopped: bool):
            if snaps:
                self.daily_tab.load_drive_data(drive, snaps)
            self.settings_tab.refresh_all()

        self._launch_background_worker(
            f"Đang phân tích lịch sử biến động ổ đĩa {drive}...",
            _work,
            _done,
        )

    def start_smart_app_analysis_async(self) -> None:
        self.btn_nav_cleanup.setChecked(True)
        self.stack.setCurrentWidget(self.cleanup_tab)

        def _work(ctrl: TaskControl):
            return self.analyzer.scan_and_probe_apps(task_control=ctrl)

        def _done(proposals, stopped: bool):
            if proposals is not None:
                self.cleanup_tab.set_proposals(proposals)
            self.settings_tab.refresh_all()

        self._launch_background_worker(
            "Đang quét các ứng dụng trên máy và chạy thử chu trình Sandbox...",
            _work,
            _done,
        )

    def start_one_click_cleanup_async(
        self, proposals: list[AppAnalysisProposal]
    ) -> None:
        def _work(ctrl: TaskControl):
            return self.analyzer.execute_one_click_cleanup(
                proposals, task_control=ctrl
            )

        def _done(res: dict | None, stopped: bool):
            self._update_drive_header(self.drive_combo.currentText())
            self.settings_tab.refresh_all()
            if res and not stopped:
                summary_msg = (
                    f"Đã thu hồi an toàn {format_bytes(res['freed_bytes'])} "
                    f"(Lưu dự phòng 7 ngày trong Kho dự phòng và kích hoạt {res['rules_saved']} quy tắc chạy ngầm)"
                )
                if res.get("errors"):
                    summary_msg += " — Lỗi: " + "; ".join(res["errors"])
                self.progress_card.update_progress(
                    100,
                    "Hoàn tất dọn dẹp và thiết lập quy tắc chạy ngầm",
                    summary_msg,
                )
                self.cleanup_tab.proposal_summary_label.setText(summary_msg)

        self._launch_background_worker(
            "Đang thực thi dọn dẹp an toàn và thiết lập quy tắc chạy ngầm...",
            _work,
            _done,
        )

    def _on_request_create_quota(
        self, app_name: str, process_name: str, service_name: str, paths: list[str]
    ) -> None:
        self.btn_nav_settings.setChecked(True)
        self.stack.setCurrentWidget(self.settings_tab)
        self.settings_tab.open_new_rule_dialog(
            app_name=app_name,
            process_name=process_name,
            service_name=service_name,
            paths=paths,
        )

    def _on_settings_changed(self) -> None:
        self._update_drive_header(self.drive_combo.currentText())

    # Compatibility helpers for automated tests
    def run_sandbox_demo(self, silent: bool = False) -> dict:
        res = self.simulator.setup_and_trigger_quota_demo()
        self.daily_tab.load_drive_data(self.drive_combo.currentText() or "C:\\")
        self.settings_tab.refresh_all()
        return res

    def simulate_auto_rollback_for_archive(
        self, archive_id: int, silent: bool = False
    ) -> dict:
        res = self.simulator.simulate_unsafe_file_access_and_auto_rollback(archive_id)
        self.settings_tab.refresh_all()
        return res

    def _run_silent_background_cycle(self) -> None:
        if self._active_worker is not None and self._active_worker.isRunning():
            return
        if self._silent_worker is not None:
            return  # previous cycle still running (e.g. archiving several GB)

        enforcer, watchdog = self.enforcer, self.watchdog

        def _work(ctrl: TaskControl) -> dict:
            created = enforcer.enforce_all_rules(should_stop=lambda: ctrl.is_stopped)
            errors = list(getattr(enforcer, "last_errors", []))
            if ctrl.is_stopped:
                return {"created": created, "safety": None, "errors": errors}
            safety = watchdog.run_safety_cycle()
            return {"created": created, "safety": safety, "errors": errors}

        worker = BackgroundTaskWorker(_work, self)
        self._silent_worker = worker
        worker.taskFinished.connect(self._on_silent_cycle_finished)
        worker.taskFailed.connect(self._on_silent_cycle_failed)
        worker.finished.connect(worker.deleteLater)
        worker.start()

    def _on_silent_cycle_finished(self, res: dict | None, _stopped: bool) -> None:
        self._silent_worker = None
        res = res or {}
        safety = res.get("safety") or {}
        if res.get("created") or safety.get("auto_rolled_back") or safety.get("purged"):
            self.settings_tab.refresh_all()
        tray_ok = getattr(self, "tray_icon", None) is not None and self.tray_icon.isVisible()
        if safety.get("auto_rolled_back") and tray_ok:
            self.tray_icon.showMessage(
                "DiskGuard — Tự động khôi phục dữ liệu",
                f"Đã tự động giải nén trả lại {len(safety['auto_rolled_back'])} gói dữ liệu cho ứng dụng.",
                QSystemTrayIcon.MessageIcon.Information,
                5000,
            )
        if res.get("errors") and tray_ok:
            self.tray_icon.showMessage(
                "DiskGuard — Không thể dọn dẹp một số ứng dụng",
                "; ".join(res["errors"])[:300],
                QSystemTrayIcon.MessageIcon.Warning,
                8000,
            )
        self._run_pending_launch()

    def _on_silent_cycle_failed(self, _err_msg: str) -> None:
        self._silent_worker = None
        self._run_pending_launch()

    def _run_pending_launch(self) -> None:
        if self._pending_launch is not None:
            pending, self._pending_launch = self._pending_launch, None
            self._launch_background_worker(*pending)

    def closeEvent(self, event) -> None:  # noqa: N802
        self._bg_timer.stop()
        self._pending_launch = None
        for worker in (self._active_worker, self._silent_worker):
            if worker is not None and worker.isRunning():
                worker.control.stop()
                # stop is polled per file, so this returns once the current file is done
                worker.wait(5000)
        if hasattr(self, "tray_icon") and self.tray_icon is not None:
            self.tray_icon.hide()
        super().closeEvent(event)
