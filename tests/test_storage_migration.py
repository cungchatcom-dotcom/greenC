from pathlib import Path

from greenc.core import storage_migration as sm
from greenc.models import VaultArchive
from greenc.storage.database import GreenCDB


def _archive(db: GreenCDB, vault: Path, name: str) -> VaultArchive:
    vault.mkdir(parents=True, exist_ok=True)
    zip_p = vault / f"{name}.zip"
    man_p = vault / f"{name}.manifest.json"
    zip_p.write_bytes(b"Z" * 4096)
    man_p.write_text("{}", encoding="utf-8")
    arch = VaultArchive(
        id=None, rule_id=1, app_name="App", zip_path=str(zip_p), manifest_path=str(man_p),
        file_count=1, original_bytes=8192, compressed_bytes=4096,
        created_at="2026-11-01T10:00:00", expires_at="2026-11-08T10:00:00",
        status="QUARANTINED", status_reason="",
    )
    arch.id = db.create_vault_archive(arch, [])
    return arch


def test_migrate_legacy_db_copies_once(tmp_path: Path):
    legacy = GreenCDB(tmp_path / "old" / "greenc.db")
    legacy.set_setting("k", "v")
    new_path = tmp_path / "new" / "greenc.db"
    assert sm.migrate_legacy_db(tmp_path / "old" / "greenc.db", new_path) is True
    assert GreenCDB(new_path).get_setting("k") == "v"
    assert sm.migrate_legacy_db(tmp_path / "old" / "greenc.db", new_path) is False


def test_migrate_skips_without_legacy(tmp_path: Path):
    assert sm.migrate_legacy_db(tmp_path / "none.db", tmp_path / "n.db") is False
    assert not (tmp_path / "n.db").exists()


def test_move_vault_moves_files_and_rewrites_paths(tmp_path: Path):
    db = GreenCDB(tmp_path / "v.db")
    old, new = tmp_path / "old_vault", tmp_path / "D" / "Vault"
    a = _archive(db, old, "a")
    res = sm.move_vault(db, new)
    assert res == {"moved": 1, "errors": []}
    moved = db.get_vault_archive(a.id)
    assert Path(moved.zip_path) == new / "a.zip" and Path(moved.zip_path).exists()
    assert Path(moved.manifest_path) == new / "a.manifest.json"
    assert not (old / "a.zip").exists()
    assert sm.remove_dir_if_empty(old) is True


def test_move_vault_skips_archives_already_there(tmp_path: Path):
    db = GreenCDB(tmp_path / "v.db")
    new = tmp_path / "Vault"
    _archive(db, new, "b")
    assert sm.move_vault(db, new)["moved"] == 0


def test_failed_copy_keeps_source_and_db(tmp_path: Path, monkeypatch):
    db = GreenCDB(tmp_path / "v.db")
    old, new = tmp_path / "old_vault", tmp_path / "Vault"
    a = _archive(db, old, "c")

    def boom(src, dst):
        raise OSError(112, "disk full")

    monkeypatch.setattr(sm.shutil, "copy2", boom)
    res = sm.move_vault(db, new)
    assert res["moved"] == 0 and len(res["errors"]) == 1
    assert Path(db.get_vault_archive(a.id).zip_path) == old / "c.zip"
    assert (old / "c.zip").exists()


def test_legacy_base_dir_follows_renamed_source_folder(tmp_path):
    from greenc.config import legacy_base_dir

    old, renamed = tmp_path / "DiskGuard", tmp_path / "GreenC"
    assert legacy_base_dir((old, renamed)) == old  # nothing anywhere -> first candidate
    (renamed / "data").mkdir(parents=True)
    (renamed / "data" / "diskguard.db").write_bytes(b"")
    assert legacy_base_dir((old, renamed)) == renamed
