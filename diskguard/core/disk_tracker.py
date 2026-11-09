from collections import defaultdict
from datetime import datetime, timedelta
import os
from pathlib import Path
import shutil

import psutil

from diskguard.core.app_profiles import discover_installed_heavy_apps
from diskguard.core.attribution import AttributionEngine
from diskguard.models import DailySnapshot, WriteEvent
from diskguard.storage.database import DiskGuardDB


class DiskTracker:
    """
    Captures daily disk usage snapshots and scans real directory modification
    history on the selected drive to populate daily bars and per-day drill-down
    events (Which folder? Why? Which day? Which App/Service?).
    """

    def __init__(self, db: DiskGuardDB, attribution: AttributionEngine | None = None):
        self.db = db
        self.attribution = attribution or AttributionEngine()

    @staticmethod
    def list_available_drives() -> list[str]:
        drives: list[str] = []
        for part in psutil.disk_partitions(all=False):
            if part.fstype and os.path.exists(part.mountpoint):
                drives.append(DiskGuardDB._normalize_drive(part.mountpoint))
        if not drives:
            drives.append("C:\\")
        return sorted(set(drives))

    def capture_current_snapshot(self, drive: str) -> DailySnapshot:
        drive_norm = DiskGuardDB._normalize_drive(drive)
        usage = shutil.disk_usage(drive_norm)
        today_str = datetime.now().strftime("%Y-%m-%d")
        return self.db.upsert_daily_snapshot(
            drive=drive_norm,
            snapshot_date=today_str,
            total_bytes=usage.total,
            used_bytes=usage.used,
            free_bytes=usage.free,
        )

    def scan_drive_history_by_day(
        self, drive: str, days_back: int = 14
    ) -> list[DailySnapshot]:
        """
        Scan real candidate directories on `drive` (including Zalo, DingTalk,
        AppData, ProgramData, Temp, and top-level folders on secondary drives like F:\\),
        group physical files (deduplicated by NTFS st_ino) by LastWriteTime date,
        record WriteEvents for drill-down, and build accurate 14-day daily snapshots.
        """
        drive_norm = DiskGuardDB._normalize_drive(drive)
        self.attribution.refresh_process_and_service_map()

        usage = shutil.disk_usage(drive_norm)
        today = datetime.now().date()
        cutoff_date = today - timedelta(days=days_back - 1)

        scan_roots = self._candidate_roots_for_drive(drive_norm)
        seen_inodes: set[tuple[int, int]] = set()

        # Group: date_str -> folder_group -> [total_bytes, count, sample_file]
        grouped: dict[str, dict[str, list]] = defaultdict(
            lambda: defaultdict(lambda: [0, 0, ""])
        )
        daily_totals: dict[str, int] = defaultdict(int)

        for root in scan_roots:
            if not root.exists():
                continue
            for dp, dirnames, filenames in os.walk(root):
                # Skip Windows internal protected / recycle / our own vault
                dirnames[:] = [
                    d
                    for d in dirnames
                    if d.lower()
                    not in (
                        "$recycle.bin",
                        "system volume information",
                        "windows",
                        ".git",
                        ".venv",
                        "vault",
                    )
                ]
                for fn in filenames:
                    fp = Path(dp) / fn
                    try:
                        st = os.stat(fp, follow_symlinks=False)
                    except OSError:
                        continue
                    if st.st_size <= 0:
                        continue
                    inode_key = (
                        (st.st_dev, st.st_ino)
                        if st.st_ino != 0
                        else (0, hash(str(fp)))
                    )
                    if inode_key in seen_inodes:
                        continue
                    seen_inodes.add(inode_key)

                    mtime_dt = datetime.fromtimestamp(st.st_mtime)
                    f_date = mtime_dt.date()
                    if f_date < cutoff_date or f_date > today:
                        continue
                    date_str = f_date.strftime("%Y-%m-%d")
                    folder_key = self._summarize_folder_path(fp.parent, root)
                    entry = grouped[date_str][folder_key]
                    entry[0] += st.st_size
                    entry[1] += 1
                    if not entry[2]:
                        entry[2] = str(fp)
                    daily_totals[date_str] += st.st_size

        # Refresh write_events for this drive and write aggregated daily events
        self.db.clear_write_events_for_drive(drive_norm)
        for date_str, folder_map in sorted(grouped.items()):
            for folder_path, (total_b, count, sample_file) in folder_map.items():
                if total_b < 64 * 1024:  # Ignore tiny <64KB noise
                    continue
                attr = self.attribution.attribute_path(sample_file or folder_path)
                self.db.record_write_event(
                    WriteEvent(
                        event_date=date_str,
                        timestamp=f"{date_str}T12:00:00",
                        drive_letter=drive_norm,
                        folder_path=folder_path,
                        file_path=sample_file or folder_path,
                        delta_bytes=int(total_b),
                        process_name=attr["process_name"],
                        pid=attr["pid"],
                        service_name=attr["service_name"],
                        service_display_name=attr["service_display_name"],
                        reason_code=attr["reason_code"],
                        reason_detail=f"{attr['reason_detail']} ({count} files)",
                    )
                )

        # Reconstruct historical daily snapshots backwards from today's used_bytes
        dates_asc = [
            (today - timedelta(days=i)).strftime("%Y-%m-%d")
            for i in range(days_back - 1, -1, -1)
        ]
        used_by_day: dict[str, int] = {}
        running_used = usage.used
        for d_str in reversed(dates_asc):
            used_by_day[d_str] = max(0, running_used)
            delta_d = daily_totals.get(d_str, 0)
            running_used = max(0, running_used - delta_d)

        for d_str in dates_asc:
            u_b = used_by_day[d_str]
            f_b = max(0, usage.total - u_b)
            n_b = daily_totals.get(d_str, 0)
            self.db.upsert_daily_snapshot(
                drive=drive_norm,
                snapshot_date=d_str,
                total_bytes=usage.total,
                used_bytes=u_b,
                free_bytes=f_b,
                net_change_bytes=n_b,
            )

        return self.db.get_daily_snapshots(drive_norm, limit_days=days_back)

    def _candidate_roots_for_drive(self, drive_norm: str) -> list[Path]:
        roots: list[Path] = []
        # Include discovered heavy app folders on this drive (Zalo, DingTalk, etc.)
        for prof in discover_installed_heavy_apps():
            for p_str in prof.reclaimable_paths + prof.protected_paths:
                p = Path(p_str)
                if p.drive.upper() + "\\" == drive_norm:
                    roots.append(p)

        if drive_norm.startswith("C:"):
            roaming = Path(os.path.expandvars(r"%APPDATA%"))
            local = Path(os.path.expandvars(r"%LOCALAPPDATA%"))
            for cand in [
                roaming / "DingTalk",
                roaming / "ZaloData",
                local / "DingTalk",
                local / "Programs" / "Zalo",
                local / "Zalo",
                local / "zalo-updater",
                local / "Temp",
                Path.home() / "Documents" / "Zalo Received Files",
                Path.home() / "Downloads",
            ]:
                if cand.exists():
                    roots.append(cand)
        else:
            # For data drives like E:\ or F:\, scan top-level user directories
            drive_path = Path(drive_norm)
            if drive_path.exists():
                for child in drive_path.iterdir():
                    if child.is_dir() and child.name.lower() not in (
                        "$recycle.bin",
                        "system volume information",
                    ):
                        roots.append(child)

        # Deduplicate root paths (avoid scanning a subfolder twice if parent is already included)
        unique_roots: list[Path] = []
        for r in sorted(roots, key=lambda x: len(str(x))):
            r_norm = os.path.normcase(os.path.abspath(str(r)))
            if not any(
                r_norm == os.path.normcase(os.path.abspath(str(u)))
                or r_norm.startswith(os.path.normcase(os.path.abspath(str(u))) + "\\")
                for u in unique_roots
            ):
                unique_roots.append(r)
        return unique_roots

    @staticmethod
    def _summarize_folder_path(folder: Path, root: Path) -> str:
        """Group deep paths (like 256 hex subfolders in resource_cache or chat IDs) to 2-3 meaningful levels."""
        f_str = str(folder)
        f_lower = f_str.lower()
        for marker in (
            "\\resource_cache\\",
            "\\zalodownloads\\resource\\",
            "\\zalodownloads\\filenoise",
            "\\eappfiles\\download",
            "\\eappfiles\\unziped",
            "\\avatarfiles",
            "\\database\\_production",
        ):
            idx = f_lower.find(marker)
            if idx != -1:
                cut = idx + len(marker.rstrip("\\"))
                return f_str[:cut]

        try:
            rel = folder.relative_to(root)
            parts = rel.parts
            if len(parts) > 2:
                return str(root.joinpath(*parts[:2]))
        except ValueError:
            pass
        return f_str
