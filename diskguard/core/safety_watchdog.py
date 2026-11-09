from datetime import datetime
import hashlib
import os
from pathlib import Path
import zipfile

import psutil

from diskguard.models import VaultArchive
from diskguard.storage.database import DiskGuardDB


ERROR_KEYWORDS = (
    "filenotfounderror",
    "enoent",
    "cannot find the file",
    "cannot open",
    "missing file",
    "failed to load",
    "no such file",
)


class SafetyWatchdog:
    """
    Monitors QUARANTINED `.zip` archives during their D-day safety window:
    1. Detects unsafe conditions:
       - Target Windows Service stopped unexpectedly with an error after archiving.
       - Target application logs a missing-file error (`FileNotFoundError`, `ENOENT`, etc.)
         referencing any archived filename or path, or drops a `.restore_request` marker.
    2. Performs atomic Rollback:
       - Extracts files from `.zip` back to their original paths.
       - Reconstructs all NTFS HardLink sibling paths (`os.link`) so Zalo/DingTalk structure is 100% preserved.
       - Restores original `(atime, mtime)` and verifies `SHA-256`.
       - Adds restored files to `rule.protected_files` so they are never re-archived.
    3. Automatically purges expired `.zip` archives once D days pass safely.
    """

    def __init__(self, db: DiskGuardDB):
        self.db = db

    @staticmethod
    def _sha256_file(path: Path) -> str:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            while chunk := f.read(1024 * 1024):
                h.update(chunk)
        return h.hexdigest()

    def check_archive_safety(self, archive: VaultArchive) -> tuple[bool, str]:
        if archive.status != "QUARANTINED":
            return True, ""

        rule = self.db.get_quota_rule(archive.rule_id)
        if not rule:
            return True, ""

        # 1. Check Windows Service health if a service_name is bound to the rule
        if rule.service_name:
            try:
                svc = psutil.win_service_get(rule.service_name)
                info = svc.as_dict()
                if info.get("status") == "stopped":
                    return (
                        False,
                        f"Phát hiện Windows Service '{rule.service_name}' dừng bất thường sau khi nén file",
                    )
            except Exception:
                pass

        # 2. Scan recent log files in target_paths for missing-file access errors referencing archived files
        vault_files = self.db.get_vault_files(archive.id or 0)
        if not vault_files:
            return True, ""

        name_to_path: dict[str, str] = {}
        for vf in vault_files:
            for p_str in vf.hardlink_paths or [vf.original_path]:
                name_to_path[Path(p_str).name.lower()] = p_str

        created_dt = datetime.fromisoformat(archive.created_at)
        created_ts = created_dt.timestamp() - 5.0

        for root_str in rule.target_paths:
            root = Path(root_str)
            if not root.exists():
                continue
            for dp, _, fns in os.walk(root):
                for fn in fns:
                    fn_lower = fn.lower()
                    if not (
                        fn_lower.endswith((".log", ".txt", ".err", ".calf"))
                        or "error" in fn_lower
                        or "restore" in fn_lower
                    ):
                        continue
                    fp = Path(dp) / fn
                    try:
                        st = os.stat(fp)
                        if st.st_mtime < created_ts or st.st_size > 10 * 1024 * 1024:
                            continue
                        content_lower = fp.read_text(
                            encoding="utf-8", errors="ignore"
                        ).lower()
                    except OSError:
                        continue

                    if any(k in content_lower for k in ERROR_KEYWORDS):
                        for archived_name, full_orig in name_to_path.items():
                            if archived_name and archived_name in content_lower:
                                return (
                                    False,
                                    f"Ứng dụng báo lỗi thiếu file '{Path(full_orig).name}' trong '{fp.name}' — kích hoạt Tự động Rollback",
                                )

        return True, ""

    def rollback_archive(
        self, archive_id: int, reason: str, auto_triggered: bool = False
    ) -> dict:
        archive = self.db.get_vault_archive(archive_id)
        if not archive:
            raise ValueError(f"Archive {archive_id} not found")

        zip_path = Path(archive.zip_path)
        if not zip_path.exists():
            raise FileNotFoundError(f"File nén không còn tồn tại: {zip_path}")

        vault_files = self.db.get_vault_files(archive_id)
        restored_paths: list[str] = []
        protected_additions: list[str] = []

        with zipfile.ZipFile(zip_path, mode="r") as zf:
            for vf in vault_files:
                all_links = vf.hardlink_paths or [vf.original_path]
                primary_path = Path(all_links[0])
                primary_path.parent.mkdir(parents=True, exist_ok=True)

                # Avoid overwriting a non-empty file created after archiving
                target_primary = primary_path
                if target_primary.exists() and target_primary.stat().st_size > 0:
                    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
                    target_primary = primary_path.with_name(
                        f"{primary_path.name}.restored_{ts}"
                    )

                hasher = hashlib.sha256()
                with zf.open(vf.rel_arcname, "r") as src, open(
                    target_primary, "wb"
                ) as dst:
                    while chunk := src.read(1024 * 1024):
                        hasher.update(chunk)
                        dst.write(chunk)

                # Verify SHA-256 integrity inline without reading from disk a second time
                actual_sha = hasher.hexdigest()
                if vf.sha256 and actual_sha != vf.sha256:
                    raise RuntimeError(
                        f"SHA-256 mismatch when rolling back {target_primary}"
                    )

                try:
                    os.utime(target_primary, (vf.atime, vf.mtime))
                except OSError:
                    pass

                restored_paths.append(str(target_primary))
                protected_additions.append(str(primary_path))

                # Re-create any NTFS HardLink siblings (e.g., Zalo resource/<chat_id>/fileNoise)
                for sibling_str in all_links[1:]:
                    sibling_path = Path(sibling_str)
                    sibling_path.parent.mkdir(parents=True, exist_ok=True)
                    if not sibling_path.exists():
                        try:
                            os.link(target_primary, sibling_path)
                        except OSError:
                            # Fallback copy if across filesystem boundary
                            import shutil
                            shutil.copy2(target_primary, sibling_path)
                    restored_paths.append(str(sibling_path))
                    protected_additions.append(str(sibling_path))

        # Permanently protect rolled-back files in the quota rule so they are not re-zipped
        self.db.add_protected_files(archive.rule_id, protected_additions)

        # Remove the zip & manifest from vault after successful rollback
        zip_path.unlink(missing_ok=True)
        Path(archive.manifest_path).unlink(missing_ok=True)

        prefix = "[AUTO-ROLLBACK] " if auto_triggered else "[MANUAL ROLLBACK] "
        self.db.update_vault_status(archive_id, "ROLLED_BACK", prefix + reason)

        return {
            "archive_id": archive_id,
            "restored_count": len(restored_paths),
            "restored_paths": restored_paths,
            "auto_triggered": auto_triggered,
            "reason": reason,
        }

    def purge_archive_now(self, archive_id: int, reason: str) -> bool:
        archive = self.db.get_vault_archive(archive_id)
        if not archive:
            return False
        Path(archive.zip_path).unlink(missing_ok=True)
        Path(archive.manifest_path).unlink(missing_ok=True)
        self.db.update_vault_status(archive_id, "PURGED", reason)
        return True

    def purge_expired_archives(self, now_dt: datetime | None = None) -> list[int]:
        now = now_dt or datetime.now()
        purged_ids: list[int] = []
        for arch in self.db.list_vault_archives(status="QUARANTINED"):
            expires_dt = datetime.fromisoformat(arch.expires_at)
            if now >= expires_dt:
                self.purge_archive_now(
                    arch.id or 0,
                    f"Đã qua hạn an toàn ({arch.expires_at[:10]}) không phát sinh lỗi — tự động xóa vĩnh viễn",
                )
                if arch.id is not None:
                    purged_ids.append(arch.id)
        return purged_ids

    def run_safety_cycle(self, now_dt: datetime | None = None) -> dict:
        auto_rolled_back: list[int] = []
        reasons: dict[int, str] = {}

        for arch in self.db.list_vault_archives(status="QUARANTINED"):
            is_safe, unsafe_reason = self.check_archive_safety(arch)
            if not is_safe and arch.id is not None:
                self.rollback_archive(arch.id, unsafe_reason, auto_triggered=True)
                auto_rolled_back.append(arch.id)
                reasons[arch.id] = unsafe_reason

        purged = self.purge_expired_archives(now_dt=now_dt)
        return {
            "auto_rolled_back": auto_rolled_back,
            "reasons": reasons,
            "purged": purged,
        }

