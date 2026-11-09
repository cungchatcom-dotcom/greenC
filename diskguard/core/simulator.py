from datetime import datetime
import os
from pathlib import Path
import time

from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.models import QuotaRule, WriteEvent
from diskguard.storage.database import DiskGuardDB


class SandboxSimulator:
    """
    Creates a realistic, completely isolated sandbox environment inside
    `C:\\DiskGuard\\sandbox_demo` simulating Zalo HardLink Media & DingTalk EApp
    growth so the user can test Quota Zipping, D-Day Countdown, and Automatic
    Rollback in 1 click without touching personal files.
    """

    def __init__(self, db: DiskGuardDB, base_dir: Path, vault_dir: Path):
        self.db = db
        self.base_dir = Path(base_dir)
        self.vault_dir = Path(vault_dir)

    def setup_and_trigger_quota_demo(self) -> dict:
        demo_app_dir = self.base_dir / "Zalo_Sandbox_Media"
        file_noise_dir = demo_app_dir / "fileNoise"
        chat_res_dir = demo_app_dir / "resource" / "chat_demo_888" / "fileNoise"
        file_noise_dir.mkdir(parents=True, exist_ok=True)
        chat_res_dir.mkdir(parents=True, exist_ok=True)

        now = time.time()

        # 1. Create protected active DB file (.db)
        (demo_app_dir / "Media.db").write_bytes(b"SQLITE_ACTIVE_DB_" * 10_000)

        # 2. Create 4 historical HardLinked media files (each 2.5 MB physical -> 10 MB total)
        created_files: list[str] = []
        for idx in range(1, 5):
            age_days = 10 - idx * 2  # 8 days, 6 days, 4 days, 2 days old
            mtime = now - age_days * 86400
            p_hash = file_noise_dir / f"demo_media_hash_{idx:02d}.bin"
            p_chat = chat_res_dir / f"179300{idx}_chat_demo_888"
            if p_chat.exists():
                p_chat.unlink()
            payload = (f"DEMO_ZALO_MEDIA_BLOCK_{idx}_".encode("ascii") * 100_000)  # ~2.4 MB
            p_hash.write_bytes(payload)
            os.utime(p_hash, (mtime, mtime))
            os.link(p_hash, p_chat)
            created_files.append(str(p_chat))

        # Record a WriteEvent for today so it shows in the Daily Dashboard too
        today_str = datetime.now().strftime("%Y-%m-%d")
        self.db.record_write_event(
            WriteEvent(
                event_date=today_str,
                timestamp=datetime.now().isoformat(timespec="seconds"),
                drive_letter="C:\\",
                folder_path=str(demo_app_dir),
                file_path=created_files[-1],
                delta_bytes=10 * 1024 * 1024,
                process_name="Zalo.exe (Sandbox Demo)",
                pid=os.getpid(),
                service_name="",
                service_display_name="Mô phỏng Zalo HardLink Media",
                reason_code="MEDIA_CHAT",
                reason_detail="Mô phỏng Zalo sinh file Media HardLink vượt ngưỡng Quota",
            )
        )

        # 3. Create or update Quota Rule with max_bytes = 6 MB (current is ~9.8 MB -> triggers zip!)
        existing_rule = None
        for r in self.db.list_quota_rules():
            if r.app_name == "🧪 [Sandbox Demo] Zalo Media HardLink":
                existing_rule = r
                break

        rule = QuotaRule(
            id=existing_rule.id if existing_rule else None,
            app_name="🧪 [Sandbox Demo] Zalo Media HardLink",
            process_name="Zalo.exe",
            service_name="",
            target_paths=[str(demo_app_dir)],
            max_bytes=6 * 1024 * 1024,  # 6 MB cap
            watermark_ratio=0.80,
            retention_days=7,
            min_file_age_seconds=10,
            exclude_patterns=["*.db", "*.log"],
            protected_files=[],
            enabled=True,
        )
        rule_id = self.db.save_quota_rule(rule)
        rule.id = rule_id

        enforcer = QuotaEnforcer(self.db, self.vault_dir)
        archive = enforcer.enforce_rule(rule)
        return {
            "rule_id": rule_id,
            "demo_dir": str(demo_app_dir),
            "archive": archive,
        }

    def simulate_unsafe_file_access_and_auto_rollback(self, archive_id: int) -> dict:
        """
        Simulate the target application trying to read one of the archived files
        and logging a FileNotFoundError, triggering SafetyWatchdog's automatic rollback!
        """
        archive = self.db.get_vault_archive(archive_id)
        if not archive:
            raise ValueError("Không tìm thấy gói nén để giả lập sự cố.")
        rule = self.db.get_quota_rule(archive.rule_id)
        vault_files = self.db.get_vault_files(archive_id)
        if not rule or not vault_files:
            raise ValueError("Không tìm thấy thông tin Rule hoặc danh sách file.")

        missing_sample = Path(vault_files[0].original_path).name
        target_dir = Path(rule.target_paths[0])
        target_dir.mkdir(parents=True, exist_ok=True)
        err_log = target_dir / "app_runtime_error.log"
        err_log.write_text(
            f"[{datetime.now().isoformat()}] CRITICAL FileNotFoundError: "
            f"Cannot open {missing_sample} (ENOENT) while user scrolled chat history!\n",
            encoding="utf-8",
        )

        watchdog = SafetyWatchdog(self.db)
        return watchdog.run_safety_cycle()
