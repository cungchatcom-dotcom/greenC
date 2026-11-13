"""
Parallel prefetch + live progress of QuotaEnforcer.enforce_rule:
- Small files are read/hashed by the prefetch pool, large ones streamed; both round-trip.
- Progress events cover every phase and reach the full file/byte totals.
- The cleanup reporter turns events into status-bar text with counters and an ETA.
"""
import hashlib
import os
from pathlib import Path
import time
import zipfile

from greenc.core import quota_enforcer as qe_mod
from greenc.core.quota_enforcer import ArchiveProgress, QuotaEnforcer
from greenc.core.smart_analyzer import _ArchiveProgressReporter, format_eta
from greenc.core.task_controller import TaskControl
from greenc.models import QuotaRule
from greenc.storage.database import GreenCDB

KB = 1024


def _rule(app_dir: Path) -> QuotaRule:
    return QuotaRule(
        id=None, app_name="Prefetch App", process_name="p.exe", service_name="",
        target_paths=[str(app_dir)], max_bytes=1, watermark_ratio=0.0,
        retention_days=7, min_file_age_seconds=60,
    )


def _make_files(app_dir: Path, count: int) -> dict[Path, bytes]:
    app_dir.mkdir(parents=True, exist_ok=True)
    files = {}
    t = time.time() - 3600
    for i in range(count):
        data = os.urandom(64) * (i + 1) * 4  # 256 B .. count*256 B, distinct content
        fp = app_dir / f"f{i:03d}.bin"
        fp.write_bytes(data)
        os.utime(fp, (t + i, t + i))
        files[fp] = data
    return files


def test_prefetch_and_streaming_round_trip_with_progress(tmp_path: Path, monkeypatch):
    # Files above 4 KB go through the streaming path, smaller ones through the pool.
    monkeypatch.setattr(qe_mod, "_PREFETCH_MAX_FILE", 4 * KB)
    monkeypatch.setattr(qe_mod, "_PREFETCH_WINDOW_FILES", 5)
    files = _make_files(tmp_path / "app", 40)
    db = GreenCDB(tmp_path / "p.db")
    rule = _rule(tmp_path / "app")
    rule.id = db.save_quota_rule(rule)

    events: list[ArchiveProgress] = []
    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(
        rule, progress=lambda p: events.append(ArchiveProgress(**vars(p)))
    )

    assert arch is not None and arch.file_count == 40
    assert not any(fp.exists() for fp in files)
    by_path = {vf.original_path: vf for vf in db.get_vault_files(arch.id)}
    with zipfile.ZipFile(arch.zip_path) as zf:
        for fp, data in files.items():
            vf = by_path[str(fp)]
            assert zf.read(vf.rel_arcname) == data
            assert vf.sha256 == hashlib.sha256(data).hexdigest()

    phases = [e.phase for e in events]
    assert phases[0] == "scan" and "select" in phases and phases[-1] == "finalize"
    compress = [e for e in events if e.phase == "compress"]
    assert compress[-1].done_files == compress[-1].total_files == 40
    assert compress[-1].done_bytes == compress[-1].total_bytes == sum(map(len, files.values()))
    done = [e.done_bytes for e in compress]
    assert done == sorted(done)  # monotonic, including per-chunk streaming updates


def test_reporter_formats_counters_and_eta(monkeypatch):
    reports: list[tuple[int, str, str]] = []
    ctrl = TaskControl()
    ctrl.set_callback(lambda pct, title, detail: reports.append((pct, title, detail)))
    clock = [100.0]
    monkeypatch.setattr("greenc.core.smart_analyzer.time.perf_counter", lambda: clock[0])

    rep = _ArchiveProgressReporter(ctrl, "Cleaning uv", 5, 95)
    rep(ArchiveProgress("scan"))
    rep(ArchiveProgress("compress", 0, 100, 0, 100 * KB * KB))
    clock[0] += 10.0
    rep(ArchiveProgress("compress", 25, 100, 25 * KB * KB, 100 * KB * KB, current="numpy.pyd"))

    pct, title, detail = reports[-1]
    assert title == "Cleaning uv"
    assert "25/100" in detail and "numpy.pyd" in detail
    assert "0:30" in detail  # 10 s for 25 % -> 30 s left
    assert 5 < pct < 95
    assert reports[0][0] == 5


def test_format_eta():
    assert format_eta(5) == "0:05"
    assert format_eta(125) == "2:05"
    assert format_eta(3725) == "1:02:05"
    assert format_eta(-3) == "0:00"


def test_precompressed_entries_choose_store_or_deflate_and_pass_crc(tmp_path: Path, monkeypatch):
    import gzip

    monkeypatch.setattr(qe_mod, "_PREFETCH_MAX_FILE", 64 * KB)
    # Small chunks: the streamed text file is deflated as ~25 parallel chunks.
    monkeypatch.setattr(qe_mod, "_COPY_CHUNK", 16 * KB)
    app = tmp_path / "app"
    app.mkdir()
    t = time.time() - 3600
    payloads = {
        # npm-style: gzip content without an extension -> STORED by magic bytes
        "content_blob": gzip.compress(os.urandom(20 * KB)),
        "notes.txt": b"hello green c\n" * 4000,  # compressible -> DEFLATED
        "random.dat": os.urandom(30 * KB),  # incompressible -> STORED after trying
        "big_text.log": b"log line 0123456789\n" * 20000,  # > 64 KB: streamed, DEFLATED
        "big_rand.bin": os.urandom(200 * KB),  # > 64 KB: streamed, STORED by sample
    }
    for i, (name, data) in enumerate(payloads.items()):
        fp = app / name
        fp.write_bytes(data)
        os.utime(fp, (t + i, t + i))

    db = GreenCDB(tmp_path / "c.db")
    rule = _rule(app)
    rule.id = db.save_quota_rule(rule)
    arch = QuotaEnforcer(db, tmp_path / "vault").enforce_rule(rule)
    assert arch is not None and arch.file_count == len(payloads)

    expected = {
        "content_blob": zipfile.ZIP_STORED,
        "notes.txt": zipfile.ZIP_DEFLATED,
        "random.dat": zipfile.ZIP_STORED,
        "big_text.log": zipfile.ZIP_DEFLATED,
        "big_rand.bin": zipfile.ZIP_STORED,
    }
    with zipfile.ZipFile(arch.zip_path) as zf:
        assert zf.testzip() is None  # every CRC matches
        for info in zf.infolist():
            name = info.filename.split("_", 2)[2]
            assert info.compress_type == expected[name], name
            assert zf.read(info) == payloads[name]
            assert info.date_time[0] >= 1980
