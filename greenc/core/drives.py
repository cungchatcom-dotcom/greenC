import ctypes
from ctypes import wintypes
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
import string
from typing import Callable

from greenc.config import DATA_FOLDER_NAME

DRIVE_FIXED = 3
# A drive is only *recommended* when it has at least this much free space.
MIN_RECOMMEND_FREE = 10 * 1024**3
_SEM_FAILCRITICALERRORS = 0x0001

_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
_kernel32.GetLogicalDrives.restype = wintypes.DWORD
_kernel32.GetDriveTypeW.argtypes = [wintypes.LPCWSTR]
_kernel32.GetDriveTypeW.restype = wintypes.UINT
_kernel32.GetVolumeInformationW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.LPWSTR,
    wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(wintypes.DWORD),
    wintypes.LPWSTR,
    wintypes.DWORD,
]
_kernel32.GetVolumeInformationW.restype = wintypes.BOOL
_kernel32.SetThreadErrorMode.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
_kernel32.SetThreadErrorMode.restype = wintypes.BOOL


@dataclass(frozen=True)
class DriveInfo:
    root: str  # "D:\\"
    label: str
    fs: str
    total: int
    free: int
    is_system: bool
    eligible: bool
    reason: str = ""

    @property
    def used(self) -> int:
        return max(0, self.total - self.free)

    @property
    def letter(self) -> str:
        return self.root[:2]


def system_drive_root() -> str:
    drive = (os.environ.get("SystemDrive") or "C:").rstrip("\\/")
    return drive.upper() + "\\"


def drive_of(path: str | Path) -> str:
    drive = os.path.splitdrive(os.path.abspath(str(path)))[0]
    return (drive.upper() + "\\") if drive else ""


def is_on_system_drive(path: str | Path, system_root: str | None = None) -> bool:
    return drive_of(path) == (system_root or system_drive_root()).upper()


def _logical_roots() -> list[str]:
    mask = _kernel32.GetLogicalDrives()
    return [f"{c}:\\" for i, c in enumerate(string.ascii_uppercase) if mask & (1 << i)]


def _probe_volume(root: str) -> tuple[int, str, str]:
    old_mode = wintypes.DWORD(0)
    # Avoid "There is no disk in the drive" popups for empty card readers / DVD drives.
    _kernel32.SetThreadErrorMode(_SEM_FAILCRITICALERRORS, ctypes.byref(old_mode))
    try:
        drive_type = int(_kernel32.GetDriveTypeW(root))
        label = ctypes.create_unicode_buffer(261)
        fs = ctypes.create_unicode_buffer(261)
        ok = _kernel32.GetVolumeInformationW(root, label, 261, None, None, None, fs, 261)
        if not ok:
            raise OSError(ctypes.get_last_error(), f"GetVolumeInformationW failed for {root}")
        return drive_type, label.value, fs.value
    finally:
        _kernel32.SetThreadErrorMode(old_mode.value, None)


def list_drives(
    roots: list[str] | None = None,
    probe: Callable[[str], tuple[int, str, str]] | None = None,
    usage: Callable = shutil.disk_usage,
    system_root: str | None = None,
) -> list[DriveInfo]:
    sys_root = (system_root or system_drive_root()).upper()
    probe_fn = probe or _probe_volume
    drives: list[DriveInfo] = []
    for root in roots if roots is not None else _logical_roots():
        try:
            drive_type, label, fs = probe_fn(root)
            u = usage(root)
        except OSError:
            continue
        reason = ""
        if drive_type != DRIVE_FIXED:
            reason = "Ổ di động hoặc ổ mạng"
        elif (fs or "").upper() != "NTFS":
            reason = f"Định dạng {fs or 'không rõ'} không được hỗ trợ"
        elif "recovery" in (label or "").lower():
            reason = "Phân vùng khôi phục của hệ thống"
        drives.append(
            DriveInfo(
                root=root.upper(),
                label=label,
                fs=fs,
                total=int(u.total),
                free=int(u.free),
                is_system=root.upper() == sys_root,
                eligible=not reason,
                reason=reason,
            )
        )
    return drives


def recommend_drive(drives: list[DriveInfo]) -> DriveInfo | None:
    eligible = [d for d in drives if d.eligible]
    roomy = [d for d in eligible if not d.is_system and d.free >= MIN_RECOMMEND_FREE]
    pool = roomy or eligible
    return max(pool, key=lambda d: d.free) if pool else None


def data_root_for(drive_root: str) -> Path:
    return Path(drive_root) / DATA_FOLDER_NAME
