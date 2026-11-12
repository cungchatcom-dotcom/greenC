import os
from pathlib import Path

APP_NAME = "Green C:\\"
APP_VERSION = "2.0.0"
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

# Locations used by DiskGuard 0.1 — migrated on first launch.
LEGACY_BASE_DIR = Path(r"C:\DiskGuard")
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
