import sys

from PySide6.QtWidgets import QApplication

from diskguard.config import (
    APP_NAME,
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


def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName(APP_NAME)
    app.setWindowIcon(app_icon())
    apply_theme(app)

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
