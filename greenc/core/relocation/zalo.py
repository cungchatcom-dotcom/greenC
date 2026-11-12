import json
import os
from pathlib import Path
import subprocess

import psutil

from greenc.core.drives import is_on_system_drive
from greenc.core.relocation.base import (
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
)
from greenc.core.relocation.fs_ops import RelocationError
from greenc.core.task_controller import TaskControl
from greenc.i18n import tr
from greenc.models import RelocationRecord


class ZaloGuidedStrategy(RelocationStrategy):
    """Zalo migrates its own database: guide the user through Zalo's setting, then verify."""

    kind = "guided"
    app_id = "zalo"

    def __init__(
        self,
        config_path: Path | str | None = None,
        system_root: str | None = None,
        localappdata: Path | str | None = None,
    ):
        self.config_path = (
            Path(config_path)
            if config_path
            else Path(os.path.expandvars(r"%APPDATA%\ZaloData\config.json"))
        )
        self.system_root = system_root
        self.localappdata = (
            Path(localappdata) if localappdata else Path(os.path.expandvars(r"%LOCALAPPDATA%"))
        )

    def data_dir(self) -> Path | None:
        if self.config_path.exists():
            try:
                data = json.loads(self.config_path.read_text(encoding="utf-8", errors="ignore"))
            except (OSError, ValueError):
                data = {}
            location = (data.get("db_path") or {}).get("location") if isinstance(data, dict) else None
            if location:
                return Path(location).parent
        default = self.localappdata / "ZaloPC"
        return default if default.is_dir() else None

    def is_on_system_drive(self) -> bool:
        d = self.data_dir()
        return d is not None and is_on_system_drive(d, self.system_root)

    def verify_done(self) -> bool:
        d = self.data_dir()
        return d is not None and not is_on_system_drive(d, self.system_root)

    def steps(self, target_root: Path) -> list[str]:
        return [
            tr("Mở Zalo trên máy tính."),
            tr("Bấm biểu tượng Cài đặt ở góc dưới bên trái, chọn Dữ liệu."),
            tr("Chọn Quản lý dữ liệu, bấm Thay đổi ở phần vị trí lưu."),
            tr("Chọn thư mục {path} rồi bấm Di chuyển.", path=Path(target_root) / "Zalo"),
            tr("Đợi Zalo chuyển xong, quay lại đây và bấm Kiểm tra lại."),
        ]

    def find_exe(self) -> str | None:
        for proc in psutil.process_iter(["name", "exe"]):
            try:
                if (proc.info.get("name") or "").lower() == "zalo.exe" and proc.info.get("exe"):
                    return proc.info["exe"]
            except (psutil.Error, OSError):
                continue
        exe = self.localappdata / "Programs" / "Zalo" / "Zalo.exe"
        return str(exe) if exe.exists() else None

    def open_app(self) -> bool:
        exe = self.find_exe()
        if not exe:
            return False
        subprocess.Popen([exe])
        return True

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        return CheckResult(True)

    def apply(self, cand, target_root, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        raise RelocationError(tr("Zalo cần được chuyển bằng tùy chọn trong ứng dụng Zalo"))

    def undo(self, rec, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        raise RelocationError(tr("Đổi lại vị trí lưu trong Cài đặt của Zalo"))
