from collections import deque
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime, timedelta
import errno
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import time
from typing import Callable
import zipfile
import zlib

from greenc.config import DEFAULT_PROTECTED_PATTERNS
from greenc.core.win_locks import compute_physical_usage, is_file_locked
from greenc.i18n import tr
from greenc.models import FileManifestItem, QuotaRule, VaultArchive
from greenc.storage.database import GreenCDB


class InsufficientVaultSpaceError(RuntimeError):
    """Raised when the Safe Vault drive has no room for the archive."""


# Always keep this much free space on the vault drive so archiving never fills it up.
VAULT_FREE_SPACE_RESERVE = 512 * 1024 * 1024

# Already-compressed formats: DEFLATE burns CPU here for ~0% gain, so they are STORED.
STORE_ONLY_EXTENSIONS = frozenset(
    {
        ".mp4", ".mkv", ".mov", ".avi", ".webm", ".m4v", ".3gp", ".flv", ".wmv",
        ".mp3", ".m4a", ".aac", ".ogg", ".opus", ".flac", ".amr", ".wma",
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".heic", ".heif", ".avif",
        ".zip", ".7z", ".rar", ".gz", ".tgz", ".bz2", ".xz", ".zst", ".lz4",
        ".cab", ".msi", ".msix", ".appx", ".apk", ".jar", ".iso", ".dmg",
        ".docx", ".xlsx", ".pptx", ".odt", ".epub",
    }
)

_COPY_CHUNK = 4 * 1024 * 1024

# Opening a file is the real cost of archiving app caches: Windows Defender scans each file
# on first open (~25 ms per file on NVMe, ~40 files/s single-threaded on a uv/npm cache).
# Those scans run in parallel, so small files are read + hashed by a worker pool ahead of
# the single zip writer. Bounded window keeps memory flat.
_PREFETCH_WORKERS = 16
_PREFETCH_MAX_FILE = 8 * 1024 * 1024  # larger files are streamed by the writer thread
_PREFETCH_WINDOW_FILES = 96
_PREFETCH_WINDOW_BYTES = 160 * 1024 * 1024
_LOCK_CHECK_BATCH = 512


@dataclass
class ArchiveProgress:
    """Live progress of one enforce_rule() run, passed to the `progress` callback."""

    phase: str  # "scan" | "select" | "compress" | "finalize"
    done_files: int = 0
    total_files: int = 0
    done_bytes: int = 0
    total_bytes: int = 0
    current: str = ""


ProgressCallback = Callable[[ArchiveProgress], None]


def _compile_patterns(patterns: list[str]) -> re.Pattern[str] | None:
    """One regex for all fnmatch patterns (case-insensitive, Windows separators)."""
    parts = [fnmatch.translate(os.path.normcase(p).lower()) for p in patterns if p]
    return re.compile("|".join(parts)) if parts else None


# Leading bytes of formats that are already compressed (DEFLATE gains ~0% on them). Caches
# often store such data without an extension (npm content-v2 = .tgz, uv wheels = zip).
_COMPRESSED_MAGIC = (
    b"\x1f\x8b",  # gzip / .tgz
    b"PK\x03\x04",  # zip, jar, whl, docx, apk, nupkg, vsix
    b"\x28\xb5\x2f\xfd",  # zstd
    b"\xfd7zXZ\x00",  # xz
    b"7z\xbc\xaf\x27\x1c",  # 7z
    b"Rar!\x1a\x07",  # rar
    b"BZh",  # bzip2
    b"\x04\x22\x4d\x18",  # lz4 frame
    b"MSCF",  # cab
    b"\x89PNG",
    b"\xff\xd8\xff",  # jpeg
    b"GIF8",
    b"OggS",
    b"fLaC",
    b"wOF2",  # woff2
)
# Sample of a large file test-compressed to decide STORED vs DEFLATED.
_SNIFF_SAMPLE = 256 * 1024
_MIN_DEFLATE_GAIN = 0.95  # keep DEFLATE only if it saves at least 5%


def _looks_compressed(path: Path, head: bytes) -> bool:
    if path.suffix.lower() in STORE_ONLY_EXTENSIONS:
        return True
    if head.startswith(_COMPRESSED_MAGIC):
        return True
    # ISO-BMFF (mp4, mov, heic, avif): "ftyp" box at offset 4; RIFF WEBP.
    return head[4:8] == b"ftyp" or (head[:4] == b"RIFF" and head[8:12] == b"WEBP")


