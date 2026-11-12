import ctypes
from ctypes import wintypes
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
from typing import Callable
import uuid

from diskguard.core.drives import is_on_system_drive
from diskguard.core.relocation.base import (
    FREE_SPACE_RESERVE,
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
    now_iso,
)
from diskguard.core.relocation.fs_ops import (
    RelocationError,
    clear_contents,
    remove_tree,
    verified_copy_tree,
)
from diskguard.core.task_controller import TaskControl
from diskguard.models import RELOCATION_DONE, RELOCATION_UNDONE, RelocationRecord


@dataclass(frozen=True)
class KnownFolder:
    app_id: str
    name: str
    guid: str


KNOWN_FOLDERS: dict[str, KnownFolder] = {
    kf.app_id: kf
    for kf in (
        KnownFolder("kf_downloads", "Downloads", "{374DE290-123F-4565-9164-39C4925E467B}"),
        KnownFolder("kf_documents", "Documents", "{FDD39AD0-238F-46AF-ADB4-6C85480369C7}"),
        KnownFolder("kf_pictures", "Pictures", "{33E28130-4E1E-4676-835A-98395C3BC3BB}"),
        KnownFolder("kf_videos", "Videos", "{18989B1D-99B5-455B-841C-AB7C74E4DDFC}"),
        KnownFolder("kf_music", "Music", "{4BD8D571-6D19-48D3-BE97-422220080E43}"),
        KnownFolder("kf_desktop", "Desktop", "{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}"),
    )
}


class _GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", wintypes.DWORD),
        ("Data2", wintypes.WORD),
        ("Data3", wintypes.WORD),
        ("Data4", ctypes.c_ubyte * 8),
    ]


def _guid(text: str) -> _GUID:
    return _GUID.from_buffer_copy(uuid.UUID(text).bytes_le)


class WindowsShellFolders:
    """SHGetKnownFolderPath / SHSetKnownFolderPath — the API behind Explorer's Location tab."""

    def __init__(self):
        self._shell32 = ctypes.WinDLL("shell32")
        self._ole32 = ctypes.WinDLL("ole32")
        self._shell32.SHGetKnownFolderPath.argtypes = [
            ctypes.POINTER(_GUID), wintypes.DWORD, wintypes.HANDLE, ctypes.POINTER(ctypes.c_void_p)
        ]
        self._shell32.SHGetKnownFolderPath.restype = ctypes.c_long
        self._shell32.SHSetKnownFolderPath.argtypes = [
            ctypes.POINTER(_GUID), wintypes.DWORD, wintypes.HANDLE, wintypes.LPCWSTR
        ]
        self._shell32.SHSetKnownFolderPath.restype = ctypes.c_long
        self._ole32.CoTaskMemFree.argtypes = [ctypes.c_void_p]

    def get(self, guid: str) -> str:
        out = ctypes.c_void_p()
        hr = self._shell32.SHGetKnownFolderPath(ctypes.byref(_guid(guid)), 0, None, ctypes.byref(out))
        if hr != 0:
            raise OSError(f"SHGetKnownFolderPath failed: 0x{hr & 0xFFFFFFFF:08X}")
        try:
            return ctypes.wstring_at(out.value)
        finally:
            self._ole32.CoTaskMemFree(out)

    def set(self, guid: str, path: str) -> None:
        hr = self._shell32.SHSetKnownFolderPath(ctypes.byref(_guid(guid)), 0, None, path)
        if hr != 0:
            raise OSError(f"SHSetKnownFolderPath failed: 0x{hr & 0xFFFFFFFF:08X}")


def is_onedrive_path(path: str | Path) -> bool:
    norm = os.path.normcase(os.path.abspath(str(path)))
    for key in ("OneDrive", "OneDriveConsumer", "OneDriveCommercial"):
        root = os.environ.get(key)
        if not root:
            continue
        root_norm = os.path.normcase(os.path.abspath(root))
        if norm == root_norm or norm.startswith(root_norm + os.sep):
            return True
    return False


class KnownFolderStrategy(RelocationStrategy):
    kind = "known_folder"

    def __init__(
        self,
        folder: KnownFolder,
        shell=None,
        disk_usage: Callable = shutil.disk_usage,
        system_root: str | None = None,
    ):
        self.folder = folder
        self.shell = shell or WindowsShellFolders()
        self._disk_usage = disk_usage
        self._system_root = system_root

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        try:
            current = Path(self.shell.get(self.folder.guid))
        except OSError as err:
            return CheckResult(False, f"Không đọc được vị trí {self.folder.name}: {err}")
        if is_onedrive_path(current):
            return CheckResult(False, "Thư mục đang đồng bộ OneDrive")
        if not is_on_system_drive(current, self._system_root):
            return CheckResult(False, "Thư mục đã nằm ngoài ổ hệ thống")
        target = Path(target_root) / self.folder.name
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
        current = Path(self.shell.get(self.folder.guid))
        target = Path(target_root) / self.folder.name
        try:
            verified_copy_tree(current, target, ctrl, cand.size_bytes, f"Đang chuyển {self.folder.name}")
        except BaseException:
            remove_tree(target)
            raise
        try:
            self.shell.set(self.folder.guid, str(target))
        except OSError as err:
            remove_tree(target)
            raise RelocationError(f"Windows không cho đổi vị trí {self.folder.name}: {err}") from err
        leftovers = clear_contents(current)
        rec = RelocationRecord(
            id=None, app_id=self.folder.app_id, app_name=self.folder.name, strategy=self.kind,
            source_path=str(current), target_path=str(target), size_bytes=cand.size_bytes,
            status=RELOCATION_DONE, created_at=now_iso(),
            detail=(f"Còn {len(leftovers)} mục đang mở chưa xóa ở vị trí cũ" if leftovers else ""),
        )
        return save(rec)

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        source, target = Path(rec.source_path), Path(rec.target_path)
        verified_copy_tree(target, source, ctrl, rec.size_bytes, f"Đang trả {rec.app_name} về ổ C:")
        try:
            self.shell.set(self.folder.guid, str(source))
        except OSError as err:
            raise RelocationError(f"Windows không cho đổi vị trí {rec.app_name}: {err}") from err
        leftovers = remove_tree(target)
        rec.status = RELOCATION_UNDONE
        rec.detail = f"Còn {len(leftovers)} mục chưa xóa được ở {target}" if leftovers else ""
        return save(rec)
