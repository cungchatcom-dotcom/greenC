import sys
from pathlib import Path

from PySide6.QtCore import QProcess
from PySide6.QtWidgets import QApplication

RESTARTED_FLAG = "--restarted"  # keep in sync with main.py


def restart_command() -> tuple[str, list[str]]:
    """Program and arguments that start a fresh copy of the app."""
    args = [a for a in sys.argv[1:] if a != RESTARTED_FLAG] + [RESTARTED_FLAG]
    if getattr(sys, "frozen", False):
        return sys.executable, args
    main_py = Path(__file__).resolve().parents[2] / "main.py"
    return sys.executable, [str(main_py), *args]


def restart_application() -> bool:
    program, args = restart_command()
    started = QProcess.startDetached(program, args)
    ok = started[0] if isinstance(started, tuple) else bool(started)
    if ok:
        # close() lets the main window stop its workers cleanly before the event loop ends;
        # the new process waits on the single-instance mutex until this one has exited.
        for widget in QApplication.topLevelWidgets():
            widget.close()
        QApplication.quit()
    return ok
