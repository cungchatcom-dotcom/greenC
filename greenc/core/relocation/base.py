from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable

from greenc.core.task_controller import TaskControl
from greenc.models import RelocationRecord

MOVING_SUFFIX = ".greenc-moving"
RESTORING_SUFFIX = ".greenc-restoring"
FREE_SPACE_RESERVE = 512 * 1024 * 1024

Saver = Callable[[RelocationRecord], RelocationRecord]


def now_iso() -> str:
    return datetime.now().isoformat(timespec="seconds")


@dataclass
class CheckResult:
    ok: bool
    reason: str = ""


class RelocationStrategy:
    kind = "base"

    def check(self, cand: "RelocationCandidate", target_root: Path) -> CheckResult:
        raise NotImplementedError

    def apply(
        self, cand: "RelocationCandidate", target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        raise NotImplementedError

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        raise NotImplementedError


@dataclass
class RelocationCandidate:
    app_id: str
    name: str
    source_path: str
    size_bytes: int
    strategy: RelocationStrategy
    how: str  # one-line explanation shown under the name
    advanced: bool = False