def _deflate_raw(data: bytes) -> bytes:
    comp = zlib.compressobj(1, zlib.DEFLATED, -15)
    return comp.compress(data) + comp.flush()


@dataclass
class _Prefetched:
    st: os.stat_result
    size: int = -1  # bytes read; -1 = too large, the writer streams it
    payload: bytes = b""  # entry data exactly as stored in the zip
    compress_type: int = zipfile.ZIP_STORED
    crc: int = 0
    sha256: str = ""


def _prefetch_small(path: Path, max_size: int) -> _Prefetched:
    """Worker: stat, then read, hash, CRC and compress the whole file if it is small.

    Everything CPU-heavy (SHA-256, CRC-32, DEFLATE) releases the GIL, so the pool keeps
    all cores busy while the single writer thread only appends ready-made entries.
    """
    st = os.stat(path, follow_symlinks=False)
    if st.st_size > max_size:
        return _Prefetched(st)
    with open(path, "rb") as fh:
        data = fh.read(max_size + 1)
    item = _Prefetched(
        st, len(data), data, zipfile.ZIP_STORED, zlib.crc32(data),
        hashlib.sha256(data).hexdigest(),
    )
    if data and not _looks_compressed(path, data[:16]):
        packed = _deflate_raw(data)
        if len(packed) < len(data) * _MIN_DEFLATE_GAIN:
            item.payload, item.compress_type = packed, zipfile.ZIP_DEFLATED
    return item


def _zipinfo_from_stat(st: os.stat_result, arcname: str, compress_type: int) -> zipfile.ZipInfo:
    """Same fields as ZipInfo.from_file, without stat-ing (= opening) the file again."""
    date_time = time.localtime(st.st_mtime)[0:6]
    if date_time[0] < 1980:
        date_time = (1980, 1, 1, 0, 0, 0)
    elif date_time[0] > 2107:
        date_time = (2107, 12, 31, 23, 59, 59)
    zinfo = zipfile.ZipInfo(arcname, date_time)
    zinfo.external_attr = (st.st_mode & 0xFFFF) << 16
    zinfo.file_size = st.st_size
    zinfo.compress_type = compress_type
    if compress_type == zipfile.ZIP_DEFLATED:
        # ZipFile.open(ZipInfo) ignores the archive-level compresslevel; set it per entry.
        attr = "compress_level" if hasattr(zinfo, "compress_level") else "_compresslevel"
        setattr(zinfo, attr, 1)  # 3.13+ renamed _compresslevel -> compress_level
    return zinfo


def _write_prepared_entry(zf: zipfile.ZipFile, zinfo: zipfile.ZipInfo, item: _Prefetched) -> None:
    """Append an entry whose data is already compressed (mirrors ZipFile._open_to_write +
    _ZipWriteFile.close for a seekable file; sizes and CRC are known, so the local header is
    written once, correct, without seeking back). Small files only: never needs ZIP64 sizes;
    ZIP64 header offsets are handled by the central directory as usual."""
    zinfo.file_size = item.size
    zinfo.compress_size = len(item.payload)
    zinfo.CRC = item.crc
    zinfo.flag_bits = 0
    zf.fp.seek(zf.start_dir)
    zinfo.header_offset = zf.fp.tell()
    zf._writecheck(zinfo)
    zf._didModify = True
    zf.fp.write(zinfo.FileHeader(False))
    zf.fp.write(item.payload)
    zf.start_dir = zf.fp.tell()
    zf.filelist.append(zinfo)
    zf.NameToInfo[zinfo.filename] = zinfo


# Parallel DEFLATE of large files (zlib level 1 is ~20-100 MB/s per core).
_DEFLATE_WORKERS = max(2, os.cpu_count() or 2)
_DEFLATE_IN_FLIGHT = _DEFLATE_WORKERS * 2  # chunks queued ahead of the writer
# Final empty DEFLATE block (BFINAL=1, fixed Huffman, end-of-block) closing a stream built
# from sync-flushed chunks.
_DEFLATE_END = zlib.compressobj(1, zlib.DEFLATED, -15).flush()


def _deflate_chunk(data: bytes) -> bytes:
    """Independent raw DEFLATE of one chunk, ending byte-aligned without the final bit."""
    comp = zlib.compressobj(1, zlib.DEFLATED, -15)
    return comp.compress(data) + comp.flush(zlib.Z_SYNC_FLUSH)


