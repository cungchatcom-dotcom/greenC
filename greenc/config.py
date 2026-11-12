import os
from pathlib import Path

from greenc import __version__

APP_NAME = "Green C:\\"
APP_VERSION = __version__
# Stable id shared by the installer (AppId) and the single-instance mutex.
APP_ID = "GreenC"
AUTHOR = "Cùng Chat"
WEBSITE_URL = "https://cungchat.com"
WEBSITE_LABEL = "cungchat.com"

# Folder name used under %LOCALAPPDATA% and at the root of the chosen data drive.
DATA_FOLDER_NAME = "Green C"


def app_home() -> Path:
    base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    return Path(base) / DATA_FOLDER_NAME


APP_HOME = app_home()
DEFAULT_DB_PATH = APP_HOME / "greenc.db"
# Used only until the user picks a data drive (see AppSettings.vault_dir()).
DEFAULT_VAULT_DIR = APP_HOME / "Vault"
SANDBOX_DEMO_DIR = APP_HOME / "sandbox"

# Version 0.1 (then named "DiskGuard") kept its database and Vault inside its source folder,
# which may since have been renamed to C:\GreenC. Migrated on first launch.
LEGACY_BASE_CANDIDATES = (Path(r"C:\DiskGuard"), Path(r"C:\GreenC"))


def legacy_base_dir(candidates: tuple[Path, ...] = LEGACY_BASE_CANDIDATES) -> Path:
    for base in candidates:
        if (base / "data" / "diskguard.db").exists() or (base / "vault").is_dir():
            return base
    return candidates[0]


LEGACY_BASE_DIR = legacy_base_dir()
LEGACY_DB_PATH = LEGACY_BASE_DIR / "data" / "diskguard.db"
LEGACY_VAULT_DIR = LEGACY_BASE_DIR / "vault"

DEFAULT_WATERMARK_RATIO = 0.85
DEFAULT_RETENTION_DAYS = 7
MIN_RETENTION_DAYS = 1
MAX_RETENTION_DAYS = 90
DEFAULT_MIN_FILE_AGE_SECONDS = 60

DEFAULT_PROTECTED_PATTERNS = [
    "*.exe",
    "*.dll",
    "*.sys",
    "*.ini",
    "*.cfg",
    "*.config",
    "*.json",
    "*.yaml",
    "*.yml",
    "*.toml",
    "*.db",
    "*.db-wal",
    "*.db-shm",
    "*.sqlite",
    "*.sqlite3",
    "*.mdf",
    "*.ldf",
    "*.rescache",
    "lockfile",
    "CURRENT",
    "MANIFEST-*",
]


def ensure_app_dirs() -> None:
    APP_HOME.mkdir(parents=True, exist_ok=True)
