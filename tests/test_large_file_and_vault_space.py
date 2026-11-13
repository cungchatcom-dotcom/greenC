"""
Regression tests for cleanup failures on large files and low vault disk space:
- Files > 2 GiB must be archived with ZIP64 (previously: RuntimeError "File size too large").
- Already-compressed media must be STORED (no wasted DEFLATE CPU), text/logs DEFLATED.
- The vault must never fill the disk: selection is trimmed to fit free space, and
  if nothing fits a clear InsufficientVaultSpaceError is raised with originals untouched.
- One failing app/rule must not abort cleanup of the others.
"""
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import time
import zipfile

import pytest

from greenc.core import quota_enforcer as qe_mod
from greenc.core.quota_enforcer import InsufficientVaultSpaceError, QuotaEnforcer
from greenc.core.smart_analyzer import AppAnalysisProposal, SmartAppAnalyzer
from greenc.models import QuotaRule
from greenc.storage.database import GreenCDB

MB = 1024 * 1024
GB = 1024 * MB


def _make_sparse(path: Path, size: int) -> None:
    """Create a sparse NTFS file of `size` bytes (reads as zeros, ~0 bytes on disk)."""
    path.write_bytes(b"")
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateFileW.restype = wintypes.HANDLE
    h = kernel32.CreateFileW(str(path), 0x40000000, 0, None, 3, 0x80, None)
    assert h != wintypes.HANDLE(-1).value
    try:
        returned = wintypes.DWORD(0)
        FSCTL_SET_SPARSE = 0x000900C4
        ok = kernel32.DeviceIoControl(
            wintypes.HANDLE(h), FSCTL_SET_SPARSE, None, 0, None, 0,
            ctypes.byref(returned), None,
        )
        assert ok, f"FSCTL_SET_SPARSE failed: {ctypes.get_last_error()}"
    finally:
        kernel32.CloseHandle(wintypes.HANDLE(h))
    with open(path, "r+b") as f:
        f.truncate(size)


def _rule(app_dir: Path, max_bytes: int, name: str = "Big App") -> QuotaRule:
    return QuotaRule(
        id=None,
        app_name=name,
        process_name="big.exe",
        service_name="",
        target_paths=[str(app_dir)],
        max_bytes=max_bytes,
        watermark_ratio=0.80,
        retention_days=7,
        min_file_age_seconds=60,
    )


