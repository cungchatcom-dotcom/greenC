import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
from typing import Callable

from greenc.core.task_controller import TaskControl

FILE_ATTRIBUTE_REPARSE_POINT = 0x400
_CHUNK = 4 * 1024 * 1024


class RelocationError(RuntimeError):
    """A relocation step failed; the message is shown to the user."""


class RelocationStopped(RelocationError):
    """The user pressed Stop."""


def is_reparse_point(path: str | Path) -> bool:
    try:
        return bool(os.lstat(path).st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT)
    except OSError:
        return False


def same_path(a: str | Path, b: str | Path) -> bool:
    return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


def fast_dir_size(path: str | Path, should_stop: Callable[[], bool] | None = None) -> int:
    """Sum of file sizes using cached DirEntry stats (no per-file open); skips reparse points."""
    total = 0
    stack = [str(path)]
    while stack:
        if should_stop is not None and should_stop():
            break
        current = stack.pop()
        try:
            it = os.scandir(current)
        except OSError:
            continue
        with it:
            for entry in it:
                try:
                    st = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                if st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT:
                    continue
                if stat.S_ISDIR(st.st_mode):
                    stack.append(entry.path)
                else:
                    total += st.st_size
    return total


def _hash_file(path: str | Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(_CHUNK):
            h.update(chunk)
    return h.hexdigest()


def _copy_with_hash(src: str, dst: Path) -> str:
    h = hashlib.sha256()
    with open(src, "rb") as fsrc, open(dst, "wb") as fdst:
        while chunk := fsrc.read(_CHUNK):
            h.update(chunk)
            fdst.write(chunk)
    return h.hexdigest()


def verified_copy_tree(
    src: str | Path,
    dst: str | Path,
    ctrl: TaskControl | None = None,
    total_bytes: int = 0,
    title: str = "",
) -> tuple[int, int]:
    """
    Copy `src` into `dst` (created if missing). Every file is hashed while reading the
    source, then the destination is re-read and compared. Nested junctions/symlinks are
    skipped (never followed). Returns (files, bytes). Raises RelocationError on mismatch,
    RelocationStopped when `ctrl` is stopped.
    """
    src, dst = Path(src), Path(dst)
    dst.mkdir(parents=True, exist_ok=True)
    files = 0
    copied = 0
    stack = [(src, dst)]
    while stack:
        s_dir, d_dir = stack.pop()
        with os.scandir(s_dir) as it:
            entries = list(it)
        for entry in entries:
            if ctrl is not None and not ctrl.wait_if_paused():
                raise RelocationStopped("Đã dừng theo yêu cầu")
            st = entry.stat(follow_symlinks=False)
            if st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT:
                continue
            target = d_dir / entry.name
            if stat.S_ISDIR(st.st_mode):
                target.mkdir(exist_ok=True)
                stack.append((Path(entry.path), target))
                continue
            src_hash = _copy_with_hash(entry.path, target)
            if _hash_file(target) != src_hash:
                raise RelocationError(f"Dữ liệu sao chép không khớp: {entry.path}")
            shutil.copystat(entry.path, target)
            files += 1
            copied += st.st_size
            if ctrl is not None and total_bytes > 0:
                pct = min(99, int(copied * 100 / total_bytes))
                if not ctrl.report(pct, title or "Đang sao chép dữ liệu", entry.name):
                    raise RelocationStopped("Đã dừng theo yêu cầu")
    return files, copied


def _remove_link(path: str) -> None:
    try:
        os.rmdir(path)  # directory junction / symlink: removes only the link
    except OSError:
        os.unlink(path)


def _retry_writable(func: Callable[[str], None], path: str) -> None:
    os.chmod(path, stat.S_IWRITE)
    func(path)


def _remove(path: str, errors: list[str]) -> None:
    try:
        st = os.lstat(path)
    except FileNotFoundError:
        return
    except OSError:
        errors.append(path)
        return
    if st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT:
        try:
            _remove_link(path)
        except OSError:
            errors.append(path)
        return
    if stat.S_ISDIR(st.st_mode):
        try:
            names = os.listdir(path)
        except OSError:
            errors.append(path)
            return
        for name in names:
            _remove(os.path.join(path, name), errors)
        try:
            os.rmdir(path)
        except OSError:
            try:
                _retry_writable(os.rmdir, path)
            except OSError:
                errors.append(path)
        return
    try:
        os.remove(path)
    except OSError:
        try:
            _retry_writable(os.remove, path)
        except OSError:
            errors.append(path)


def remove_tree(path: str | Path) -> list[str]:
    """Delete a tree without ever following junctions/symlinks inside it. Returns leftovers."""
    errors: list[str] = []
    if os.path.lexists(path):
        _remove(str(path), errors)
    return errors


def clear_contents(path: str | Path) -> list[str]:
    """Delete everything inside `path` but keep the folder itself."""
    errors: list[str] = []
    try:
        names = os.listdir(path)
    except OSError:
        return [str(path)]
    for name in names:
        _remove(os.path.join(str(path), name), errors)
    return errors


def create_junction(link: str | Path, target: str | Path) -> None:
    try:
        import _winapi

        _winapi.CreateJunction(str(target), str(link))
    except (ImportError, AttributeError):
        subprocess.run(
            ["cmd", "/c", "mklink", "/J", str(link), str(target)],
            check=True,
            capture_output=True,
        )


def junction_target_matches(link: str | Path, target: str | Path) -> bool:
    if not is_reparse_point(link):
        return False
    try:
        return same_path(os.path.realpath(link), os.path.realpath(target))
    except OSError:
        return False
