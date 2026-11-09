import ctypes
from ctypes import wintypes
import os
from pathlib import Path

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
OPEN_EXISTING = 3
FILE_ATTRIBUTE_NORMAL = 0x80
INVALID_HANDLE_VALUE = wintypes.HANDLE(-1).value
ERROR_SHARING_VIOLATION = 32
ERROR_LOCK_VIOLATION = 33
ERROR_ACCESS_DENIED = 5


def is_file_locked(path: str | Path) -> bool:
    """
    Check if a file is currently open/locked by any process on Windows
    by attempting to open it with dwShareMode = 0 (exclusive access).
    """
    p_str = str(path)
    if not os.path.exists(p_str):
        return False
    handle = kernel32.CreateFileW(
        p_str,
        GENERIC_READ | GENERIC_WRITE,
        0,  # Exclusive access: fails if any process has an open handle
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        None,
    )
    if handle == INVALID_HANDLE_VALUE or handle == -1:
        err = ctypes.get_last_error()
        if err in (ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION, ERROR_ACCESS_DENIED):
            return True
        return False
    kernel32.CloseHandle(handle)
    return False


def compute_physical_usage(
    paths: list[str | Path],
) -> tuple[int, int, dict[tuple[int, int], list[Path]]]:
    """
    Walk the given directories/files and compute:
      1. physical_bytes: deduplicated by NTFS (st_dev, st_ino) so HardLinks are counted once
      2. naive_bytes: simple sum of st_size across all directory entries
      3. inode_map: mapping from (st_dev, st_ino) -> list of Path objects sharing that inode
    """
    naive_bytes = 0
    inode_map: dict[tuple[int, int], list[Path]] = {}
    inode_sizes: dict[tuple[int, int], int] = {}

    seen_paths: set[str] = set()
    for root_item in paths:
        root_path = Path(root_item)
        if not root_path.exists():
            continue
        if root_path.is_file():
            candidates = [root_path]
        else:
            candidates = []
            for dp, _, fns in os.walk(root_path):
                for fn in fns:
                    candidates.append(Path(dp) / fn)

        for fp in candidates:
            norm = os.path.normcase(os.path.abspath(str(fp)))
            if norm in seen_paths:
                continue
            seen_paths.add(norm)
            try:
                st = os.stat(fp, follow_symlinks=False)
            except OSError:
                continue
            naive_bytes += st.st_size
            key = (st.st_dev, st.st_ino) if st.st_ino != 0 else (id(fp), len(inode_map))
            inode_map.setdefault(key, []).append(fp)
            inode_sizes[key] = st.st_size

    physical_bytes = sum(inode_sizes.values())
    return physical_bytes, naive_bytes, inode_map

