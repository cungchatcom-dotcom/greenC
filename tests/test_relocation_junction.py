from collections import namedtuple
import os
from pathlib import Path

import pytest

from diskguard.core.relocation import junction as jmod
from diskguard.core.relocation.base import MOVING_SUFFIX, RelocationCandidate
from diskguard.core.relocation.fs_ops import RelocationError, create_junction, is_reparse_point
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.recovery import recover_interrupted
from diskguard.core.relocation.service import RelocationService
from diskguard.core.settings import AppSettings
from diskguard.models import RELOCATION_DONE, RELOCATION_FAILED, RELOCATION_MOVING, RELOCATION_UNDONE, RelocationRecord
from diskguard.storage.database import DiskGuardDB

Usage = namedtuple("Usage", "total used free")
BIG = lambda _p: Usage(10**13, 0, 10**13)  # noqa: E731
FULL = lambda _p: Usage(10**9, 10**9, 1024)  # noqa: E731


def _app(tmp_path: Path) -> Path:
    src = tmp_path / "C" / "AppData" / "Roaming" / "Telegram Desktop"
    (src / "tdata" / "user").mkdir(parents=True)
    (src / "tdata" / "user" / "map.bin").write_bytes(os.urandom(50_000))
    (src / "log.txt").write_text("hello")
    return src


def _cand(src: Path, strategy: JunctionStrategy) -> RelocationCandidate:
    return RelocationCandidate("telegram", "Telegram Desktop", str(src), 50_005, strategy, "how")


def _strategy(running=False, usage=BIG) -> JunctionStrategy:
    return JunctionStrategy(["Telegram.exe"], is_running=lambda names, folder: running, disk_usage=usage)


class _Saver:
    def __init__(self):
        self.statuses: list[str] = []

    def __call__(self, rec: RelocationRecord) -> RelocationRecord:
        self.statuses.append(rec.status)
        rec.id = rec.id or 1
        return rec


def test_apply_moves_data_and_leaves_working_junction(tmp_path: Path):
    src = _app(tmp_path)
    original = (src / "tdata" / "user" / "map.bin").read_bytes()
    root = tmp_path / "D" / "Green C"
    save = _Saver()
    rec = _strategy().apply(_cand(src, _strategy()), root, None, save)
    target = root / "Apps" / "telegram"
    assert save.statuses == [RELOCATION_MOVING, RELOCATION_DONE]
    assert rec.target_path == str(target)
    assert is_reparse_point(src)
    assert (src / "tdata" / "user" / "map.bin").read_bytes() == original
    assert (target / "log.txt").read_text() == "hello"
    assert not Path(str(src) + MOVING_SUFFIX).exists()


def test_blocked_when_app_running(tmp_path: Path):
    src = _app(tmp_path)
    st = _strategy(running=True)
    res = st.check(_cand(src, st), tmp_path / "D")
    assert not res.ok and "Cần đóng Telegram Desktop" in res.reason
    with pytest.raises(RelocationError):
        st.apply(_cand(src, st), tmp_path / "D", None, _Saver())
    assert not is_reparse_point(src)


def test_blocked_when_target_drive_full(tmp_path: Path):
    src = _app(tmp_path)
    st = _strategy(usage=FULL)
    res = st.check(_cand(src, st), tmp_path / "D")
    assert not res.ok and "không đủ dung lượng" in res.reason


def test_rename_failure_rolls_back(tmp_path: Path, monkeypatch):
    src = _app(tmp_path)
    root = tmp_path / "D"

    def busy(a, b):
        raise PermissionError(32, "in use")

    monkeypatch.setattr(jmod.os, "rename", busy)
    save = _Saver()
    with pytest.raises(RelocationError, match="đang được sử dụng"):
        _strategy().apply(_cand(src, _strategy()), root, None, save)
    assert save.statuses[-1] == RELOCATION_FAILED
    assert src.is_dir() and not is_reparse_point(src)
    assert not (root / "Apps" / "telegram").exists()


def test_undo_restores_real_folder(tmp_path: Path):
    src = _app(tmp_path)
    root = tmp_path / "D"
    st = _strategy()
    rec = st.apply(_cand(src, st), root, None, _Saver())
    st.undo(rec, None, _Saver())
    assert rec.status == RELOCATION_UNDONE
    assert src.is_dir() and not is_reparse_point(src)
    assert (src / "log.txt").read_text() == "hello"
    assert not (root / "Apps" / "telegram").exists()


def _moving_record(db: DiskGuardDB, src: Path, target: Path) -> RelocationRecord:
    rec = RelocationRecord(None, "telegram", "Telegram Desktop", "junction", str(src), str(target), 5, RELOCATION_MOVING)
    rec.id = db.save_relocation(rec)
    return rec


def test_recover_after_rename_without_junction(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "r.db")
    src = _app(tmp_path)
    target = tmp_path / "D" / "Apps" / "telegram"
    target.mkdir(parents=True)
    (target / "log.txt").write_text("hello")
    os.rename(src, str(src) + MOVING_SUFFIX)
    rec = _moving_record(db, src, target)
    recover_interrupted(db)
    assert src.is_dir() and not is_reparse_point(src) and (src / "log.txt").exists()
    assert not target.exists()
    assert db.get_relocation(rec.id).status == RELOCATION_FAILED


def test_recover_after_junction_created(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "r.db")
    src = _app(tmp_path)
    target = tmp_path / "D" / "Apps" / "telegram"
    target.mkdir(parents=True)
    (target / "log.txt").write_text("hello")
    os.rename(src, str(src) + MOVING_SUFFIX)
    create_junction(src, target)
    rec = _moving_record(db, src, target)
    recover_interrupted(db)
    assert is_reparse_point(src) and (src / "log.txt").read_text() == "hello"
    assert not Path(str(src) + MOVING_SUFFIX).exists()
    assert db.get_relocation(rec.id).status == RELOCATION_DONE


def test_service_persists_records_and_requires_data_root(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "s.db")
    settings = AppSettings(db)
    svc = RelocationService(db, settings)
    src = _app(tmp_path)
    st = _strategy()
    with pytest.raises(RelocationError, match="Chưa chọn ổ"):
        svc.apply(_cand(src, st))
    settings.set_data_root(tmp_path / "D" / "Green C")
    rec = svc.apply(_cand(src, st))
    assert [r.id for r in svc.records()] == [rec.id]
    stored = db.get_relocation(rec.id)
    assert stored.status == RELOCATION_DONE and stored.process_names == "Telegram.exe"
    svc.strategy_factory = lambda r: st
    svc.undo(stored)
    assert db.get_relocation(rec.id).status == RELOCATION_UNDONE
