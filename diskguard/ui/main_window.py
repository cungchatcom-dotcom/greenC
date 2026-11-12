from pathlib import Path
import shutil

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QAction
from PySide6.QtWidgets import (
    QApplication,
    QButtonGroup,
    QFrame,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMenu,
    QMessageBox,
    QPushButton,
    QStackedWidget,
    QSystemTrayIcon,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import APP_NAME, AUTHOR, DEFAULT_DB_PATH, SANDBOX_DEMO_DIR, WEBSITE_LABEL, WEBSITE_URL
from diskguard.core.app_profiles import discover_installed_heavy_apps
from diskguard.core.disk_tracker import DiskTracker
from diskguard.core.drives import is_on_system_drive, list_drives
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.relocation.catalog import discover_candidates
from diskguard.core.relocation.fs_ops import RelocationError, RelocationStopped
from diskguard.core.relocation.service import RelocationService
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.settings import AppSettings
from diskguard.core.simulator import SandboxSimulator
from diskguard.core.smart_analyzer import AppAnalysisProposal, SmartAppAnalyzer
from diskguard.core.storage_migration import move_vault, remove_dir_if_empty
from diskguard.core.task_controller import TaskControl
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.data_location import FirstRunDialog
from diskguard.ui.dialogs import GuidedDialog
from diskguard.ui.icons import app_icon
from diskguard.ui.overview_page import OverviewPage
from diskguard.ui.relocation_page import RelocationPage
from diskguard.ui.settings_tab import SettingsTab
from diskguard.ui.smart_cleanup_tab import SmartCleanupTab
from diskguard.ui.theme import apply_theme
from diskguard.ui.vault_page import VaultPage
from diskguard.ui.workers import BackgroundTaskWorker, TaskProgressCard


class MainWindow(QMainWindow):
    """Green C:\\ main window: left navigation, five pages, compact status bar."""

    def __init__(
        self,
        db: DiskGuardDB | None = None,
        vault_dir: Path | str | None = None,
        sandbox_dir: Path | str | None = None,
        auto_scan_on_init: bool = True,
        settings: AppSettings | None = None,
    ):
        super().__init__()
        self.db = db or DiskGuardDB(DEFAULT_DB_PATH)
        self.settings = settings or AppSettings(self.db)
        self.vault_dir = Path(vault_dir) if vault_dir else self.settings.vault_dir()
        self.sandbox_dir = Path(sandbox_dir) if sandbox_dir else SANDBOX_DEMO_DIR

        self.tracker = DiskTracker(self.db)
        self.enforcer = QuotaEnforcer(self.db, self.vault_dir)
        self.watchdog = SafetyWatchdog(self.db)
        self.simulator = SandboxSimulator(self.db, self.sandbox_dir, self.vault_dir)
        self.analyzer = SmartAppAnalyzer(self.db, self.vault_dir, self.sandbox_dir / "canary_probes")
        self.relocation_service = RelocationService(self.db, self.settings)
        self._active_worker: BackgroundTaskWorker | None = None
        # Silent 45s cycle runs on its own QThread (never on the GUI thread) and yields to
        # user-started tasks: they ask it to stop, then start as soon as it has finished.
        self._silent_worker: BackgroundTaskWorker | None = None
        self._pending_launch: tuple | None = None
        self._last_proposals: list[AppAnalysisProposal] = []
        self._last_candidates: list = []

        self.setWindowTitle(APP_NAME)
        self.setWindowIcon(app_icon())
        self.resize(1280, 820)
        apply_theme(self)

        central = QWidget(self)
        central.setObjectName("Content")
        self.setCentralWidget(central)
        root_h = QHBoxLayout(central)
        root_h.setContentsMargins(0, 0, 0, 0)
        root_h.setSpacing(0)

        sidebar = QFrame()
        sidebar.setObjectName("Sidebar")
        sidebar.setFixedWidth(208)
        sb = QVBoxLayout(sidebar)
        sb.setContentsMargins(8, 18, 8, 12)
        sb.setSpacing(2)
        self.brand_label = QLabel('Green <span style="color:#107C10">C:\\</span>')
        self.brand_label.setObjectName("Brand")
        self.brand_label.setTextFormat(Qt.TextFormat.RichText)
        sb.addWidget(self.brand_label)

        self.nav_group = QButtonGroup(self)
        self.nav_group.setExclusive(True)
        self.btn_nav_overview = self._create_nav_button("Tổng quan", 0, checked=True)
        self.btn_nav_cleanup = self._create_nav_button("Dọn dẹp", 1)
        self.btn_nav_relocation = self._create_nav_button("Di chuyển dữ liệu", 2)
        self.btn_nav_vault = self._create_nav_button("Kho dự phòng", 3)
        self.btn_nav_settings = self._create_nav_button("Cài đặt", 4)
        for btn in (self.btn_nav_overview, self.btn_nav_cleanup, self.btn_nav_relocation,
                    self.btn_nav_vault, self.btn_nav_settings):
            sb.addWidget(btn)
        sb.addStretch(1)
        self.author_label = QLabel(f'Tác giả: <b>{AUTHOR}</b><br><a href="{WEBSITE_URL}">{WEBSITE_LABEL}</a>')
        self.author_label.setObjectName("AuthorBlock")
        self.author_label.setTextFormat(Qt.TextFormat.RichText)
        self.author_label.setOpenExternalLinks(True)
        sb.addWidget(self.author_label)
        root_h.addWidget(sidebar)

        content = QWidget()
        content.setObjectName("Content")
        cl = QVBoxLayout(content)
        cl.setContentsMargins(0, 0, 0, 0)
        cl.setSpacing(0)

        self.stack = QStackedWidget()
        self.overview_page = OverviewPage(self.db, self.tracker.list_available_drives(), self)
        self.daily_tab = self.overview_page.daily_tab
        self.drive_combo = self.overview_page.drive_combo
        self.cleanup_tab = SmartCleanupTab(self)
        self.cleanup_tab.set_retention_days(self.settings.retention_days_default)
        self.relocation_page = self._build_relocation_page()
        self.vault_page = VaultPage(self.db, self.watchdog, lambda: self.vault_dir, self)
        self.settings_tab = SettingsTab(
            self.db, self.enforcer, self.watchdog, self.simulator, self, settings=self.settings
        )
        for page in (self.overview_page, self.cleanup_tab, self.relocation_page, self.vault_page, self.settings_tab):
            self.stack.addWidget(page)
        cl.addWidget(self.stack, 1)
        self.stack.currentChanged.connect(lambda i: self.nav_group.button(i).setChecked(True))

        self.progress_card = TaskProgressCard(self)
        cl.addWidget(self.progress_card)
        root_h.addWidget(content, 1)

        self.overview_page.analyzeRequested.connect(self.start_smart_app_analysis_async)
        self.overview_page.cleanupRequested.connect(self.btn_nav_cleanup.click)
        self.overview_page.refreshHistoryRequested.connect(self.start_drive_scan_async)
        self.overview_page.driveChanged.connect(self._on_drive_changed)
        self.daily_tab.requestCreateQuota.connect(self._on_request_create_quota)
        self.cleanup_tab.scanAppsRequested.connect(self.start_smart_app_analysis_async)
        self.cleanup_tab.executeCleanupRequested.connect(self.start_one_click_cleanup_async)
        self.settings_tab.settingsDataChanged.connect(self._on_settings_changed)
        self.settings_tab.dataRootChangeRequested.connect(self._on_change_data_root)
        self.vault_page.dataChanged.connect(self.refresh_data_views)
        self.relocation_page.relocateRequested.connect(self._on_relocate)
        self.relocation_page.guideRequested.connect(self._on_guide)
        self.relocation_page.undoRequested.connect(self._on_undo_relocation)
        self.relocation_page.refreshRequested.connect(self.start_smart_app_analysis_async)
        self._refresh_relocation_target()
        self.relocation_page.set_records(self.relocation_service.records())

        self._setup_system_tray()

        # Background periodic timer (every 45s) for silent rules & D-day safety check
        self._bg_timer = QTimer(self)
        self._bg_timer.timeout.connect(self._run_silent_background_cycle)
        self._bg_timer.start(45_000)

        if auto_scan_on_init:
            self._seed_default_silent_rules()
            cur_drv = self.drive_combo.currentText() or "C:\\"
            existing = self.db.get_daily_snapshots(cur_drv, limit_days=14)
            if existing:
                self.daily_tab.load_drive_data(cur_drv, existing)
                self.refresh_data_views()
            else:
                self.start_drive_scan_async()
        else:
            self.refresh_data_views()

    # ----- layout helpers -------------------------------------------------

    def _create_nav_button(self, text: str, page_index: int, checked: bool = False) -> QPushButton:
        btn = QPushButton(text)
        btn.setObjectName("NavButton")
        btn.setCheckable(True)
        btn.setChecked(checked)
        btn.setCursor(Qt.CursorShape.PointingHandCursor)
        self.nav_group.addButton(btn, page_index)
        btn.clicked.connect(lambda: self.stack.setCurrentIndex(page_index))
        return btn

    def _build_relocation_page(self) -> QWidget:
        return RelocationPage(self)

    def _refresh_relocation_target(self) -> None:
        root = self.settings.data_root
        free = None
        if root is not None:
            try:
                free = shutil.disk_usage(root.anchor).free
            except OSError:
                free = None
        self.relocation_page.set_target(root, free)
        if root is None:
            self.relocation_page.set_disabled_reason(
                "Chưa chọn ổ lưu dữ liệu. Chọn trong Cài đặt → Vị trí lưu dữ liệu."
            )
        elif is_on_system_drive(root):
            self.relocation_page.set_disabled_reason(
                "Ổ lưu dữ liệu đang là ổ C:. Chọn một ổ khác trong Cài đặt để chuyển dữ liệu ứng dụng."
            )
        else:
            self.relocation_page.set_disabled_reason(None)

    def _setup_system_tray(self) -> None:
        self.tray_icon = QSystemTrayIcon(self)
        self.tray_icon.setIcon(app_icon())
        self.tray_icon.setToolTip(f"{APP_NAME} — đang chạy nền")
        menu = QMenu(self)
        act_show = QAction(f"Mở {APP_NAME}", self)
        act_show.triggered.connect(self.showNormal)
        act_quit = QAction("Thoát", self)
        act_quit.triggered.connect(QApplication.instance().quit)
        menu.addAction(act_show)
        menu.addSeparator()
        menu.addAction(act_quit)
        self.tray_icon.setContextMenu(menu)
        self.tray_icon.show()

    def refresh_data_views(self) -> None:
        self.settings_tab.refresh_all()
        self.vault_page.refresh()

    def _target_label(self) -> str:
        root = self.settings.data_root
        return root.drive if root else "ổ khác"

    def _apply_vault_dir(self, vault_dir: Path) -> None:
        self.vault_dir = vault_dir
        self.enforcer.vault_dir = vault_dir
        self.analyzer.vault_dir = vault_dir
        vault_dir.mkdir(parents=True, exist_ok=True)

    def _on_change_data_root(self) -> None:
        dlg = FirstRunDialog(
            list_drives(), self.settings.data_root, self.settings.retention_days_default, self,
            title="Đổi nơi lưu dữ liệu", allow_later=False,
        )
        if not dlg.exec():
            return
        new_root = dlg.chosen_root()
        self.settings.set_retention_days_default(dlg.chosen_retention())
        self.settings_tab.retention_spin.setValue(self.settings.retention_days_default)
        if new_root is None or new_root == self.settings.data_root:
            return
        new_vault = new_root / "Vault"
        old_vault = self.vault_dir

        def _work(ctrl: TaskControl):
            return move_vault(self.db, new_vault, ctrl)

        def _done(res, stopped: bool):
            res = res or {"moved": 0, "errors": []}
            if res["errors"]:
                QMessageBox.warning(self, "Chưa chuyển hết Kho dự phòng", "\n".join(res["errors"]))
                return
            self.settings.set_data_root(new_root)
            self._apply_vault_dir(new_vault)
            remove_dir_if_empty(old_vault)
            self.refresh_data_views()
            self._refresh_relocation_target()

        self._launch_background_worker("Đang chuyển Kho dự phòng", _work, _done)

    def _on_relocate(self, cand) -> None:
        try:
            check = self.relocation_service.check(cand)
        except RelocationError as err:
            QMessageBox.warning(self, "Chưa thể chuyển", str(err))
            return
        if not check.ok:
            QMessageBox.warning(self, "Chưa thể chuyển", check.reason)
            return
        answer = QMessageBox.question(
            self, f"Chuyển {cand.name}",
            f"Chuyển {cand.name} ({format_size(cand.size_bytes)}) sang {self.settings.data_root}?\n\n"
            "Dữ liệu được sao chép và kiểm tra trước, sau đó mới xóa ở ổ C:. Có thể hoàn tác.",
        )
        if answer != QMessageBox.StandardButton.Yes:
            return

        def _work(ctrl: TaskControl):
            try:
                return {"record": self.relocation_service.apply(cand, ctrl)}
            except RelocationStopped:
                return {"error": "Đã dừng. Dữ liệu vẫn ở vị trí cũ."}
            except RelocationError as err:
                return {"error": str(err)}

        def _done(res, stopped: bool):
            res = res or {}
            if res.get("error"):
                QMessageBox.warning(self, f"Không chuyển được {cand.name}", res["error"])
            elif res.get("record") is not None:
                self._last_candidates = [c for c in self._last_candidates if c is not cand]
                self.relocation_page.set_candidates(self._last_candidates)
                self.overview_page.set_freeable(self._last_proposals, self._last_candidates, self._target_label())
                self.progress_card.update_progress(100, f"Đã chuyển {cand.name}", res["record"].target_path)
            self.relocation_page.set_records(self.relocation_service.records())
            self.overview_page.update_drive_usage(self.drive_combo.currentText())
            self._refresh_relocation_target()

        self._launch_background_worker(f"Đang chuyển {cand.name}", _work, _done)

    def _on_guide(self, cand) -> None:
        try:
            root = self.relocation_service.target_root()
        except RelocationError as err:
            QMessageBox.warning(self, "Chưa thể chuyển", str(err))
            return
        try:
            (root / "Zalo").mkdir(parents=True, exist_ok=True)
        except OSError:
            pass
        dlg = GuidedDialog(cand, root, self.relocation_service, self)
        dlg.exec()
        if dlg.record is not None:
            self._last_candidates = [c for c in self._last_candidates if c is not cand]
            self.relocation_page.set_candidates(self._last_candidates)
            self.relocation_page.set_records(self.relocation_service.records())
            self.overview_page.set_freeable(self._last_proposals, self._last_candidates, self._target_label())

    def _on_undo_relocation(self, rec) -> None:
        answer = QMessageBox.question(
            self, f"Hoàn tác {rec.app_name}",
            f"Trả {rec.app_name} về vị trí cũ trên ổ C: ({format_size(rec.size_bytes)})?",
        )
        if answer != QMessageBox.StandardButton.Yes:
            return

        def _work(ctrl: TaskControl):
            try:
                self.relocation_service.undo(rec, ctrl)
                return {}
            except RelocationError as err:
                return {"error": str(err)}

        def _done(res, stopped: bool):
            if (res or {}).get("error"):
                QMessageBox.warning(self, f"Không hoàn tác được {rec.app_name}", res["error"])
            self.relocation_page.set_records(self.relocation_service.records())
            self.overview_page.update_drive_usage(self.drive_combo.currentText())

        self._launch_background_worker(f"Đang hoàn tác {rec.app_name}", _work, _done)

    def _seed_default_silent_rules(self) -> None:
        """Register rules for Zalo, DingTalk and other detected heavy apps; they run in the background."""
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
            self.db.save_quota_rule(
                QuotaRule(
                    id=None,
                    app_name=prof.app_name,
                    process_name=prof.process_name,
                    service_name=prof.service_name,
                    target_paths=prof.reclaimable_paths,
                    max_bytes=cap_bytes,
                    watermark_ratio=0.88,
                    retention_days=self.settings.retention_days_default,
                    min_file_age_seconds=60,
                    exclude_patterns=prof.exclude_patterns,
                    protected_files=[],
                    enabled=True,
                )
            )

    # ----- background work ------------------------------------------------

    def _launch_background_worker(self, initial_title: str, fn, on_done) -> None:
        if self._active_worker is not None and self._active_worker.isRunning():
            return
        if self._silent_worker is not None:
            self._pending_launch = (initial_title, fn, on_done)
            self._silent_worker.control.stop()
            self.progress_card.setVisible(True)
            self.progress_card.update_progress(0, initial_title, "Đang chờ tác vụ nền dừng…")
            return
        worker = BackgroundTaskWorker(fn, self)
        self._active_worker = worker
        self.progress_card.attach_worker(worker, initial_title)
        worker.taskFinished.connect(lambda res, stopped: on_done(res, stopped))
        worker.start()

    def _on_drive_changed(self, drive: str) -> None:
        if not drive:
            return
        existing = self.db.get_daily_snapshots(drive, limit_days=14)
        if existing:
            self.daily_tab.load_drive_data(drive, existing)
        else:
            self.start_drive_scan_async()

    def start_drive_scan_async(self) -> None:
        drive = self.drive_combo.currentText() or "C:\\"
        self.overview_page.update_drive_usage(drive)

        def _work(ctrl: TaskControl):
            return self.tracker.scan_drive_history_by_day(drive, days_back=14, task_control=ctrl)

        def _done(snaps, stopped: bool):
            if snaps:
                self.daily_tab.load_drive_data(drive, snaps)
            self.refresh_data_views()

        self._launch_background_worker(f"Đang đọc lịch sử ổ {drive[:2]}", _work, _done)

    def start_smart_app_analysis_async(self) -> None:
        def _work(ctrl: TaskControl):
            proposals = self.analyzer.scan_and_probe_apps(task_control=ctrl)
            candidates = []
            if not ctrl.is_stopped:
                ctrl.report(97, "Đang tìm dữ liệu có thể chuyển", "")
                candidates = discover_candidates(self.db, should_stop=lambda: ctrl.is_stopped)
            return {"proposals": proposals, "candidates": candidates}

        def _done(res, stopped: bool):
            res = res or {}
            self._last_proposals = res.get("proposals", [])
            self._last_candidates = res.get("candidates", [])
            self.cleanup_tab.set_proposals(self._last_proposals)
            self.relocation_page.set_candidates(self._last_candidates)
            self._refresh_relocation_target()
            self.overview_page.set_freeable(self._last_proposals, self._last_candidates, self._target_label())

        self._launch_background_worker("Đang phân tích ứng dụng", _work, _done)

    def start_one_click_cleanup_async(self, proposals: list[AppAnalysisProposal]) -> None:
        retention = self.settings.retention_days_default

        def _work(ctrl: TaskControl):
            return self.analyzer.execute_one_click_cleanup(proposals, task_control=ctrl, retention_days=retention)

        def _done(res: dict | None, stopped: bool):
            self.overview_page.update_drive_usage(self.drive_combo.currentText())
            self.refresh_data_views()
            if res and not stopped:
                msg = f"Đã giải phóng {format_size(res['freed_bytes'])}, giữ dự phòng {retention} ngày."
                if res.get("errors"):
                    msg += " Lỗi: " + "; ".join(res["errors"])
                self.progress_card.update_progress(100, "Đã dọn xong", msg)
                self.cleanup_tab.proposal_summary_label.setText(msg)

        self._launch_background_worker("Đang dọn dẹp", _work, _done)

    def _on_request_create_quota(self, app_name: str, process_name: str, service_name: str, paths: list[str]) -> None:
        self.btn_nav_settings.click()
        self.settings_tab.open_new_rule_dialog(
            app_name=app_name, process_name=process_name, service_name=service_name, paths=paths
        )

    def _on_settings_changed(self) -> None:
        self.cleanup_tab.set_retention_days(self.settings.retention_days_default)
        self.overview_page.update_drive_usage(self.drive_combo.currentText())

    # Compatibility helpers for automated tests
    def run_sandbox_demo(self, silent: bool = False) -> dict:
        res = self.simulator.setup_and_trigger_quota_demo()
        self.daily_tab.load_drive_data(self.drive_combo.currentText() or "C:\\")
        self.refresh_data_views()
        return res

    def simulate_auto_rollback_for_archive(self, archive_id: int, silent: bool = False) -> dict:
        res = self.simulator.simulate_unsafe_file_access_and_auto_rollback(archive_id)
        self.refresh_data_views()
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
            self.refresh_data_views()
        tray_ok = getattr(self, "tray_icon", None) is not None and self.tray_icon.isVisible()
        if safety.get("auto_rolled_back") and tray_ok:
            self.tray_icon.showMessage(
                APP_NAME,
                f"Đã tự khôi phục {len(safety['auto_rolled_back'])} gói dữ liệu cho ứng dụng.",
                QSystemTrayIcon.MessageIcon.Information,
                5000,
            )
        if res.get("errors") and tray_ok:
            self.tray_icon.showMessage(
                APP_NAME,
                ("Không dọn được: " + "; ".join(res["errors"]))[:300],
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
