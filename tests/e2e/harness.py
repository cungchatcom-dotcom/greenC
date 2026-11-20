"""
GreenC v2 Opaque-Box E2E Test Harness.
Provides isolated sandboxing, CLI execution with strict contract assertion,
Win32 mock/native lock simulation, and verifiable artifact checks.
"""

import contextlib
from dataclasses import dataclass
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from typing import Any, Dict, List, Optional

WORKSPACE_ROOT = Path("C:/DiskGuard").resolve()
FIXTURES_ROOT = WORKSPACE_ROOT / "tests" / "fixtures"


@dataclass
class ExecutionResult:
    exit_code: int
    stdout: str
    stderr: str
    elapsed_ms: float
    json_data: Optional[Dict[str, Any]] = None

    @property
    def is_success(self) -> bool:
        return self.exit_code == 0

    def json(self) -> Dict[str, Any]:
        if self.json_data is not None:
            return self.json_data
        try:
            return json.loads(self.stdout)
        except Exception as e:
            raise ValueError(f"Stdout is not valid JSON: {self.stdout[:200]}") from e


class MockCliEngine:
    """
    Reference simulation engine matching contracts/cli-contract.md and spec.md.
    Allows E2E tests to run deterministically against the specification even
    before the native binary is compiled.
    """

    def __init__(self, sandbox_dir: Path):
        self.sandbox_dir = sandbox_dir
        self.db_path = sandbox_dir / "greenc.db"
        self._init_db()

    def _init_db(self):
        conn = sqlite3.connect(str(self.db_path))
        conn.executescript("""
            CREATE TABLE IF NOT EXISTS daily_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                drive_letter TEXT NOT NULL,
                snapshot_date TEXT NOT NULL,
                total_bytes INTEGER NOT NULL,
                used_bytes INTEGER NOT NULL,
                free_bytes INTEGER NOT NULL,
                net_change_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL,
                UNIQUE(drive_letter, snapshot_date)
            );
            CREATE TABLE IF NOT EXISTS write_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_date TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                drive_letter TEXT NOT NULL,
                folder_path TEXT NOT NULL,
                file_path TEXT NOT NULL,
                delta_bytes INTEGER NOT NULL,
                process_name TEXT NOT NULL,
                pid INTEGER NOT NULL DEFAULT 0,
                service_name TEXT NOT NULL DEFAULT '',
                service_display_name TEXT NOT NULL DEFAULT '',
                reason_code TEXT NOT NULL,
                reason_detail TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS quota_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL,
                process_name TEXT NOT NULL DEFAULT '',
                service_name TEXT NOT NULL DEFAULT '',
                target_paths_json TEXT NOT NULL,
                max_bytes INTEGER NOT NULL,
                watermark_ratio REAL NOT NULL DEFAULT 0.85,
                retention_days INTEGER NOT NULL DEFAULT 7,
                min_file_age_seconds INTEGER NOT NULL DEFAULT 60,
                exclude_patterns_json TEXT NOT NULL DEFAULT '[]',
                protected_files_json TEXT NOT NULL DEFAULT '[]',
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS vault_archives (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rule_id INTEGER NOT NULL,
                app_name TEXT NOT NULL,
                zip_path TEXT NOT NULL,
                manifest_path TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                original_bytes INTEGER NOT NULL,
                compressed_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                status TEXT NOT NULL,
                status_reason TEXT NOT NULL,
                resolved_at TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS vault_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                archive_id INTEGER NOT NULL,
                original_path TEXT NOT NULL,
                rel_arcname TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                mtime REAL NOT NULL,
                atime REAL NOT NULL,
                hardlink_paths_json TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS relocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_id TEXT NOT NULL,
                app_name TEXT NOT NULL,
                strategy TEXT NOT NULL,
                source_path TEXT NOT NULL,
                target_path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                process_names TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                detail TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS compaction_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_path TEXT NOT NULL,
                algorithm TEXT NOT NULL,
                original_bytes INTEGER NOT NULL,
                compacted_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
        """)
        conn.commit()
        conn.close()

    def dispatch(self, args: List[str]) -> ExecutionResult:
        start_time = time.time()
        is_json = "--json" in args

        if not args:
            return ExecutionResult(2, "", "No command provided", (time.time() - start_time) * 1000)

        cmd = args[0]
        sub = args[1] if len(args) > 1 and not args[1].startswith("--") else ""

        # Precondition checks
        for arg in args:
            if "OneDrive" in arg:
                err_msg = "Precondition blocked: OneDrive path cannot be relocated"
                return ExecutionResult(2, json.dumps({"error": err_msg}) if is_json else "", err_msg, (time.time() - start_time) * 1000)
            if "non_ntfs" in arg.lower() or "fat32" in arg.lower():
                err_msg = "Precondition blocked: Target volume does not support NTFS reparse points"
                return ExecutionResult(2, json.dumps({"error": err_msg}) if is_json else "", err_msg, (time.time() - start_time) * 1000)

        if cmd == "status":
            data = {
                "drives": [
                    {"drive_letter": "C:\\", "total_bytes": 512000000000, "free_bytes": 45000000000, "is_system": True, "is_eligible": True},
                    {"drive_letter": "D:\\", "total_bytes": 1000000000000, "free_bytes": 650000000000, "is_system": False, "is_eligible": True}
                ],
                "recommended_drive": "D:\\",
                "burn_rate_forecast": {"daily_net_bps": 125000000, "days_until_90_pct": 18, "days_until_full": 32},
                "vault_summary": {"total_archives": 3, "total_compressed_bytes": 150000000},
                "active_quota_rules": 2,
                "relocation_health": {"total_junctions": 1, "healthy": 1, "broken": 0}
            }
            out = json.dumps(data, indent=2) if is_json else "GreenC Status: C: OK, D: Recommended"
            return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "scan":
            data = {
                "drive": "C:\\",
                "daily_deltas": [{"date": "2026-11-15", "net_change_bytes": 1200000000}],
                "top_consumers": [
                    {"path": "C:\\Users\\User\\AppData\\Local\\Zalo", "physical_bytes": 4200000000, "reason_code": "MEDIA_CHAT"}
                ],
                "space_lens": {"name": "C:\\", "physical_bytes": 450000000000, "children": []}
            }
            out = json.dumps(data, indent=2) if is_json else "Scan completed"
            return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "analyze":
            include_orphans = "--include-orphans" in args
            proposals = [
                {"id": "zalo", "name": "Zalo Chat Media", "reclaimable_bytes": 2500000000, "canary_passed": True, "is_orphan": False},
                {"id": "dingtalk", "name": "DingTalk MiniApps & Cache", "reclaimable_bytes": 1200000000, "canary_passed": True, "is_orphan": False}
            ]
            if include_orphans:
                proposals.append({"id": "old_uninstalled_tool", "name": "Leftover App Debris", "reclaimable_bytes": 600000000, "canary_passed": True, "is_orphan": True})
            data = {"proposals": proposals}
            out = json.dumps(data, indent=2) if is_json else f"Found {len(proposals)} proposals"
            return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "clean":
            dry_run = "--dry-run" in args
            data = {
                "dry_run": dry_run,
                "cleaned_files": 1200,
                "reclaimed_bytes": 3500000000,
                "archive_id": "vault_20261116_001" if not dry_run else None,
                "quarantine_status": "QUARANTINED" if not dry_run else None
            }
            out = json.dumps(data, indent=2) if is_json else "Cleanup completed"
            return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "vault":
            if sub == "list":
                data = {
                    "archives": [
                        {"id": 1, "archive_id": "vault_20261116_001", "app_name": "Zalo", "status": "QUARANTINED", "file_count": 450, "original_bytes": 2500000000, "compressed_bytes": 1200000000}
                    ]
                }
                out = json.dumps(data, indent=2) if is_json else "1 archive listed"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "files":
                archive_id = args[2] if len(args) > 2 else "1"
                data = {
                    "archive_id": archive_id,
                    "files": [
                        {"file_id": 101, "original_path": "C:\\AppData\\Zalo\\photo.jpg", "size_bytes": 450000, "sha256": "abcdef1234567890"}
                    ]
                }
                out = json.dumps(data, indent=2) if is_json else "1 file in archive"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "restore":
                archive_id = args[2] if len(args) > 2 else "1"
                # Check for tampered error simulation
                if "tampered" in archive_id:
                    err = "Operational error: SHA-256 integrity mismatch"
                    return ExecutionResult(1, json.dumps({"error": err}) if is_json else "", err, (time.time() - start_time) * 1000)
                data = {
                    "restored_archive_id": archive_id,
                    "restored_files_count": 1,
                    "status": "RESTORED",
                    "sha256_verified": True
                }
                out = json.dumps(data, indent=2) if is_json else "Archive restored"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "purge":
                data = {"purged_archives": 1, "freed_bytes": 1200000000}
                out = json.dumps(data, indent=2) if is_json else "Archive purged"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "relocate":
            if sub == "list":
                data = {
                    "candidates": [
                        {"app_id": "telegram", "app_name": "Telegram Desktop", "size_bytes": 2100000000, "strategy": "junction"},
                        {"app_id": "downloads", "app_name": "Downloads Folder", "size_bytes": 15000000000, "strategy": "known_folder"},
                        {"app_id": "npm", "app_name": "npm Cache", "size_bytes": 850000000, "strategy": "tool_config"}
                    ]
                }
                out = json.dumps(data, indent=2) if is_json else "3 candidates listed"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "apply":
                app_id = args[2] if len(args) > 2 else "telegram"
                if "running_app" in app_id:
                    err = "Precondition blocked: Target process is running. Please close the app."
                    return ExecutionResult(2, json.dumps({"error": err}) if is_json else "", err, (time.time() - start_time) * 1000)
                data = {
                    "record_id": 1,
                    "app_id": app_id,
                    "status": "done",
                    "strategy": "junction",
                    "target_path": f"D:\\GreenC\\Apps\\{app_id}",
                    "moved_bytes": 2100000000
                }
                out = json.dumps(data, indent=2) if is_json else "Relocation applied"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "undo":
                data = {"record_id": 1, "status": "undone", "restored_path": "C:\\Users\\User\\AppData\\Roaming\\Telegram"}
                out = json.dumps(data, indent=2) if is_json else "Relocation undone"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "check-health":
                data = {"total_records": 1, "healthy_junctions": 1, "broken_junctions": 0}
                out = json.dumps(data, indent=2) if is_json else "All junctions healthy"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "compact":
            if sub == "list":
                data = {
                    "candidates": [
                        {"path": "C:\\Projects\\node_modules", "original_bytes": 1000000000, "estimated_saving_bytes": 600000000, "algorithm": "xpress8k"}
                    ]
                }
                out = json.dumps(data, indent=2) if is_json else "1 candidate"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "apply":
                target = args[2] if len(args) > 2 else "C:\\Sample"
                data = {
                    "target_path": target,
                    "algorithm": "xpress8k",
                    "original_bytes": 1000000000,
                    "compacted_bytes": 400000000,
                    "saved_bytes": 600000000,
                    "ratio": 0.40
                }
                out = json.dumps(data, indent=2) if is_json else "Compacted successfully"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)
            elif sub == "undo":
                data = {"target_path": args[2] if len(args) > 2 else "C:\\Sample", "restored": True}
                out = json.dumps(data, indent=2) if is_json else "Uncompacted successfully"
                return ExecutionResult(0, out, "", (time.time() - start_time) * 1000, data if is_json else None)

        elif cmd == "policy":
            if sub == "export":
                out_path = Path(args[2] if len(args) > 2 else str(self.sandbox_dir / "policy.json"))
                policy_data = {
                    "version": "2.0",
                    "rules": [
                        {"app_name": "Zalo", "max_bytes": 5368709120, "watermark_ratio": 0.85, "retention_days": 7}
                    ],
                    "settings": {"language": "vi", "theme_mode": "system", "low_disk_shield_gb": 10}
                }
                out_path.parent.mkdir(parents=True, exist_ok=True)
                with open(out_path, "w", encoding="utf-8") as f:
                    json.dump(policy_data, f, indent=2)
                return ExecutionResult(0, f"Policy exported to {out_path}", "", (time.time() - start_time) * 1000)
            elif sub == "import":
                in_path = Path(args[2] if len(args) > 2 else str(self.sandbox_dir / "policy.json"))
                if not in_path.exists():
                    err = f"Operational error: File not found {in_path}"
                    return ExecutionResult(1, "", err, (time.time() - start_time) * 1000)
                return ExecutionResult(0, f"Policy imported from {in_path}", "", (time.time() - start_time) * 1000)

        # Fallback unknown command
        return ExecutionResult(2, "", f"Unknown command: {args}", (time.time() - start_time) * 1000)


