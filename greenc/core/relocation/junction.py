import os
from pathlib import Path
import shutil
from typing import Callable, Iterable

import psutil

from greenc.core.relocation.base import (
    FREE_SPACE_RESERVE,
    MOVING_SUFFIX,
    RESTORING_SUFFIX,
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
    now_iso,
)
from greenc.core.relocation.fs_ops import (
    RelocationError,
    create_junction,
    is_reparse_point,
    junction_target_matches,
    remove_tree,
    verified_copy_tree,
)
from greenc.core.task_controller import TaskControl
from greenc.models import (
    RELOCATION_DONE,
    RELOCATION_FAILED,
    RELOCATION_MOVING,
    RELOCATION_UNDONE,
    RelocationRecord,
)


def default_is_running(process_names: Iterable[str], folder: str) -> bool:
    """True if a process with one of the names, or whose exe lives in `folder`, is running."""
    names = {n.lower() for n in process_names if n}
    folder_norm = os.path.normcase(os.path.abspath(folder)) + os.sep
    for proc in psutil.process_iter(["name", "exe"]):
        try:
            name = (proc.info.get("name") or "").lower()
            exe = proc.info.get("exe") or ""
        except (psutil.Error, OSError):
            continue
        if name in names:
            return True
        if exe and os.path.normcase(exe).startswith(folder_norm):
            return True
    return False


class JunctionStrategy(RelocationStrategy):
    """Copy (verified) → rename original aside → junction → verify → delete original."""

    kind = "junction"

    def __init__(
        self,
        process_names: Iterable[str] = (),
        is_running: Callable[[Iterable[str], str], bool] | None = None,
        disk_usage: Callable = shutil.disk_usage,
    ):
        self.process_names = [p for p in process_names if p]
        self._is_running = is_running or default_is_running
        self._disk_usage = disk_usage

    @staticmethod
    def target_for(cand: RelocationCandidate, target_root: Path) -> Path:
        return Path(target_root) / "Apps" / cand.app_id

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        src = Path(cand.source_path)
        if not src.is_dir():
            return CheckResult(False, "Thư mục nguồn không còn tồn tại")
        if is_reparse_point(src):
            return CheckResult(False, "Thư mục này đã được chuyển trước đó")
        if self._is_running(self.process_names, str(src)):
            return CheckResult(False, f"Cần đóng {cand.name}")
        target = self.target_for(cand, target_root)
        if target.exists() and any(target.iterdir()):
            return CheckResult(False, f"Thư mục đích đã có dữ liệu: {target}")
        anchor = Path(target_root).anchor or str(target_root)
        if self._disk_usage(anchor).free < cand.size_bytes + FREE_SPACE_RESERVE:
            return CheckResult(False, "Ổ đích không đủ dung lượng trống")
        return CheckResult(True)

    def apply(
        self, cand: RelocationCandidate, target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        res = self.check(cand, target_root)
        if not res.ok:
            raise RelocationError(res.reason)
        src = Path(cand.source_path)
        target = self.target_for(cand, target_root)
        moving = Path(str(src) + MOVING_SUFFIX)

        try:
            verified_copy_tree(src, target, ctrl, cand.size_bytes, f"Đang chuyển {cand.name}")
        except BaseException:
            remove_tree(target)
            raise

        rec = save(
            RelocationRecord(
                id=None, app_id=cand.app_id, app_name=cand.name, strategy=self.kind,
                source_path=str(src), target_path=str(target), size_bytes=cand.size_bytes,
                status=RELOCATION_MOVING, process_names=",".join(self.process_names),
                created_at=now_iso(),
            )
        )

        try:
            os.rename(src, moving)
        except OSError as err:
            remove_tree(target)
            rec.status = RELOCATION_FAILED
            rec.detail = f"Thư mục đang được sử dụng: {err.strerror or err}"
            save(rec)
            raise RelocationError(
                f"Không thể chuyển: thư mục đang được sử dụng. Hãy đóng {cand.name} rồi thử lại."
            ) from err

        try:
            create_junction(src, target)
            if not junction_target_matches(src, target):
                raise RelocationError("Liên kết tạo ra không trỏ đúng thư mục đích")
        except BaseException as err:
            if is_reparse_point(src):
                os.rmdir(src)
            os.rename(moving, src)
            remove_tree(target)
            rec.status = RELOCATION_FAILED
            rec.detail = str(err)
            save(rec)
            raise

        leftovers = remove_tree(moving)
        rec.status = RELOCATION_DONE
        rec.detail = (
            f"Còn {len(leftovers)} mục cũ chưa xóa được trong {moving}" if leftovers else ""
        )
        return save(rec)

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        src, target = Path(rec.source_path), Path(rec.target_path)
        if self._is_running(self.process_names, str(target)):
            raise RelocationError(f"Cần đóng {rec.app_name}")
        if not junction_target_matches(src, target):
            raise RelocationError("Liên kết không còn hợp lệ, không thể hoàn tác tự động")
        restoring = Path(str(src) + RESTORING_SUFFIX)
        try:
            verified_copy_tree(target, restoring, ctrl, rec.size_bytes, f"Đang trả {rec.app_name} về ổ C:")
        except BaseException:
            remove_tree(restoring)
            raise
        os.rmdir(src)  # removes the junction only
        os.rename(restoring, src)
        leftovers = remove_tree(target)
        rec.status = RELOCATION_UNDONE
        rec.detail = f"Còn {len(leftovers)} mục chưa xóa được ở {target}" if leftovers else ""
        return save(rec)
