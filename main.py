import sys
import time

from PySide6.QtCore import QLocale, Qt
from PySide6.QtWidgets import QApplication, QMessageBox

from greenc import i18n
from greenc.config import (
    APP_NAME,
    APP_VERSION,
    DEFAULT_DB_PATH,
    LEGACY_DB_PATH,
    LEGACY_VAULT_DIR,
    ensure_app_dirs,
)
from greenc.core.settings import AppSettings
from greenc.core.storage_migration import migrate_legacy_db
from greenc.storage.database import GreenCDB

SINGLE_INSTANCE_MUTEX = "GreenCAppMutex"  # also listed as AppMutex in packaging/greenc.iss
RESTARTED_FLAG = "--restarted"


def acquire_single_instance(name: str = SINGLE_INSTANCE_MUTEX):
    """Return a mutex handle, or None when another instance already holds it.

    The installer uses the same mutex to ask the user to close the app before upgrading.
    """
    if sys.platform != "win32":
        return object()
    import ctypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateMutexW.restype = ctypes.c_void_p
    handle = kernel32.CreateMutexW(None, False, name)
    if handle and ctypes.get_last_error() == 183:  # ERROR_ALREADY_EXISTS
        kernel32.CloseHandle(ctypes.c_void_p(handle))
        return None
    return handle


def wait_for_single_instance(timeout: float, name: str = SINGLE_INSTANCE_MUTEX):
    """After a restart the previous process may still be exiting; retry for a moment."""
    deadline = time.monotonic() + timeout
    while True:
        handle = acquire_single_instance(name)
        if handle is not None or time.monotonic() >= deadline:
            return handle
        time.sleep(0.2)


def startup_language(settings: AppSettings) -> str:
    return settings.language or i18n.match_system_language(QLocale.system().name())


def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName(APP_NAME)
    app.setApplicationVersion(APP_VERSION)

    instance = wait_for_single_instance(10.0 if RESTARTED_FLAG in sys.argv else 0.0)
    if instance is None:
        QMessageBox.information(None, APP_NAME, i18n.tr("{app} đang chạy.", app=APP_NAME))
        return 0

    ensure_app_dirs()
    migrate_legacy_db(LEGACY_DB_PATH, DEFAULT_DB_PATH)
    db = GreenCDB(DEFAULT_DB_PATH)
    settings = AppSettings(db)

    # The language must be active before any UI module is imported: some build their
    # labels at import time.
    language = i18n.set_language(startup_language(settings))
    if i18n.is_rtl(language):
        app.setLayoutDirection(Qt.LayoutDirection.RightToLeft)

    from greenc.core.drives import list_drives
    from greenc.core.relocation.recovery import recover_interrupted
    from greenc.core.storage_migration import move_vault, remove_dir_if_empty
    from greenc.ui.data_location import run_first_run
    from greenc.ui.icons import app_icon
    from greenc.ui.main_window import MainWindow
    from greenc.ui.theme import apply_theme

    app.setWindowIcon(app_icon())
    apply_theme(app)
    recover_interrupted(db)

    if settings.data_root is None:
        run_first_run(settings, list_drives())
    vault_dir = settings.vault_dir()
    vault_dir.mkdir(parents=True, exist_ok=True)
    if LEGACY_VAULT_DIR.exists():
        move_vault(db, vault_dir)
        remove_dir_if_empty(LEGACY_VAULT_DIR)

    window = MainWindow(db=db, vault_dir=vault_dir, auto_scan_on_init=True, settings=settings)
    window.show()
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
