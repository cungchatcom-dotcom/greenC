from pathlib import Path

from greenc.config import (
    DEFAULT_RETENTION_DAYS,
    DEFAULT_VAULT_DIR,
    MAX_RETENTION_DAYS,
    MIN_RETENTION_DAYS,
)
from greenc.i18n import LANGUAGE_CODES
from greenc.storage.database import GreenCDB


class AppSettings:
    """User preferences persisted in the `app_settings` table."""

    KEY_DATA_ROOT = "data_root"
    KEY_RETENTION = "retention_days_default"
    KEY_LANGUAGE = "language"

    def __init__(self, db: GreenCDB):
        self.db = db

    @property
    def data_root(self) -> Path | None:
        value = (self.db.get_setting(self.KEY_DATA_ROOT, "") or "").strip()
        return Path(value) if value else None

    def set_data_root(self, path: Path | str) -> None:
        self.db.set_setting(self.KEY_DATA_ROOT, str(Path(path)))

    @property
    def retention_days_default(self) -> int:
        raw = self.db.get_setting(self.KEY_RETENTION)
        if raw is None:
            return DEFAULT_RETENTION_DAYS
        try:
            return self._clamp(int(raw))
        except ValueError:
            return DEFAULT_RETENTION_DAYS

    def set_retention_days_default(self, days: int) -> int:
        clamped = self._clamp(int(days))
        self.db.set_setting(self.KEY_RETENTION, str(clamped))
        return clamped

    @staticmethod
    def _clamp(days: int) -> int:
        return max(MIN_RETENTION_DAYS, min(MAX_RETENTION_DAYS, days))

    @property
    def language(self) -> str | None:
        """Saved UI language code, or None when the user never picked one."""
        value = (self.db.get_setting(self.KEY_LANGUAGE, "") or "").strip()
        return value if value in LANGUAGE_CODES else None

    def set_language(self, code: str) -> None:
        if code not in LANGUAGE_CODES:
            raise ValueError(f"Unsupported language: {code}")
        self.db.set_setting(self.KEY_LANGUAGE, code)

    def vault_dir(self) -> Path:
        root = self.data_root
        return root / "Vault" if root else DEFAULT_VAULT_DIR

    def apps_dir(self) -> Path | None:
        root = self.data_root
        return root / "Apps" if root else None
