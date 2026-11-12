"""
Safety guards for the archive -> delete pipeline:
- Nested junctions / symlinks (reparse points) are never followed, so files outside the
  rule's target folder can never be archived and deleted.
- A file modified while being archived is left in place (its zip copy would be stale).
- Stop requests finish the archive with what is done so far instead of blocking.
"""
import os
from pathlib import Path
import subprocess
import time

from greenc.core import quota_enforcer as qe_mod
from greenc.core.quota_enforcer import QuotaEnforcer
from greenc.core.win_locks import compute_physical_usage
from greenc.models import QuotaRule
from greenc.storage.database import GreenCDB

MB = 1024 * 1024


def _rule(app_dir: Path, name: str = "Guard App") -> QuotaRule:
    return QuotaRule(
        id=None,
        app_name=name,
        process_name="guard.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=1024,  # everything is over quota
        watermark_ratio=0.80,
        retention_days=7,
        min_file_age_seconds=60,
    )


def _old_file(path: Path, data: bytes, age_s: float) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    t = time.time() - age_s
    os.utime(path, (t, t))
    return path


def _mklink_junction(link: Path, target: Path) -> None:
    subprocess.run(
        ["cmd", "/c", "mklink", "/J", str(link), str(target)],
        check=True, capture_output=True,
    )


def test_nested_junction_is_not_followed_and_outside_files_survive(tmp_path: Path):
    outside = tmp_path / "Documents_outside"
    precious = _old_file(outside / "thesis.docx", b"P" * MB, 99_999)
    app_dir = tmp_path / "app"
    inside = _old_file(app_dir / "cache" / "old.bin", b"C" * MB, 50_000)
    _mklink_junction(app_dir / "linked", outside)

    phys, _, inode_map = compute_physical_usage([app_dir])
    seen = {str(p) for group in inode_map.values() for p in group}
    assert str(inside) in seen
    assert not any("linked" in s for s in seen)
    assert phys == MB

    db = GreenCDB(tmp_path / "j.db")
    rule = _rule(app_dir)
    rule.id = db.save_quota_rule(rule)
    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(rule)

    assert arch is not None and arch.file_count == 1
    assert precious.exists(), "file behind a junction must never be deleted"
    assert not inside.exists()


def test_root_target_that_is_a_junction_is_still_scanned(tmp_path: Path):
    real = tmp_path / "F_drive_ZaloData"
    f = _old_file(real / "img" / "a.bin", b"A" * MB, 50_000)
    root_link = tmp_path / "ZaloData"
    _mklink_junction(root_link, real)

    phys, _, inode_map = compute_physical_usage([root_link])
    assert phys == MB
    assert len(inode_map) == 1
    assert f.exists()


def test_file_modified_during_archiving_is_not_deleted(tmp_path: Path, monkeypatch):
    app_dir = tmp_path / "app"
    changing = _old_file(app_dir / "a_chat.log", b"L" * MB, 60_000)
    stable = _old_file(app_dir / "b_media.bin", b"M" * MB, 50_000)

    real_make = qe_mod._make_zipinfo

    def _make_and_touch(path, arcname):
        zinfo = real_make(path, arcname)
        if path.name == "a_chat.log":  # the app appends a line right as we start copying
            with open(path, "ab") as fh:
                fh.write(b"new message\n")
        return zinfo

    monkeypatch.setattr(qe_mod, "_make_zipinfo", _make_and_touch)

    db = GreenCDB(tmp_path / "m.db")
    rule = _rule(app_dir)
    rule.id = db.save_quota_rule(rule)
    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(rule)

    assert arch is not None and arch.file_count == 1
    assert changing.exists() and changing.read_bytes().endswith(b"new message\n")
    assert not stable.exists()
    assert [vf.original_path for vf in db.get_vault_files(arch.id)] == [str(stable)]


def test_stop_request_finalizes_partial_archive_and_keeps_the_rest(tmp_path: Path):
    app_dir = tmp_path / "app"
    files = [
        _old_file(app_dir / f"f{i}.dat", b"X" * MB, 90_000 - i * 1000) for i in range(5)
    ]
    db = GreenCDB(tmp_path / "s.db")
    rule = _rule(app_dir)
    rule.id = db.save_quota_rule(rule)

    calls = {"n": 0}

    def _stop_after_two():
        # polled once before starting, then once before each file
        calls["n"] += 1
        return calls["n"] > 3

    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(
        rule, should_stop=_stop_after_two
    )
    assert arch is not None and arch.file_count == 2
    assert [f.exists() for f in files] == [False, False, True, True, True]


def test_enforce_all_rules_stops_between_rules(tmp_path: Path):
    db = GreenCDB(tmp_path / "all.db")
    for name in ("R1", "R2", "R3"):
        d = tmp_path / name
        _old_file(d / "old.dat", b"X" * MB, 50_000)
        r = _rule(d, name=name)
        r.id = db.save_quota_rule(r)

    enforcer = QuotaEnforcer(db, tmp_path / "vault")
    created = enforcer.enforce_all_rules(should_stop=lambda: True)
    assert created == []
    assert all((tmp_path / n / "old.dat").exists() for n in ("R1", "R2", "R3"))
