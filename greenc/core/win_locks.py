import ctypes
from ctypes import wintypes
import os
from pathlib import Path

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
DELETE_ACCESS = 0x00010000
OPEN_EXISTING = 3
FILE_ATTRIBUTE_NORMAL = 0x80
INVALID_HANDLE_VALUE = wintypes.HANDLE(-1).value
ERROR_SHARING_VIOLATION = 32
ERROR_LOCK_VIOLATION = 33
ERROR_ACCESS_DENIED = 5
_FILE_ATTRIBUTE_REPARSE_POINT = 0x400

kernel32.CreateFileW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.LPVOID,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.HANDLE,
]
kernel32.CreateFileW.restype = wintypes.HANDLE
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
kernel32.CloseHandle.restype = wintypes.BOOL


def is_file_locked(path: str | Path) -> bool:
    """
    Check if a file is currently open/locked by any process on Windows
    by attempting to open it with dwShareMode = 0 (exclusive access).
    """
    p_str = str(path)
    if not os.path.exists(p_str):
        return False
    # Request DELETE_ACCESS (0x00010000) with dwShareMode = 0 (exclusive):
    # - NTFS IoCheckShareAccess rejects this with ERROR_SHARING_VIOLATION (32) if ANY process
    #   holds an open handle with Read, Write, or Delete access.
    # - Unlike GENERIC_WRITE, closing this handle does not trigger Windows Defender (WdFilter.sys)
    #   synchronous post-write antivirus scans (~75x faster on Windows 11).
    handle = kernel32.CreateFileW(
        p_str,
        DELETE_ACCESS,
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
    stats_out: dict | None = None,
) -> tuple[int, int, dict[tuple[int, int], list[Path]]]:
    """
    Walk the given directories/files and compute:
      1. physical_bytes: deduplicated by NTFS (st_dev, st_ino) so HardLinks are counted once
      2. naive_bytes: simple sum of st_size across all directory entries
      3. inode_map: mapping from (st_dev, st_ino) -> list of Path objects sharing that inode
    stats_out: if given, filled with {inode key: stat_result of the group's first path}
    so callers need not stat every file a second time.
    """
    naive_bytes = 0
    inode_map: dict[tuple[int, int], list[Path]] = {}
    inode_sizes: dict[tuple[int, int], int] = {}

    seen_paths: set[str] = set()

    def _add(path_str: str) -> None:
        nonlocal naive_bytes
        # Paths are built from an absolute root, so normcase alone dedupes overlapping roots.
        norm = os.path.normcase(path_str)
        if norm in seen_paths:
            return
        seen_paths.add(norm)
        try:
            # Full stat (not DirEntry.stat): on Windows only this fills st_ino/st_dev,
            # which HardLink deduplication needs.
            st = os.stat(path_str, follow_symlinks=False)
        except OSError:
            return
        naive_bytes += st.st_size
        fp = Path(path_str)
        key = (st.st_dev, st.st_ino) if st.st_ino != 0 else (id(fp), len(inode_map))
        group = inode_map.setdefault(key, [])
        if not group and stats_out is not None:
            stats_out[key] = st
        group.append(fp)
        inode_sizes[key] = st.st_size

    for root_item in paths:
        root_str = os.path.abspath(str(root_item))
        if os.path.isfile(root_str):
            _add(root_str)
            continue
        if not os.path.isdir(root_str):
            continue
        # Iterative os.scandir walk: avoids os.walk's per-directory lists and a Path +
        # abspath per file (~2x faster warm, much more on a cold cache).
        stack = [root_str]
        while stack:
            current = stack.pop()
            try:
                it = os.scandir(current)
            except OSError:
                continue
            with it:
                subdirs: list[str] = []
                for entry in it:
                    try:
                        # Never follow nested reparse points (junctions, symlinks, OneDrive
                        # placeholders): a junction inside an app folder can point at the
                        # user's Documents, which would then be archived and deleted.
                        # The root itself is followed — it was chosen explicitly.
                        # DirEntry.stat(follow_symlinks=False) is cached on Windows: no syscall.
                        attrs = entry.stat(follow_symlinks=False).st_file_attributes
                        if attrs & _FILE_ATTRIBUTE_REPARSE_POINT:
                            continue
                        if entry.is_dir(follow_symlinks=False):
                            subdirs.append(entry.path)
                            continue
                        if not entry.is_file(follow_symlinks=False):
                            continue
                    except OSError:
                        continue
                    _add(entry.path)
            # Reverse so directories are visited in the same top-down order as os.walk.
            stack.extend(reversed(subdirs))

    physical_bytes = sum(inode_sizes.values())
    return physical_bytes, naive_bytes, inode_map