def _begin_raw_entry(zf: zipfile.ZipFile, zinfo: zipfile.ZipInfo) -> None:
    """Start an entry whose compressed bytes the caller writes to zf.fp itself
    (ZipFile._open_to_write for a seekable file, always with ZIP64 sizes)."""
    zinfo.compress_size = 0
    zinfo.CRC = 0
    zinfo.flag_bits = 0
    zf.fp.seek(zf.start_dir)
    zinfo.header_offset = zf.fp.tell()
    zf._writecheck(zinfo)
    zf._didModify = True
    zf.fp.write(zinfo.FileHeader(True))


def _end_raw_entry(
    zf: zipfile.ZipFile, zinfo: zipfile.ZipInfo, file_size: int, compress_size: int, crc: int
) -> None:
    """Finish an entry started by _begin_raw_entry (_ZipWriteFile.close equivalent)."""
    zinfo.file_size = file_size
    zinfo.compress_size = compress_size
    zinfo.CRC = crc
    zf.start_dir = zf.fp.tell()
    zf.fp.seek(zinfo.header_offset)
    zf.fp.write(zinfo.FileHeader(True))
    zf.fp.seek(zf.start_dir)
    zf.filelist.append(zinfo)
    zf.NameToInfo[zinfo.filename] = zinfo


_ERROR_DISK_FULL = 112  # Win32 ERROR_DISK_FULL
_ERROR_HANDLE_DISK_FULL = 39


def _is_disk_full(err: OSError) -> bool:
    return err.errno == errno.ENOSPC or getattr(err, "winerror", None) in (
        _ERROR_DISK_FULL,
        _ERROR_HANDLE_DISK_FULL,
    )


