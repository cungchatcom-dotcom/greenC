from collections import namedtuple
import os
from pathlib import Path

import pytest

from greenc.core.relocation.base import RelocationCandidate
from greenc.core.relocation.fs_ops import RelocationError
from greenc.core.relocation.known_folders import (
    KNOWN_FOLDERS,
    KnownFolderStrategy,
    WindowsShellFolders,
    is_onedrive_path,
)
from greenc.core.relocation.service import strategy_for_record
from greenc.models import RELOCATION_DONE, RELOCATION_UNDONE

Usage = namedtuple("Usage", "total used free")
DL = KNOWN_FOLDERS["kf_downloads"]


class FakeShell:
    def __init__(self, paths):
        self.paths = dict(paths)
        self.set_calls = []

    def get(self, guid):
        return self.paths[guid]

    def set(self, guid, path):
        self.set_calls.append((guid, path))
        self.paths[guid] = path


def _setup(tmp_path):
    dl = tmp_path / "Users" / "me" / "Downloads"
    (dl / "sub").mkdir(parents=True)
    (dl / "setup.exe").write_bytes(b"E" * 3000)
    (dl / "sub" / "a.pdf").write_bytes(b"P" * 2000)
    shell = FakeShell({DL.guid: str(dl)})
    st = KnownFolderStrategy(DL, shell=shell, disk_usage=lambda p: Usage(10**12, 0, 10**12))
    cand = RelocationCandidate(DL.app_id, "Downloads", str(dl), 5000, st, "how")
    return dl, shell, st, cand


def _saver(rec):
    rec.id = rec.id or 7
    return rec


def test_apply_moves_contents_and_repoints_folder(tmp_path: Path):
    dl, shell, st, cand = _setup(tmp_path)
    root = tmp_path / "D" / "Green C"
    rec = st.apply(cand, root, None, _saver)
    assert rec.status == RELOCATION_DONE
    assert shell.paths[DL.guid] == str(root / "Downloads")
    assert (root / "Downloads" / "sub" / "a.pdf").read_bytes() == b"P" * 2000
    assert dl.is_dir() and list(dl.iterdir()) == []


def test_undo_moves_back(tmp_path: Path):
    dl, shell, st, cand = _setup(tmp_path)
    root = tmp_path / "D"
    rec = st.apply(cand, root, None, _saver)
    st.undo(rec, None, _saver)
    assert rec.status == RELOCATION_UNDONE
    assert shell.paths[DL.guid] == str(dl)
    assert (dl / "setup.exe").exists()
    assert not (root / "Downloads").exists()


def test_onedrive_folder_is_blocked(tmp_path: Path, monkeypatch):
    dl, shell, st, cand = _setup(tmp_path)
    monkeypatch.setenv("OneDrive", str(tmp_path / "Users"))
    assert is_onedrive_path(dl)
    res = st.check(cand, tmp_path / "D")
    assert not res.ok and "OneDrive" in res.reason
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path / "D", None, _saver)


def test_shell_failure_keeps_original(tmp_path: Path):
    dl, shell, st, cand = _setup(tmp_path)

    def refuse(guid, path):
        raise OSError("E_ACCESSDENIED")

    shell.set = refuse
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path / "D", None, _saver)
    assert (dl / "setup.exe").exists()
    assert not (tmp_path / "D" / "Downloads").exists()


def test_real_shell_reads_downloads():
    path = WindowsShellFolders().get(DL.guid)
    assert os.path.isabs(path)


def test_factory_builds_known_folder_strategy():
    from greenc.models import RelocationRecord

    rec = RelocationRecord(1, "kf_downloads", "Downloads", "known_folder", "a", "b", 0, "done")
    assert isinstance(strategy_for_record(rec), KnownFolderStrategy)
