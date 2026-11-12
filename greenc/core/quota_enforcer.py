from concurrent.futures import Future, ThreadPoolExecutor
from datetime import datetime, timedelta
import errno
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import shutil
import time
from typing import Callable
import zipfile

from greenc.config import DEFAULT_PROTECTED_PATTERNS
from greenc.core.win_locks import compute_physical_usage, is_file_locked
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
_ERROR_DISK_FULL = 112  # Win32 ERROR_DISK_FULL
_ERROR_HANDLE_DISK_FULL = 39


def _is_disk_full(err: OSError) -> bool:
    return err.errno == errno.ENOSPC or getattr(err, "winerror", None) in (
        _ERROR_DISK_FULL,
        _ERROR_HANDLE_DISK_FULL,
    )


def _make_zipinfo(path: Path, arcname: str) -> zipfile.ZipInfo:
    zinfo = zipfile.ZipInfo.from_file(path, arcname, strict_timestamps=False)
    if path.suffix.lower() in STORE_ONLY_EXTENSIONS:
        zinfo.compress_type = zipfile.ZIP_STORED
    else:
        zinfo.compress_type = zipfile.ZIP_DEFLATED
        # ZipFile.open(ZipInfo) ignores the archive-level compresslevel; set it per entry.
        attr = "compress_level" if hasattr(zinfo, "compress_level") else "_compresslevel"
        setattr(zinfo, attr, 1)  # 3.13+ renamed _compresslevel -> compress_level
    return zinfo


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
        p_str = str(path)
        p_norm = os.path.normcase(os.path.abspath(p_str))
        if p_norm in protected_norm:
            return True
        name_lower = path.name.lower()
        p_lower = p_str.lower()
        all_patterns = list(DEFAULT_PROTECTED_PATTERNS) + list(exclude_patterns)
        for pat in all_patterns:
            pat_l = pat.lower()
            if fnmatch.fnmatch(name_lower, pat_l) or fnmatch.fnmatch(p_lower, pat_l):
                return True
        return False

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
    ) -> VaultArchive | None:
        """
        should_stop: polled before each file. When it returns True the archive is
        finalized with the files done so far (the rest stays untouched), so Stop / app
        exit never waits for a multi-GB rule to finish.
        """
        if not rule.enabled:
            return None
        stop_requested = should_stop or (lambda: False)
        if stop_requested():
            return None

        now = now_ts if now_ts is not None else time.time()
        phys_bytes, _, inode_map = compute_physical_usage(rule.target_paths)
        if phys_bytes <= rule.max_bytes:
            return None

        target_bytes = int(rule.max_bytes * rule.watermark_ratio)
        bytes_to_free = phys_bytes - target_bytes
        if bytes_to_free <= 0:
            return None

        protected_norm = {
            os.path.normcase(os.path.abspath(p)) for p in rule.protected_files
        }

        # Gather candidate inode groups with mtime & size first, then sort oldest-first
        # so we only perform Win32 lock checks lazily until we reach bytes_to_free.
        raw_groups: list[tuple[float, int, list[Path]]] = []
        for _, group_paths in inode_map.items():
            primary = group_paths[0]
            try:
                st = os.stat(primary, follow_symlinks=False)
            except OSError:
                continue
            if st.st_size <= 0:
                continue
            if (now - st.st_mtime) < rule.min_file_age_seconds:
                continue
            raw_groups.append((st.st_mtime, st.st_size, group_paths))

        # Sort oldest first (by mtime ascending)
        raw_groups.sort(key=lambda item: item[0])

        selected: list[tuple[float, int, list[Path]]] = []
        accumulated = 0
        for item in raw_groups:
            _, size_b, group_paths = item
            # If ANY path in the hardlink group is excluded, protected, or locked -> skip group
            skip_group = False
            for gp in group_paths:
                if self._is_excluded(gp, rule.exclude_patterns, protected_norm):
                    skip_group = True
                    break
                if is_file_locked(gp):
                    skip_group = True
                    break
            if skip_group:
                continue

            selected.append(item)
            accumulated += size_b
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
                f"Ổ chứa Kho dự phòng ({self.vault_dir}) không đủ dung lượng trống để nén "
                f"'{selected[0][2][0].name}' ({selected[0][1] / (1024 * 1024):.0f} MB). "
                "Hãy giải phóng ổ đĩa hoặc chuyển Kho dự phòng sang ổ khác."
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
        try:
            with ThreadPoolExecutor(
                max_workers=1, thread_name_prefix="dg-sha256"
            ) as hash_pool, zipfile.ZipFile(zip_path, mode="w", allowZip64=True) as zf:
                for idx, (_, _, group_paths) in enumerate(selected, start=1):
                    if stop_requested():
                        break
                    primary = group_paths[0]
                    arcname = f"item_{idx:05d}_{primary.name}"
                    try:
                        st = os.stat(primary, follow_symlinks=False)
                        zinfo = _make_zipinfo(primary, arcname)
                        hasher = hashlib.sha256()
                        copied = 0
                        pending: Future | None = None
                        # force_zip64: entries > 2 GiB (or files that grow while being
                        # read) would otherwise fail with "File size too large".
                        with open(primary, "rb") as src, zf.open(
                            zinfo, "w", force_zip64=True
                        ) as dst:
                            while chunk := src.read(_COPY_CHUNK):
                                # SHA-256 (OpenSSL) and DEFLATE/CRC (zlib) both release
                                # the GIL: hash chunk N on a worker while this thread
                                # compresses it. One chunk in flight keeps order + memory.
                                if pending is not None:
                                    pending.result()
                                pending = hash_pool.submit(hasher.update, chunk)
                                dst.write(chunk)
                                copied += len(chunk)
                            if pending is not None:
                                pending.result()
                    except OSError as err:
                        if _is_disk_full(err):
                            raise
                        continue  # vanished / access denied: skip this group
                    written_sizes[arcname] = copied
                    if copied != st.st_size:
                        continue  # file changed while being read: zip copy is stale
                    snapshots[str(primary)] = (st.st_size, st.st_mtime_ns)
                    manifest_items.append(
                        FileManifestItem(
                            original_path=str(primary),
                            rel_arcname=arcname,
                            size_bytes=copied,
                            sha256=hasher.hexdigest(),
                            mtime=float(st.st_mtime),
                            atime=float(st.st_atime),
                            hardlink_paths=[str(p) for p in group_paths],
                        )
                    )
                    total_orig_bytes += copied

            if manifest_items:
                # Make the archive durable on disk before any original is deleted.
                with open(zip_path, "rb+") as fh:
                    os.fsync(fh.fileno())
                self._verify_archive(zip_path, written_sizes)
        except OSError as err:
            zip_path.unlink(missing_ok=True)
            if _is_disk_full(err):
                raise InsufficientVaultSpaceError(
                    f"Ổ chứa Kho dự phòng ({self.vault_dir}) bị đầy trong lúc nén. "
                    "Đã hủy gói nén dở dang, file gốc được giữ nguyên."
                ) from err
            raise
        except BaseException:
            zip_path.unlink(missing_ok=True)
            raise

        # Last check right before deletion: drop any group whose primary file was modified
        # (size/mtime) after it was copied, or that some process opened meanwhile. Those
        # originals stay in place and are not recorded as archived.
        manifest_items = [m for m in manifest_items if self._still_safe_to_delete(m, snapshots)]
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
        for m in manifest_items:
            all_paths = m.hardlink_paths or [m.original_path]
            for p_str in all_paths:
                try:
                    os.remove(p_str)
                except OSError:
                    pass

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
            status_reason=f"Đang theo dõi an toàn {rule.retention_days} ngày trước khi xóa vĩnh viễn",
        )
        archive.id = self.db.create_vault_archive(archive, manifest_items)
        return archive

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

