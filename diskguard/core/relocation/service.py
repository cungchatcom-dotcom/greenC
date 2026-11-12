from pathlib import Path
from typing import Callable

from diskguard.core.relocation.base import CheckResult, RelocationCandidate, RelocationStrategy, now_iso
from diskguard.core.relocation.fs_ops import RelocationError
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.known_folders import KNOWN_FOLDERS, KnownFolderStrategy
from diskguard.core.relocation.tool_config import TOOL_STRATEGIES
from diskguard.core.settings import AppSettings
from diskguard.core.task_controller import TaskControl
from diskguard.models import RELOCATION_DONE, RelocationRecord
from diskguard.storage.database import DiskGuardDB


def strategy_for_record(rec: RelocationRecord) -> RelocationStrategy:
    if rec.strategy == "junction":
        return JunctionStrategy([p for p in rec.process_names.split(",") if p])
    if rec.strategy == "known_folder" and rec.app_id in KNOWN_FOLDERS:
        return KnownFolderStrategy(KNOWN_FOLDERS[rec.app_id])
    if rec.strategy == "tool_config" and rec.app_id in TOOL_STRATEGIES:
        return TOOL_STRATEGIES[rec.app_id]()
    raise RelocationError("Không thể hoàn tác mục này tự động")


class RelocationService:
    def __init__(
        self,
        db: DiskGuardDB,
        settings: AppSettings,
        strategy_factory: Callable[[RelocationRecord], RelocationStrategy] | None = None,
    ):
        self.db = db
        self.settings = settings
        self.strategy_factory = strategy_factory or strategy_for_record

    def target_root(self) -> Path:
        root = self.settings.data_root
        if root is None:
            raise RelocationError("Chưa chọn ổ lưu dữ liệu. Chọn trong Cài đặt.")
        return root

    def _save(self, rec: RelocationRecord) -> RelocationRecord:
        rec.id = self.db.save_relocation(rec)
        return rec

    def check(self, cand: RelocationCandidate) -> CheckResult:
        return cand.strategy.check(cand, self.target_root())

    def apply(self, cand: RelocationCandidate, ctrl: TaskControl | None = None) -> RelocationRecord:
        return cand.strategy.apply(cand, self.target_root(), ctrl, self._save)

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None = None) -> RelocationRecord:
        return self.strategy_factory(rec).undo(rec, ctrl, self._save)

    def records(self) -> list[RelocationRecord]:
        return self.db.list_relocations(status=RELOCATION_DONE)

    def complete_guided(self, cand: RelocationCandidate) -> RelocationRecord | None:
        strategy = cand.strategy
        if not hasattr(strategy, "verify_done") or not strategy.verify_done():
            return None
        return self._save(
            RelocationRecord(
                id=None, app_id=cand.app_id, app_name=cand.name, strategy=strategy.kind,
                source_path=cand.source_path, target_path=str(strategy.data_dir()),
                size_bytes=cand.size_bytes, status=RELOCATION_DONE, created_at=now_iso(),
            )
        )
