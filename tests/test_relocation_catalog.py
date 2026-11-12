import json
import os
from pathlib import Path
import subprocess

from greenc.core.relocation.catalog import discover_candidates
from greenc.core.relocation.junction import JunctionStrategy
from greenc.core.relocation.known_folders import KNOWN_FOLDERS, KnownFolderStrategy
from greenc.core.relocation.service import RelocationService
from greenc.core.relocation.zalo import ZaloGuidedStrategy
from greenc.core.settings import AppSettings
from greenc.models import RELOCATION_DONE, RelocationRecord
from greenc.storage.database import GreenCDB

SYS = os.path.splitdrive(os.getcwd())[0].upper() + "\\"


class FakeShell:
    def __init__(self, paths):
        self.paths = paths

    def get(self, guid):
        if guid not in self.paths:
            raise OSError("no folder")
        return self.paths[guid]

    def set(self, guid, path):
        self.paths[guid] = path


def _fill(d: Path, size: int) -> Path:
    d.mkdir(parents=True, exist_ok=True)
    (d / "blob.bin").write_bytes(b"x" * size)
    return d


def _env(tmp_path: Path):
    appdata = tmp_path / "Roaming"
    local = tmp_path / "Local"
    _fill(appdata / "Telegram Desktop", 3000)
    _fill(appdata / "BigTool", 4000)
    _fill(appdata / "Tiny", 10)
    _fill(local / "Microsoft", 9000)  # skip list
    _fill(local / "Programs", 9000)  # skip list
    dl = _fill(tmp_path / "Users" / "me" / "Downloads", 5000)
    shell = FakeShell({KNOWN_FOLDERS["kf_downloads"].guid: str(dl)})
    zcfg = tmp_path / "ZaloData" / "config.json"
    zcfg.parent.mkdir()
    zalo_data = _fill(tmp_path / "ZaloPCData" / "Zalo Data", 6000)
    zcfg.write_text(json.dumps({"db_path": {"location": str(zalo_data / "message")}}), encoding="utf-8")
    return appdata, local, shell, zcfg, dl, zalo_data


def _discover(db, tmp_path, **kw):
    appdata, local, shell, zcfg, dl, zalo_data = _env(tmp_path)
    args = dict(
        system_root=SYS, appdata=appdata, localappdata=local, shell=shell,
        runner=lambda a: (1, ""), which=lambda n: None, zalo_config=zcfg, min_bytes=1000,
    )
    args.update(kw)
    return discover_candidates(db, **args), (appdata, local, shell, zcfg, dl, zalo_data)


def test_discovers_each_strategy_and_orders_advanced_last(tmp_path: Path):
    db = GreenCDB(tmp_path / "c.db")
    cands, (appdata, *_rest) = _discover(db, tmp_path)
    by_id = {c.app_id: c for c in cands}
    assert isinstance(by_id["kf_downloads"].strategy, KnownFolderStrategy)
    assert isinstance(by_id["zalo"].strategy, ZaloGuidedStrategy)
    assert isinstance(by_id["telegram"].strategy, JunctionStrategy)
    assert not by_id["telegram"].advanced
    assert by_id["dir-bigtool"].advanced
    assert "dir-tiny" not in by_id and "dir-microsoft" not in by_id and "dir-programs" not in by_id
    flags = [c.advanced for c in cands]
    assert flags == sorted(flags)


def test_done_relocations_and_junctions_are_excluded(tmp_path: Path):
    db = GreenCDB(tmp_path / "c.db")
    appdata = tmp_path / "Roaming"
    db.save_relocation(
        RelocationRecord(None, "telegram", "Telegram Desktop", "junction",
                         str(appdata / "Telegram Desktop"), "X", 1, RELOCATION_DONE)
    )
    outside = _fill(tmp_path / "outside", 5000)
    appdata.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cmd", "/c", "mklink", "/J", str(appdata / "Linked"), str(outside)], check=True, capture_output=True)
    cands, _ = _discover(db, tmp_path)
    ids = {c.app_id for c in cands}
    assert "telegram" not in ids and "dir-linked" not in ids


def test_zalo_off_system_drive_is_not_a_candidate(tmp_path: Path):
    db = GreenCDB(tmp_path / "c.db")
    zcfg = tmp_path / "z.json"
    zcfg.write_text(json.dumps({"db_path": {"location": "Q:\\Zalo Data\\message"}}), encoding="utf-8")
    cands, _ = _discover(db, tmp_path, zalo_config=zcfg)
    assert "zalo" not in {c.app_id for c in cands}
    z = ZaloGuidedStrategy(config_path=zcfg, system_root=SYS, localappdata=tmp_path)
    assert z.verify_done()


def test_complete_guided_records_only_when_moved(tmp_path: Path):
    db = GreenCDB(tmp_path / "c.db")
    cands, (_a, _l, _s, zcfg, _dl, zalo_data) = _discover(db, tmp_path)
    zalo = next(c for c in cands if c.app_id == "zalo")
    svc = RelocationService(db, AppSettings(db))
    assert svc.complete_guided(zalo) is None
    zcfg.write_text(json.dumps({"db_path": {"location": "Q:\\Zalo Data\\message"}}), encoding="utf-8")
    rec = svc.complete_guided(zalo)
    assert rec is not None and rec.status == RELOCATION_DONE and rec.target_path == "Q:\\Zalo Data"
    steps = zalo.strategy.steps(Path("D:\\Green C"))
    assert any("Quản lý dữ liệu" in s for s in steps) and any("D:\\Green C\\Zalo" in s for s in steps)


def test_advanced_skips_parent_of_tool_cache(tmp_path: Path):
    db = GreenCDB(tmp_path / "c.db")
    local = tmp_path / "Local"
    cache = _fill(local / "pip" / "cache", 5000)

    def runner(args):
        return (0, str(cache)) if args[1:] == ["cache", "dir"] else (1, "")

    cands, _ = _discover(db, tmp_path, runner=runner, which=lambda n: "pip.exe" if n == "pip" else None)
    ids = [c.app_id for c in cands]
    assert "pip" in ids and "dir-pip" not in ids
