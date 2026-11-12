import os
from pathlib import Path
import re
import shutil
from typing import Callable

from diskguard.core.drives import is_on_system_drive, system_drive_root
from diskguard.core.relocation.base import RelocationCandidate
from diskguard.core.relocation.fs_ops import fast_dir_size, is_reparse_point
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.known_folders import (
    KNOWN_FOLDERS,
    KnownFolderStrategy,
    WindowsShellFolders,
    is_onedrive_path,
)
from diskguard.core.relocation.tool_config import (
    NpmCacheStrategy,
    PipCacheStrategy,
    subprocess_runner,
)
from diskguard.core.relocation.zalo import ZaloGuidedStrategy
from diskguard.models import RELOCATION_DONE
from diskguard.storage.database import DiskGuardDB

MIN_CANDIDATE_BYTES = 200 * 1024**2

# Top-level %APPDATA% / %LOCALAPPDATA% folders never offered for junction relocation.
ADVANCED_SKIP = {
    "microsoft", "packages", "programs", "temp", "windows", "comms",
    "connecteddevicesplatform", "d3dscache", "crashdumps", "green c",
    "zalodata", "zalopc", "zalo", "telegram desktop",
}


def _slug(name: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-") or "app"


def _norm(p: str | Path) -> str:
    return os.path.normcase(os.path.abspath(str(p)))


def _overlaps(path: Path, taken: set[str]) -> bool:
    """True if `path` equals, contains, or lies inside an already-offered candidate."""
    p = _norm(path)
    return any(p == t or t.startswith(p + os.sep) or p.startswith(t + os.sep) for t in taken)


def discover_candidates(
    db: DiskGuardDB,
    system_root: str | None = None,
    appdata: Path | str | None = None,
    localappdata: Path | str | None = None,
    shell=None,
    runner=None,
    which: Callable | None = None,
    zalo_config: Path | str | None = None,
    min_bytes: int = MIN_CANDIDATE_BYTES,
    should_stop: Callable[[], bool] | None = None,
    size_of: Callable = fast_dir_size,
) -> list[RelocationCandidate]:
    sys_root = system_root or system_drive_root()
    appdata = Path(appdata) if appdata else Path(os.path.expandvars(r"%APPDATA%"))
    localappdata = Path(localappdata) if localappdata else Path(os.path.expandvars(r"%LOCALAPPDATA%"))
    shell = shell or WindowsShellFolders()
    done = {_norm(r.source_path) for r in db.list_relocations(status=RELOCATION_DONE)}
    stop = should_stop or (lambda: False)
    cands: list[RelocationCandidate] = []

    def eligible(path: Path) -> bool:
        return (
            path.is_dir()
            and not is_reparse_point(path)
            and is_on_system_drive(path, sys_root)
            and _norm(path) not in done
        )

    # 1. Windows known folders
    for kf in KNOWN_FOLDERS.values():
        if stop():
            return cands
        try:
            current = Path(shell.get(kf.guid))
        except OSError:
            continue
        if not eligible(current) or is_onedrive_path(current):
            continue
        size = size_of(current, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                kf.app_id, kf.name, str(current), size,
                KnownFolderStrategy(kf, shell=shell, system_root=sys_root),
                "Dùng tính năng Vị trí của Windows",
            ))

    # 2. Developer tool caches
    for cls in (NpmCacheStrategy, PipCacheStrategy):
        if stop():
            return cands
        st = cls(runner=runner or subprocess_runner, which=which or shutil.which)
        if st.exe() is None:
            continue
        cache = st.current_cache()
        if cache is None or not eligible(cache):
            continue
        size = size_of(cache, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                st.app_id, st.name, str(cache), size, st,
                f"Đổi cấu hình {st.exe_name}, xóa bộ đệm cũ",
            ))

    # 3. Zalo (guided)
    zalo = ZaloGuidedStrategy(config_path=zalo_config, system_root=sys_root, localappdata=localappdata)
    zalo_dir = zalo.data_dir()
    if zalo_dir is not None and eligible(zalo_dir):
        size = size_of(zalo_dir, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                "zalo", "Zalo", str(zalo_dir), size, zalo,
                "Dùng tùy chọn lưu trữ của Zalo · Hướng dẫn từng bước, tự kiểm tra sau khi đổi",
            ))

    # 4. Curated junction apps
    telegram = appdata / "Telegram Desktop"
    if eligible(telegram):
        size = size_of(telegram, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                "telegram", "Telegram Desktop", str(telegram), size,
                JunctionStrategy(["Telegram.exe"]),
                "Chuyển thư mục và tạo liên kết · Cần đóng Telegram",
            ))

    # 5. Advanced: other large app folders (junction, unchecked by default)
    taken = {_norm(c.source_path) for c in cands}
    for base in (appdata, localappdata):
        if not base.is_dir():
            continue
        for child in sorted(base.iterdir(), key=lambda p: p.name.lower()):
            if stop():
                return cands
            if child.name.lower() in ADVANCED_SKIP or _overlaps(child, taken) or not eligible(child):
                continue
            size = size_of(child, should_stop)
            if size >= min_bytes:
                cands.append(RelocationCandidate(
                    f"dir-{_slug(child.name)}", child.name, str(child), size,
                    JunctionStrategy([]),
                    "Chuyển thư mục và tạo liên kết · Cần đóng ứng dụng đang dùng thư mục này",
                    advanced=True,
                ))

    cands.sort(key=lambda c: (c.advanced, -c.size_bytes))
    return cands
