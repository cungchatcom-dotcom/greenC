from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import psutil

from greenc.config import DEFAULT_PROTECTED_PATTERNS
from greenc.core.win_locks import compute_physical_usage
from greenc.i18n import tr


@dataclass
class AppProfile:
    app_id: str
    app_name: str
    process_name: str
    service_name: str
    description: str
    reclaimable_paths: list[str]
    protected_paths: list[str]
    exclude_patterns: list[str] = field(default_factory=list)
    recommended_max_gb: float = 2.0
    physical_bytes: int = 0
    naive_bytes: int = 0


def discover_zalo_profile_from_config(
    config_file: Path | str | None = None,
    local_appdata: Path | str | None = None,
) -> AppProfile | None:
    """
    Discover Zalo PC across drives by parsing %APPDATA%\\ZaloData\\config.json
    (which reveals custom media/database drive paths like F:\\Zalo Data) plus
    %APPDATA%\\ZaloData\\Cache and %LOCALAPPDATA%\\Programs\\Zalo old versions.
    """
    roaming = Path(os.path.expandvars(r"%APPDATA%"))
    local = Path(local_appdata) if local_appdata else Path(os.path.expandvars(r"%LOCALAPPDATA%"))
    cfg_path = Path(config_file) if config_file else (roaming / "ZaloData" / "config.json")

    reclaimable: list[str] = []
    protected: list[str] = []

    if cfg_path.exists():
        try:
            data = json.loads(cfg_path.read_text(encoding="utf-8", errors="ignore"))
            db_loc = data.get("db_path", {}).get("location")
            res_loc = data.get("resource", {}).get("location", {}).get("path")
            if db_loc:
                db_root = Path(db_loc).parent  # e.g. F:\Zalo Data
                media_dir = db_root / "media"
                msg_dir = db_root / "message"
                if media_dir.exists():
                    reclaimable.append(str(media_dir))
                if msg_dir.exists():
                    protected.append(str(msg_dir))
            elif res_loc:
                cand = Path(res_loc) / "Zalo Data" / "media"
                if cand.exists():
                    reclaimable.append(str(cand))
        except Exception:
            pass

    zalo_roaming_cache = cfg_path.parent / "Cache"
    if zalo_roaming_cache.exists():
        reclaimable.append(str(zalo_roaming_cache))

    zalo_docs = Path.home() / "Documents" / "Zalo Received Files"
    if zalo_docs.exists():
        reclaimable.append(str(zalo_docs))

    # Check old Zalo program versions in %LOCALAPPDATA%\Programs\Zalo
    running_exe_dirs: set[str] = set()
    try:
        for p in psutil.process_iter(["name", "exe"]):
            if (p.info.get("name") or "").lower() == "zalo.exe" and p.info.get("exe"):
                running_exe_dirs.add(os.path.normcase(str(Path(p.info["exe"]).parent)))
    except Exception:
        pass

    programs_zalo = local / "Programs" / "Zalo"
    if programs_zalo.exists():
        for sub in sorted(programs_zalo.iterdir()):
            if sub.is_dir() and sub.name.lower().startswith("zalo-"):
                if os.path.normcase(str(sub)) not in running_exe_dirs:
                    reclaimable.append(str(sub))
                else:
                    protected.append(str(sub))

    pending_dir = local / "Zalo" / "pending"
    if pending_dir.exists():
        reclaimable.append(str(pending_dir))

    if not reclaimable:
        return None

    phys, naive, _ = compute_physical_usage(reclaimable)
    excludes = list(DEFAULT_PROTECTED_PATTERNS) + [
        "*.rescache",
        "*.db",
        "*.db-wal",
        "*.db-shm",
        "*\\Database\\*",
        "*\\message\\*",
    ]
    return AppProfile(
        app_id="zalo_pc",
        app_name="Zalo Desktop (Media, Cache & Old Versions)",
        process_name="Zalo.exe",
        service_name="",
        description=tr(
            "Quản lý kho Media Zalo (ZaloDownloads/resource, fileNoise hỗ trợ NTFS HardLink), "
            "Zalo Cache và các bản cài đặt Zalo cũ bị bỏ quên."
        ),
        reclaimable_paths=reclaimable,
        protected_paths=protected,
        exclude_patterns=excludes,
        recommended_max_gb=2.5,
        physical_bytes=phys,
        naive_bytes=naive,
    )


def discover_dingtalk_profile_from_dirs(
    roaming_dingtalk: Path | str | None = None,
    local_dingtalk: Path | str | None = None,
) -> AppProfile | None:
    """
    Discover DingTalk storage folders (%APPDATA%\\DingTalk\\*_v2\\EAppFiles, resource_cache,
    logs, and %LOCALAPPDATA%\\DingTalk\\dumps) while protecting DBFiles.
    """
    roaming_dt = (
        Path(roaming_dingtalk)
        if roaming_dingtalk
        else Path(os.path.expandvars(r"%APPDATA%\DingTalk"))
    )
    local_dt = (
        Path(local_dingtalk)
        if local_dingtalk
        else Path(os.path.expandvars(r"%LOCALAPPDATA%\DingTalk"))
    )

    reclaimable: list[str] = []
    protected: list[str] = []

    if roaming_dt.exists():
        for child in sorted(roaming_dt.iterdir()):
            if not child.is_dir():
                continue
            if child.name.endswith("_v2"):
                for sub_name in ("EAppFiles\\download", "EAppFiles\\unziped", "resource_cache", "AvatarFiles"):
                    sub_p = child / sub_name
                    if sub_p.exists():
                        reclaimable.append(str(sub_p))
                for prot_name in ("DBFiles", "dtnest_db", "nest_module_shared_db"):
                    prot_p = child / prot_name
                    if prot_p.exists():
                        protected.append(str(prot_p))
            elif child.name.lower() in ("log", "holmeslogs", "updaterlogs"):
                reclaimable.append(str(child))

    if local_dt.exists():
        dumps_dir = local_dt / "dumps"
        if dumps_dir.exists():
            reclaimable.append(str(dumps_dir))

    if not reclaimable:
        return None

    phys, naive, _ = compute_physical_usage(reclaimable)
    excludes = list(DEFAULT_PROTECTED_PATTERNS) + [
        "*\\DBFiles\\*",
        "*\\dtnest_db\\*",
        "*.db",
        "*.sqlite",
    ]
    return AppProfile(
        app_id="dingtalk_pc",
        app_name="DingTalk Desktop (EApp Bundles, Resource Cache & Dumps)",
        process_name="DingTalk.exe",
        service_name="",
        description=tr(
            "Quản lý các gói Mini-App (.amr) trong EAppFiles/download, EAppFiles/unziped, "
            "kho ảnh resource_cache và Crash Dumps của DingTalk."
        ),
        reclaimable_paths=reclaimable,
        protected_paths=protected,
        exclude_patterns=excludes,
        recommended_max_gb=3.0,
        physical_bytes=phys,
        naive_bytes=naive,
    )


def discover_installed_heavy_apps() -> list[AppProfile]:
    profiles: list[AppProfile] = []
    zalo = discover_zalo_profile_from_config()
    if zalo:
        profiles.append(zalo)
    dt = discover_dingtalk_profile_from_dirs()
    if dt:
        profiles.append(dt)
    return profiles

