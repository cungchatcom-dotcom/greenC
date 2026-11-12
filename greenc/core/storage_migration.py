import os
from pathlib import Path
import shutil
import sqlite3

from greenc.core.task_controller import TaskControl
from greenc.i18n import tr
from greenc.storage.database import GreenCDB


def migrate_legacy_db(legacy_db: Path | str, new_db: Path | str) -> bool:
    """Copy the version 0.1 (DiskGuard) database to the new location once (SQLite online backup)."""
    legacy_db, new_db = Path(legacy_db), Path(new_db)
    if not legacy_db.exists() or new_db.exists():
        return False
    new_db.parent.mkdir(parents=True, exist_ok=True)
    src = sqlite3.connect(str(legacy_db))
    try:
        dst = sqlite3.connect(str(new_db))
        try:
            src.backup(dst)
        finally:
            dst.close()
    finally:
        src.close()
    return True


def _same_dir(a: Path, b: Path) -> bool:
    return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


def move_vault(db: GreenCDB, new_dir: Path | str, ctrl: TaskControl | None = None) -> dict:
    """
    Move every live archive (zip + manifest) into `new_dir`. For each archive both files
    are copied and size-checked first, then the DB is updated, then the sources deleted —
    a failure at any point leaves the archive usable at its old location.
    """
    new_dir = Path(new_dir)
    new_dir.mkdir(parents=True, exist_ok=True)
    archives = [
        a
        for a in db.list_vault_archives(status="QUARANTINED")
        if not _same_dir(Path(a.zip_path).parent, new_dir)
    ]
    moved = 0
    errors: list[str] = []
    total = max(1, len(archives))
    for idx, arch in enumerate(archives, start=1):
        zip_src = Path(arch.zip_path)
        man_src = Path(arch.manifest_path)
        if ctrl is not None and not ctrl.report(
            int(idx * 100 / total), tr("Đang chuyển Kho dự phòng"), f"{idx}/{len(archives)}: {zip_src.name}"
        ):
            break
        pairs = [(zip_src, new_dir / zip_src.name), (man_src, new_dir / man_src.name)]
        copied: list[tuple[Path, Path]] = []
        try:
            for src, dst in pairs:
                if not src.exists():
                    continue
                shutil.copy2(src, dst)
                if dst.stat().st_size != src.stat().st_size:
                    raise OSError(tr("Sao chép không đầy đủ: {name}", name=src.name))
                copied.append((src, dst))
        except OSError as err:
            for _, dst in copied:
                dst.unlink(missing_ok=True)
            errors.append(f"{zip_src.name}: {err}")
            continue
        db.update_vault_paths(arch.id or 0, str(pairs[0][1]), str(pairs[1][1]))
        for src, _ in copied:
            try:
                os.remove(src)
            except OSError:
                pass
        moved += 1
    return {"moved": moved, "errors": errors}


def remove_dir_if_empty(path: Path | str) -> bool:
    try:
        os.rmdir(path)
        return True
    except OSError:
        return False