class QuotaEnforcer:
    """
    Enforces Soft Rolling Quotas for applications/services:
    - Measures real physical usage (deduplicated by NTFS st_ino so Zalo HardLinks are accurate).
    - When usage exceeds `rule.max_bytes`, selects the oldest unlocked files (or HardLink groups)
      not matching `exclude_patterns` or `protected_files` until usage drops to `watermark_ratio` (85%).
    - Compresses selected files into a verified `.zip` archive in Safe Vault with a JSON manifest
      before removing all hardlink siblings from the target directory.
    """

    def __init__(
        self, db: GreenCDB, vault_dir: Path | str, full_crc_verify: bool = False
    ):
        """
        full_crc_verify: also re-read and decompress the whole archive (zipfile.testzip)
        before deleting originals. Off by default: it doubles the I/O, and every entry is
        already covered by a fsync'd write, a central-directory size check, and a SHA-256
        that rollback verifies.
        """
        self.db = db
        self.vault_dir = Path(vault_dir)
        self.vault_dir.mkdir(parents=True, exist_ok=True)
        self.full_crc_verify = full_crc_verify

    def _vault_free_bytes(self) -> int:
        return shutil.disk_usage(self.vault_dir).free - VAULT_FREE_SPACE_RESERVE

    def evaluate_rule_usage(self, rule: QuotaRule) -> dict:
        phys_bytes, naive_bytes, inode_map = compute_physical_usage(rule.target_paths)
        target_bytes = int(rule.max_bytes * rule.watermark_ratio)
        over_bytes = max(0, phys_bytes - rule.max_bytes)
        pct = (phys_bytes / rule.max_bytes * 100.0) if rule.max_bytes > 0 else 0.0
        return {
            "physical_bytes": phys_bytes,
            "naive_bytes": naive_bytes,
            "max_bytes": rule.max_bytes,
            "target_bytes": target_bytes,
            "over_bytes": over_bytes,
            "usage_pct": round(pct, 1),
            "unique_file_count": len(inode_map),
            "total_path_count": sum(len(v) for v in inode_map.values()),
        }

    @staticmethod
    def _is_excluded(
        path: Path, exclude_patterns: list[str], protected_norm: set[str]
    ) -> bool:
        matcher = _compile_patterns(list(DEFAULT_PROTECTED_PATTERNS) + list(exclude_patterns))
        return QuotaEnforcer._is_excluded_fast(path, matcher, protected_norm)

    @staticmethod
    def _is_excluded_fast(
        path: Path, matcher: re.Pattern[str] | None, protected_norm: set[str]
    ) -> bool:
        p_norm = os.path.normcase(os.path.abspath(str(path)))
        if p_norm in protected_norm:
            return True
        if matcher is None:
            return False
        return bool(matcher.match(path.name.lower()) or matcher.match(p_norm.lower()))

    @staticmethod
    def _sha256_file(path: Path) -> str:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            while chunk := f.read(1024 * 1024):
                h.update(chunk)
        return h.hexdigest()

    def enforce_rule(
        self,
        rule: QuotaRule,
        now_ts: float | None = None,
        should_stop: Callable[[], bool] | None = None,
        progress: ProgressCallback | None = None,
    ) -> VaultArchive | None:
        """
        should_stop: polled before each file. When it returns True the archive is
        finalized with the files done so far (the rest stays untouched), so Stop / app
        exit never waits for a multi-GB rule to finish.
        progress: optional callback receiving ArchiveProgress while scanning, selecting,
        compressing (per file, and per chunk of large files) and finalizing.
        """
        if not rule.enabled:
            return None
        stop_requested = should_stop or (lambda: False)
        if stop_requested():
            return None
        report: ProgressCallback = progress or (lambda _p: None)

        now = now_ts if now_ts is not None else time.time()
        report(ArchiveProgress("scan"))
        stats: dict[tuple[int, int], os.stat_result] = {}
        phys_bytes, _, inode_map = compute_physical_usage(rule.target_paths, stats_out=stats)
        if phys_bytes <= rule.max_bytes:
            return None

        target_bytes = int(rule.max_bytes * rule.watermark_ratio)
        bytes_to_free = phys_bytes - target_bytes
        if bytes_to_free <= 0:
            return None

        protected_norm = {
            os.path.normcase(os.path.abspath(p)) for p in rule.protected_files
        }
        matcher = _compile_patterns(
            list(DEFAULT_PROTECTED_PATTERNS) + list(rule.exclude_patterns)
        )

        # Gather candidate inode groups with mtime & size first, then sort oldest-first
        # so we only perform Win32 lock checks lazily until we reach bytes_to_free.
        # Excluded/protected groups are dropped before stat: they can never be archived.
        raw_groups: list[tuple[float, int, list[Path]]] = []
        for key, group_paths in inode_map.items():
            if any(self._is_excluded_fast(gp, matcher, protected_norm) for gp in group_paths):
                continue
            st = stats.get(key)  # stat of group_paths[0], taken by the scan just now
            if st is None:
                continue
            if st.st_size <= 0:
                continue
            if (now - st.st_mtime) < rule.min_file_age_seconds:
                continue
            raw_groups.append((st.st_mtime, st.st_size, group_paths))

        # Sort oldest first (by mtime ascending)
        raw_groups.sort(key=lambda item: item[0])

        def _group_locked(item: tuple[float, int, list[Path]]) -> bool:
            # If ANY path in the hardlink group is locked -> skip the group
            return any(is_file_locked(gp) for gp in item[2])

        selected: list[tuple[float, int, list[Path]]] = []
        accumulated = 0
        # CreateFileW releases the GIL: check a batch of groups concurrently.
        with ThreadPoolExecutor(
            max_workers=_PREFETCH_WORKERS, thread_name_prefix="dg-lockcheck"
        ) as lock_pool:
            for start in range(0, len(raw_groups), _LOCK_CHECK_BATCH):
                report(
                    ArchiveProgress("select", done_files=start, total_files=len(raw_groups))
                )
                batch = raw_groups[start : start + _LOCK_CHECK_BATCH]
                for item, locked in zip(batch, lock_pool.map(_group_locked, batch)):
                    if locked:
                        continue
                    selected.append(item)
                    accumulated += item[1]
                    if accumulated >= bytes_to_free:
                        break
                if accumulated >= bytes_to_free:
                    break

        if not selected:
            return None

        # Never let the vault fill the disk: keep only the oldest groups that fit in the
        # free space (worst case = stored uncompressed). The rest is handled next cycle,
        # once the originals archived now have been deleted.
        free_budget = self._vault_free_bytes()
        fitting: list[tuple[float, int, list[Path]]] = []
        planned = 0
        for item in selected:
            if planned + item[1] > free_budget:
                break
            fitting.append(item)
            planned += item[1]
        if not fitting:
            raise InsufficientVaultSpaceError(
                tr(
                    "Ổ chứa Kho dự phòng ({path}) không đủ dung lượng trống để nén "
                    "'{name}' ({size_mb:.0f} MB). "
                    "Hãy giải phóng ổ đĩa hoặc chuyển Kho dự phòng sang ổ khác.",
                    path=self.vault_dir,
                    name=selected[0][2][0].name,
                    size_mb=selected[0][1] / (1024 * 1024),
                )
            )
        selected = fitting

        ts_str = datetime.now().strftime("%Y%m%d_%H%M%S_%f")
        rule_id = rule.id or 0
        base_name = f"vault_rule{rule_id}_{ts_str}"
        zip_path = self.vault_dir / f"{base_name}.zip"
        manifest_path = self.vault_dir / f"{base_name}.manifest.json"

        manifest_items: list[FileManifestItem] = []
        total_orig_bytes = 0

        written_sizes: dict[str, int] = {}
        # (size, mtime_ns) captured right before copying each primary path
        snapshots: dict[str, tuple[int, int]] = {}
        prog = ArchiveProgress("compress", total_files=len(selected), total_bytes=planned)
        report(prog)
        try:
            with ThreadPoolExecutor(
                max_workers=1, thread_name_prefix="dg-sha256"
            ) as hash_pool, ThreadPoolExecutor(
                max_workers=_PREFETCH_WORKERS, thread_name_prefix="dg-prefetch"
            ) as read_pool, ThreadPoolExecutor(
                max_workers=_DEFLATE_WORKERS, thread_name_prefix="dg-deflate"
            ) as deflate_pool, zipfile.ZipFile(zip_path, mode="w", allowZip64=True) as zf:
                # Sliding window of prefetched files, consumed strictly in order. Stat is
                # taken by the worker before reading, so the snapshot still predates the copy.
                window: deque[tuple[Future, int]] = deque()
                next_submit = 0
                window_bytes = 0

                def _fill_window() -> None:
                    nonlocal next_submit, window_bytes
                    while next_submit < len(selected) and (
                        not window
                        or (
                            len(window) < _PREFETCH_WINDOW_FILES
                            and window_bytes < _PREFETCH_WINDOW_BYTES
                        )
                    ):
                        _, size_b, paths = selected[next_submit]
                        cost = size_b if size_b <= _PREFETCH_MAX_FILE else 0
                        fut = read_pool.submit(_prefetch_small, paths[0], _PREFETCH_MAX_FILE)
                        window.append((fut, cost))
                        window_bytes += cost
                        next_submit += 1

                try:
                    for idx, (_, planned_size, group_paths) in enumerate(selected, start=1):
                        if stop_requested():
                            break
                        _fill_window()
                        fut, cost = window.popleft()
                        primary = group_paths[0]
                        arcname = f"item_{idx:05d}_{primary.name}"
                        bytes_before = prog.done_bytes
                        prog.current = primary.name
                        try:
                            try:
                                item: _Prefetched = fut.result()
                            finally:
                                window_bytes -= cost
                            st = item.st
                            if item.size >= 0:
                                zinfo = _zipinfo_from_stat(st, arcname, item.compress_type)
                                _write_prepared_entry(zf, zinfo, item)
                                copied, digest = item.size, item.sha256
                                del item  # release the payload before the next entry
                            else:
                                copied, digest = self._stream_into_zip(
                                    primary, st, arcname, zf, hash_pool, deflate_pool,
                                    prog, report,
                                )
                        except OSError as err:
                            if _is_disk_full(err):
                                raise
                            continue  # vanished / access denied: skip this group
                        finally:
                            prog.done_files = idx
                            prog.done_bytes = min(prog.total_bytes, bytes_before + planned_size)
                            report(prog)
                        written_sizes[arcname] = copied
                        if copied != st.st_size:
                            continue  # file changed while being read: zip copy is stale
                        snapshots[str(primary)] = (st.st_size, st.st_mtime_ns)
                        manifest_items.append(
                            FileManifestItem(
                                original_path=str(primary),
                                rel_arcname=arcname,
                                size_bytes=copied,
                                sha256=digest,
                                mtime=float(st.st_mtime),
                                atime=float(st.st_atime),
                                hardlink_paths=[str(p) for p in group_paths],
                            )
                        )
                        total_orig_bytes += copied
                finally:
                    for pending, _ in window:
                        pending.cancel()

            if manifest_items:
                report(
                    ArchiveProgress(
                        "finalize",
                        done_files=prog.done_files,
                        total_files=prog.total_files,
                        done_bytes=prog.done_bytes,
                        total_bytes=prog.total_bytes,
                    )
                )
                # Make the archive durable on disk before any original is deleted.
                with open(zip_path, "rb+") as fh:
                    os.fsync(fh.fileno())
                self._verify_archive(zip_path, written_sizes)
        except OSError as err:
            zip_path.unlink(missing_ok=True)
            if _is_disk_full(err):
                raise InsufficientVaultSpaceError(
                    tr(
                        "Ổ chứa Kho dự phòng ({path}) bị đầy trong lúc nén. "
                        "Đã hủy gói nén dở dang, file gốc được giữ nguyên.",
                        path=self.vault_dir,
                    )
                ) from err
            raise
        except BaseException:
            zip_path.unlink(missing_ok=True)
            raise

        # Last check right before deletion: drop any group whose primary file was modified
        # (size/mtime) after it was copied, or that some process opened meanwhile. Those
        # originals stay in place and are not recorded as archived.
        with ThreadPoolExecutor(
            max_workers=_PREFETCH_WORKERS, thread_name_prefix="dg-recheck"
        ) as check_pool:
            still_safe = list(
                check_pool.map(
                    lambda m: self._still_safe_to_delete(m, snapshots), manifest_items
                )
            )
        manifest_items = [m for m, ok in zip(manifest_items, still_safe) if ok]
        total_orig_bytes = sum(m.size_bytes for m in manifest_items)

        if not manifest_items:
            zip_path.unlink(missing_ok=True)
            return None

        created_dt = datetime.now()
        expires_dt = created_dt + timedelta(days=rule.retention_days)
        compressed_bytes = zip_path.stat().st_size

        manifest_payload = {
            "rule_id": rule_id,
            "app_name": rule.app_name,
            "process_name": rule.process_name,
            "service_name": rule.service_name,
            "created_at": created_dt.isoformat(timespec="seconds"),
            "expires_at": expires_dt.isoformat(timespec="seconds"),
            "retention_days": rule.retention_days,
            "original_bytes": total_orig_bytes,
            "compressed_bytes": compressed_bytes,
            "files": [
                {
                    "original_path": m.original_path,
                    "rel_arcname": m.rel_arcname,
                    "size_bytes": m.size_bytes,
                    "sha256": m.sha256,
                    "mtime": m.mtime,
                    "atime": m.atime,
                    "hardlink_paths": m.hardlink_paths,
                }
                for m in manifest_items
            ],
        }
        manifest_path.write_text(
            json.dumps(manifest_payload, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )

        # Now safely remove all hardlink siblings of the archived items so NTFS frees physical space
        def _remove_group(m: FileManifestItem) -> None:
            for p_str in m.hardlink_paths or [m.original_path]:
                try:
                    os.remove(p_str)
                except OSError:
                    pass

        with ThreadPoolExecutor(
            max_workers=_PREFETCH_WORKERS, thread_name_prefix="dg-remove"
        ) as remove_pool:
            list(remove_pool.map(_remove_group, manifest_items))

        archive = VaultArchive(
            id=None,
            rule_id=rule_id,
            app_name=rule.app_name,
            zip_path=str(zip_path),
            manifest_path=str(manifest_path),
            file_count=len(manifest_items),
            original_bytes=total_orig_bytes,
            compressed_bytes=compressed_bytes,
            created_at=created_dt.isoformat(timespec="seconds"),
            expires_at=expires_dt.isoformat(timespec="seconds"),
            status="QUARANTINED",
            status_reason=tr(
                "Đang theo dõi an toàn {days} ngày trước khi xóa vĩnh viễn",
                days=rule.retention_days,
            ),
        )
        archive.id = self.db.create_vault_archive(archive, manifest_items)
        return archive

    @staticmethod
    def _stream_into_zip(
        path: Path,
        st: os.stat_result,
        arcname: str,
        zf: zipfile.ZipFile,
        hash_pool: ThreadPoolExecutor,
        deflate_pool: ThreadPoolExecutor,
        prog: ArchiveProgress,
        report: ProgressCallback,
    ) -> tuple[int, str]:
        """Copy a large file chunk by chunk; returns (bytes copied, sha256 hex).

        Compressible files are deflated in parallel, pigz-style: every chunk is compressed
        independently on `deflate_pool` and ends with a sync flush (byte-aligned, no final
        bit), so the chunks concatenate into one valid raw DEFLATE stream. This thread only
        reads, CRCs (~1 GB/s) and appends the results in order. SHA-256 runs on `hash_pool`.
        """
        hasher = hashlib.sha256()
        crc = 0
        copied = 0
        start_bytes = prog.done_bytes
        pending_hash: Future | None = None

        def _hash(data: bytes) -> None:
            nonlocal pending_hash
            if pending_hash is not None:
                pending_hash.result()
            pending_hash = hash_pool.submit(hasher.update, data)

        def _advance(n: int) -> None:
            nonlocal copied
            copied += n
            prog.done_bytes = min(prog.total_bytes, start_bytes + copied)
            report(prog)

        with open(path, "rb") as src:
            chunk = src.read(_COPY_CHUNK)
            sample = chunk[:_SNIFF_SAMPLE]
            store = _looks_compressed(path, sample[:16]) or (
                len(_deflate_raw(sample)) >= len(sample) * _MIN_DEFLATE_GAIN
            )
            zinfo = _zipinfo_from_stat(
                st, arcname, zipfile.ZIP_STORED if store else zipfile.ZIP_DEFLATED
            )
            if store:
                # force_zip64: entries > 2 GiB (or files that grow while being read)
                # would otherwise fail with "File size too large".
                with zf.open(zinfo, "w", force_zip64=True) as dst:
                    while chunk:
                        _hash(chunk)
                        dst.write(chunk)
                        _advance(len(chunk))
                        chunk = src.read(_COPY_CHUNK)
            else:
                _begin_raw_entry(zf, zinfo)
                compressed = 0
                in_flight: deque[tuple[Future, int]] = deque()

                def _drain(keep: int) -> None:
                    nonlocal compressed
                    while len(in_flight) > keep:
                        fut, n = in_flight.popleft()
                        out = fut.result()
                        zf.fp.write(out)
                        compressed += len(out)
                        _advance(n)

                while chunk:
                    _hash(chunk)
                    crc = zlib.crc32(chunk, crc)
                    in_flight.append((deflate_pool.submit(_deflate_chunk, chunk), len(chunk)))
                    _drain(_DEFLATE_IN_FLIGHT)
                    chunk = src.read(_COPY_CHUNK)
                _drain(0)
                zf.fp.write(_DEFLATE_END)
                compressed += len(_DEFLATE_END)
                _end_raw_entry(zf, zinfo, copied, compressed, crc)
            if pending_hash is not None:
                pending_hash.result()
        return copied, hasher.hexdigest()

    @staticmethod
    def _still_safe_to_delete(
        item: FileManifestItem, snapshots: dict[str, tuple[int, int]]
    ) -> bool:
        snap = snapshots.get(item.original_path)
        if snap is None:
            return False
        try:
            st = os.stat(item.original_path, follow_symlinks=False)
        except OSError:
            return False
        if (st.st_size, st.st_mtime_ns) != snap:
            return False
        return not any(is_file_locked(p) for p in (item.hardlink_paths or [item.original_path]))

    def _verify_archive(self, zip_path: Path, written_sizes: dict[str, int]) -> None:
        """Re-read the central directory from disk and check every entry before deletion."""
        with zipfile.ZipFile(zip_path, mode="r") as zf_check:
            on_disk = {i.filename: i.file_size for i in zf_check.infolist()}
            for arcname, size in written_sizes.items():
                if on_disk.get(arcname) != size:
                    raise RuntimeError(
                        f"Zip integrity check failed on {arcname}: "
                        f"expected {size} bytes, found {on_disk.get(arcname)}"
                    )
            if self.full_crc_verify:
                bad_member = zf_check.testzip()
                if bad_member is not None:
                    raise RuntimeError(f"Zip integrity check failed on {bad_member}")

    def enforce_all_rules(
        self, should_stop: Callable[[], bool] | None = None
    ) -> list[VaultArchive]:
        """Background cycle: a failure on one rule (e.g. vault drive full) must not stop the others."""
        created: list[VaultArchive] = []
        self.last_errors: list[str] = []
        for rule in self.db.list_quota_rules(enabled_only=True):
            if should_stop is not None and should_stop():
                break
            try:
                arch = self.enforce_rule(rule, should_stop=should_stop)
            except Exception as err:  # noqa: BLE001 - reported, cycle continues
                self.last_errors.append(f"{rule.app_name}: {err}")
                continue
            if arch is not None:
                created.append(arch)
        return created

