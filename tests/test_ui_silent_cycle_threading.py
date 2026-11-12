"""
The 45s silent background cycle (quota enforcement + safety watchdog) must run on a
QThread: archiving GBs on the GUI thread froze the whole window.
"""
import os
from pathlib import Path
import threading
import time

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from diskguard.storage.database import DiskGuardDB
from diskguard.ui.main_window import MainWindow


def _pump_until(app, cond, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        app.processEvents()
        if cond():
            return True
        time.sleep(0.01)
    return False


def _make_window(tmp_path: Path) -> tuple[QApplication, MainWindow]:
    app = QApplication.instance() or QApplication([])
    win = MainWindow(
        db=DiskGuardDB(tmp_path / "ui.db"),
        vault_dir=tmp_path / "vault",
        sandbox_dir=tmp_path / "sandbox",
        auto_scan_on_init=False,
    )
    win._bg_timer.stop()
    return app, win


def test_silent_cycle_runs_off_the_gui_thread(tmp_path: Path, monkeypatch):
    app, win = _make_window(tmp_path)
    seen = {}

    def _slow_enforce(should_stop=None):
        seen["thread"] = threading.current_thread()
        time.sleep(0.6)
        return []

    monkeypatch.setattr(win.enforcer, "enforce_all_rules", _slow_enforce)

    t0 = time.perf_counter()
    win._run_silent_background_cycle()
    assert time.perf_counter() - t0 < 0.2, "silent cycle blocked the GUI thread"

    assert _pump_until(app, lambda: win._silent_worker is None)
    assert seen["thread"] is not threading.main_thread()

    win.close()
    win.deleteLater()
    app.processEvents()


def test_foreground_task_waits_for_silent_cycle_then_runs(tmp_path: Path, monkeypatch):
    app, win = _make_window(tmp_path)
    release = threading.Event()
    order: list[str] = []

    def _blocking_enforce(should_stop=None):
        order.append("silent-start")
        while not release.is_set() and not (should_stop and should_stop()):
            time.sleep(0.01)
        order.append("silent-end")
        return []

    monkeypatch.setattr(win.enforcer, "enforce_all_rules", _blocking_enforce)
    win._run_silent_background_cycle()
    assert _pump_until(app, lambda: order == ["silent-start"])

    done = {}
    win._launch_background_worker(
        "Foreground", lambda ctrl: order.append("foreground") or 42,
        lambda res, stopped: done.setdefault("res", res),
    )
    # The user's task asks the silent cycle to stop, then starts right after it.
    assert _pump_until(app, lambda: done.get("res") == 42)
    assert order == ["silent-start", "silent-end", "foreground"]

    win.close()
    win.deleteLater()
    app.processEvents()
