from pathlib import Path
import shutil
import subprocess
from typing import Callable

from greenc.core.relocation.base import (
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
    now_iso,
)
from greenc.core.relocation.fs_ops import RelocationError, remove_tree, same_path
from greenc.core.task_controller import TaskControl
from greenc.models import RELOCATION_DONE, RELOCATION_UNDONE, RelocationRecord

Runner = Callable[[list[str]], tuple[int, str]]
_CREATE_NO_WINDOW = 0x08000000


def subprocess_runner(args: list[str]) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            args, capture_output=True, text=True, timeout=60, creationflags=_CREATE_NO_WINDOW
        )
    except (OSError, subprocess.TimeoutExpired):
        return 1, ""
    return proc.returncode, (proc.stdout or "").strip()


class ToolCacheStrategy(RelocationStrategy):
    """Point a developer tool's cache at the data drive; the old cache is simply deleted."""

    kind = "tool_config"
    app_id = ""
    name = ""
    exe_name = ""

    def __init__(self, runner: Runner = subprocess_runner, which: Callable = shutil.which):
        self._run = runner
        self._which = which

    def exe(self) -> str | None:
        return self._which(self.exe_name)

    def current_cache(self) -> Path | None:
        raise NotImplementedError

    def _set(self, path: Path) -> None:
        raise NotImplementedError

    def _unset(self) -> None:
        raise NotImplementedError

    def _call(self, *args: str) -> tuple[int, str]:
        exe = self.exe()
        if not exe:
            raise RelocationError(f"Không tìm thấy {self.exe_name}")
        return self._run([exe, *args])

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        if not self.exe():
            return CheckResult(False, f"Không tìm thấy {self.exe_name}")
        return CheckResult(True)

    def apply(
        self, cand: RelocationCandidate, target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        res = self.check(cand, target_root)
        if not res.ok:
            raise RelocationError(res.reason)
        old = self.current_cache()
        new = Path(target_root) / "Apps" / self.app_id
        new.mkdir(parents=True, exist_ok=True)
        self._set(new)
        now = self.current_cache()
        if now is None or not same_path(now, new):
            raise RelocationError(f"{self.exe_name} không nhận cấu hình mới")
        leftovers: list[str] = []
        if old is not None and old.exists() and not same_path(old, new):
            leftovers = remove_tree(old)
        return save(
            RelocationRecord(
                id=None, app_id=self.app_id, app_name=self.name, strategy=self.kind,
                source_path=str(old or cand.source_path), target_path=str(new),
                size_bytes=cand.size_bytes, status=RELOCATION_DONE, created_at=now_iso(),
                detail=f"Còn {len(leftovers)} mục chưa xóa được" if leftovers else "",
            )
        )

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        self._unset()
        remove_tree(Path(rec.target_path))
        rec.status = RELOCATION_UNDONE
        return save(rec)


class NpmCacheStrategy(ToolCacheStrategy):
    app_id = "npm"
    name = "npm cache"
    exe_name = "npm"

    def current_cache(self) -> Path | None:
        rc, out = self._call("config", "get", "cache")
        return Path(out) if rc == 0 and out else None

    def _set(self, path: Path) -> None:
        rc, _ = self._call("config", "set", "cache", str(path))
        if rc != 0:
            raise RelocationError("npm từ chối đổi thư mục cache")

    def _unset(self) -> None:
        self._call("config", "delete", "cache")


class PipCacheStrategy(ToolCacheStrategy):
    app_id = "pip"
    name = "pip cache"
    exe_name = "pip"

    def current_cache(self) -> Path | None:
        rc, out = self._call("cache", "dir")
        return Path(out) if rc == 0 and out else None

    def _set(self, path: Path) -> None:
        rc, _ = self._call("config", "--user", "set", "global.cache-dir", str(path))
        if rc != 0:
            raise RelocationError("pip từ chối đổi thư mục cache")

    def _unset(self) -> None:
        self._call("config", "--user", "unset", "global.cache-dir")


TOOL_STRATEGIES: dict[str, type[ToolCacheStrategy]] = {
    "npm": NpmCacheStrategy,
    "pip": PipCacheStrategy,
}
