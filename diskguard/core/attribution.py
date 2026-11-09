import os
from pathlib import Path
import psutil


REASON_LABELS = {
    "MEDIA_CHAT": "Media & File đính kèm phát sinh khi chat (Zalo / DingTalk / Chat App)",
    "EAPP_BUNDLE": "Gói Mini-App / EApp tải về & giải nén lưu cữu (DingTalk EAppFiles)",
    "CACHE_TEMP": "Bộ nhớ đệm (Cache / Temp / Resource Cache / ShaderCache)",
    "OLD_APP_VERSION": "Bản cài đặt phiên bản cũ bị bỏ lại sau khi tự cập nhật (Update Leftover)",
    "CRASH_DUMP": "File đổ lỗi bộ nhớ (Crash Dump .dmp / WER)",
    "LOG_GROWTH": "File nhật ký hệ thống / ứng dụng phình to (.log / .etl / .calf)",
    "DB_STORAGE": "Cơ sở dữ liệu & Nhật ký giao dịch (.db / .wal / .sqlite)",
    "CONTAINER_VM": "Ảnh đĩa ảo Container / Máy ảo (.vhdx / WSL / Docker)",
    "MEDIA_DOWNLOAD": "Tập tin tải về / Bộ cài / Video dung lượng lớn",
    "APP_DATA": "Dữ liệu phát sinh trong quá trình vận hành ứng dụng",
}


def classify_write_reason(file_path: str | Path) -> tuple[str, str]:
    p_str = str(file_path).replace("/", "\\")
    p_lower = p_str.lower()
    ext = Path(p_str).suffix.lower()

    if "eappfiles\\download" in p_lower or "eappfiles\\unziped" in p_lower or ext == ".amr":
        return "EAPP_BUNDLE", REASON_LABELS["EAPP_BUNDLE"]

    if (
        "zalodownloads\\resource" in p_lower
        or "zalodownloads\\filenoise" in p_lower
        or "zalodownloads\\picture" in p_lower
        or "zalodownloads\\video" in p_lower
        or "zalo received files" in p_lower
        or "imagefiles" in p_lower
        or "videofiles" in p_lower
        or "audiofiles" in p_lower
    ):
        return "MEDIA_CHAT", REASON_LABELS["MEDIA_CHAT"]

    if (
        "programs\\zalo\\zalo-" in p_lower
        or "zalo-updater" in p_lower
        or "zalo\\pending" in p_lower
        or "media\\update" in p_lower
    ):
        return "OLD_APP_VERSION", REASON_LABELS["OLD_APP_VERSION"]

    if ext in (".dmp", ".hdmp", ".mdmp") or "\\dumps\\" in p_lower or "\\crashdumps\\" in p_lower or "\\wer\\" in p_lower:
        return "CRASH_DUMP", REASON_LABELS["CRASH_DUMP"]

    if (
        ext in (".log", ".etl", ".evtx", ".calf", ".trace")
        or "\\logs\\" in p_lower
        or "\\log\\" in p_lower
        or "\\holmeslogs\\" in p_lower
        or "\\updaterlogs\\" in p_lower
        or "\\cal\\" in p_lower
    ):
        return "LOG_GROWTH", REASON_LABELS["LOG_GROWTH"]

    if (
        "\\cache\\" in p_lower
        or "\\code cache\\" in p_lower
        or "\\gpucache\\" in p_lower
        or "\\dawncache\\" in p_lower
        or "\\resource_cache\\" in p_lower
        or "\\temp\\" in p_lower
        or ext in (".tmp", ".temp", ".cache")
    ):
        return "CACHE_TEMP", REASON_LABELS["CACHE_TEMP"]

    if ext in (".db", ".sqlite", ".sqlite3", ".db-wal", ".db-shm", ".mdf", ".ldf", ".ldb") or "\\dbfiles\\" in p_lower or "\\database\\" in p_lower:
        return "DB_STORAGE", REASON_LABELS["DB_STORAGE"]

    if ext in (".vhdx", ".vhd", ".vmdk"):
        return "CONTAINER_VM", REASON_LABELS["CONTAINER_VM"]

    if ext in (".iso", ".zip", ".7z", ".rar", ".mp4", ".mkv", ".mov", ".msi", ".exe"):
        return "MEDIA_DOWNLOAD", REASON_LABELS["MEDIA_DOWNLOAD"]

    return "APP_DATA", REASON_LABELS["APP_DATA"]


