from pathlib import Path

import pytest

from diskguard.core.relocation.base import RelocationCandidate
from diskguard.core.relocation.fs_ops import RelocationError
from diskguard.core.relocation.service import strategy_for_record
from diskguard.core.relocation.tool_config import NpmCacheStrategy, PipCacheStrategy
from diskguard.models import RELOCATION_DONE, RELOCATION_UNDONE, RelocationRecord


class FakeNpm:
    def __init__(self, cache: Path):
        self.cache = str(cache)
        self.default = str(cache)
        self.calls = []

    def __call__(self, args):
        self.calls.append(args[1:])
        if args[1:3] == ["config", "get"]:
            return 0, self.cache
        if args[1:3] == ["config", "set"]:
            self.cache = args[4]
            return 0, ""
        if args[1:3] == ["config", "delete"]:
            self.cache = self.default
            return 0, ""
        return 1, ""


def _saver(rec):
    rec.id = rec.id or 3
    return rec


def test_npm_switches_cache_and_deletes_old(tmp_path: Path):
    old = tmp_path / "C" / "npm-cache"
    (old / "_cacache").mkdir(parents=True)
    (old / "_cacache" / "blob").write_bytes(b"x" * 1000)
    runner = FakeNpm(old)
    st = NpmCacheStrategy(runner=runner, which=lambda n: "C:\\npm.cmd")
    assert st.current_cache() == old
    cand = RelocationCandidate("npm", "npm cache", str(old), 1000, st, "how")
    rec = st.apply(cand, tmp_path / "D" / "Green C", None, _saver)
    new = tmp_path / "D" / "Green C" / "Apps" / "npm"
    assert rec.status == RELOCATION_DONE and runner.cache == str(new) and new.is_dir()
    assert not old.exists()
    st.undo(rec, None, _saver)
    assert rec.status == RELOCATION_UNDONE and runner.cache == str(old)


def test_missing_tool_is_blocked(tmp_path: Path):
    st = PipCacheStrategy(runner=lambda a: (1, ""), which=lambda n: None)
    cand = RelocationCandidate("pip", "pip cache", str(tmp_path), 0, st, "how")
    assert not st.check(cand, tmp_path).ok
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path, None, _saver)


def test_set_that_does_not_stick_raises(tmp_path: Path):
    st = NpmCacheStrategy(runner=lambda a: (0, str(tmp_path / "old")), which=lambda n: "npm")
    (tmp_path / "old").mkdir()
    cand = RelocationCandidate("npm", "npm cache", str(tmp_path / "old"), 0, st, "how")
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path / "D", None, _saver)
    assert (tmp_path / "old").exists()


def test_pip_commands(tmp_path: Path):
    calls = []

    def runner(args):
        calls.append(args[1:])
        return 0, str(tmp_path / "pipcache") if args[1:] == ["cache", "dir"] else ""

    st = PipCacheStrategy(runner=runner, which=lambda n: "pip.exe")
    assert st.current_cache() == tmp_path / "pipcache"
    st._unset()
    assert ["config", "--user", "unset", "global.cache-dir"] in calls


def test_factory_builds_tool_strategy():
    rec = RelocationRecord(1, "npm", "npm cache", "tool_config", "a", "b", 0, "done")
    assert isinstance(strategy_for_record(rec), NpmCacheStrategy)