def test_file_larger_than_2gib_is_archived_with_zip64(tmp_path: Path):
    db = GreenCDB(tmp_path / "big.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    big = app_dir / "huge_cache.bin"
    size = (1 << 31) + MB  # just past zipfile.ZIP64_LIMIT (2 GiB - 1)
    _make_sparse(big, size)
    os.utime(big, (now - 3600, now - 3600))

    rule = _rule(app_dir, max_bytes=1 * GB)
    rule.id = db.save_quota_rule(rule)

    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(rule, now_ts=now)

    assert arch is not None
    assert arch.file_count == 1
    assert arch.original_bytes == size
    assert not big.exists()
    with zipfile.ZipFile(arch.zip_path) as zf:
        (info,) = zf.infolist()
        assert info.file_size == size
        assert info.compress_type == zipfile.ZIP_DEFLATED


def test_media_is_stored_and_text_is_deflated(tmp_path: Path):
    db = GreenCDB(tmp_path / "media.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()

    video = app_dir / "clip.MP4"
    video.write_bytes(os.urandom(512 * 1024))
    log = app_dir / "trace.log"
    log.write_bytes(b"LOG LINE 0123456789\n" * 30_000)
    for i, p in enumerate((video, log)):
        os.utime(p, (now - 3600 - i, now - 3600 - i))

    rule = _rule(app_dir, max_bytes=1024)
    rule.id = db.save_quota_rule(rule)
    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(rule, now_ts=now)
    assert arch is not None and arch.file_count == 2

    with zipfile.ZipFile(arch.zip_path) as zf:
        types = {i.filename.split("_", 2)[2]: i.compress_type for i in zf.infolist()}
    assert types["clip.MP4"] == zipfile.ZIP_STORED
    assert types["trace.log"] == zipfile.ZIP_DEFLATED


def _fake_disk_usage(free: int):
    def _usage(_path):
        return qe_mod.shutil._ntuple_diskusage(10 * GB, 10 * GB - free, free)
    return _usage


def test_selection_is_trimmed_to_fit_vault_free_space(tmp_path: Path, monkeypatch):
    db = GreenCDB(tmp_path / "trim.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()
    files = []
    for i in range(4):
        fp = app_dir / f"old_{i}.dat"
        fp.write_bytes(b"X" * MB)
        os.utime(fp, (now - 10_000 + i, now - 10_000 + i))  # old_0 is the oldest
        files.append(fp)

    rule = _rule(app_dir, max_bytes=512 * 1024)  # wants to archive everything
    rule.id = db.save_quota_rule(rule)

    # Room for exactly 2 files on top of the safety reserve
    monkeypatch.setattr(
        qe_mod.shutil, "disk_usage",
        _fake_disk_usage(qe_mod.VAULT_FREE_SPACE_RESERVE + int(2.5 * MB)),
    )
    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(rule, now_ts=now)

    assert arch is not None
    assert arch.file_count == 2
    assert not files[0].exists() and not files[1].exists()
    assert files[2].exists() and files[3].exists()


def test_no_vault_space_raises_clear_error_and_keeps_originals(tmp_path: Path, monkeypatch):
    db = GreenCDB(tmp_path / "full.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()
    fp = app_dir / "old.dat"
    fp.write_bytes(b"X" * MB)
    os.utime(fp, (now - 3600, now - 3600))

    rule = _rule(app_dir, max_bytes=1024)
    rule.id = db.save_quota_rule(rule)
    monkeypatch.setattr(qe_mod.shutil, "disk_usage", _fake_disk_usage(10 * MB))

    vault = tmp_path / "vault"
    with pytest.raises(InsufficientVaultSpaceError):
        QuotaEnforcer(db, vault).enforce_rule(rule, now_ts=now)

    assert fp.exists()
    assert list(vault.glob("*.zip")) == []


@pytest.mark.parametrize("path_kind", ["prefetched", "streamed_stored", "streamed_deflate"])
def test_disk_full_during_write_removes_partial_zip_and_keeps_originals(
    tmp_path: Path, monkeypatch, path_kind
):
    db = GreenCDB(tmp_path / "enospc.db")
    app_dir = tmp_path / "app"
    app_dir.mkdir()
    now = time.time()
    fp = app_dir / "old.dat"
    fp.write_bytes(os.urandom(MB) if path_kind == "streamed_stored" else b"X" * MB)
    os.utime(fp, (now - 3600, now - 3600))
    rule = _rule(app_dir, max_bytes=1024)
    rule.id = db.save_quota_rule(rule)

    real_open = qe_mod.zipfile.ZipFile.open

    def _open_then_fail(self, name, mode="r", *a, **kw):
        handle = real_open(self, name, mode, *a, **kw)
        if mode == "w":
            def _write(_data):
                err = OSError(28, "No space left on device")
                err.winerror = 112
                raise err
            handle.write = _write
        return handle

    def _entry_fails(*_a, **_kw):
        err = OSError(28, "No space left on device")
        err.winerror = 112
        raise err

    if path_kind == "prefetched":
        monkeypatch.setattr(qe_mod, "_write_prepared_entry", _entry_fails)
    else:
        monkeypatch.setattr(qe_mod, "_PREFETCH_MAX_FILE", 64 * 1024)
        monkeypatch.setattr(qe_mod.zipfile.ZipFile, "open", _open_then_fail)
        monkeypatch.setattr(qe_mod, "_end_raw_entry", _entry_fails)
    vault = tmp_path / "vault"
    with pytest.raises(InsufficientVaultSpaceError):
        QuotaEnforcer(db, vault).enforce_rule(rule, now_ts=now)

    assert fp.exists()
    assert list(vault.glob("*.zip")) == []


def test_one_failing_app_does_not_abort_other_apps(tmp_path: Path, monkeypatch):
    db = GreenCDB(tmp_path / "multi.db")
    now = time.time()
    proposals = []
    for name in ("App Fails", "App Works"):
        d = tmp_path / name.replace(" ", "_")
        d.mkdir()
        fp = d / "old.dat"
        fp.write_bytes(b"X" * MB)
        os.utime(fp, (now - 3600, now - 3600))
        proposals.append(
            AppAnalysisProposal(
                app_name=name, process_name="x.exe", service_name="",
                is_running=False, storage_summary="", target_paths=[str(d)],
                total_physical_bytes=MB, reclaimable_bytes=MB,
                recommended_max_bytes=1024, probe_passed=True,
                probe_strategy="", probe_detail="",
            )
        )

    real_enforce = QuotaEnforcer.enforce_rule

    def _enforce(self, rule, now_ts=None, **kw):
        if rule.app_name == "App Fails":
            raise InsufficientVaultSpaceError("Ổ chứa Kho dự phòng không đủ dung lượng")
        return real_enforce(self, rule, now_ts=now_ts, **kw)

    monkeypatch.setattr(QuotaEnforcer, "enforce_rule", _enforce)
    analyzer = SmartAppAnalyzer(db, tmp_path / "vault", probe_dir=tmp_path / "probe")
    res = analyzer.execute_one_click_cleanup(proposals)

    assert res["archives_created"] == 1
    assert res["rules_saved"] == 2
    assert len(res["errors"]) == 1 and "App Fails" in res["errors"][0]

    # Background path: enforce_all_rules must also continue past the failing rule
    created = QuotaEnforcer(db, tmp_path / "vault").enforce_all_rules()
    assert isinstance(created, list)