class AttributionEngine:
    """
    Resolves which application process and/or Windows service is responsible
    for writing to a given path by combining live process/service tables,
    process I/O write deltas, and known application path signatures.
    """

    def __init__(self) -> None:
        self._pid_to_service: dict[int, tuple[str, str]] = {}
        self._proc_cache: dict[str, int] = {}
        self.refresh_process_and_service_map()

    def refresh_process_and_service_map(self) -> None:
        self._pid_to_service.clear()
        self._proc_cache.clear()
        try:
            for svc in psutil.win_service_iter():
                try:
                    info = svc.as_dict()
                    pid = info.get("pid")
                    if pid:
                        self._pid_to_service[int(pid)] = (
                            info.get("name") or "",
                            info.get("display_name") or "",
                        )
                except Exception:
                    continue
        except Exception:
            pass

        try:
            for proc in psutil.process_iter(["pid", "name"]):
                name = (proc.info.get("name") or "").lower()
                if name and name not in self._proc_cache:
                    self._proc_cache[name] = int(proc.info["pid"])
        except Exception:
            pass

    def attribute_path(self, file_path: str | Path) -> dict:
        p_str = str(file_path).replace("/", "\\")
        p_lower = p_str.lower()
        reason_code, reason_detail = classify_write_reason(p_str)

        # Signature matching for known heavy applications & Windows services
        if "zalo" in p_lower:
            pid = self._proc_cache.get("zalo.exe", 0)
            return {
                "process_name": "Zalo.exe",
                "pid": pid,
                "service_name": "",
                "service_display_name": "Zalo Desktop (Chat & Media)",
                "reason_code": reason_code,
                "reason_detail": reason_detail,
            }

        if "dingtalk" in p_lower:
            pid = self._proc_cache.get("dingtalk.exe", 0)
            return {
                "process_name": "DingTalk.exe",
                "pid": pid,
                "service_name": "",
                "service_display_name": "DingTalk Desktop (EApp & Media Cache)",
                "reason_code": reason_code,
                "reason_detail": reason_detail,
            }

        if "\\google\\chrome\\" in p_lower:
            pid = self._proc_cache.get("chrome.exe", 0)
            return {
                "process_name": "chrome.exe",
                "pid": pid,
                "service_name": "",
                "service_display_name": "Google Chrome Browser",
                "reason_code": reason_code,
                "reason_detail": reason_detail,
            }

        if "\\microsoft\\edge\\" in p_lower:
            pid = self._proc_cache.get("msedge.exe", 0)
            return {
                "process_name": "msedge.exe",
                "pid": pid,
                "service_name": "",
                "service_display_name": "Microsoft Edge Browser",
                "reason_code": reason_code,
                "reason_detail": reason_detail,
            }

        if "\\windows\\softwaredistribution\\" in p_lower or "\\windows\\logs\\cbs\\" in p_lower:
            return {
                "process_name": "svchost.exe (wuauserv)",
                "pid": 0,
                "service_name": "wuauserv",
                "service_display_name": "Windows Update Service",
                "reason_code": reason_code,
                "reason_detail": reason_detail,
            }

        if "\\windows\\temp\\" in p_lower or "\\appdata\\local\\temp\\" in p_lower:
            return {
                "process_name": "Windows Temp / App Installer",
                "pid": 0,
                "service_name": "System/Temp",
                "service_display_name": "Bộ nhớ tạm hệ thống & trình cài đặt",
                "reason_code": reason_code,
                "reason_detail": reason_detail,
            }

        # Extract probable application name from AppData or ProgramData folder
        parts = Path(p_str).parts
        for idx, part in enumerate(parts):
            if part.lower() in ("roaming", "local", "locallow", "programdata", "programs") and idx + 1 < len(parts):
                app_folder = parts[idx + 1]
                exe_guess = f"{app_folder}.exe"
                pid = self._proc_cache.get(exe_guess.lower(), 0)
                return {
                    "process_name": exe_guess,
                    "pid": pid,
                    "service_name": "",
                    "service_display_name": f"Ứng dụng {app_folder}",
                    "reason_code": reason_code,
                    "reason_detail": reason_detail,
                }

        top_folder = parts[1] if len(parts) > 1 else p_str
        return {
            "process_name": f"{top_folder} (User/App)",
            "pid": 0,
            "service_name": "",
            "service_display_name": f"Thư mục {top_folder}",
            "reason_code": reason_code,
            "reason_detail": reason_detail,
        }