class GreenCTestHarness:
    """Manages test sandbox directories and drives execution of GreenC commands."""

    def __init__(self, binary_path: Optional[Path] = None, force_mock: bool = False):
        self.binary_path = binary_path
        self.force_mock = force_mock
        self.sandboxes: List[Path] = []
        self._find_cli_binary()

    def _find_cli_binary(self):
        if self.binary_path and self.binary_path.exists():
            return
        candidate_paths = [
            WORKSPACE_ROOT / "target" / "release" / "greenc-cli.exe",
            WORKSPACE_ROOT / "target" / "debug" / "greenc-cli.exe",
            WORKSPACE_ROOT / "bin" / "greenc-cli.exe",
        ]
        for p in candidate_paths:
            if p.exists():
                self.binary_path = p
                return

    def create_sandbox(self) -> Path:
        """Create an isolated sandbox directory for test execution."""
        temp_dir = Path(tempfile.mkdtemp(prefix="greenc_e2e_"))
        self.sandboxes.append(temp_dir)

        # Structure mock environment
        (temp_dir / "AppData" / "Local" / "GreenC").mkdir(parents=True, exist_ok=True)
        (temp_dir / "AppData" / "Roaming").mkdir(parents=True, exist_ok=True)
        (temp_dir / "DataRoot").mkdir(parents=True, exist_ok=True)

        return temp_dir

    def cleanup(self):
        """Clean up all temporary sandboxes."""
        for s in self.sandboxes:
            if s.exists():
                try:
                    shutil.rmtree(s, ignore_errors=True)
                except Exception:
                    pass
        self.sandboxes.clear()

    def run_command(
        self,
        args: List[str],
        sandbox: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
        timeout: float = 30.0,
    ) -> ExecutionResult:
        """Run CLI command against real binary or reference mock engine."""
        sb = sandbox or (self.sandboxes[0] if self.sandboxes else self.create_sandbox())

        if self.binary_path and self.binary_path.exists() and not self.force_mock:
            # Native binary execution
            custom_env = os.environ.copy()
            custom_env["LOCALAPPDATA"] = str(sb / "AppData" / "Local")
            custom_env["APPDATA"] = str(sb / "AppData" / "Roaming")
            custom_env["USERPROFILE"] = str(sb)
            # Isolate every location the cleanup profiles read, so a real (non dry-run) `clean`
            # can never reach the host's Temp or Windows folders.
            fake_windows = sb / "Windows"
            (fake_windows / "Temp").mkdir(parents=True, exist_ok=True)
            (sb / "AppData" / "Local" / "Temp").mkdir(parents=True, exist_ok=True)
            custom_env["windir"] = str(fake_windows)
            custom_env["SystemRoot"] = str(fake_windows)
            custom_env["TEMP"] = str(sb / "AppData" / "Local" / "Temp")
            custom_env["TMP"] = str(sb / "AppData" / "Local" / "Temp")
            # The Orphan Radar reads the machine-wide Start Menu under %ProgramData%; sandbox it so
            # "installed app" detection only sees shortcuts a test created.
            (sb / "ProgramData").mkdir(parents=True, exist_ok=True)
            custom_env["ProgramData"] = str(sb / "ProgramData")
            custom_env["ALLUSERSPROFILE"] = str(sb / "ProgramData")
            # Never import the machine-wide legacy DB (real Vault archives) into a sandbox.
            custom_env["GREENC_SKIP_LEGACY_MIGRATION"] = "1"
            if env:
                custom_env.update(env)

            start_t = time.time()
            try:
                proc = subprocess.run(
                    [str(self.binary_path)] + args,
                    capture_output=True,
                    text=True,
                    # The CLI writes UTF-8 (Vietnamese); the Windows default codec (cp1252) would
                    # crash the reader thread and leave stdout as None.
                    encoding="utf-8",
                    errors="replace",
                    cwd=str(sb),
                    env=custom_env,
                    timeout=timeout
                )
                elapsed = (time.time() - start_t) * 1000
                json_data = None
                if "--json" in args:
                    try:
                        json_data = json.loads(proc.stdout)
                    except Exception:
                        pass
                return ExecutionResult(proc.returncode, proc.stdout, proc.stderr, elapsed, json_data)
            except subprocess.TimeoutExpired:
                return ExecutionResult(1, "", f"Command timed out after {timeout:.0f}s", timeout * 1000.0)
            except Exception as e:
                return ExecutionResult(1, "", str(e), 0.0)

        # The mock engine only proves the tests agree with themselves. Refuse it unless asked for
        # explicitly, so a missing binary can never turn into a green report again.
        if not (self.force_mock or os.environ.get("GREENC_E2E_ALLOW_MOCK") == "1"):
            raise RuntimeError(
                "greenc-cli.exe not found (looked in target/release, target/debug, bin). "
                "Build it with `cargo build --release -p greenc-cli`, or set GREENC_E2E_ALLOW_MOCK=1 "
                "to run against the reference mock engine."
            )
        engine = MockCliEngine(sb)
        return engine.dispatch(args)

    # Verification and assertion helpers
    @staticmethod
    def assert_exit_code(res: ExecutionResult, expected: int, msg: str = ""):
        assert res.exit_code == expected, f"{msg} Expected exit code {expected}, got {res.exit_code}. Stderr: {res.stderr}"

    @staticmethod
    def assert_valid_json(res: ExecutionResult) -> Dict[str, Any]:
        data = res.json()
        assert isinstance(data, dict), f"JSON output must be an object: {data}"
        return data

    @staticmethod
    def assert_file_exists(path: Path):
        assert path.exists(), f"Expected path to exist: {path}"

    @staticmethod
    def assert_file_not_exists(path: Path):
        assert not path.exists(), f"Expected path to NOT exist: {path}"

    @staticmethod
    def assert_sha256(path: Path, expected_hash: str):
        hasher = hashlib.sha256()
        with open(path, "rb") as f:
            while chunk := f.read(65536):
                hasher.update(chunk)
        actual = hasher.hexdigest().lower()
        assert actual == expected_hash.lower(), f"SHA256 mismatch for {path}: expected {expected_hash}, got {actual}"

    # ------------------------------------------------------------------
    # Sandbox fixtures. Everything below writes ONLY inside the sandbox; the real binary is run
    # with LOCALAPPDATA/APPDATA/USERPROFILE/windir/TEMP/ProgramData pointing into it.
    # ------------------------------------------------------------------
    @staticmethod
    def db_path(sb: Path) -> Path:
        return sb / "AppData" / "Local" / "GreenC" / "greenc.db"

    def ensure_db(self, sb: Path) -> Path:
        """Lets the CLI create and migrate the sandbox DB (cheap read-only command)."""
        db = self.db_path(sb)
        if not db.exists():
            res = self.run_command(["vault", "list", "--json"], sandbox=sb)
            assert res.exit_code == 0, f"could not initialise sandbox DB: {res.stderr}"
        return db

    def set_setting(self, sb: Path, key: str, value: str) -> None:
        conn = sqlite3.connect(str(self.ensure_db(sb)))
        try:
            conn.execute("INSERT OR REPLACE INTO app_settings(key, value) VALUES (?, ?)", (key, value))
            conn.commit()
        finally:
            conn.close()

    def set_sandbox_data_root(self, sb: Path) -> Path:
        """Points the data root into the sandbox. Relocation only derives its target DRIVE from it,
        so this makes candidate eligibility depend on the sandbox's own (system) volume instead of
        a hard-coded D: that may not exist."""
        root = sb / "DataRoot"
        root.mkdir(parents=True, exist_ok=True)
        self.set_setting(sb, "data_root", str(root))
        return root

    @staticmethod
    def age_path(path: Path, days: float) -> None:
        ts = time.time() - days * 86400
        os.utime(path, (ts, ts))

    @staticmethod
    def sha256_file(path: Path) -> str:
        h = hashlib.sha256()
        with open(path, "rb") as f:
            while chunk := f.read(1 << 20):
                h.update(chunk)
        return h.hexdigest()

    @staticmethod
    def is_reparse_point(path: Path) -> bool:
        try:
            st = os.lstat(path)
        except OSError:
            return False
        return bool(getattr(st, "st_file_attributes", 0) & 0x400)

    def seed_cleanup_fixtures(self, sb: Path, count: int = 3, size: int = 200 * 1024, days: float = 90) -> List[Path]:
        """Aged junk under the sandbox %TEMP% (windows_temp ignores files younger than 60 s)."""
        junk = sb / "AppData" / "Local" / "Temp" / "junk"
        junk.mkdir(parents=True, exist_ok=True)
        files = []
        for i in range(count):
            p = junk / f"junk_{i:02d}.tmp"
            line = f"greenc-e2e junk file {i} ".encode()
            p.write_bytes((line * (size // len(line) + 1))[:size])
            self.age_path(p, days)
            files.append(p)
        return files

    def clean_into_vault(self, sb: Path) -> Dict[str, Any]:
        """Seeds aged Temp junk and quarantines it with a real `clean --all`. Returns
        {archive_id, files: [vault file rows], seeded: [paths], clean: clean JSON}."""
        seeded = self.seed_cleanup_fixtures(sb)
        res = self.run_command(["clean", "--all", "--json"], sandbox=sb)
        assert res.exit_code == 0, f"clean failed: {res.stdout} {res.stderr}"
        data = res.json()
        archive_id = data.get("archive_id")
        assert archive_id, f"clean produced no archive: {data}"
        files_res = self.run_command(["vault", "files", str(archive_id), "--json"], sandbox=sb)
        assert files_res.exit_code == 0, f"vault files failed: {files_res.stderr}"
        return {"archive_id": archive_id, "files": files_res.json()["files"], "seeded": seeded, "clean": data}

    def seed_daily_snapshots(self, sb: Path, drive: str = "C:\\", days: int = 14, daily_bytes: int = 1 << 30) -> None:
        """Writes `days` consecutive daily snapshots with a constant positive net growth."""
        conn = sqlite3.connect(str(self.ensure_db(sb)))
        try:
            today = datetime.date.today()
            now = datetime.datetime.now(datetime.timezone.utc).isoformat()
            for i in range(days, 0, -1):
                d = (today - datetime.timedelta(days=i)).isoformat()
                conn.execute(
                    "INSERT OR REPLACE INTO daily_snapshots(drive_letter, snapshot_date, total_bytes, used_bytes,"
                    " free_bytes, net_change_bytes, reclaimed_bytes, updated_at) VALUES (?,?,?,?,?,?,0,?)",
                    (drive, d, 0, 0, 0, daily_bytes, now),
                )
            conn.commit()
        finally:
            conn.close()

    @staticmethod
    def seed_telegram(sb: Path) -> Path:
        """Catalog app `telegram` = %APPDATA%\\Telegram Desktop (junction strategy)."""
        src = sb / "AppData" / "Roaming" / "Telegram Desktop"
        (src / "tdata").mkdir(parents=True, exist_ok=True)
        (src / "tdata" / "a.bin").write_bytes(b"telegram-e2e" * 1024)
        # Top-level file: the relocation lock probe inspects the folder's immediate children.
        (src / "settings.dat").write_bytes(b"telegram-settings" * 64)
        return src

    @staticmethod
    def seed_npm_cache(sb: Path) -> Path:
        """Tool cache `tool_npm` = %USERPROFILE%\\.npm."""
        src = sb / ".npm" / "_cacache"
        src.mkdir(parents=True, exist_ok=True)
        (src / "x").write_bytes(b"npm-cache-e2e" * 512)
        return sb / ".npm"

    def seed_junction_relocation(self, sb: Path, process_names: str = "") -> Dict[str, Any]:
        """Builds a completed junction relocation entirely inside the sandbox:
        <sb>\\AppData\\Roaming\\DemoApp  ->  <sb>\\DataRoot\\Relocated\\demo  plus its DB record."""
        target = sb / "DataRoot" / "Relocated" / "demo"
        (target / "nested").mkdir(parents=True, exist_ok=True)
        payload = {"a.txt": b"demo-a " * 1000, "nested/b.bin": os.urandom(50_000)}
        for rel, data in payload.items():
            (target / rel).write_bytes(data)
        hashes = {rel: hashlib.sha256(data).hexdigest() for rel, data in payload.items()}
        source = sb / "AppData" / "Roaming" / "DemoApp"
        proc = subprocess.run(["cmd", "/c", "mklink", "/J", str(source), str(target)], capture_output=True, text=True)
        assert proc.returncode == 0 and self.is_reparse_point(source), f"mklink /J failed: {proc.stdout} {proc.stderr}"
        size = sum(len(d) for d in payload.values())
        conn = sqlite3.connect(str(self.ensure_db(sb)))
        try:
            cur = conn.execute(
                "INSERT INTO relocations(app_id, app_name, strategy, source_path, target_path, size_bytes, status,"
                " health_status, process_names, created_at, detail) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                ("demoapp", "Demo App", "junction", str(source), str(target), size, "done", "healthy",
                 process_names, datetime.datetime.now(datetime.timezone.utc).isoformat(), ""),
            )
            record_id = cur.lastrowid
            conn.commit()
        finally:
            conn.close()
        return {"record_id": record_id, "source": source, "target": target, "hashes": hashes}

    @staticmethod
    @contextlib.contextmanager
    def exclusive_lock(path: Path):
        """Holds `path` open with share mode 0 (no read/write/delete sharing), the way a running
        app holds its database, so GreenC's real lock probe sees it as in use."""
        import ctypes
        from ctypes import wintypes
        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        k32.CreateFileW.restype = wintypes.HANDLE
        k32.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                                    wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        GENERIC_READ, GENERIC_WRITE, OPEN_EXISTING = 0x80000000, 0x40000000, 3
        handle = k32.CreateFileW(str(path), GENERIC_READ | GENERIC_WRITE, 0, None, OPEN_EXISTING, 0x80, None)
        if handle in (None, wintypes.HANDLE(-1).value):
            raise OSError(ctypes.get_last_error(), f"CreateFileW failed for {path}")
        try:
            yield handle
        finally:
            k32.CloseHandle(wintypes.HANDLE(handle))

    @staticmethod
    def make_compaction_fixture(sb: Path, files: int = 4, size: int = 600 * 1024) -> Path:
        """<sb>\\Sample\\sub\\f{i}.txt, highly compressible text (WOF needs >=10% gain per file)."""
        root = sb / "Sample"
        (root / "sub").mkdir(parents=True, exist_ok=True)
        for i in range(files):
            line = f"GreenC compaction fixture line {i} lorem ipsum dolor sit amet\n".encode()
            (root / "sub" / f"f{i}.txt").write_bytes((line * (size // len(line) + 1))[:size])
        return root

    @staticmethod
    def seed_cargo_registry(sb: Path, files: int = 12, size: int = 1536 * 1024) -> Path:
        """>=10 MiB of compressible files under %USERPROFILE%\\.cargo (a `compact list` probe root)."""
        reg = sb / ".cargo" / "registry"
        reg.mkdir(parents=True, exist_ok=True)
        for i in range(files):
            line = f'{{"crate": "crate{i}", "vers": "1.0.{i}", "deps": []}}\n'.encode()
            (reg / f"crate{i}.json").write_bytes((line * (size // len(line) + 1))[:size])
        return sb / ".cargo"

    @staticmethod
    def seed_orphan(sb: Path, name: str, size: int = 21 * 1024 * 1024, days: float = 45) -> Path:
        """An inactive >=20 MiB folder in %LOCALAPPDATA% (Orphan Radar thresholds: 20 MiB, 30 days)."""
        d = sb / "AppData" / "Local" / name
        d.mkdir(parents=True, exist_ok=True)
        blob = d / "blob.bin"
        blob.write_bytes(os.urandom(size))
        GreenCTestHarness.age_path(blob, days)
        GreenCTestHarness.age_path(d, days)
        return d

    @staticmethod
    def expected_recommended_drive(drives: List[Dict[str, Any]]) -> Optional[str]:
        """FR-002: the eligible non-system drive with most free space if it has >= 10 GiB free,
        otherwise the eligible drive with the most free space."""
        eligible = [d for d in drives if d.get("is_eligible")]
        if not eligible:
            return None
        non_system = [d for d in eligible if not d.get("is_system")]
        if non_system:
            best = max(non_system, key=lambda d: d["free_bytes"])
            if best["free_bytes"] >= 10 * 1024 ** 3:
                return best["drive_letter"]
        return max(eligible, key=lambda d: d["free_bytes"])["drive_letter"]


# Module self-test
if __name__ == "__main__":
    harness = GreenCTestHarness()
    sb = harness.create_sandbox()
    res = harness.run_command(["status", "--json"], sandbox=sb)
    print("Harness verification result:", res.exit_code, res.json())
    harness.cleanup()
    print("Harness self-test passed.")
