from datetime import datetime, timedelta
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import time
import zipfile

from diskguard.config import DEFAULT_PROTECTED_PATTERNS
from diskguard.core.win_locks import compute_physical_usage, is_file_locked
from diskguard.models import FileManifestItem, QuotaRule, VaultArchive
from diskguard.storage.database import DiskGuardDB


class QuotaEnforcer:
    """
    Enforces Soft Rolling Quotas for applications/services:
    - Measures real physical usage (deduplicated by NTFS st_ino so Zalo HardLinks are accurate).
    - When usage exceeds `rule.max_bytes`, selects the oldest unlocked files (or HardLink groups)
      not matching `exclude_patterns` or `protected_files` until usage drops to `watermark_ratio` (85%).
    - Compresses selected files into a verified `.zip` archive in Safe Vault with a JSON manifest
      before removing all hardlink siblings from the target directory.
    """

    def __init__(self, db: DiskGuardDB, vault_dir: Path | str):
        self.db = db
        self.vault_dir = Path(vault_dir)
        self.vault_dir.mkdir(parents=True, exist_ok=True)

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
        self, rule: QuotaRule, now_ts: float | None = None
    ) -> VaultArchive | None:
        if not rule.enabled:
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

        # Build candidate inode groups: every path in the hardlink group must be unlocked & eligible
        candidates: list[tuple[float, int, list[Path]]] = []
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

            candidates.append((st.st_mtime, st.st_size, group_paths))

        # Sort oldest first (by mtime ascending)
        candidates.sort(key=lambda item: item[0])

        selected: list[tuple[float, int, list[Path]]] = []
        accumulated = 0
        for item in candidates:
            selected.append(item)
            accumulated += item[1]
            if accumulated >= bytes_to_free:
                break

        if not selected:
            return None

        ts_str = datetime.now().strftime("%Y%m%d_%H%M%S_%f")
        rule_id = rule.id or 0
        base_name = f"vault_rule{rule_id}_{ts_str}"
        zip_path = self.vault_dir / f"{base_name}.zip"
        manifest_path = self.vault_dir / f"{base_name}.manifest.json"

        manifest_items: list[FileManifestItem] = []
        total_orig_bytes = 0

        with zipfile.ZipFile(
            zip_path, mode="w", compression=zipfile.ZIP_DEFLATED, compresslevel=6
        ) as zf:
            for idx, (_, size_b, group_paths) in enumerate(selected, start=1):
                primary = group_paths[0]
                if is_file_locked(primary) or not primary.exists():
                    continue
                try:
                    st = os.stat(primary, follow_symlinks=False)
                    digest = self._sha256_file(primary)
                    arcname = f"item_{idx:05d}_{primary.name}"
                    zf.write(primary, arcname=arcname)
                    manifest_items.append(
                        FileManifestItem(
                            original_path=str(primary),
                            rel_arcname=arcname,
                            size_bytes=int(size_b),
                            sha256=digest,
                            mtime=float(st.st_mtime),
                            atime=float(st.st_atime),
                            hardlink_paths=[str(p) for p in group_paths],
                        )
                    )
                    total_orig_bytes += int(size_b)
                except OSError:
                    continue

        if not manifest_items:
            if zip_path.exists():
                zip_path.unlink(missing_ok=True)
            return None

        # Verify archive integrity before deleting any original files
        with zipfile.ZipFile(zip_path, mode="r") as zf_check:
            bad_member = zf_check.testzip()
            if bad_member is not None:
                zip_path.unlink(missing_ok=True)
                raise RuntimeError(f"Zip integrity check failed on {bad_member}")

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
            if any(is_file_locked(p) for p in all_paths):
                continue
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

    def enforce_all_rules(self) -> list[VaultArchive]:
        created: list[VaultArchive] = []
        for rule in self.db.list_quota_rules(enabled_only=True):
            arch = self.enforce_rule(rule)
            if arch is not None:
                created.append(arch)
        return created

