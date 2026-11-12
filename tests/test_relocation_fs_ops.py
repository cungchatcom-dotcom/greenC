import os
from pathlib import Path
import subprocess

import pytest

from diskguard.core.relocation import fs_ops
from diskguard.core.task_controller import TaskControl


def _tree(root: Path) -> None:
    (root / "a" / "b").mkdir(parents=True)
    (root / "top.txt").write_bytes(b"T" * 1000)
    (root / "a" / "b" / "deep.bin").write_bytes(os.urandom(70_000))
    os.utime(root / "top.txt", (1_600_000_000, 1_600_000_000))


def _junction(link: Path, target: Path) -> None:
    subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)], check=True, capture_output=True)


def test_verified_copy_tree_copies_content_and_times(tmp_path: Path):
    src, dst = tmp_path / "src", tmp_path / "dst"
    _tree(src)
    files, size = fs_ops.verified_copy_tree(src, dst)
    assert (files, size) == (2, 71_000)
    assert (dst / "a" / "b" / "deep.bin").read_bytes() == (src / "a" / "b" / "deep.bin").read_bytes()
    assert int(os.stat(dst / "top.txt").st_mtime) == 1_600_000_000


def test_nested_junction_is_not_copied_or_followed(tmp_path: Path):
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret.txt").write_text("x")
    src = tmp_path / "src"
    _tree(src)
    _junction(src / "link", outside)
    fs_ops.verified_copy_tree(src, tmp_path / "dst")
    assert not (tmp_path / "dst" / "link").exists()
    assert fs_ops.fast_dir_size(src) == 71_000


def test_hash_mismatch_aborts(tmp_path: Path, monkeypatch):
    src = tmp_path / "src"
    _tree(src)
    monkeypatch.setattr(fs_ops, "_hash_file", lambda p: "different")
    with pytest.raises(fs_ops.RelocationError):
        fs_ops.verified_copy_tree(src, tmp_path / "dst")


def test_stop_request_raises_stopped(tmp_path: Path):
    src = tmp_path / "src"
    _tree(src)
    ctrl = TaskControl()
    ctrl.stop()
    with pytest.raises(fs_ops.RelocationStopped):
        fs_ops.verified_copy_tree(src, tmp_path / "dst", ctrl=ctrl)


def test_remove_tree_never_follows_junctions(tmp_path: Path):
    outside = tmp_path / "outside"
    outside.mkdir()
    keep = outside / "keep.txt"
    keep.write_text("keep")
    victim = tmp_path / "victim"
    _tree(victim)
    os.chmod(victim / "top.txt", 0o444)  # read-only must still be deleted
    _junction(victim / "link", outside)
    assert fs_ops.remove_tree(victim) == []
    assert not victim.exists()
    assert keep.exists()


def test_clear_contents_keeps_folder(tmp_path: Path):
    d = tmp_path / "d"
    _tree(d)
    assert fs_ops.clear_contents(d) == []
    assert d.is_dir() and list(d.iterdir()) == []


def test_create_junction_and_verify(tmp_path: Path):
    target = tmp_path / "target"
    target.mkdir()
    (target / "f.txt").write_text("hi")
    link = tmp_path / "link"
    fs_ops.create_junction(link, target)
    assert fs_ops.is_reparse_point(link)
    assert (link / "f.txt").read_text() == "hi"
    assert fs_ops.junction_target_matches(link, target)
    assert not fs_ops.junction_target_matches(link, tmp_path)
    assert fs_ops.same_path(str(target).upper(), target)
