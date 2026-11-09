from pathlib import Path

BASE_DIR = Path(r"C:\DiskGuard")
DATA_DIR = BASE_DIR / "data"
DEFAULT_DB_PATH = DATA_DIR / "diskguard.db"
DEFAULT_VAULT_DIR = BASE_DIR / "vault"
SANDBOX_DEMO_DIR = BASE_DIR / "sandbox_demo"

DEFAULT_WATERMARK_RATIO = 0.85
DEFAULT_RETENTION_DAYS = 7
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
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    DEFAULT_VAULT_DIR.mkdir(parents=True, exist_ok=True)

