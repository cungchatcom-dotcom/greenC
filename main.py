import sys

from PySide6.QtWidgets import QApplication, QMessageBox

from diskguard.config import (
    APP_NAME,
    APP_VERSION,
    DEFAULT_DB_PATH,
    LEGACY_DB_PATH,
    LEGACY_VAULT_DIR,
    ensure_app_dirs,
)
from diskguard.core.drives import list_drives
from diskguard.core.relocation.recovery import recover_interrupted
from diskguard.core.settings import AppSettings
from diskguard.core.storage_migration import migrate_legacy_db, move_vault, remove_dir_if_empty
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.data_location import run_first_run
from diskguard.ui.icons import app_icon
from diskguard.ui.main_window import MainWindow
from diskguard.ui.theme import apply_theme


SINGLE_INSTANCE_MUTEX = "GreenCAppMutex"  # also listed as AppMutex in packaging/greenc.iss


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


def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName(APP_NAME)
    app.setApplicationVersion(APP_VERSION)
    app.setWindowIcon(app_icon())
    apply_theme(app)

    instance = acquire_single_instance()
    if instance is None:
        QMessageBox.information(None, APP_NAME, f"{APP_NAME} đang chạy.")
        return 0

    ensure_app_dirs()
    migrate_legacy_db(LEGACY_DB_PATH, DEFAULT_DB_PATH)
    db = DiskGuardDB(DEFAULT_DB_PATH)
    settings = AppSettings(db)
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
