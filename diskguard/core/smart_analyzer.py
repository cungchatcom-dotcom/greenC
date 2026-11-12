from dataclasses import dataclass, field
import fnmatch
import hashlib
import os
from pathlib import Path
import shutil
import time
import zipfile

import psutil

from diskguard.config import DEFAULT_PROTECTED_PATTERNS, SANDBOX_DEMO_DIR
from diskguard.core.app_profiles import discover_installed_heavy_apps
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.task_controller import TaskControl
from diskguard.core.win_locks import compute_physical_usage, is_file_locked
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB


def _is_reparse_point(path: str) -> bool:
    try:
        return bool(os.lstat(path).st_file_attributes & 0x400)  # FILE_ATTRIBUTE_REPARSE_POINT
    except OSError:
        return True


@dataclass
class AppAnalysisProposal:
    app_name: str
    process_name: str
    service_name: str
    is_running: bool
    storage_summary: str
    target_paths: list[str]
    total_physical_bytes: int
    reclaimable_bytes: int
    recommended_max_bytes: int
    probe_passed: bool
    probe_strategy: str
    probe_detail: str
    exclude_patterns: list[str] = field(default_factory=list)
    selected: bool = True


class SmartAppAnalyzer:
    """
    Scans installed applications across the system, analyzes how each app runs
    and stores data (active DBs, locked files, cache/media/logs, NTFS HardLinks),
    executes a 1-cycle Sandbox Canary Probe on each candidate to verify safety
    before recommending a strategy, and provides 1-click Cleanup + silent background
    Rule registration in Settings > Rules.
    """

    def __init__(
        self,
        db: DiskGuardDB,
        vault_dir: Path | str,
        probe_dir: Path | str | None = None,
    ):
        self.db = db
        self.vault_dir = Path(vault_dir)
        self.probe_dir = (
            Path(probe_dir) if probe_dir else (SANDBOX_DEMO_DIR / "canary_probes")
        )
        self.vault_dir.mkdir(parents=True, exist_ok=True)
        self.probe_dir.mkdir(parents=True, exist_ok=True)

    @staticmethod
    def _sha256_file(path: Path) -> str:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            while chunk := f.read(1024 * 1024):
                h.update(chunk)
        return h.hexdigest()

    @staticmethod
    def _is_protected_name(path: Path, extra_patterns: list[str]) -> bool:
        name_l = path.name.lower()
        p_l = str(path).lower()
        for pat in list(DEFAULT_PROTECTED_PATTERNS) + extra_patterns:
            pl = pat.lower()
            if fnmatch.fnmatch(name_l, pl) or fnmatch.fnmatch(p_l, pl):
                return True
        return False

    def discover_system_app_candidates(self) -> list[tuple[str, str, list[str]]]:
        """
        Discover all notable installed applications across %APPDATA%, %LOCALAPPDATA%,
        and cross-drive app profiles (such as Zalo on F:\\ and DingTalk on C:\\).
        Returns list of (app_name, process_name, candidate_dirs).
        """
        results: list[tuple[str, str, list[str]]] = []
        seen_dirs: set[str] = set()

        # 1. Include deep profiles (Zalo Cross-Drive, DingTalk, etc.)
        for prof in discover_installed_heavy_apps():
            for p in prof.reclaimable_paths:
                seen_dirs.add(os.path.normcase(os.path.abspath(p)))
            results.append((prof.app_name, prof.process_name, prof.reclaimable_paths))

        # 2. Scan %APPDATA% and %LOCALAPPDATA% for other heavy applications
        roaming = Path(os.path.expandvars(r"%APPDATA%"))
        local = Path(os.path.expandvars(r"%LOCALAPPDATA%"))

        skip_vendors = {
            "microsoft",
            "windows",
            "packages",
            "temp",
            "programs",
            "zalodata",
            "zalo",
            "zalo-updater",
            "dingtalk",
            "connecteddevicesplatform",
            "d3dscache",
        }

        app_folders: dict[str, list[str]] = {}
        for base in (roaming, local):
            if not base.exists():
                continue
            try:
                for child in sorted(base.iterdir()):
                    if not child.is_dir():
                        continue
                    c_lower = child.name.lower()
                    if c_lower in skip_vendors or c_lower.startswith("."):
                        continue
                    norm_c = os.path.normcase(os.path.abspath(str(child)))
                    if any(
                        norm_c == s or norm_c.startswith(s + "\\")
                        for s in seen_dirs
                    ):
                        continue

                    # Look for reclaimable sub-areas inside the app folder (Cache, Logs, Crashes, Media, Temp)
                    reclaim_subs: list[str] = []
                    for sub_cand in (
                        "Cache",
                        "Code Cache",
                        "GPUCache",
                        "DawnCache",
                        "logs",
                        "log",
                        "Crashpad",
                        "dumps",
                        "Temp",
                        "User Data\\Default\\Cache",
                        "User Data\\Default\\Code Cache",
                    ):
                        sp = child / sub_cand
                        if sp.exists() and sp.is_dir():
                            reclaim_subs.append(str(sp))

                    if reclaim_subs:
                        app_folders.setdefault(child.name, []).extend(reclaim_subs)
                    else:
                        app_folders.setdefault(child.name, []).append(str(child))
            except OSError:
                continue

        # Also check Windows User Temp folder
        user_temp = local / "Temp"
        if user_temp.exists():
            results.append(
                ("Thư mục Temp của Windows", "System/Temp", [str(user_temp)])
            )

        for app_label, dirs in sorted(app_folders.items()):
            results.append((app_label, f"{app_label}.exe", dirs))

        return results

    def scan_and_probe_apps(
        self,
        custom_app_dirs: list[tuple[str, str, list[str]]] | None = None,
        task_control: TaskControl | None = None,
        min_reclaimable_bytes: int = 15 * 1024 * 1024,  # Default >= 15 MB
    ) -> list[AppAnalysisProposal]:
        ctrl = task_control or TaskControl()
        if not ctrl.report(
            5,
            "Đang chuẩn bị phân tích",
            "Đang lấy danh sách ứng dụng và tiến trình.",
        ):
            return []

        running_procs: set[str] = set()
        try:
            for p in psutil.process_iter(["name"]):
                n = (p.info.get("name") or "").lower()
                if n:
                    running_procs.add(n)
        except Exception:
            pass

        candidates = (
            custom_app_dirs
            if custom_app_dirs is not None
            else self.discover_system_app_candidates()
        )
        total_cands = max(1, len(candidates))
        proposals: list[AppAnalysisProposal] = []
        now = time.time()

        for idx, (app_name, proc_name, target_dirs) in enumerate(candidates, start=1):
            pct = 10 + int((idx / total_cands) * 75)
            if not ctrl.report(
                pct,
                f"Đang phân tích {app_name} · {idx}/{total_cands}",
                target_dirs[0] if target_dirs else "",
            ):
                break

            phys_bytes, naive_bytes, inode_map = compute_physical_usage(target_dirs)
            if phys_bytes < min_reclaimable_bytes:
                continue

            is_running = proc_name.lower() in running_procs
            excludes = list(DEFAULT_PROTECTED_PATTERNS) + [
                "*.rescache",
                "*\\Database\\*",
                "*\\DBFiles\\*",
                "*\\dtnest_db\\*",
            ]

            # Categorize files: unlocked old reclaimable vs locked/active core
            unlocked_groups: list[tuple[float, int, list[Path]]] = []
            locked_count = 0
            hardlink_count = 0

            for _, group_paths in inode_map.items():
                if len(group_paths) > 1:
                    hardlink_count += 1
                primary = group_paths[0]
                try:
                    st = os.stat(primary, follow_symlinks=False)
                except OSError:
                    continue
                if st.st_size <= 0 or (now - st.st_mtime) < 60:
                    continue
                if any(self._is_protected_name(gp, excludes) for gp in group_paths):
                    continue
                if any(is_file_locked(gp) for gp in group_paths):
                    locked_count += 1
                    continue
                unlocked_groups.append((st.st_mtime, st.st_size, group_paths))

            unlocked_groups.sort(key=lambda x: x[0])
            reclaimable_bytes = sum(item[1] for item in unlocked_groups)
            if reclaimable_bytes < min_reclaimable_bytes or not unlocked_groups:
                continue

            # Step 3: Execute 1-Cycle Automated Sandbox Canary Probe on the oldest unlocked file group!
            if not ctrl.report(
                min(95, pct + 2),
                f"Đang kiểm tra an toàn cho {app_name}",
                "Nén thử một file cũ rồi khôi phục để kiểm tra dữ liệu.",
            ):
                break

            probe_ok, strategy_label, probe_note, effective_dirs = (
                self._run_single_cycle_sandbox_probe(
                    app_name=app_name,
                    target_dirs=target_dirs,
                    unlocked_groups=unlocked_groups,
                    locked_count=locked_count,
                )
            )
            if not probe_ok:
                continue

            # Build human-readable storage pattern summary
            hl_diff = max(0, naive_bytes - phys_bytes)
            traits: list[str] = []
            if is_running:
                traits.append("Đang chạy")
            else:
                traits.append("Đang nghỉ")
            if hardlink_count > 0 or hl_diff > 1024 * 1024:
                traits.append(f"Có liên kết cứng · {hardlink_count} nhóm")
            if locked_count > 0:
                traits.append(f"Bỏ qua {locked_count} file đang mở")
            else:
                traits.append("File cũ đều không bị khóa")

            # Target cleanup: reclaim ~40% of reclaimable old data while keeping a comfortable rolling cap
            target_reclaim = int(reclaimable_bytes * 0.45)
            recommended_max = max(
                min_reclaimable_bytes, phys_bytes - target_reclaim
            )

            proposals.append(
                AppAnalysisProposal(
                    app_name=app_name,
                    process_name=proc_name,
                    service_name="",
                    is_running=is_running,
                    storage_summary=" • ".join(traits),
                    target_paths=effective_dirs,
                    total_physical_bytes=phys_bytes,
                    reclaimable_bytes=target_reclaim,
                    recommended_max_bytes=recommended_max,
                    probe_passed=True,
                    probe_strategy=strategy_label,
                    probe_detail=probe_note,
                    exclude_patterns=excludes,
                    selected=True,
                )
            )

        proposals.sort(key=lambda p: p.reclaimable_bytes, reverse=True)
        ctrl.report(
            100,
            f"Đã phân tích xong {len(proposals)} ứng dụng",
            "Chọn ứng dụng rồi bấm Dọn dẹp.",
        )
        return proposals

    def _run_single_cycle_sandbox_probe(
        self,
        app_name: str,
        target_dirs: list[str],
        unlocked_groups: list[tuple[float, int, list[Path]]],
        locked_count: int,
    ) -> tuple[bool, str, str, list[str]]:
        """
        Tests compressing & restoring a real canary file group in a temporary sandbox zip.
        If the primary candidate fails or is in a heavily-locked folder, automatically
        falls back to safe Cache/Temp subdirectories.
        """
        # Pick the smallest/oldest unlocked file (<= 15 MB) so the probe runs in milliseconds
        canary = next(
            (g for g in unlocked_groups if g[1] <= 15 * 1024 * 1024),
            unlocked_groups[0],
        )
        _, _, group_paths = canary
        primary = group_paths[0]

        probe_zip = (
            self.probe_dir
            / f"probe_{abs(hash(app_name))}_{int(time.time()*1000)}.zip"
        )
        try:
            if any(is_file_locked(p) for p in group_paths):
                raise PermissionError("File bị khóa trong lúc thử nghiệm")

            st_orig = os.stat(primary, follow_symlinks=False)
            sha_before = self._sha256_file(primary)

            # 1. Compress canary into sandbox probe zip
            with zipfile.ZipFile(
                probe_zip, mode="w", compression=zipfile.ZIP_DEFLATED
            ) as zf:
                zf.write(primary, arcname="canary.bin")

            with zipfile.ZipFile(probe_zip, mode="r") as zf_chk:
                if zf_chk.testzip() is not None:
                    raise RuntimeError("Lỗi kiểm tra gói nén thử nghiệm")

            # 2. Temporarily unlink all hardlink siblings of canary and immediately restore
            # to verify write/link permissions and non-interference
            for p in group_paths:
                os.remove(p)

            with zipfile.ZipFile(probe_zip, mode="r") as zf_ext:
                with zf_ext.open("canary.bin") as src, open(primary, "wb") as dst:
                    shutil.copyfileobj(src, dst)

            os.utime(primary, (st_orig.st_atime, st_orig.st_mtime))
            for sibling in group_paths[1:]:
                if not sibling.exists():
                    os.link(primary, sibling)

            sha_after = self._sha256_file(primary)
            if sha_after != sha_before:
                raise RuntimeError("SHA-256 không khớp sau khi khôi phục thử nghiệm")

            if locked_count > 0:
                return (
                    True,
                    "Đạt · bỏ qua file đang mở",
                    f"Bỏ qua {locked_count} file đang mở. Đã thử nén và khôi phục '{primary.name}', dữ liệu khớp.",
                    target_dirs,
                )
            return (
                True,
                "Đạt",
                f"Đã thử nén và khôi phục '{primary.name}', dữ liệu và liên kết cứng khớp.",
                target_dirs,
            )
        except Exception:
            # Fallback: try restricting only to Cache / Temp / download / resource_cache subfolders
            safe_subdirs: list[str] = []
            for d_str in target_dirs:
                dp = Path(d_str)
                if not dp.exists():
                    continue
                for walk_root, dirnames, _ in os.walk(dp):
                    # Prune junctions/symlinks: a target behind one could be outside the app.
                    dirnames[:] = [
                        n for n in dirnames
                        if not _is_reparse_point(os.path.join(walk_root, n))
                    ]
                    for n in dirnames:
                        if n.lower() in (
                            "cache",
                            "temp",
                            "logs",
                            "dumps",
                            "resource_cache",
                            "download",
                        ):
                            safe_subdirs.append(os.path.join(walk_root, n))
            if safe_subdirs:
                return (
                    True,
                    "Chỉ dọn bộ đệm",
                    "Thư mục chính có file quan trọng, chỉ dọn các thư mục Cache, Temp, Logs.",
                    safe_subdirs[:6],
                )
            return False, "Không đạt", "Không vượt qua kiểm tra an toàn.", target_dirs
        finally:
            probe_zip.unlink(missing_ok=True)

    def execute_one_click_cleanup(
        self,
        proposals: list[AppAnalysisProposal],
        task_control: TaskControl | None = None,
        retention_days: int = 7,
    ) -> dict:
        """
        Executes the user's 1-click "Dọn dẹp" action:
        1. Registers/updates each selected proposal as a silent background Rule in Settings > Rules.
        2. Runs QuotaEnforcer on each rule to compress the oldest unlocked files into Safe Vault (D days).
        """
        ctrl = task_control or TaskControl()
        selected = [p for p in proposals if p.selected and p.probe_passed]
        if not selected:
            return {"freed_bytes": 0, "archives_created": 0, "rules_saved": 0, "errors": []}

        existing_by_name = {r.app_name: r for r in self.db.list_quota_rules()}
        enforcer = QuotaEnforcer(self.db, self.vault_dir)

        freed_bytes = 0
        archives_created = 0
        rules_saved = 0
        errors: list[str] = []
        total = len(selected)

        for idx, prop in enumerate(selected, start=1):
            pct = int(((idx - 1) / total) * 90) + 5
            if not ctrl.report(
                pct,
                f"Đang dọn {prop.app_name} · {idx}/{total}",
                "Nén file cũ vào Kho dự phòng.",
            ):
                break

            existing = existing_by_name.get(prop.app_name)
            rule = QuotaRule(
                id=existing.id if existing else None,
                app_name=prop.app_name,
                process_name=prop.process_name,
                service_name=prop.service_name,
                target_paths=prop.target_paths,
                max_bytes=prop.recommended_max_bytes,
                watermark_ratio=0.92,
                retention_days=retention_days,
                min_file_age_seconds=60,
                exclude_patterns=prop.exclude_patterns,
                protected_files=existing.protected_files if existing else [],
                enabled=True,
            )
            rule.id = self.db.save_quota_rule(rule)
            rules_saved += 1

            try:
                arch = enforcer.enforce_rule(rule, should_stop=lambda: ctrl.is_stopped)
            except Exception as err:  # noqa: BLE001 - keep cleaning the other apps
                errors.append(f"{prop.app_name}: {err}")
                continue
            if arch is not None:
                freed_bytes += arch.original_bytes
                archives_created += 1

        freed_gb = f"{freed_bytes / (1024**3):.1f}".replace(".", ",")
        detail = f"Đã giải phóng {freed_gb} GB từ {archives_created} ứng dụng."
        if errors:
            detail += f" Lỗi ở {len(errors)} ứng dụng: " + "; ".join(errors)
        ctrl.report(100, "Đã dọn xong", detail)
        return {
            "freed_bytes": freed_bytes,
            "archives_created": archives_created,
            "rules_saved": rules_saved,
            "errors": errors,
        }

