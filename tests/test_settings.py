from pathlib import Path

from greenc.config import DEFAULT_RETENTION_DAYS, DEFAULT_VAULT_DIR
from greenc.core.settings import AppSettings
from greenc.storage.database import GreenCDB


def test_db_setting_roundtrip(tmp_path: Path):
    db = GreenCDB(tmp_path / "s.db")
    assert db.get_setting("x") is None
    assert db.get_setting("x", "d") == "d"
    db.set_setting("x", "1")
    db.set_setting("x", "2")
    assert db.get_setting("x") == "2"


def test_defaults(tmp_path: Path):
    s = AppSettings(GreenCDB(tmp_path / "s.db"))
    assert s.data_root is None
    assert s.retention_days_default == DEFAULT_RETENTION_DAYS == 7
    assert s.vault_dir() == DEFAULT_VAULT_DIR
    assert s.apps_dir() is None


def test_data_root_persists_across_instances(tmp_path: Path):
    root = tmp_path / "D" / "Green C"
    AppSettings(GreenCDB(tmp_path / "s.db")).set_data_root(root)
    s2 = AppSettings(GreenCDB(tmp_path / "s.db"))
    assert s2.data_root == root
    assert s2.vault_dir() == root / "Vault"
    assert s2.apps_dir() == root / "Apps"


def test_retention_clamped_and_tolerates_garbage(tmp_path: Path):
    db = GreenCDB(tmp_path / "s.db")
    s = AppSettings(db)
    assert s.set_retention_days_default(0) == 1
    assert s.set_retention_days_default(500) == 90
    assert s.set_retention_days_default(14) == 14
    assert s.retention_days_default == 14
    db.set_setting(AppSettings.KEY_RETENTION, "abc")
    assert s.retention_days_default == 7


def test_branding_constants():
    from greenc import config

    assert config.APP_NAME == "Green C:\\"
    assert config.AUTHOR == "Cùng Chat"
    assert config.WEBSITE_URL == "https://cungchat.com"
    assert config.APP_HOME.name == "Green C"
