# Green C:\ — Data drive, relocation & UI redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user choose a data drive (first run + Settings), relocate app data off C: (official mechanism first, junction second), and rebrand/redesign the UI as "Green C:\".

**Architecture:** Settings live in a new `app_settings` table behind `AppSettings`. Drive discovery (`core/drives.py`), legacy migration (`core/storage_migration.py`) and a `core/relocation/` package (fs ops, strategies, catalog, service, crash recovery) are pure-Python and unit-tested on temp dirs with injected Win32/subprocess calls. The UI gets one theme module, an icon module and reusable widgets, then the shell and pages are rebuilt on top of them.

**Tech Stack:** Python 3.12, PySide6 (QtWidgets, QtSvg), psutil, SQLite, ctypes (kernel32/shell32/ole32), pytest.

Spec: `docs/superpowers/specs/2026-11-12-green-c-data-drive-relocation-ui-design.md`

## Global Constraints

- Platform: Windows 10/11 only; data drive must be fixed + NTFS.
- Python package name stays `diskguard`; user-visible name is `Green C:\` (`APP_NAME = "Green C:\\"`).
- Author block: `Tác giả: Cùng Chat` + link `https://cungchat.com` (label `cungchat.com`).
- Retention default 7 days, allowed range 1–90.
- Relocation candidate threshold: 200 MB, source on the system drive.
- Free-space reserve on the target drive: 512 MB.
- UI copy: Vietnamese only, no English in parentheses, sentence case, no "thông minh" / "1-click" / "AI".
- Colors: accent `#0067C0`, brand green `#107C10` (brand + success only), danger `#C42B1C`.
- New UI sizes use `format_size()` (`18,4 GB`); `format_bytes()` stays unchanged (it is covered by existing tests).
- Tests never touch the real user profile, registry, npm/pip config or real app folders.
- Run tests with `.venv/Scripts/python.exe -m pytest` from `C:\DiskGuard`.

## File map

| File | Responsibility |
|---|---|
| `diskguard/config.py` | Branding constants, app-home paths, legacy paths, retention bounds |
| `diskguard/core/settings.py` | `AppSettings` (data root, retention) over the DB |
| `diskguard/core/drives.py` | Drive enumeration, eligibility, recommendation, path→drive helpers |
| `diskguard/core/storage_migration.py` | Legacy DB copy, verified Vault move |
| `diskguard/core/relocation/base.py` | `CheckResult`, `RelocationCandidate`, `RelocationStrategy`, suffixes |
| `diskguard/core/relocation/fs_ops.py` | Verified copy, junction-safe delete, junction create/verify, fast size |
| `diskguard/core/relocation/junction.py` | Move + junction strategy |
| `diskguard/core/relocation/known_folders.py` | Windows known-folder strategy + shell API |
| `diskguard/core/relocation/tool_config.py` | npm / pip cache strategies |
| `diskguard/core/relocation/zalo.py` | Zalo guided strategy |
| `diskguard/core/relocation/recovery.py` | Crash recovery of interrupted junction moves |
| `diskguard/core/relocation/service.py` | Apply / undo / guided completion + record persistence |
| `diskguard/core/relocation/catalog.py` | Candidate discovery |
| `diskguard/models.py` | + `RelocationRecord`, status constants |
| `diskguard/storage/database.py` | + `app_settings`, `relocations` tables, vault path update |
| `diskguard/ui/theme.py` | Color tokens + the single QSS |
| `diskguard/ui/icons.py` | SVG line icons → `QIcon` |
| `diskguard/ui/widgets.py` | `PathCell`, `ExpandableTableCard`, `Toast`, table helpers |
| `diskguard/ui/data_location.py` | `DataLocationWidget`, `FirstRunDialog`, `run_first_run` |
| `diskguard/ui/overview_page.py` | Tổng quan page |
| `diskguard/ui/vault_page.py` | Kho dự phòng page |
| `diskguard/ui/relocation_page.py` | Di chuyển dữ liệu page |
| `diskguard/ui/settings_tab.py` | Cài đặt page (rewritten) |
| `diskguard/ui/main_window.py` | Shell, navigation, workers wiring |
| `main.py` | Startup: migration, recovery, first run |

---

### Task 1: Branding constants, settings store

**Files:**
- Modify: `diskguard/config.py` (whole file)
- Modify: `diskguard/storage/database.py` (schema + 2 methods)
- Create: `diskguard/core/settings.py`
- Test: `tests/test_settings.py`

**Interfaces:**
- Produces: `config.APP_NAME, APP_VERSION, AUTHOR, WEBSITE_URL, WEBSITE_LABEL, DATA_FOLDER_NAME, APP_HOME, DEFAULT_DB_PATH, DEFAULT_VAULT_DIR, SANDBOX_DEMO_DIR, LEGACY_DB_PATH, LEGACY_VAULT_DIR, DEFAULT_RETENTION_DAYS, MIN_RETENTION_DAYS, MAX_RETENTION_DAYS, ensure_app_dirs()`.
- Produces: `DiskGuardDB.get_setting(key: str, default: str | None = None) -> str | None`, `DiskGuardDB.set_setting(key: str, value: str) -> None`.
- Produces: `AppSettings(db)` with `.data_root -> Path | None`, `.set_data_root(path)`, `.retention_days_default -> int`, `.set_retention_days_default(days) -> int`, `.vault_dir() -> Path`, `.apps_dir() -> Path | None`.

- [ ] **Step 1: Write the failing test** — `tests/test_settings.py`

```python
from pathlib import Path

from diskguard.config import DEFAULT_RETENTION_DAYS, DEFAULT_VAULT_DIR
from diskguard.core.settings import AppSettings
from diskguard.storage.database import DiskGuardDB


def test_db_setting_roundtrip(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "s.db")
    assert db.get_setting("x") is None
    assert db.get_setting("x", "d") == "d"
    db.set_setting("x", "1")
    db.set_setting("x", "2")
    assert db.get_setting("x") == "2"


def test_defaults(tmp_path: Path):
    s = AppSettings(DiskGuardDB(tmp_path / "s.db"))
    assert s.data_root is None
    assert s.retention_days_default == DEFAULT_RETENTION_DAYS == 7
    assert s.vault_dir() == DEFAULT_VAULT_DIR
    assert s.apps_dir() is None


def test_data_root_persists_across_instances(tmp_path: Path):
    root = tmp_path / "D" / "Green C"
    AppSettings(DiskGuardDB(tmp_path / "s.db")).set_data_root(root)
    s2 = AppSettings(DiskGuardDB(tmp_path / "s.db"))
    assert s2.data_root == root
    assert s2.vault_dir() == root / "Vault"
    assert s2.apps_dir() == root / "Apps"


def test_retention_clamped_and_tolerates_garbage(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "s.db")
    s = AppSettings(db)
    assert s.set_retention_days_default(0) == 1
    assert s.set_retention_days_default(500) == 90
    assert s.set_retention_days_default(14) == 14
    assert s.retention_days_default == 14
    db.set_setting(AppSettings.KEY_RETENTION, "abc")
    assert s.retention_days_default == 7


def test_branding_constants():
    from diskguard import config

    assert config.APP_NAME == "Green C:\\"
    assert config.AUTHOR == "Cùng Chat"
    assert config.WEBSITE_URL == "https://cungchat.com"
    assert config.APP_HOME.name == "Green C"
```

- [ ] **Step 2: Run to verify it fails**

Run: `.venv/Scripts/python.exe -m pytest tests/test_settings.py -q`
Expected: FAIL — `ModuleNotFoundError: No module named 'diskguard.core.settings'`.

- [ ] **Step 3: Replace `diskguard/config.py`** (keep `DEFAULT_PROTECTED_PATTERNS` list exactly as it is today)

```python
import os
from pathlib import Path

APP_NAME = "Green C:\\"
APP_VERSION = "2.0.0"
AUTHOR = "Cùng Chat"
WEBSITE_URL = "https://cungchat.com"
WEBSITE_LABEL = "cungchat.com"

# Folder name used under %LOCALAPPDATA% and at the root of the chosen data drive.
DATA_FOLDER_NAME = "Green C"


def app_home() -> Path:
    base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    return Path(base) / DATA_FOLDER_NAME


APP_HOME = app_home()
DEFAULT_DB_PATH = APP_HOME / "greenc.db"
# Used only until the user picks a data drive (see AppSettings.vault_dir()).
DEFAULT_VAULT_DIR = APP_HOME / "Vault"
SANDBOX_DEMO_DIR = APP_HOME / "sandbox"

# Locations used by DiskGuard 0.1 — migrated on first launch.
LEGACY_BASE_DIR = Path(r"C:\DiskGuard")
LEGACY_DB_PATH = LEGACY_BASE_DIR / "data" / "diskguard.db"
LEGACY_VAULT_DIR = LEGACY_BASE_DIR / "vault"

DEFAULT_WATERMARK_RATIO = 0.85
DEFAULT_RETENTION_DAYS = 7
MIN_RETENTION_DAYS = 1
MAX_RETENTION_DAYS = 90
DEFAULT_MIN_FILE_AGE_SECONDS = 60

DEFAULT_PROTECTED_PATTERNS = [
    "*.exe",
    "*.dll",
    "*.sys",
    "*.ini",
    "*.cfg",
    "*.config",
    "*.json",
    "*.yaml",
    "*.yml",
    "*.toml",
    "*.db",
    "*.db-wal",
    "*.db-shm",
    "*.sqlite",
    "*.sqlite3",
    "*.mdf",
    "*.ldf",
    "*.rescache",
    "lockfile",
    "CURRENT",
    "MANIFEST-*",
]


def ensure_app_dirs() -> None:
    APP_HOME.mkdir(parents=True, exist_ok=True)
```

- [ ] **Step 4: Add the settings table and accessors to `diskguard/storage/database.py`**

Inside the `executescript` in `_init_schema`, append before the closing `"""`:

```sql
                CREATE TABLE IF NOT EXISTS app_settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
```

Add these methods to `DiskGuardDB` (after `_normalize_drive`):

```python
    def get_setting(self, key: str, default: str | None = None) -> str | None:
        with self._lock, self._connect() as conn:
            row = conn.execute(
                "SELECT value FROM app_settings WHERE key = ?", (key,)
            ).fetchone()
        return row["value"] if row else default

    def set_setting(self, key: str, value: str) -> None:
        with self._lock, self._connect() as conn:
            conn.execute(
                "INSERT INTO app_settings(key, value) VALUES(?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
```

- [ ] **Step 5: Create `diskguard/core/settings.py`**

```python
from pathlib import Path

from diskguard.config import (
    DEFAULT_RETENTION_DAYS,
    DEFAULT_VAULT_DIR,
    MAX_RETENTION_DAYS,
    MIN_RETENTION_DAYS,
)
from diskguard.storage.database import DiskGuardDB


class AppSettings:
    """User preferences persisted in the `app_settings` table."""

    KEY_DATA_ROOT = "data_root"
    KEY_RETENTION = "retention_days_default"

    def __init__(self, db: DiskGuardDB):
        self.db = db

    @property
    def data_root(self) -> Path | None:
        value = (self.db.get_setting(self.KEY_DATA_ROOT, "") or "").strip()
        return Path(value) if value else None

    def set_data_root(self, path: Path | str) -> None:
        self.db.set_setting(self.KEY_DATA_ROOT, str(Path(path)))

    @property
    def retention_days_default(self) -> int:
        raw = self.db.get_setting(self.KEY_RETENTION)
        if raw is None:
            return DEFAULT_RETENTION_DAYS
        try:
            return self._clamp(int(raw))
        except ValueError:
            return DEFAULT_RETENTION_DAYS

    def set_retention_days_default(self, days: int) -> int:
        clamped = self._clamp(int(days))
        self.db.set_setting(self.KEY_RETENTION, str(clamped))
        return clamped

    @staticmethod
    def _clamp(days: int) -> int:
        return max(MIN_RETENTION_DAYS, min(MAX_RETENTION_DAYS, days))

    def vault_dir(self) -> Path:
        root = self.data_root
        return root / "Vault" if root else DEFAULT_VAULT_DIR

    def apps_dir(self) -> Path | None:
        root = self.data_root
        return root / "Apps" if root else None
```

- [ ] **Step 6: Run tests**

Run: `.venv/Scripts/python.exe -m pytest tests/test_settings.py -q` → PASS.
Run: `.venv/Scripts/python.exe -m pytest -q -x` → all pass (MainWindow still imports `DEFAULT_DB_PATH`, `DEFAULT_VAULT_DIR`, `SANDBOX_DEMO_DIR`, which still exist).

- [ ] **Step 7: Commit**

```bash
git add diskguard/config.py diskguard/storage/database.py diskguard/core/settings.py tests/test_settings.py
git commit -m "feat: Green C:\\ branding constants and persistent app settings"
```

---

### Task 2: Drive discovery

**Files:**
- Create: `diskguard/core/drives.py`
- Test: `tests/test_drives.py`

**Interfaces:**
- Consumes: `config.DATA_FOLDER_NAME`.
- Produces: `DriveInfo(root, label, fs, total, free, is_system, eligible, reason)` with `.used`, `.letter`; `DRIVE_FIXED = 3`; `system_drive_root() -> str`; `drive_of(path) -> str`; `is_on_system_drive(path, system_root=None) -> bool`; `list_drives(roots=None, probe=None, usage=shutil.disk_usage, system_root=None) -> list[DriveInfo]`; `recommend_drive(drives) -> DriveInfo | None`; `data_root_for(drive_root: str) -> Path`.

- [ ] **Step 1: Write the failing test** — `tests/test_drives.py`

```python
from collections import namedtuple
from pathlib import Path

from diskguard.core.drives import (
    DRIVE_FIXED,
    data_root_for,
    drive_of,
    is_on_system_drive,
    list_drives,
    recommend_drive,
)

GB = 1024**3
Usage = namedtuple("Usage", "total used free")

VOLS = {
    "C:\\": (DRIVE_FIXED, "Windows", "NTFS"),
    "D:\\": (DRIVE_FIXED, "Data", "NTFS"),
    "E:\\": (2, "USB", "FAT32"),
    "F:\\": (DRIVE_FIXED, "New Volume", "NTFS"),
    "G:\\": (DRIVE_FIXED, "Old", "exFAT"),
}
SIZES = {
    "C:\\": (237 * GB, 18 * GB),
    "D:\\": (931 * GB, 612 * GB),
    "E:\\": (64 * GB, 60 * GB),
    "F:\\": (465 * GB, 133 * GB),
    "G:\\": (100 * GB, 99 * GB),
}


def _usage(root):
    total, free = SIZES[root]
    return Usage(total, total - free, free)


def test_eligibility_and_recommendation():
    drives = list_drives(
        roots=list(VOLS), probe=lambda r: VOLS[r], usage=_usage, system_root="C:\\"
    )
    by = {d.root: d for d in drives}
    assert by["C:\\"].is_system and by["C:\\"].eligible
    assert not by["E:\\"].eligible and "di động" in by["E:\\"].reason
    assert not by["G:\\"].eligible and "exFAT" in by["G:\\"].reason
    assert by["D:\\"].used == 319 * GB
    assert by["D:\\"].letter == "D:"
    assert recommend_drive(drives).root == "D:\\"


def test_system_drive_recommended_only_when_alone():
    drives = list_drives(
        roots=["C:\\", "E:\\"], probe=lambda r: VOLS[r], usage=_usage, system_root="C:\\"
    )
    assert recommend_drive(drives).root == "C:\\"
    assert recommend_drive([]) is None


def test_unreadable_drive_is_skipped():
    def probe(root):
        if root == "X:\\":
            raise OSError(21, "not ready")
        return VOLS[root]

    drives = list_drives(roots=["C:\\", "X:\\"], probe=probe, usage=_usage, system_root="C:\\")
    assert [d.root for d in drives] == ["C:\\"]


def test_path_helpers():
    assert drive_of("d:\\a\\b") == "D:\\"
    assert is_on_system_drive("C:\\Users\\x", "C:\\")
    assert not is_on_system_drive("D:\\x", "C:\\")
    assert data_root_for("D:\\") == Path("D:\\Green C")


def test_real_machine_lists_system_drive():
    assert any(d.is_system for d in list_drives())
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_drives.py -q` → `ModuleNotFoundError`.

- [ ] **Step 3: Create `diskguard/core/drives.py`**

```python
import ctypes
from ctypes import wintypes
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
import string
from typing import Callable

from diskguard.config import DATA_FOLDER_NAME

DRIVE_FIXED = 3
_SEM_FAILCRITICALERRORS = 0x0001

_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
_kernel32.GetLogicalDrives.restype = wintypes.DWORD
_kernel32.GetDriveTypeW.argtypes = [wintypes.LPCWSTR]
_kernel32.GetDriveTypeW.restype = wintypes.UINT
_kernel32.GetVolumeInformationW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.LPWSTR,
    wintypes.DWORD,
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(wintypes.DWORD),
    ctypes.POINTER(wintypes.DWORD),
    wintypes.LPWSTR,
    wintypes.DWORD,
]
_kernel32.GetVolumeInformationW.restype = wintypes.BOOL
_kernel32.SetThreadErrorMode.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
_kernel32.SetThreadErrorMode.restype = wintypes.BOOL


@dataclass(frozen=True)
class DriveInfo:
    root: str  # "D:\\"
    label: str
    fs: str
    total: int
    free: int
    is_system: bool
    eligible: bool
    reason: str = ""

    @property
    def used(self) -> int:
        return max(0, self.total - self.free)

    @property
    def letter(self) -> str:
        return self.root[:2]


def system_drive_root() -> str:
    drive = (os.environ.get("SystemDrive") or "C:").rstrip("\\/")
    return drive.upper() + "\\"


def drive_of(path: str | Path) -> str:
    drive = os.path.splitdrive(os.path.abspath(str(path)))[0]
    return (drive.upper() + "\\") if drive else ""


def is_on_system_drive(path: str | Path, system_root: str | None = None) -> bool:
    return drive_of(path) == (system_root or system_drive_root()).upper()


def _logical_roots() -> list[str]:
    mask = _kernel32.GetLogicalDrives()
    return [f"{c}:\\" for i, c in enumerate(string.ascii_uppercase) if mask & (1 << i)]


def _probe_volume(root: str) -> tuple[int, str, str]:
    old_mode = wintypes.DWORD(0)
    # Avoid "There is no disk in the drive" popups for empty card readers / DVD drives.
    _kernel32.SetThreadErrorMode(_SEM_FAILCRITICALERRORS, ctypes.byref(old_mode))
    try:
        drive_type = int(_kernel32.GetDriveTypeW(root))
        label = ctypes.create_unicode_buffer(261)
        fs = ctypes.create_unicode_buffer(261)
        ok = _kernel32.GetVolumeInformationW(root, label, 261, None, None, None, fs, 261)
        if not ok:
            raise OSError(ctypes.get_last_error(), f"GetVolumeInformationW failed for {root}")
        return drive_type, label.value, fs.value
    finally:
        _kernel32.SetThreadErrorMode(old_mode.value, None)


def list_drives(
    roots: list[str] | None = None,
    probe: Callable[[str], tuple[int, str, str]] | None = None,
    usage: Callable = shutil.disk_usage,
    system_root: str | None = None,
) -> list[DriveInfo]:
    sys_root = (system_root or system_drive_root()).upper()
    probe_fn = probe or _probe_volume
    drives: list[DriveInfo] = []
    for root in roots if roots is not None else _logical_roots():
        try:
            drive_type, label, fs = probe_fn(root)
            u = usage(root)
        except OSError:
            continue
        reason = ""
        if drive_type != DRIVE_FIXED:
            reason = "Ổ di động hoặc ổ mạng"
        elif (fs or "").upper() != "NTFS":
            reason = f"Định dạng {fs or 'không rõ'} không được hỗ trợ"
        drives.append(
            DriveInfo(
                root=root.upper(),
                label=label,
                fs=fs,
                total=int(u.total),
                free=int(u.free),
                is_system=root.upper() == sys_root,
                eligible=not reason,
                reason=reason,
            )
        )
    return drives


def recommend_drive(drives: list[DriveInfo]) -> DriveInfo | None:
    eligible = [d for d in drives if d.eligible]
    pool = [d for d in eligible if not d.is_system] or eligible
    return max(pool, key=lambda d: d.free) if pool else None


def data_root_for(drive_root: str) -> Path:
    return Path(drive_root) / DATA_FOLDER_NAME
```

- [ ] **Step 4: Run tests** — `pytest tests/test_drives.py -q` → PASS.

- [ ] **Step 5: Commit**

```bash
git add diskguard/core/drives.py tests/test_drives.py
git commit -m "feat: fixed NTFS drive discovery and data drive recommendation"
```

---

### Task 3: Legacy DB migration and verified Vault move

**Files:**
- Create: `diskguard/core/storage_migration.py`
- Modify: `diskguard/storage/database.py` (add `update_vault_paths`)
- Test: `tests/test_storage_migration.py`

**Interfaces:**
- Consumes: `DiskGuardDB.list_vault_archives(status=...)`, `create_vault_archive(archive, items)`, `TaskControl.report(pct, title, detail) -> bool`.
- Produces: `migrate_legacy_db(legacy_db, new_db) -> bool`; `move_vault(db, new_dir, ctrl=None) -> dict` (`{"moved": int, "errors": list[str]}`); `remove_dir_if_empty(path) -> bool`; `DiskGuardDB.update_vault_paths(archive_id, zip_path, manifest_path)`.

- [ ] **Step 1: Write the failing test** — `tests/test_storage_migration.py`

```python
from pathlib import Path

from diskguard.core import storage_migration as sm
from diskguard.models import VaultArchive
from diskguard.storage.database import DiskGuardDB


def _archive(db: DiskGuardDB, vault: Path, name: str) -> VaultArchive:
    vault.mkdir(parents=True, exist_ok=True)
    zip_p = vault / f"{name}.zip"
    man_p = vault / f"{name}.manifest.json"
    zip_p.write_bytes(b"Z" * 4096)
    man_p.write_text("{}", encoding="utf-8")
    arch = VaultArchive(
        id=None, rule_id=1, app_name="App", zip_path=str(zip_p), manifest_path=str(man_p),
        file_count=1, original_bytes=8192, compressed_bytes=4096,
        created_at="2026-11-01T10:00:00", expires_at="2026-11-08T10:00:00",
        status="QUARANTINED", status_reason="",
    )
    arch.id = db.create_vault_archive(arch, [])
    return arch


def test_migrate_legacy_db_copies_once(tmp_path: Path):
    legacy = DiskGuardDB(tmp_path / "old" / "diskguard.db")
    legacy.set_setting("k", "v")
    new_path = tmp_path / "new" / "greenc.db"
    assert sm.migrate_legacy_db(tmp_path / "old" / "diskguard.db", new_path) is True
    assert DiskGuardDB(new_path).get_setting("k") == "v"
    assert sm.migrate_legacy_db(tmp_path / "old" / "diskguard.db", new_path) is False


def test_migrate_skips_without_legacy(tmp_path: Path):
    assert sm.migrate_legacy_db(tmp_path / "none.db", tmp_path / "n.db") is False
    assert not (tmp_path / "n.db").exists()


def test_move_vault_moves_files_and_rewrites_paths(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "v.db")
    old, new = tmp_path / "old_vault", tmp_path / "D" / "Vault"
    a = _archive(db, old, "a")
    res = sm.move_vault(db, new)
    assert res == {"moved": 1, "errors": []}
    moved = db.get_vault_archive(a.id)
    assert Path(moved.zip_path) == new / "a.zip" and Path(moved.zip_path).exists()
    assert Path(moved.manifest_path) == new / "a.manifest.json"
    assert not (old / "a.zip").exists()
    assert sm.remove_dir_if_empty(old) is True


def test_move_vault_skips_archives_already_there(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "v.db")
    new = tmp_path / "Vault"
    _archive(db, new, "b")
    assert sm.move_vault(db, new)["moved"] == 0


def test_failed_copy_keeps_source_and_db(tmp_path: Path, monkeypatch):
    db = DiskGuardDB(tmp_path / "v.db")
    old, new = tmp_path / "old_vault", tmp_path / "Vault"
    a = _archive(db, old, "c")

    def boom(src, dst):
        raise OSError(112, "disk full")

    monkeypatch.setattr(sm.shutil, "copy2", boom)
    res = sm.move_vault(db, new)
    assert res["moved"] == 0 and len(res["errors"]) == 1
    assert Path(db.get_vault_archive(a.id).zip_path) == old / "c.zip"
    assert (old / "c.zip").exists()
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_storage_migration.py -q` → `ImportError`.

- [ ] **Step 3: Add to `DiskGuardDB`** (after `update_vault_status`)

```python
    def update_vault_paths(self, archive_id: int, zip_path: str, manifest_path: str) -> None:
        with self._lock, self._connect() as conn:
            conn.execute(
                "UPDATE vault_archives SET zip_path = ?, manifest_path = ? WHERE id = ?",
                (zip_path, manifest_path, archive_id),
            )
```

- [ ] **Step 4: Create `diskguard/core/storage_migration.py`**

```python
import os
from pathlib import Path
import shutil
import sqlite3

from diskguard.core.task_controller import TaskControl
from diskguard.storage.database import DiskGuardDB


def migrate_legacy_db(legacy_db: Path | str, new_db: Path | str) -> bool:
    """Copy the DiskGuard 0.1 database to the new location once (SQLite online backup)."""
    legacy_db, new_db = Path(legacy_db), Path(new_db)
    if not legacy_db.exists() or new_db.exists():
        return False
    new_db.parent.mkdir(parents=True, exist_ok=True)
    src = sqlite3.connect(str(legacy_db))
    try:
        dst = sqlite3.connect(str(new_db))
        try:
            src.backup(dst)
        finally:
            dst.close()
    finally:
        src.close()
    return True


def _same_dir(a: Path, b: Path) -> bool:
    return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


def move_vault(db: DiskGuardDB, new_dir: Path | str, ctrl: TaskControl | None = None) -> dict:
    """
    Move every live archive (zip + manifest) into `new_dir`. For each archive both files
    are copied and size-checked first, then the DB is updated, then the sources deleted —
    a failure at any point leaves the archive usable at its old location.
    """
    new_dir = Path(new_dir)
    new_dir.mkdir(parents=True, exist_ok=True)
    archives = [
        a
        for a in db.list_vault_archives(status="QUARANTINED")
        if not _same_dir(Path(a.zip_path).parent, new_dir)
    ]
    moved = 0
    errors: list[str] = []
    total = max(1, len(archives))
    for idx, arch in enumerate(archives, start=1):
        zip_src = Path(arch.zip_path)
        man_src = Path(arch.manifest_path)
        if ctrl is not None and not ctrl.report(
            int(idx * 100 / total), "Đang chuyển Kho dự phòng", f"{idx}/{len(archives)}: {zip_src.name}"
        ):
            break
        pairs = [(zip_src, new_dir / zip_src.name), (man_src, new_dir / man_src.name)]
        copied: list[tuple[Path, Path]] = []
        try:
            for src, dst in pairs:
                if not src.exists():
                    continue
                shutil.copy2(src, dst)
                if dst.stat().st_size != src.stat().st_size:
                    raise OSError(f"Sao chép không đầy đủ: {src.name}")
                copied.append((src, dst))
        except OSError as err:
            for _, dst in copied:
                dst.unlink(missing_ok=True)
            errors.append(f"{zip_src.name}: {err}")
            continue
        db.update_vault_paths(arch.id or 0, str(pairs[0][1]), str(pairs[1][1]))
        for src, _ in copied:
            try:
                os.remove(src)
            except OSError:
                pass
        moved += 1
    return {"moved": moved, "errors": errors}


def remove_dir_if_empty(path: Path | str) -> bool:
    try:
        os.rmdir(path)
        return True
    except OSError:
        return False
```

- [ ] **Step 5: Run tests** — `pytest tests/test_storage_migration.py -q` → PASS.

- [ ] **Step 6: Commit**

```bash
git add diskguard/core/storage_migration.py diskguard/storage/database.py tests/test_storage_migration.py
git commit -m "feat: legacy DB migration and verified Vault move"
```

---

### Task 4: Relocation filesystem operations

**Files:**
- Create: `diskguard/core/relocation/__init__.py`, `diskguard/core/relocation/fs_ops.py`
- Test: `tests/test_relocation_fs_ops.py`

**Interfaces:**
- Consumes: `TaskControl.report(...) -> bool`, `TaskControl.wait_if_paused() -> bool`.
- Produces (in `fs_ops`): `RelocationError(RuntimeError)`, `RelocationStopped(RelocationError)`, `is_reparse_point(path) -> bool`, `fast_dir_size(path, should_stop=None) -> int`, `verified_copy_tree(src, dst, ctrl=None, total_bytes=0, title="") -> tuple[int, int]`, `remove_tree(path) -> list[str]`, `clear_contents(path) -> list[str]`, `create_junction(link, target) -> None`, `junction_target_matches(link, target) -> bool`, `same_path(a, b) -> bool`.

- [ ] **Step 1: Write the failing test** — `tests/test_relocation_fs_ops.py`

```python
import os
from pathlib import Path
import subprocess

import pytest

from diskguard.core.relocation import fs_ops
from diskguard.core.task_controller import TaskControl


def _tree(root: Path) -> None:
    (root / "a" / "b").mkdir(parents=True)
    (root / "top.txt").write_bytes(b"T" * 1000)
    (root / "a" / "b" / "deep.bin").write_bytes(os.urandom(70_000))
    os.utime(root / "top.txt", (1_600_000_000, 1_600_000_000))


def _junction(link: Path, target: Path) -> None:
    subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)], check=True, capture_output=True)


def test_verified_copy_tree_copies_content_and_times(tmp_path: Path):
    src, dst = tmp_path / "src", tmp_path / "dst"
    _tree(src)
    files, size = fs_ops.verified_copy_tree(src, dst)
    assert (files, size) == (2, 71_000)
    assert (dst / "a" / "b" / "deep.bin").read_bytes() == (src / "a" / "b" / "deep.bin").read_bytes()
    assert int(os.stat(dst / "top.txt").st_mtime) == 1_600_000_000


def test_nested_junction_is_not_copied_or_followed(tmp_path: Path):
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret.txt").write_text("x")
    src = tmp_path / "src"
    _tree(src)
    _junction(src / "link", outside)
    fs_ops.verified_copy_tree(src, tmp_path / "dst")
    assert not (tmp_path / "dst" / "link").exists()
    assert fs_ops.fast_dir_size(src) == 71_000


def test_hash_mismatch_aborts(tmp_path: Path, monkeypatch):
    src = tmp_path / "src"
    _tree(src)
    monkeypatch.setattr(fs_ops, "_hash_file", lambda p: "different")
    with pytest.raises(fs_ops.RelocationError):
        fs_ops.verified_copy_tree(src, tmp_path / "dst")


def test_stop_request_raises_stopped(tmp_path: Path):
    src = tmp_path / "src"
    _tree(src)
    ctrl = TaskControl()
    ctrl.stop()
    with pytest.raises(fs_ops.RelocationStopped):
        fs_ops.verified_copy_tree(src, tmp_path / "dst", ctrl=ctrl)


def test_remove_tree_never_follows_junctions(tmp_path: Path):
    outside = tmp_path / "outside"
    outside.mkdir()
    keep = outside / "keep.txt"
    keep.write_text("keep")
    victim = tmp_path / "victim"
    _tree(victim)
    os.chmod(victim / "top.txt", 0o444)  # read-only must still be deleted
    _junction(victim / "link", outside)
    assert fs_ops.remove_tree(victim) == []
    assert not victim.exists()
    assert keep.exists()


def test_clear_contents_keeps_folder(tmp_path: Path):
    d = tmp_path / "d"
    _tree(d)
    assert fs_ops.clear_contents(d) == []
    assert d.is_dir() and list(d.iterdir()) == []


def test_create_junction_and_verify(tmp_path: Path):
    target = tmp_path / "target"
    target.mkdir()
    (target / "f.txt").write_text("hi")
    link = tmp_path / "link"
    fs_ops.create_junction(link, target)
    assert fs_ops.is_reparse_point(link)
    assert (link / "f.txt").read_text() == "hi"
    assert fs_ops.junction_target_matches(link, target)
    assert not fs_ops.junction_target_matches(link, tmp_path)
    assert fs_ops.same_path(str(target).upper(), target)
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_relocation_fs_ops.py -q` → `ModuleNotFoundError`.

- [ ] **Step 3: Create `diskguard/core/relocation/__init__.py`**

```python
"""Moving app data off the system drive: strategies, safe filesystem ops, discovery."""
```

- [ ] **Step 4: Create `diskguard/core/relocation/fs_ops.py`**

```python
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
from typing import Callable

from diskguard.core.task_controller import TaskControl

FILE_ATTRIBUTE_REPARSE_POINT = 0x400
_CHUNK = 4 * 1024 * 1024


class RelocationError(RuntimeError):
    """A relocation step failed; the message is shown to the user."""


class RelocationStopped(RelocationError):
    """The user pressed Stop."""


def is_reparse_point(path: str | Path) -> bool:
    try:
        return bool(os.lstat(path).st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT)
    except OSError:
        return False


def same_path(a: str | Path, b: str | Path) -> bool:
    return os.path.normcase(os.path.abspath(a)) == os.path.normcase(os.path.abspath(b))


def fast_dir_size(path: str | Path, should_stop: Callable[[], bool] | None = None) -> int:
    """Sum of file sizes using cached DirEntry stats (no per-file open); skips reparse points."""
    total = 0
    stack = [str(path)]
    while stack:
        if should_stop is not None and should_stop():
            break
        current = stack.pop()
        try:
            it = os.scandir(current)
        except OSError:
            continue
        with it:
            for entry in it:
                try:
                    st = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                if st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT:
                    continue
                if stat.S_ISDIR(st.st_mode):
                    stack.append(entry.path)
                else:
                    total += st.st_size
    return total


def _hash_file(path: str | Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(_CHUNK):
            h.update(chunk)
    return h.hexdigest()


def _copy_with_hash(src: str, dst: Path) -> str:
    h = hashlib.sha256()
    with open(src, "rb") as fsrc, open(dst, "wb") as fdst:
        while chunk := fsrc.read(_CHUNK):
            h.update(chunk)
            fdst.write(chunk)
    return h.hexdigest()


def verified_copy_tree(
    src: str | Path,
    dst: str | Path,
    ctrl: TaskControl | None = None,
    total_bytes: int = 0,
    title: str = "",
) -> tuple[int, int]:
    """
    Copy `src` into `dst` (created if missing). Every file is hashed while reading the
    source, then the destination is re-read and compared. Nested junctions/symlinks are
    skipped (never followed). Returns (files, bytes). Raises RelocationError on mismatch,
    RelocationStopped when `ctrl` is stopped.
    """
    src, dst = Path(src), Path(dst)
    dst.mkdir(parents=True, exist_ok=True)
    files = 0
    copied = 0
    stack = [(src, dst)]
    while stack:
        s_dir, d_dir = stack.pop()
        with os.scandir(s_dir) as it:
            entries = list(it)
        for entry in entries:
            if ctrl is not None and not ctrl.wait_if_paused():
                raise RelocationStopped("Đã dừng theo yêu cầu")
            st = entry.stat(follow_symlinks=False)
            if st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT:
                continue
            target = d_dir / entry.name
            if stat.S_ISDIR(st.st_mode):
                target.mkdir(exist_ok=True)
                stack.append((Path(entry.path), target))
                continue
            src_hash = _copy_with_hash(entry.path, target)
            if _hash_file(target) != src_hash:
                raise RelocationError(f"Dữ liệu sao chép không khớp: {entry.path}")
            shutil.copystat(entry.path, target)
            files += 1
            copied += st.st_size
            if ctrl is not None and total_bytes > 0:
                pct = min(99, int(copied * 100 / total_bytes))
                if not ctrl.report(pct, title or "Đang sao chép dữ liệu", entry.name):
                    raise RelocationStopped("Đã dừng theo yêu cầu")
    return files, copied


def _remove_link(path: str) -> None:
    try:
        os.rmdir(path)  # directory junction / symlink: removes only the link
    except OSError:
        os.unlink(path)


def _retry_writable(func: Callable[[str], None], path: str) -> None:
    os.chmod(path, stat.S_IWRITE)
    func(path)


def _remove(path: str, errors: list[str]) -> None:
    try:
        st = os.lstat(path)
    except FileNotFoundError:
        return
    except OSError:
        errors.append(path)
        return
    if st.st_file_attributes & FILE_ATTRIBUTE_REPARSE_POINT:
        try:
            _remove_link(path)
        except OSError:
            errors.append(path)
        return
    if stat.S_ISDIR(st.st_mode):
        try:
            names = os.listdir(path)
        except OSError:
            errors.append(path)
            return
        for name in names:
            _remove(os.path.join(path, name), errors)
        try:
            os.rmdir(path)
        except OSError:
            try:
                _retry_writable(os.rmdir, path)
            except OSError:
                errors.append(path)
        return
    try:
        os.remove(path)
    except OSError:
        try:
            _retry_writable(os.remove, path)
        except OSError:
            errors.append(path)


def remove_tree(path: str | Path) -> list[str]:
    """Delete a tree without ever following junctions/symlinks inside it. Returns leftovers."""
    errors: list[str] = []
    if os.path.lexists(path):
        _remove(str(path), errors)
    return errors


def clear_contents(path: str | Path) -> list[str]:
    """Delete everything inside `path` but keep the folder itself."""
    errors: list[str] = []
    try:
        names = os.listdir(path)
    except OSError:
        return [str(path)]
    for name in names:
        _remove(os.path.join(str(path), name), errors)
    return errors


def create_junction(link: str | Path, target: str | Path) -> None:
    try:
        import _winapi

        _winapi.CreateJunction(str(target), str(link))
    except (ImportError, AttributeError):
        subprocess.run(
            ["cmd", "/c", "mklink", "/J", str(link), str(target)],
            check=True,
            capture_output=True,
        )


def junction_target_matches(link: str | Path, target: str | Path) -> bool:
    if not is_reparse_point(link):
        return False
    try:
        return same_path(os.path.realpath(link), os.path.realpath(target))
    except OSError:
        return False
```

- [ ] **Step 5: Run tests** — `pytest tests/test_relocation_fs_ops.py -q` → PASS.

- [ ] **Step 6: Commit**

```bash
git add diskguard/core/relocation tests/test_relocation_fs_ops.py
git commit -m "feat: verified copy and junction-safe filesystem ops for relocation"
```

---

### Task 5: Relocation records, junction strategy, crash recovery, service

**Files:**
- Modify: `diskguard/models.py` (append), `diskguard/storage/database.py` (table + 3 methods)
- Create: `diskguard/core/relocation/base.py`, `junction.py`, `recovery.py`, `service.py`
- Test: `tests/test_relocation_junction.py`

**Interfaces:**
- Consumes: Task 4 `fs_ops.*`; Task 1 `AppSettings.data_root`.
- Produces (models): `RELOCATION_MOVING="moving"`, `RELOCATION_DONE="done"`, `RELOCATION_FAILED="failed"`, `RELOCATION_UNDONE="undone"`; `RelocationRecord(id, app_id, app_name, strategy, source_path, target_path, size_bytes, status, process_names="", created_at="", detail="")`.
- Produces (DB): `save_relocation(rec) -> int`, `list_relocations(status: str | None = None) -> list[RelocationRecord]`, `get_relocation(rec_id) -> RelocationRecord | None`.
- Produces (base): `MOVING_SUFFIX=".greenc-moving"`, `RESTORING_SUFFIX=".greenc-restoring"`, `FREE_SPACE_RESERVE=512*1024**2`, `CheckResult(ok, reason="")`, `RelocationCandidate(app_id, name, source_path, size_bytes, strategy, how, advanced=False)`, `Saver = Callable[[RelocationRecord], RelocationRecord]`, `RelocationStrategy` with `kind`, `check(cand, target_root) -> CheckResult`, `apply(cand, target_root, ctrl, save) -> RelocationRecord`, `undo(rec, ctrl, save) -> RelocationRecord`, `now_iso() -> str`.
- Produces: `JunctionStrategy(process_names=(), is_running=None, disk_usage=shutil.disk_usage)` with `target_for(cand, target_root) -> Path`; `recover_interrupted(db) -> list[RelocationRecord]`; `RelocationService(db, settings, strategy_factory=None)` with `target_root()`, `check(cand)`, `apply(cand, ctrl=None)`, `undo(rec, ctrl=None)`, `records() -> list[RelocationRecord]` (status done); module function `strategy_for_record(rec) -> RelocationStrategy`.

- [ ] **Step 1: Write the failing test** — `tests/test_relocation_junction.py`

```python
from collections import namedtuple
import os
from pathlib import Path

import pytest

from diskguard.core.relocation import junction as jmod
from diskguard.core.relocation.base import MOVING_SUFFIX, RelocationCandidate
from diskguard.core.relocation.fs_ops import RelocationError, create_junction, is_reparse_point
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.recovery import recover_interrupted
from diskguard.core.relocation.service import RelocationService
from diskguard.core.settings import AppSettings
from diskguard.models import RELOCATION_DONE, RELOCATION_FAILED, RELOCATION_MOVING, RELOCATION_UNDONE, RelocationRecord
from diskguard.storage.database import DiskGuardDB

Usage = namedtuple("Usage", "total used free")
BIG = lambda _p: Usage(10**13, 0, 10**13)  # noqa: E731
FULL = lambda _p: Usage(10**9, 10**9, 1024)  # noqa: E731


def _app(tmp_path: Path) -> Path:
    src = tmp_path / "C" / "AppData" / "Roaming" / "Telegram Desktop"
    (src / "tdata" / "user").mkdir(parents=True)
    (src / "tdata" / "user" / "map.bin").write_bytes(os.urandom(50_000))
    (src / "log.txt").write_text("hello")
    return src


def _cand(src: Path, strategy: JunctionStrategy) -> RelocationCandidate:
    return RelocationCandidate("telegram", "Telegram Desktop", str(src), 50_005, strategy, "how")


def _strategy(running=False, usage=BIG) -> JunctionStrategy:
    return JunctionStrategy(["Telegram.exe"], is_running=lambda names, folder: running, disk_usage=usage)


class _Saver:
    def __init__(self):
        self.statuses: list[str] = []

    def __call__(self, rec: RelocationRecord) -> RelocationRecord:
        self.statuses.append(rec.status)
        rec.id = rec.id or 1
        return rec


def test_apply_moves_data_and_leaves_working_junction(tmp_path: Path):
    src = _app(tmp_path)
    original = (src / "tdata" / "user" / "map.bin").read_bytes()
    root = tmp_path / "D" / "Green C"
    save = _Saver()
    rec = _strategy().apply(_cand(src, _strategy()), root, None, save)
    target = root / "Apps" / "telegram"
    assert save.statuses == [RELOCATION_MOVING, RELOCATION_DONE]
    assert rec.target_path == str(target)
    assert is_reparse_point(src)
    assert (src / "tdata" / "user" / "map.bin").read_bytes() == original
    assert (target / "log.txt").read_text() == "hello"
    assert not Path(str(src) + MOVING_SUFFIX).exists()


def test_blocked_when_app_running(tmp_path: Path):
    src = _app(tmp_path)
    st = _strategy(running=True)
    res = st.check(_cand(src, st), tmp_path / "D")
    assert not res.ok and "Cần đóng Telegram Desktop" in res.reason
    with pytest.raises(RelocationError):
        st.apply(_cand(src, st), tmp_path / "D", None, _Saver())
    assert not is_reparse_point(src)


def test_blocked_when_target_drive_full(tmp_path: Path):
    src = _app(tmp_path)
    st = _strategy(usage=FULL)
    res = st.check(_cand(src, st), tmp_path / "D")
    assert not res.ok and "không đủ dung lượng" in res.reason


def test_rename_failure_rolls_back(tmp_path: Path, monkeypatch):
    src = _app(tmp_path)
    root = tmp_path / "D"

    def busy(a, b):
        raise PermissionError(32, "in use")

    monkeypatch.setattr(jmod.os, "rename", busy)
    save = _Saver()
    with pytest.raises(RelocationError, match="đang được sử dụng"):
        _strategy().apply(_cand(src, _strategy()), root, None, save)
    assert save.statuses[-1] == RELOCATION_FAILED
    assert src.is_dir() and not is_reparse_point(src)
    assert not (root / "Apps" / "telegram").exists()


def test_undo_restores_real_folder(tmp_path: Path):
    src = _app(tmp_path)
    root = tmp_path / "D"
    st = _strategy()
    rec = st.apply(_cand(src, st), root, None, _Saver())
    st.undo(rec, None, _Saver())
    assert rec.status == RELOCATION_UNDONE
    assert src.is_dir() and not is_reparse_point(src)
    assert (src / "log.txt").read_text() == "hello"
    assert not (root / "Apps" / "telegram").exists()


def _moving_record(db: DiskGuardDB, src: Path, target: Path) -> RelocationRecord:
    rec = RelocationRecord(None, "telegram", "Telegram Desktop", "junction", str(src), str(target), 5, RELOCATION_MOVING)
    rec.id = db.save_relocation(rec)
    return rec


def test_recover_after_rename_without_junction(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "r.db")
    src = _app(tmp_path)
    target = tmp_path / "D" / "Apps" / "telegram"
    target.mkdir(parents=True)
    (target / "log.txt").write_text("hello")
    os.rename(src, str(src) + MOVING_SUFFIX)
    rec = _moving_record(db, src, target)
    recover_interrupted(db)
    assert src.is_dir() and not is_reparse_point(src) and (src / "log.txt").exists()
    assert not target.exists()
    assert db.get_relocation(rec.id).status == RELOCATION_FAILED


def test_recover_after_junction_created(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "r.db")
    src = _app(tmp_path)
    target = tmp_path / "D" / "Apps" / "telegram"
    target.mkdir(parents=True)
    (target / "log.txt").write_text("hello")
    os.rename(src, str(src) + MOVING_SUFFIX)
    create_junction(src, target)
    rec = _moving_record(db, src, target)
    recover_interrupted(db)
    assert is_reparse_point(src) and (src / "log.txt").read_text() == "hello"
    assert not Path(str(src) + MOVING_SUFFIX).exists()
    assert db.get_relocation(rec.id).status == RELOCATION_DONE


def test_service_persists_records_and_requires_data_root(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "s.db")
    settings = AppSettings(db)
    svc = RelocationService(db, settings)
    src = _app(tmp_path)
    st = _strategy()
    with pytest.raises(RelocationError, match="Chưa chọn ổ"):
        svc.apply(_cand(src, st))
    settings.set_data_root(tmp_path / "D" / "Green C")
    rec = svc.apply(_cand(src, st))
    assert [r.id for r in svc.records()] == [rec.id]
    stored = db.get_relocation(rec.id)
    assert stored.status == RELOCATION_DONE and stored.process_names == "Telegram.exe"
    svc.strategy_factory = lambda r: st
    svc.undo(stored)
    assert db.get_relocation(rec.id).status == RELOCATION_UNDONE
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_relocation_junction.py -q` → `ImportError`.

- [ ] **Step 3: Append to `diskguard/models.py`**

```python
RELOCATION_MOVING = "moving"
RELOCATION_DONE = "done"
RELOCATION_FAILED = "failed"
RELOCATION_UNDONE = "undone"


@dataclass
class RelocationRecord:
    id: int | None
    app_id: str
    app_name: str
    strategy: str  # junction | known_folder | tool_config | guided
    source_path: str
    target_path: str
    size_bytes: int
    status: str
    process_names: str = ""  # comma-separated exe names (junction strategy)
    created_at: str = ""
    detail: str = ""
```

- [ ] **Step 4: Database** — add `RelocationRecord` to the `from diskguard.models import (...)` list, append this to the `_init_schema` script:

```sql
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
```

and add the methods:

```python
    def save_relocation(self, rec: RelocationRecord) -> int:
        with self._lock, self._connect() as conn:
            if rec.id is None:
                rec.created_at = rec.created_at or datetime.now().isoformat(timespec="seconds")
                cur = conn.execute(
                    "INSERT INTO relocations(app_id, app_name, strategy, source_path, target_path, "
                    "size_bytes, status, process_names, created_at, detail) "
                    "VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (rec.app_id, rec.app_name, rec.strategy, rec.source_path, rec.target_path,
                     int(rec.size_bytes), rec.status, rec.process_names, rec.created_at, rec.detail),
                )
                rec.id = int(cur.lastrowid)
            else:
                conn.execute(
                    "UPDATE relocations SET app_id=?, app_name=?, strategy=?, source_path=?, "
                    "target_path=?, size_bytes=?, status=?, process_names=?, detail=? WHERE id=?",
                    (rec.app_id, rec.app_name, rec.strategy, rec.source_path, rec.target_path,
                     int(rec.size_bytes), rec.status, rec.process_names, rec.detail, rec.id),
                )
        return rec.id

    @staticmethod
    def _row_to_relocation(r: sqlite3.Row) -> RelocationRecord:
        return RelocationRecord(
            id=r["id"], app_id=r["app_id"], app_name=r["app_name"], strategy=r["strategy"],
            source_path=r["source_path"], target_path=r["target_path"],
            size_bytes=r["size_bytes"], status=r["status"], process_names=r["process_names"],
            created_at=r["created_at"], detail=r["detail"],
        )

    def list_relocations(self, status: str | None = None) -> list[RelocationRecord]:
        with self._lock, self._connect() as conn:
            if status:
                rows = conn.execute(
                    "SELECT * FROM relocations WHERE status = ? ORDER BY id DESC", (status,)
                ).fetchall()
            else:
                rows = conn.execute("SELECT * FROM relocations ORDER BY id DESC").fetchall()
        return [self._row_to_relocation(r) for r in rows]

    def get_relocation(self, rec_id: int) -> RelocationRecord | None:
        with self._lock, self._connect() as conn:
            row = conn.execute("SELECT * FROM relocations WHERE id = ?", (rec_id,)).fetchone()
        return self._row_to_relocation(row) if row else None
```

- [ ] **Step 5: Create `diskguard/core/relocation/base.py`**

```python
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable

from diskguard.core.task_controller import TaskControl
from diskguard.models import RelocationRecord

MOVING_SUFFIX = ".greenc-moving"
RESTORING_SUFFIX = ".greenc-restoring"
FREE_SPACE_RESERVE = 512 * 1024 * 1024

Saver = Callable[[RelocationRecord], RelocationRecord]


def now_iso() -> str:
    return datetime.now().isoformat(timespec="seconds")


@dataclass
class CheckResult:
    ok: bool
    reason: str = ""


class RelocationStrategy:
    kind = "base"

    def check(self, cand: "RelocationCandidate", target_root: Path) -> CheckResult:
        raise NotImplementedError

    def apply(
        self, cand: "RelocationCandidate", target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        raise NotImplementedError

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        raise NotImplementedError


@dataclass
class RelocationCandidate:
    app_id: str
    name: str
    source_path: str
    size_bytes: int
    strategy: RelocationStrategy
    how: str  # one-line explanation shown under the name
    advanced: bool = False
```

- [ ] **Step 6: Create `diskguard/core/relocation/junction.py`**

```python
import os
from pathlib import Path
import shutil
from typing import Callable, Iterable

import psutil

from diskguard.core.relocation.base import (
    FREE_SPACE_RESERVE,
    MOVING_SUFFIX,
    RESTORING_SUFFIX,
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
    now_iso,
)
from diskguard.core.relocation.fs_ops import (
    RelocationError,
    create_junction,
    is_reparse_point,
    junction_target_matches,
    remove_tree,
    verified_copy_tree,
)
from diskguard.core.task_controller import TaskControl
from diskguard.models import (
    RELOCATION_DONE,
    RELOCATION_FAILED,
    RELOCATION_MOVING,
    RELOCATION_UNDONE,
    RelocationRecord,
)


def default_is_running(process_names: Iterable[str], folder: str) -> bool:
    """True if a process with one of the names, or whose exe lives in `folder`, is running."""
    names = {n.lower() for n in process_names if n}
    folder_norm = os.path.normcase(os.path.abspath(folder)) + os.sep
    for proc in psutil.process_iter(["name", "exe"]):
        try:
            name = (proc.info.get("name") or "").lower()
            exe = proc.info.get("exe") or ""
        except (psutil.Error, OSError):
            continue
        if name in names:
            return True
        if exe and os.path.normcase(exe).startswith(folder_norm):
            return True
    return False


class JunctionStrategy(RelocationStrategy):
    """Copy (verified) → rename original aside → junction → verify → delete original."""

    kind = "junction"

    def __init__(
        self,
        process_names: Iterable[str] = (),
        is_running: Callable[[Iterable[str], str], bool] | None = None,
        disk_usage: Callable = shutil.disk_usage,
    ):
        self.process_names = [p for p in process_names if p]
        self._is_running = is_running or default_is_running
        self._disk_usage = disk_usage

    @staticmethod
    def target_for(cand: RelocationCandidate, target_root: Path) -> Path:
        return Path(target_root) / "Apps" / cand.app_id

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        src = Path(cand.source_path)
        if not src.is_dir():
            return CheckResult(False, "Thư mục nguồn không còn tồn tại")
        if is_reparse_point(src):
            return CheckResult(False, "Thư mục này đã được chuyển trước đó")
        if self._is_running(self.process_names, str(src)):
            return CheckResult(False, f"Cần đóng {cand.name}")
        target = self.target_for(cand, target_root)
        if target.exists() and any(target.iterdir()):
            return CheckResult(False, f"Thư mục đích đã có dữ liệu: {target}")
        anchor = Path(target_root).anchor or str(target_root)
        if self._disk_usage(anchor).free < cand.size_bytes + FREE_SPACE_RESERVE:
            return CheckResult(False, "Ổ đích không đủ dung lượng trống")
        return CheckResult(True)

    def apply(
        self, cand: RelocationCandidate, target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        res = self.check(cand, target_root)
        if not res.ok:
            raise RelocationError(res.reason)
        src = Path(cand.source_path)
        target = self.target_for(cand, target_root)
        moving = Path(str(src) + MOVING_SUFFIX)

        try:
            verified_copy_tree(src, target, ctrl, cand.size_bytes, f"Đang chuyển {cand.name}")
        except BaseException:
            remove_tree(target)
            raise

        rec = save(
            RelocationRecord(
                id=None, app_id=cand.app_id, app_name=cand.name, strategy=self.kind,
                source_path=str(src), target_path=str(target), size_bytes=cand.size_bytes,
                status=RELOCATION_MOVING, process_names=",".join(self.process_names),
                created_at=now_iso(),
            )
        )

        try:
            os.rename(src, moving)
        except OSError as err:
            remove_tree(target)
            rec.status = RELOCATION_FAILED
            rec.detail = f"Thư mục đang được sử dụng: {err.strerror or err}"
            save(rec)
            raise RelocationError(
                f"Không thể chuyển: thư mục đang được sử dụng. Hãy đóng {cand.name} rồi thử lại."
            ) from err

        try:
            create_junction(src, target)
            if not junction_target_matches(src, target):
                raise RelocationError("Liên kết tạo ra không trỏ đúng thư mục đích")
        except BaseException as err:
            if is_reparse_point(src):
                os.rmdir(src)
            os.rename(moving, src)
            remove_tree(target)
            rec.status = RELOCATION_FAILED
            rec.detail = str(err)
            save(rec)
            raise

        leftovers = remove_tree(moving)
        rec.status = RELOCATION_DONE
        rec.detail = (
            f"Còn {len(leftovers)} mục cũ chưa xóa được trong {moving}" if leftovers else ""
        )
        return save(rec)

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        src, target = Path(rec.source_path), Path(rec.target_path)
        if self._is_running(self.process_names, str(target)):
            raise RelocationError(f"Cần đóng {rec.app_name}")
        if not junction_target_matches(src, target):
            raise RelocationError("Liên kết không còn hợp lệ, không thể hoàn tác tự động")
        restoring = Path(str(src) + RESTORING_SUFFIX)
        try:
            verified_copy_tree(target, restoring, ctrl, rec.size_bytes, f"Đang trả {rec.app_name} về ổ C:")
        except BaseException:
            remove_tree(restoring)
            raise
        os.rmdir(src)  # removes the junction only
        os.rename(restoring, src)
        leftovers = remove_tree(target)
        rec.status = RELOCATION_UNDONE
        rec.detail = f"Còn {len(leftovers)} mục chưa xóa được ở {target}" if leftovers else ""
        return save(rec)
```

- [ ] **Step 7: Create `diskguard/core/relocation/recovery.py`**

```python
import os
from pathlib import Path

from diskguard.core.relocation.base import MOVING_SUFFIX
from diskguard.core.relocation.fs_ops import is_reparse_point, junction_target_matches, remove_tree
from diskguard.models import RELOCATION_DONE, RELOCATION_FAILED, RELOCATION_MOVING, RelocationRecord
from diskguard.storage.database import DiskGuardDB


def recover_interrupted(db: DiskGuardDB) -> list[RelocationRecord]:
    """Finish or roll back junction moves interrupted by a crash or power loss."""
    fixed: list[RelocationRecord] = []
    for rec in db.list_relocations(status=RELOCATION_MOVING):
        src, target = Path(rec.source_path), Path(rec.target_path)
        moving = Path(rec.source_path + MOVING_SUFFIX)
        if target.is_dir() and junction_target_matches(src, target):
            if moving.exists():
                remove_tree(moving)
            rec.status = RELOCATION_DONE
            rec.detail = "Đã hoàn tất sau khi ứng dụng khởi động lại"
        else:
            if is_reparse_point(src):
                os.rmdir(src)
            if moving.exists() and not src.exists():
                os.rename(moving, src)
            if src.is_dir() and target.exists():
                remove_tree(target)
            rec.status = RELOCATION_FAILED
            rec.detail = "Bị gián đoạn, đã khôi phục về vị trí cũ"
        db.save_relocation(rec)
        fixed.append(rec)
    return fixed
```

- [ ] **Step 8: Create `diskguard/core/relocation/service.py`**

```python
from pathlib import Path
from typing import Callable

from diskguard.core.relocation.base import CheckResult, RelocationCandidate, RelocationStrategy
from diskguard.core.relocation.fs_ops import RelocationError
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.settings import AppSettings
from diskguard.core.task_controller import TaskControl
from diskguard.models import RELOCATION_DONE, RelocationRecord
from diskguard.storage.database import DiskGuardDB


def strategy_for_record(rec: RelocationRecord) -> RelocationStrategy:
    if rec.strategy == "junction":
        return JunctionStrategy([p for p in rec.process_names.split(",") if p])
    raise RelocationError("Không thể hoàn tác mục này tự động")


class RelocationService:
    def __init__(
        self,
        db: DiskGuardDB,
        settings: AppSettings,
        strategy_factory: Callable[[RelocationRecord], RelocationStrategy] | None = None,
    ):
        self.db = db
        self.settings = settings
        self.strategy_factory = strategy_factory or strategy_for_record

    def target_root(self) -> Path:
        root = self.settings.data_root
        if root is None:
            raise RelocationError("Chưa chọn ổ lưu dữ liệu. Chọn trong Cài đặt.")
        return root

    def _save(self, rec: RelocationRecord) -> RelocationRecord:
        rec.id = self.db.save_relocation(rec)
        return rec

    def check(self, cand: RelocationCandidate) -> CheckResult:
        return cand.strategy.check(cand, self.target_root())

    def apply(self, cand: RelocationCandidate, ctrl: TaskControl | None = None) -> RelocationRecord:
        return cand.strategy.apply(cand, self.target_root(), ctrl, self._save)

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None = None) -> RelocationRecord:
        return self.strategy_factory(rec).undo(rec, ctrl, self._save)

    def records(self) -> list[RelocationRecord]:
        return self.db.list_relocations(status=RELOCATION_DONE)
```

- [ ] **Step 9: Run tests** — `pytest tests/test_relocation_junction.py tests/test_relocation_fs_ops.py -q` → PASS; full suite `pytest -q -x` → PASS.

- [ ] **Step 10: Commit**

```bash
git add diskguard/models.py diskguard/storage/database.py diskguard/core/relocation tests/test_relocation_junction.py
git commit -m "feat: junction relocation strategy with crash recovery and undo"
```

---

### Task 6: Windows known-folder strategy

**Files:**
- Create: `diskguard/core/relocation/known_folders.py`
- Modify: `diskguard/core/relocation/service.py` (`strategy_for_record`)
- Test: `tests/test_relocation_known_folders.py`

**Interfaces:**
- Consumes: Task 4 fs_ops, Task 5 base, Task 2 `is_on_system_drive`.
- Produces: `KnownFolder(app_id, name, guid)`, `KNOWN_FOLDERS: dict[str, KnownFolder]` (app ids `kf_downloads, kf_documents, kf_pictures, kf_videos, kf_music, kf_desktop`), `WindowsShellFolders` with `get(guid) -> str` and `set(guid, path) -> None`, `is_onedrive_path(path) -> bool`, `KnownFolderStrategy(folder, shell=None, disk_usage=shutil.disk_usage, system_root=None)`.

- [ ] **Step 1: Write the failing test** — `tests/test_relocation_known_folders.py`

```python
from collections import namedtuple
import os
from pathlib import Path

import pytest

from diskguard.core.relocation.base import RelocationCandidate
from diskguard.core.relocation.fs_ops import RelocationError
from diskguard.core.relocation.known_folders import (
    KNOWN_FOLDERS,
    KnownFolderStrategy,
    WindowsShellFolders,
    is_onedrive_path,
)
from diskguard.core.relocation.service import strategy_for_record
from diskguard.models import RELOCATION_DONE, RELOCATION_UNDONE

Usage = namedtuple("Usage", "total used free")
DL = KNOWN_FOLDERS["kf_downloads"]


class FakeShell:
    def __init__(self, paths):
        self.paths = dict(paths)
        self.set_calls = []

    def get(self, guid):
        return self.paths[guid]

    def set(self, guid, path):
        self.set_calls.append((guid, path))
        self.paths[guid] = path


def _setup(tmp_path):
    dl = tmp_path / "Users" / "me" / "Downloads"
    (dl / "sub").mkdir(parents=True)
    (dl / "setup.exe").write_bytes(b"E" * 3000)
    (dl / "sub" / "a.pdf").write_bytes(b"P" * 2000)
    shell = FakeShell({DL.guid: str(dl)})
    st = KnownFolderStrategy(DL, shell=shell, disk_usage=lambda p: Usage(10**12, 0, 10**12))
    cand = RelocationCandidate(DL.app_id, "Downloads", str(dl), 5000, st, "how")
    return dl, shell, st, cand


def _saver(rec):
    rec.id = rec.id or 7
    return rec


def test_apply_moves_contents_and_repoints_folder(tmp_path: Path):
    dl, shell, st, cand = _setup(tmp_path)
    root = tmp_path / "D" / "Green C"
    rec = st.apply(cand, root, None, _saver)
    assert rec.status == RELOCATION_DONE
    assert shell.paths[DL.guid] == str(root / "Downloads")
    assert (root / "Downloads" / "sub" / "a.pdf").read_bytes() == b"P" * 2000
    assert dl.is_dir() and list(dl.iterdir()) == []


def test_undo_moves_back(tmp_path: Path):
    dl, shell, st, cand = _setup(tmp_path)
    root = tmp_path / "D"
    rec = st.apply(cand, root, None, _saver)
    st.undo(rec, None, _saver)
    assert rec.status == RELOCATION_UNDONE
    assert shell.paths[DL.guid] == str(dl)
    assert (dl / "setup.exe").exists()
    assert not (root / "Downloads").exists()


def test_onedrive_folder_is_blocked(tmp_path: Path, monkeypatch):
    dl, shell, st, cand = _setup(tmp_path)
    monkeypatch.setenv("OneDrive", str(tmp_path / "Users"))
    assert is_onedrive_path(dl)
    res = st.check(cand, tmp_path / "D")
    assert not res.ok and "OneDrive" in res.reason
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path / "D", None, _saver)


def test_shell_failure_keeps_original(tmp_path: Path):
    dl, shell, st, cand = _setup(tmp_path)

    def refuse(guid, path):
        raise OSError("E_ACCESSDENIED")

    shell.set = refuse
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path / "D", None, _saver)
    assert (dl / "setup.exe").exists()
    assert not (tmp_path / "D" / "Downloads").exists()


def test_real_shell_reads_downloads():
    path = WindowsShellFolders().get(DL.guid)
    assert os.path.isabs(path)


def test_factory_builds_known_folder_strategy():
    from diskguard.models import RelocationRecord

    rec = RelocationRecord(1, "kf_downloads", "Downloads", "known_folder", "a", "b", 0, "done")
    assert isinstance(strategy_for_record(rec), KnownFolderStrategy)
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_relocation_known_folders.py -q` → `ModuleNotFoundError`.

- [ ] **Step 3: Create `diskguard/core/relocation/known_folders.py`**

```python
import ctypes
from ctypes import wintypes
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
from typing import Callable
import uuid

from diskguard.core.drives import is_on_system_drive
from diskguard.core.relocation.base import (
    FREE_SPACE_RESERVE,
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
    now_iso,
)
from diskguard.core.relocation.fs_ops import (
    RelocationError,
    clear_contents,
    remove_tree,
    verified_copy_tree,
)
from diskguard.core.task_controller import TaskControl
from diskguard.models import RELOCATION_DONE, RELOCATION_UNDONE, RelocationRecord


@dataclass(frozen=True)
class KnownFolder:
    app_id: str
    name: str
    guid: str


KNOWN_FOLDERS: dict[str, KnownFolder] = {
    kf.app_id: kf
    for kf in (
        KnownFolder("kf_downloads", "Downloads", "{374DE290-123F-4565-9164-39C4925E467B}"),
        KnownFolder("kf_documents", "Documents", "{FDD39AD0-238F-46AF-ADB4-6C85480369C7}"),
        KnownFolder("kf_pictures", "Pictures", "{33E28130-4E1E-4676-835A-98395C3BC3BB}"),
        KnownFolder("kf_videos", "Videos", "{18989B1D-99B5-455B-841C-AB7C74E4DDFC}"),
        KnownFolder("kf_music", "Music", "{4BD8D571-6D19-48D3-BE97-422220080E43}"),
        KnownFolder("kf_desktop", "Desktop", "{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}"),
    )
}


class _GUID(ctypes.Structure):
    _fields_ = [
        ("Data1", wintypes.DWORD),
        ("Data2", wintypes.WORD),
        ("Data3", wintypes.WORD),
        ("Data4", ctypes.c_ubyte * 8),
    ]


def _guid(text: str) -> _GUID:
    return _GUID.from_buffer_copy(uuid.UUID(text).bytes_le)


class WindowsShellFolders:
    """SHGetKnownFolderPath / SHSetKnownFolderPath — the API behind Explorer's Location tab."""

    def __init__(self):
        self._shell32 = ctypes.WinDLL("shell32")
        self._ole32 = ctypes.WinDLL("ole32")
        self._shell32.SHGetKnownFolderPath.argtypes = [
            ctypes.POINTER(_GUID), wintypes.DWORD, wintypes.HANDLE, ctypes.POINTER(ctypes.c_void_p)
        ]
        self._shell32.SHGetKnownFolderPath.restype = ctypes.c_long
        self._shell32.SHSetKnownFolderPath.argtypes = [
            ctypes.POINTER(_GUID), wintypes.DWORD, wintypes.HANDLE, wintypes.LPCWSTR
        ]
        self._shell32.SHSetKnownFolderPath.restype = ctypes.c_long
        self._ole32.CoTaskMemFree.argtypes = [ctypes.c_void_p]

    def get(self, guid: str) -> str:
        out = ctypes.c_void_p()
        hr = self._shell32.SHGetKnownFolderPath(ctypes.byref(_guid(guid)), 0, None, ctypes.byref(out))
        if hr != 0:
            raise OSError(f"SHGetKnownFolderPath failed: 0x{hr & 0xFFFFFFFF:08X}")
        try:
            return ctypes.wstring_at(out.value)
        finally:
            self._ole32.CoTaskMemFree(out)

    def set(self, guid: str, path: str) -> None:
        hr = self._shell32.SHSetKnownFolderPath(ctypes.byref(_guid(guid)), 0, None, path)
        if hr != 0:
            raise OSError(f"SHSetKnownFolderPath failed: 0x{hr & 0xFFFFFFFF:08X}")


def is_onedrive_path(path: str | Path) -> bool:
    norm = os.path.normcase(os.path.abspath(str(path)))
    for key in ("OneDrive", "OneDriveConsumer", "OneDriveCommercial"):
        root = os.environ.get(key)
        if not root:
            continue
        root_norm = os.path.normcase(os.path.abspath(root))
        if norm == root_norm or norm.startswith(root_norm + os.sep):
            return True
    return False


class KnownFolderStrategy(RelocationStrategy):
    kind = "known_folder"

    def __init__(
        self,
        folder: KnownFolder,
        shell=None,
        disk_usage: Callable = shutil.disk_usage,
        system_root: str | None = None,
    ):
        self.folder = folder
        self.shell = shell or WindowsShellFolders()
        self._disk_usage = disk_usage
        self._system_root = system_root

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        try:
            current = Path(self.shell.get(self.folder.guid))
        except OSError as err:
            return CheckResult(False, f"Không đọc được vị trí {self.folder.name}: {err}")
        if is_onedrive_path(current):
            return CheckResult(False, "Thư mục đang đồng bộ OneDrive")
        if not is_on_system_drive(current, self._system_root):
            return CheckResult(False, "Thư mục đã nằm ngoài ổ hệ thống")
        target = Path(target_root) / self.folder.name
        if target.exists() and any(target.iterdir()):
            return CheckResult(False, f"Thư mục đích đã có dữ liệu: {target}")
        anchor = Path(target_root).anchor or str(target_root)
        if self._disk_usage(anchor).free < cand.size_bytes + FREE_SPACE_RESERVE:
            return CheckResult(False, "Ổ đích không đủ dung lượng trống")
        return CheckResult(True)

    def apply(
        self, cand: RelocationCandidate, target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        res = self.check(cand, target_root)
        if not res.ok:
            raise RelocationError(res.reason)
        current = Path(self.shell.get(self.folder.guid))
        target = Path(target_root) / self.folder.name
        try:
            verified_copy_tree(current, target, ctrl, cand.size_bytes, f"Đang chuyển {self.folder.name}")
        except BaseException:
            remove_tree(target)
            raise
        try:
            self.shell.set(self.folder.guid, str(target))
        except OSError as err:
            remove_tree(target)
            raise RelocationError(f"Windows không cho đổi vị trí {self.folder.name}: {err}") from err
        leftovers = clear_contents(current)
        rec = RelocationRecord(
            id=None, app_id=self.folder.app_id, app_name=self.folder.name, strategy=self.kind,
            source_path=str(current), target_path=str(target), size_bytes=cand.size_bytes,
            status=RELOCATION_DONE, created_at=now_iso(),
            detail=(f"Còn {len(leftovers)} mục đang mở chưa xóa ở vị trí cũ" if leftovers else ""),
        )
        return save(rec)

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        source, target = Path(rec.source_path), Path(rec.target_path)
        verified_copy_tree(target, source, ctrl, rec.size_bytes, f"Đang trả {rec.app_name} về ổ C:")
        try:
            self.shell.set(self.folder.guid, str(source))
        except OSError as err:
            raise RelocationError(f"Windows không cho đổi vị trí {rec.app_name}: {err}") from err
        leftovers = remove_tree(target)
        rec.status = RELOCATION_UNDONE
        rec.detail = f"Còn {len(leftovers)} mục chưa xóa được ở {target}" if leftovers else ""
        return save(rec)
```

- [ ] **Step 4: Extend `strategy_for_record` in `service.py`**

Add import `from diskguard.core.relocation.known_folders import KNOWN_FOLDERS, KnownFolderStrategy` and, before the `raise`, insert:

```python
    if rec.strategy == "known_folder" and rec.app_id in KNOWN_FOLDERS:
        return KnownFolderStrategy(KNOWN_FOLDERS[rec.app_id])
```

- [ ] **Step 5: Run tests** — `pytest tests/test_relocation_known_folders.py -q` → PASS.

- [ ] **Step 6: Commit**

```bash
git add diskguard/core/relocation tests/test_relocation_known_folders.py
git commit -m "feat: relocate Windows known folders via SHSetKnownFolderPath"
```

---

### Task 7: npm / pip cache strategies

**Files:**
- Create: `diskguard/core/relocation/tool_config.py`
- Modify: `diskguard/core/relocation/service.py` (`strategy_for_record`)
- Test: `tests/test_relocation_tool_config.py`

**Interfaces:**
- Produces: `Runner = Callable[[list[str]], tuple[int, str]]`, `subprocess_runner(args)`, `ToolCacheStrategy(runner=subprocess_runner, which=shutil.which)` with `exe() -> str | None`, `current_cache() -> Path | None`; subclasses `NpmCacheStrategy` (`app_id="npm"`, `name="npm cache"`, `exe_name="npm"`) and `PipCacheStrategy` (`app_id="pip"`, `name="pip cache"`, `exe_name="pip"`); `TOOL_STRATEGIES = {"npm": NpmCacheStrategy, "pip": PipCacheStrategy}`.

- [ ] **Step 1: Write the failing test** — `tests/test_relocation_tool_config.py`

```python
from pathlib import Path

import pytest

from diskguard.core.relocation.base import RelocationCandidate
from diskguard.core.relocation.fs_ops import RelocationError
from diskguard.core.relocation.service import strategy_for_record
from diskguard.core.relocation.tool_config import NpmCacheStrategy, PipCacheStrategy
from diskguard.models import RELOCATION_DONE, RELOCATION_UNDONE, RelocationRecord


class FakeNpm:
    def __init__(self, cache: Path):
        self.cache = str(cache)
        self.default = str(cache)
        self.calls = []

    def __call__(self, args):
        self.calls.append(args[1:])
        if args[1:3] == ["config", "get"]:
            return 0, self.cache
        if args[1:3] == ["config", "set"]:
            self.cache = args[4]
            return 0, ""
        if args[1:3] == ["config", "delete"]:
            self.cache = self.default
            return 0, ""
        return 1, ""


def _saver(rec):
    rec.id = rec.id or 3
    return rec


def test_npm_switches_cache_and_deletes_old(tmp_path: Path):
    old = tmp_path / "C" / "npm-cache"
    (old / "_cacache").mkdir(parents=True)
    (old / "_cacache" / "blob").write_bytes(b"x" * 1000)
    runner = FakeNpm(old)
    st = NpmCacheStrategy(runner=runner, which=lambda n: "C:\\npm.cmd")
    assert st.current_cache() == old
    cand = RelocationCandidate("npm", "npm cache", str(old), 1000, st, "how")
    rec = st.apply(cand, tmp_path / "D" / "Green C", None, _saver)
    new = tmp_path / "D" / "Green C" / "Apps" / "npm"
    assert rec.status == RELOCATION_DONE and runner.cache == str(new) and new.is_dir()
    assert not old.exists()
    st.undo(rec, None, _saver)
    assert rec.status == RELOCATION_UNDONE and runner.cache == str(old)


def test_missing_tool_is_blocked(tmp_path: Path):
    st = PipCacheStrategy(runner=lambda a: (1, ""), which=lambda n: None)
    cand = RelocationCandidate("pip", "pip cache", str(tmp_path), 0, st, "how")
    assert not st.check(cand, tmp_path).ok
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path, None, _saver)


def test_set_that_does_not_stick_raises(tmp_path: Path):
    st = NpmCacheStrategy(runner=lambda a: (0, str(tmp_path / "old")), which=lambda n: "npm")
    (tmp_path / "old").mkdir()
    cand = RelocationCandidate("npm", "npm cache", str(tmp_path / "old"), 0, st, "how")
    with pytest.raises(RelocationError):
        st.apply(cand, tmp_path / "D", None, _saver)
    assert (tmp_path / "old").exists()


def test_pip_commands(tmp_path: Path):
    calls = []

    def runner(args):
        calls.append(args[1:])
        return 0, str(tmp_path / "pipcache") if args[1:] == ["cache", "dir"] else ""

    st = PipCacheStrategy(runner=runner, which=lambda n: "pip.exe")
    assert st.current_cache() == tmp_path / "pipcache"
    st._unset()
    assert ["config", "--user", "unset", "global.cache-dir"] in calls


def test_factory_builds_tool_strategy():
    rec = RelocationRecord(1, "npm", "npm cache", "tool_config", "a", "b", 0, "done")
    assert isinstance(strategy_for_record(rec), NpmCacheStrategy)
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_relocation_tool_config.py -q` → `ModuleNotFoundError`.

- [ ] **Step 3: Create `diskguard/core/relocation/tool_config.py`**

```python
from pathlib import Path
import shutil
import subprocess
from typing import Callable

from diskguard.core.relocation.base import (
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
    now_iso,
)
from diskguard.core.relocation.fs_ops import RelocationError, remove_tree, same_path
from diskguard.core.task_controller import TaskControl
from diskguard.models import RELOCATION_DONE, RELOCATION_UNDONE, RelocationRecord

Runner = Callable[[list[str]], tuple[int, str]]
_CREATE_NO_WINDOW = 0x08000000


def subprocess_runner(args: list[str]) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            args, capture_output=True, text=True, timeout=60, creationflags=_CREATE_NO_WINDOW
        )
    except (OSError, subprocess.TimeoutExpired):
        return 1, ""
    return proc.returncode, (proc.stdout or "").strip()


class ToolCacheStrategy(RelocationStrategy):
    """Point a developer tool's cache at the data drive; the old cache is simply deleted."""

    kind = "tool_config"
    app_id = ""
    name = ""
    exe_name = ""

    def __init__(self, runner: Runner = subprocess_runner, which: Callable = shutil.which):
        self._run = runner
        self._which = which

    def exe(self) -> str | None:
        return self._which(self.exe_name)

    def current_cache(self) -> Path | None:
        raise NotImplementedError

    def _set(self, path: Path) -> None:
        raise NotImplementedError

    def _unset(self) -> None:
        raise NotImplementedError

    def _call(self, *args: str) -> tuple[int, str]:
        exe = self.exe()
        if not exe:
            raise RelocationError(f"Không tìm thấy {self.exe_name}")
        return self._run([exe, *args])

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        if not self.exe():
            return CheckResult(False, f"Không tìm thấy {self.exe_name}")
        return CheckResult(True)

    def apply(
        self, cand: RelocationCandidate, target_root: Path, ctrl: TaskControl | None, save: Saver
    ) -> RelocationRecord:
        res = self.check(cand, target_root)
        if not res.ok:
            raise RelocationError(res.reason)
        old = self.current_cache()
        new = Path(target_root) / "Apps" / self.app_id
        new.mkdir(parents=True, exist_ok=True)
        self._set(new)
        now = self.current_cache()
        if now is None or not same_path(now, new):
            raise RelocationError(f"{self.exe_name} không nhận cấu hình mới")
        leftovers: list[str] = []
        if old is not None and old.exists() and not same_path(old, new):
            leftovers = remove_tree(old)
        return save(
            RelocationRecord(
                id=None, app_id=self.app_id, app_name=self.name, strategy=self.kind,
                source_path=str(old or cand.source_path), target_path=str(new),
                size_bytes=cand.size_bytes, status=RELOCATION_DONE, created_at=now_iso(),
                detail=f"Còn {len(leftovers)} mục chưa xóa được" if leftovers else "",
            )
        )

    def undo(self, rec: RelocationRecord, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        self._unset()
        remove_tree(Path(rec.target_path))
        rec.status = RELOCATION_UNDONE
        return save(rec)


class NpmCacheStrategy(ToolCacheStrategy):
    app_id = "npm"
    name = "npm cache"
    exe_name = "npm"

    def current_cache(self) -> Path | None:
        rc, out = self._call("config", "get", "cache")
        return Path(out) if rc == 0 and out else None

    def _set(self, path: Path) -> None:
        rc, _ = self._call("config", "set", "cache", str(path))
        if rc != 0:
            raise RelocationError("npm từ chối đổi thư mục cache")

    def _unset(self) -> None:
        self._call("config", "delete", "cache")


class PipCacheStrategy(ToolCacheStrategy):
    app_id = "pip"
    name = "pip cache"
    exe_name = "pip"

    def current_cache(self) -> Path | None:
        rc, out = self._call("cache", "dir")
        return Path(out) if rc == 0 and out else None

    def _set(self, path: Path) -> None:
        rc, _ = self._call("config", "--user", "set", "global.cache-dir", str(path))
        if rc != 0:
            raise RelocationError("pip từ chối đổi thư mục cache")

    def _unset(self) -> None:
        self._call("config", "--user", "unset", "global.cache-dir")


TOOL_STRATEGIES: dict[str, type[ToolCacheStrategy]] = {
    "npm": NpmCacheStrategy,
    "pip": PipCacheStrategy,
}
```

- [ ] **Step 4: Extend `strategy_for_record`** — import `TOOL_STRATEGIES` and insert before the `raise`:

```python
    if rec.strategy == "tool_config" and rec.app_id in TOOL_STRATEGIES:
        return TOOL_STRATEGIES[rec.app_id]()
```

- [ ] **Step 5: Run tests** — `pytest tests/test_relocation_tool_config.py -q` → PASS.

- [ ] **Step 6: Commit**

```bash
git add diskguard/core/relocation tests/test_relocation_tool_config.py
git commit -m "feat: move npm and pip caches to the data drive"
```

---

### Task 8: Zalo guided strategy and candidate discovery

**Files:**
- Create: `diskguard/core/relocation/zalo.py`, `diskguard/core/relocation/catalog.py`
- Modify: `diskguard/core/relocation/service.py` (add `complete_guided`)
- Test: `tests/test_relocation_catalog.py`

**Interfaces:**
- Produces: `ZaloGuidedStrategy(config_path=None, system_root=None, localappdata=None)` with `data_dir() -> Path | None`, `is_on_system_drive() -> bool`, `verify_done() -> bool`, `steps(target_root) -> list[str]`, `find_exe() -> str | None`, `open_app() -> bool`; `RelocationService.complete_guided(cand) -> RelocationRecord | None`; `MIN_CANDIDATE_BYTES = 200 * 1024**2`; `discover_candidates(db, system_root=None, appdata=None, localappdata=None, shell=None, runner=None, which=None, zalo_config=None, min_bytes=MIN_CANDIDATE_BYTES, should_stop=None, size_of=fast_dir_size) -> list[RelocationCandidate]`.

Note: Zalo's data location comes from `%APPDATA%\ZaloData\config.json` → `db_path.location` (verified on a real machine: `F:\Zalo Data\message`). When the key is absent, the default `%LOCALAPPDATA%\ZaloPC` is used if it exists.

- [ ] **Step 1: Write the failing test** — `tests/test_relocation_catalog.py`

```python
import json
import os
from pathlib import Path
import subprocess

from diskguard.core.relocation.catalog import discover_candidates
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.known_folders import KNOWN_FOLDERS, KnownFolderStrategy
from diskguard.core.relocation.service import RelocationService
from diskguard.core.relocation.zalo import ZaloGuidedStrategy
from diskguard.core.settings import AppSettings
from diskguard.models import RELOCATION_DONE, RelocationRecord
from diskguard.storage.database import DiskGuardDB

SYS = os.path.splitdrive(os.getcwd())[0].upper() + "\\"


class FakeShell:
    def __init__(self, paths):
        self.paths = paths

    def get(self, guid):
        if guid not in self.paths:
            raise OSError("no folder")
        return self.paths[guid]

    def set(self, guid, path):
        self.paths[guid] = path


def _fill(d: Path, size: int) -> Path:
    d.mkdir(parents=True, exist_ok=True)
    (d / "blob.bin").write_bytes(b"x" * size)
    return d


def _env(tmp_path: Path):
    appdata = tmp_path / "Roaming"
    local = tmp_path / "Local"
    _fill(appdata / "Telegram Desktop", 3000)
    _fill(appdata / "BigTool", 4000)
    _fill(appdata / "Tiny", 10)
    _fill(local / "Microsoft", 9000)  # skip list
    _fill(local / "Programs", 9000)  # skip list
    dl = _fill(tmp_path / "Users" / "me" / "Downloads", 5000)
    shell = FakeShell({KNOWN_FOLDERS["kf_downloads"].guid: str(dl)})
    zcfg = tmp_path / "ZaloData" / "config.json"
    zcfg.parent.mkdir()
    zalo_data = _fill(tmp_path / "ZaloPCData" / "Zalo Data", 6000)
    zcfg.write_text(json.dumps({"db_path": {"location": str(zalo_data / "message")}}), encoding="utf-8")
    return appdata, local, shell, zcfg, dl, zalo_data


def _discover(db, tmp_path, **kw):
    appdata, local, shell, zcfg, dl, zalo_data = _env(tmp_path)
    args = dict(
        system_root=SYS, appdata=appdata, localappdata=local, shell=shell,
        runner=lambda a: (1, ""), which=lambda n: None, zalo_config=zcfg, min_bytes=1000,
    )
    args.update(kw)
    return discover_candidates(db, **args), (appdata, local, shell, zcfg, dl, zalo_data)


def test_discovers_each_strategy_and_orders_advanced_last(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "c.db")
    cands, (appdata, *_rest) = _discover(db, tmp_path)
    by_id = {c.app_id: c for c in cands}
    assert isinstance(by_id["kf_downloads"].strategy, KnownFolderStrategy)
    assert isinstance(by_id["zalo"].strategy, ZaloGuidedStrategy)
    assert isinstance(by_id["telegram"].strategy, JunctionStrategy)
    assert not by_id["telegram"].advanced
    assert by_id["dir-bigtool"].advanced
    assert "dir-tiny" not in by_id and "dir-microsoft" not in by_id and "dir-programs" not in by_id
    flags = [c.advanced for c in cands]
    assert flags == sorted(flags)


def test_done_relocations_and_junctions_are_excluded(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "c.db")
    appdata = tmp_path / "Roaming"
    db.save_relocation(
        RelocationRecord(None, "telegram", "Telegram Desktop", "junction",
                         str(appdata / "Telegram Desktop"), "X", 1, RELOCATION_DONE)
    )
    outside = _fill(tmp_path / "outside", 5000)
    appdata.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cmd", "/c", "mklink", "/J", str(appdata / "Linked"), str(outside)], check=True, capture_output=True)
    cands, _ = _discover(db, tmp_path)
    ids = {c.app_id for c in cands}
    assert "telegram" not in ids and "dir-linked" not in ids


def test_zalo_off_system_drive_is_not_a_candidate(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "c.db")
    zcfg = tmp_path / "z.json"
    zcfg.write_text(json.dumps({"db_path": {"location": "Q:\\Zalo Data\\message"}}), encoding="utf-8")
    cands, _ = _discover(db, tmp_path, zalo_config=zcfg)
    assert "zalo" not in {c.app_id for c in cands}
    z = ZaloGuidedStrategy(config_path=zcfg, system_root=SYS, localappdata=tmp_path)
    assert z.verify_done()


def test_complete_guided_records_only_when_moved(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "c.db")
    cands, (_a, _l, _s, zcfg, _dl, zalo_data) = _discover(db, tmp_path)
    zalo = next(c for c in cands if c.app_id == "zalo")
    svc = RelocationService(db, AppSettings(db))
    assert svc.complete_guided(zalo) is None
    zcfg.write_text(json.dumps({"db_path": {"location": "Q:\\Zalo Data\\message"}}), encoding="utf-8")
    rec = svc.complete_guided(zalo)
    assert rec is not None and rec.status == RELOCATION_DONE and rec.target_path == "Q:\\Zalo Data"
    steps = zalo.strategy.steps(Path("D:\\Green C"))
    assert any("Quản lý dữ liệu" in s for s in steps) and any("D:\\Green C\\Zalo" in s for s in steps)
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_relocation_catalog.py -q` → `ModuleNotFoundError`.

- [ ] **Step 3: Create `diskguard/core/relocation/zalo.py`**

```python
import json
import os
from pathlib import Path
import subprocess

import psutil

from diskguard.core.drives import is_on_system_drive
from diskguard.core.relocation.base import (
    CheckResult,
    RelocationCandidate,
    RelocationStrategy,
    Saver,
)
from diskguard.core.relocation.fs_ops import RelocationError
from diskguard.core.task_controller import TaskControl
from diskguard.models import RelocationRecord


class ZaloGuidedStrategy(RelocationStrategy):
    """Zalo migrates its own database: guide the user through Zalo's setting, then verify."""

    kind = "guided"
    app_id = "zalo"

    def __init__(
        self,
        config_path: Path | str | None = None,
        system_root: str | None = None,
        localappdata: Path | str | None = None,
    ):
        self.config_path = (
            Path(config_path)
            if config_path
            else Path(os.path.expandvars(r"%APPDATA%\ZaloData\config.json"))
        )
        self.system_root = system_root
        self.localappdata = (
            Path(localappdata) if localappdata else Path(os.path.expandvars(r"%LOCALAPPDATA%"))
        )

    def data_dir(self) -> Path | None:
        if self.config_path.exists():
            try:
                data = json.loads(self.config_path.read_text(encoding="utf-8", errors="ignore"))
            except (OSError, ValueError):
                data = {}
            location = (data.get("db_path") or {}).get("location") if isinstance(data, dict) else None
            if location:
                return Path(location).parent
        default = self.localappdata / "ZaloPC"
        return default if default.is_dir() else None

    def is_on_system_drive(self) -> bool:
        d = self.data_dir()
        return d is not None and is_on_system_drive(d, self.system_root)

    def verify_done(self) -> bool:
        d = self.data_dir()
        return d is not None and not is_on_system_drive(d, self.system_root)

    def steps(self, target_root: Path) -> list[str]:
        return [
            "Mở Zalo trên máy tính.",
            "Bấm biểu tượng Cài đặt ở góc dưới bên trái, chọn Dữ liệu.",
            "Chọn Quản lý dữ liệu, bấm Thay đổi ở phần vị trí lưu.",
            f"Chọn thư mục {Path(target_root) / 'Zalo'} rồi bấm Di chuyển.",
            "Đợi Zalo chuyển xong, quay lại đây và bấm Kiểm tra lại.",
        ]

    def find_exe(self) -> str | None:
        for proc in psutil.process_iter(["name", "exe"]):
            try:
                if (proc.info.get("name") or "").lower() == "zalo.exe" and proc.info.get("exe"):
                    return proc.info["exe"]
            except (psutil.Error, OSError):
                continue
        exe = self.localappdata / "Programs" / "Zalo" / "Zalo.exe"
        return str(exe) if exe.exists() else None

    def open_app(self) -> bool:
        exe = self.find_exe()
        if not exe:
            return False
        subprocess.Popen([exe])
        return True

    def check(self, cand: RelocationCandidate, target_root: Path) -> CheckResult:
        return CheckResult(True)

    def apply(self, cand, target_root, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        raise RelocationError("Zalo cần được chuyển bằng tùy chọn trong ứng dụng Zalo")

    def undo(self, rec, ctrl: TaskControl | None, save: Saver) -> RelocationRecord:
        raise RelocationError("Đổi lại vị trí lưu trong Cài đặt của Zalo")
```

- [ ] **Step 4: Add `complete_guided` to `RelocationService`** (imports: `RELOCATION_DONE` already imported; add `from diskguard.core.relocation.base import now_iso`)

```python
    def complete_guided(self, cand: RelocationCandidate) -> RelocationRecord | None:
        strategy = cand.strategy
        if not hasattr(strategy, "verify_done") or not strategy.verify_done():
            return None
        return self._save(
            RelocationRecord(
                id=None, app_id=cand.app_id, app_name=cand.name, strategy=strategy.kind,
                source_path=cand.source_path, target_path=str(strategy.data_dir()),
                size_bytes=cand.size_bytes, status=RELOCATION_DONE, created_at=now_iso(),
            )
        )
```

- [ ] **Step 5: Create `diskguard/core/relocation/catalog.py`**

```python
import os
from pathlib import Path
import re
import shutil
from typing import Callable

from diskguard.core.drives import is_on_system_drive, system_drive_root
from diskguard.core.relocation.base import RelocationCandidate
from diskguard.core.relocation.fs_ops import fast_dir_size, is_reparse_point
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.known_folders import (
    KNOWN_FOLDERS,
    KnownFolderStrategy,
    WindowsShellFolders,
    is_onedrive_path,
)
from diskguard.core.relocation.tool_config import (
    NpmCacheStrategy,
    PipCacheStrategy,
    subprocess_runner,
)
from diskguard.core.relocation.zalo import ZaloGuidedStrategy
from diskguard.models import RELOCATION_DONE
from diskguard.storage.database import DiskGuardDB

MIN_CANDIDATE_BYTES = 200 * 1024**2

# Top-level %APPDATA% / %LOCALAPPDATA% folders never offered for junction relocation.
ADVANCED_SKIP = {
    "microsoft", "packages", "programs", "temp", "windows", "comms",
    "connecteddevicesplatform", "d3dscache", "crashdumps", "green c",
    "zalodata", "zalopc", "zalo", "telegram desktop",
}


def _slug(name: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-") or "app"


def _norm(p: str | Path) -> str:
    return os.path.normcase(os.path.abspath(str(p)))


def discover_candidates(
    db: DiskGuardDB,
    system_root: str | None = None,
    appdata: Path | str | None = None,
    localappdata: Path | str | None = None,
    shell=None,
    runner=None,
    which: Callable | None = None,
    zalo_config: Path | str | None = None,
    min_bytes: int = MIN_CANDIDATE_BYTES,
    should_stop: Callable[[], bool] | None = None,
    size_of: Callable = fast_dir_size,
) -> list[RelocationCandidate]:
    sys_root = system_root or system_drive_root()
    appdata = Path(appdata) if appdata else Path(os.path.expandvars(r"%APPDATA%"))
    localappdata = Path(localappdata) if localappdata else Path(os.path.expandvars(r"%LOCALAPPDATA%"))
    shell = shell or WindowsShellFolders()
    done = {_norm(r.source_path) for r in db.list_relocations(status=RELOCATION_DONE)}
    stop = should_stop or (lambda: False)
    cands: list[RelocationCandidate] = []

    def eligible(path: Path) -> bool:
        return (
            path.is_dir()
            and not is_reparse_point(path)
            and is_on_system_drive(path, sys_root)
            and _norm(path) not in done
        )

    # 1. Windows known folders
    for kf in KNOWN_FOLDERS.values():
        if stop():
            return cands
        try:
            current = Path(shell.get(kf.guid))
        except OSError:
            continue
        if not eligible(current) or is_onedrive_path(current):
            continue
        size = size_of(current, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                kf.app_id, kf.name, str(current), size,
                KnownFolderStrategy(kf, shell=shell, system_root=sys_root),
                "Dùng tính năng Vị trí của Windows",
            ))

    # 2. Developer tool caches
    for cls in (NpmCacheStrategy, PipCacheStrategy):
        if stop():
            return cands
        st = cls(runner=runner or subprocess_runner, which=which or shutil.which)
        if st.exe() is None:
            continue
        cache = st.current_cache()
        if cache is None or not eligible(cache):
            continue
        size = size_of(cache, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                st.app_id, st.name, str(cache), size, st,
                f"Đổi cấu hình {st.exe_name}, xóa bộ đệm cũ",
            ))

    # 3. Zalo (guided)
    zalo = ZaloGuidedStrategy(config_path=zalo_config, system_root=sys_root, localappdata=localappdata)
    zalo_dir = zalo.data_dir()
    if zalo_dir is not None and eligible(zalo_dir):
        size = size_of(zalo_dir, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                "zalo", "Zalo", str(zalo_dir), size, zalo,
                "Dùng tùy chọn lưu trữ của Zalo · Hướng dẫn từng bước, tự kiểm tra sau khi đổi",
            ))

    # 4. Curated junction apps
    telegram = appdata / "Telegram Desktop"
    if eligible(telegram):
        size = size_of(telegram, should_stop)
        if size >= min_bytes:
            cands.append(RelocationCandidate(
                "telegram", "Telegram Desktop", str(telegram), size,
                JunctionStrategy(["Telegram.exe"]),
                "Chuyển thư mục và tạo liên kết · Cần đóng Telegram",
            ))

    # 5. Advanced: other large app folders (junction, unchecked by default)
    taken = {_norm(c.source_path) for c in cands}
    for base in (appdata, localappdata):
        if not base.is_dir():
            continue
        for child in sorted(base.iterdir(), key=lambda p: p.name.lower()):
            if stop():
                return cands
            if child.name.lower() in ADVANCED_SKIP or _norm(child) in taken or not eligible(child):
                continue
            size = size_of(child, should_stop)
            if size >= min_bytes:
                cands.append(RelocationCandidate(
                    f"dir-{_slug(child.name)}", child.name, str(child), size,
                    JunctionStrategy([]),
                    "Chuyển thư mục và tạo liên kết · Cần đóng ứng dụng đang dùng thư mục này",
                    advanced=True,
                ))

    cands.sort(key=lambda c: (c.advanced, -c.size_bytes))
    return cands
```

- [ ] **Step 6: Run tests** — `pytest tests/test_relocation_catalog.py -q` → PASS; full `pytest -q -x` → PASS.

- [ ] **Step 7: Commit**

```bash
git add diskguard/core/relocation tests/test_relocation_catalog.py
git commit -m "feat: Zalo guided relocation and relocation candidate discovery"
```

---

### Task 9: Theme, icons, reusable widgets

**Files:**
- Create: `diskguard/ui/theme.py`, `diskguard/ui/icons.py`, `diskguard/ui/widgets.py`
- Modify: `diskguard/ui/__init__.py` (add `format_size`)
- Test: `tests/test_ui_widgets.py`

**Interfaces:**
- Produces: `theme` tokens (`ACCENT, BRAND_GREEN, DANGER, TEXT, TEXT_MUTED, BORDER, SURFACE, BG_SHELL, BG_CONTENT, SELECTION, WARNING_BG, WARNING_FG`), `theme.QSS`, `theme.apply_theme(target)`, `theme.repolish(widget)`.
- Produces: `icons.icon(name, color=TEXT_MUTED, size=16) -> QIcon`, `icons.app_icon() -> QIcon`; names `eye, copy, folder, check, warning, drive`.
- Produces: `format_size(num_bytes) -> str` (`"18,4 GB"`).
- Produces (widgets): `icon_button(name, tooltip, checkable=False) -> QToolButton`, `page_title(text)`, `card_title(text)`, `muted_label(text)`, `make_card() -> QFrame`, `make_table(headers, stretch_col, right_cols=()) -> QTableWidget`, `fit_table_height(table, max_rows=12)`, `open_in_explorer(path) -> bool`, `Toast.show_message(anchor, text, ms=1200)`, `PathCell(path)` (`.path`, `.label`, `.copy_button`, `.open_button`, `.copy_to_clipboard()`), `PathValue(path)`, `TagValue(text, warning=False)`, `TableRow(simple: tuple[str, str, str], detail: list, sort_key: int = 0)`, `set_cell(table, row, col, value, right=False)`, `ExpandableTableCard(title, detail_headers, right_detail_cols=())` (`.eye_button`, `.simple_table`, `.detail_table`, `.header_actions`, `.set_rows(rows)`, `.set_empty_text(text)`, `.is_expanded()`, `.set_expanded(on)`).

- [ ] **Step 1: Write the failing test** — `tests/test_ui_widgets.py`

```python
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import QApplication

from diskguard.ui import format_size
from diskguard.ui.icons import icon
from diskguard.ui.widgets import ExpandableTableCard, PathCell, PathValue, TableRow, TagValue


def _app():
    return QApplication.instance() or QApplication([])


def test_format_size_vietnamese():
    assert format_size(0) == "0 B"
    assert format_size(1536) == "2 KB"
    assert format_size(int(18.4 * 1024**3)) == "18,4 GB"
    assert format_size(5 * 1024**2) == "5,0 MB"


def test_icons_render():
    _app()
    for name in ("eye", "copy", "folder", "check", "warning", "drive"):
        assert not icon(name).isNull()


def test_path_cell_copies_full_path():
    _app()
    path = r"C:\Users\duong\AppData\Roaming\Telegram Desktop\tdata\very\long\folder"
    cell = PathCell(path)
    cell.resize(120, 28)
    cell.show()
    assert cell.label.toolTip() == path
    cell.copy_button.click()
    assert QGuiApplication.clipboard().text() == path
    assert cell.open_button.toolTip() == "Mở trong Explorer"


def test_expandable_card_toggles_between_simple_and_detail():
    _app()
    card = ExpandableTableCard("Có thể giải phóng", ["Ứng dụng", "Vị trí", "Dung lượng", "Giải phóng", "Đề xuất"], right_detail_cols=(2, 3))
    card.set_rows([
        TableRow(("Zalo", "Chuyển sang D:", "32,1 GB"),
                 ["Zalo", PathValue(r"C:\Zalo"), "34,0 GB", "32,1 GB", TagValue("Chuyển sang D:")], 32),
        TableRow(("DingTalk", "Dọn file cũ", "1,9 GB"),
                 ["DingTalk", PathValue(r"C:\DT"), "3,3 GB", "1,9 GB", TagValue("Đang chạy", warning=True)], 2),
    ])
    assert not card.is_expanded()
    assert card.simple_table.rowCount() == 2
    assert card.simple_table.item(0, 0).text() == "Zalo"
    card.eye_button.click()
    assert card.is_expanded()
    assert card.detail_table.rowCount() == 2
    assert isinstance(card.detail_table.cellWidget(0, 1), PathCell)
    card.eye_button.click()
    assert not card.is_expanded()


def test_empty_card_shows_hint():
    _app()
    card = ExpandableTableCard("X", ["A"])
    card.set_empty_text("Chưa phân tích.")
    card.set_rows([])
    assert card.empty_label.text() == "Chưa phân tích."
    assert not card.empty_label.isHidden()
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_ui_widgets.py -q` → `ImportError`.

- [ ] **Step 3: Add `format_size` to `diskguard/ui/__init__.py`** (keep `format_bytes` unchanged)

```python
def format_size(num_bytes: int | float) -> str:
    """Vietnamese size string: 0 B, 2 KB, 5,0 MB, 18,4 GB."""
    val = float(num_bytes)
    sign = "-" if val < 0 else ""
    val = abs(val)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if val < 1024.0 or unit == "TB":
            if unit in ("B", "KB"):
                return f"{sign}{val:.0f} {unit}"
            return f"{sign}{val:.1f}".replace(".", ",") + f" {unit}"
        val /= 1024.0
    return f"{sign}{val:.1f} TB"
```

- [ ] **Step 4: Create `diskguard/ui/theme.py`**

```python
from PySide6.QtWidgets import QWidget

ACCENT = "#0067C0"
ACCENT_HOVER = "#005BA9"
ACCENT_PRESSED = "#004F94"
BRAND_GREEN = "#107C10"
DANGER = "#C42B1C"
WARNING_BG = "#FFF4E5"
WARNING_FG = "#8A5300"
TEXT = "#1B1B1B"
TEXT_MUTED = "#616161"
BG_SHELL = "#F3F3F3"
BG_CONTENT = "#FAFAFA"
SURFACE = "#FFFFFF"
BORDER = "#E5E5E5"
BORDER_STRONG = "#D1D1D1"
NAV_HOVER = "#EAEAEA"
NAV_SELECTED = "#E5E5E5"
SELECTION = "#E5F0FB"
FONT_FAMILY = '"Segoe UI Variable Text", "Segoe UI"'
MONO_FAMILY = '"Cascadia Mono", Consolas'

QSS = f"""
* {{ font-family: {FONT_FAMILY}; font-size: 13px; color: {TEXT}; }}
QMainWindow, QDialog {{ background: {BG_CONTENT}; }}
QFrame#Sidebar {{ background: {BG_SHELL}; border: none; border-right: 1px solid {BORDER}; }}
QWidget#Content, QWidget#PageBody {{ background: {BG_CONTENT}; }}
QScrollArea#PageScroll {{ background: {BG_CONTENT}; border: none; }}
QLabel#Brand {{ font-size: 17px; font-weight: 600; padding: 2px 10px 16px 10px; }}
QLabel#AuthorBlock {{ color: {TEXT_MUTED}; font-size: 12px; padding: 10px 10px 2px 10px; border-top: 1px solid {BORDER}; }}
QPushButton#NavButton {{ background: transparent; border: none; border-radius: 4px; padding: 8px 12px; text-align: left; }}
QPushButton#NavButton:hover {{ background: {NAV_HOVER}; }}
QPushButton#NavButton:checked {{ background: {NAV_SELECTED}; font-weight: 600; border-left: 3px solid {ACCENT}; padding-left: 9px; }}
QLabel#PageTitle {{ font-size: 20px; font-weight: 600; }}
QLabel#CardTitle {{ font-size: 14px; font-weight: 600; }}
QLabel#Muted {{ color: {TEXT_MUTED}; }}
QLabel#PathText {{ font-family: {MONO_FAMILY}; font-size: 12px; color: {TEXT_MUTED}; }}
QLabel#Tag {{ background: {SELECTION}; color: #0B5394; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QLabel#TagWarning {{ background: {WARNING_BG}; color: {WARNING_FG}; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QLabel#TagSuccess {{ background: #E9F5E9; color: {BRAND_GREEN}; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QLabel#TagNeutral {{ background: #F0F0F0; color: {TEXT_MUTED}; border-radius: 9px; padding: 1px 8px; font-size: 12px; }}
QFrame#Card {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 6px; }}
QFrame#Banner {{ background: {WARNING_BG}; border: 1px solid #F3D9B1; border-radius: 6px; }}
QPushButton {{ background: {SURFACE}; border: 1px solid {BORDER_STRONG}; border-radius: 4px; padding: 5px 14px; }}
QPushButton:hover {{ background: #F6F6F6; }}
QPushButton:pressed {{ background: #EDEDED; }}
QPushButton:disabled {{ color: #A0A0A0; background: {BG_CONTENT}; border-color: {BORDER}; }}
QPushButton#PrimaryButton {{ background: {ACCENT}; color: #FFFFFF; border: 1px solid {ACCENT}; font-weight: 600; }}
QPushButton#PrimaryButton:hover {{ background: {ACCENT_HOVER}; }}
QPushButton#PrimaryButton:pressed {{ background: {ACCENT_PRESSED}; }}
QPushButton#PrimaryButton:disabled {{ background: #9DC1E4; border-color: #9DC1E4; color: #FFFFFF; }}
QToolButton#IconButton {{ background: transparent; border: none; border-radius: 4px; padding: 3px; }}
QToolButton#IconButton:hover {{ background: #F0F0F0; }}
QToolButton#IconButton:checked {{ background: {SELECTION}; }}
QTableWidget {{ background: {SURFACE}; border: none; gridline-color: transparent; selection-background-color: {SELECTION}; selection-color: {TEXT}; }}
QTableWidget::item {{ padding: 4px 8px; border-bottom: 1px solid #F0F0F0; }}
QHeaderView::section {{ background: {SURFACE}; color: {TEXT_MUTED}; font-weight: 600; font-size: 12px; padding: 6px 8px; border: none; border-bottom: 1px solid {BORDER}; }}
QProgressBar {{ background: {BORDER}; border: none; border-radius: 3px; max-height: 6px; min-height: 6px; }}
QProgressBar::chunk {{ background: {ACCENT}; border-radius: 3px; }}
QProgressBar#DangerBar::chunk {{ background: {DANGER}; }}
QComboBox, QLineEdit, QTextEdit, QSpinBox, QDoubleSpinBox {{ background: {SURFACE}; border: 1px solid {BORDER_STRONG}; border-radius: 4px; padding: 4px 8px; }}
QComboBox:focus, QLineEdit:focus, QTextEdit:focus, QSpinBox:focus, QDoubleSpinBox:focus {{ border-bottom: 2px solid {ACCENT}; }}
QFrame#DriveOption {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 6px; }}
QFrame#DriveOption[selected="true"] {{ border: 2px solid {ACCENT}; background: #F6FAFF; }}
QFrame#StatusBar {{ background: {SURFACE}; border-top: 1px solid {BORDER}; }}
QFrame#DialogFooter {{ background: #F7F7F7; border-top: 1px solid {BORDER}; }}
QLabel#Toast {{ background: #1B1B1B; color: #FFFFFF; border-radius: 4px; padding: 6px 12px; }}
QCheckBox::indicator {{ width: 16px; height: 16px; }}
"""


def apply_theme(target) -> None:
    """Apply the single app stylesheet to a QApplication or a top-level widget."""
    target.setStyleSheet(QSS)


def repolish(widget: QWidget) -> None:
    widget.style().unpolish(widget)
    widget.style().polish(widget)
    widget.update()
```

- [ ] **Step 5: Create `diskguard/ui/icons.py`**

```python
from PySide6.QtCore import QByteArray, Qt
from PySide6.QtGui import QIcon, QPainter, QPixmap
from PySide6.QtSvg import QSvgRenderer

from diskguard.ui.theme import BRAND_GREEN, TEXT_MUTED

_PATHS = {
    "eye": '<path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8s-2.4 4.5-6.5 4.5S1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/>',
    "copy": '<rect x="5" y="5" width="8.5" height="8.5" rx="1.5"/><path d="M3.5 10.5H3A1.5 1.5 0 0 1 1.5 9V3A1.5 1.5 0 0 1 3 1.5h6A1.5 1.5 0 0 1 10.5 3v.5"/>',
    "folder": '<path d="M1.5 4.5V12a1 1 0 0 0 1 1h11a1 1 0 0 0 1-1V6a1 1 0 0 0-1-1H8L6.5 3.5h-4a1 1 0 0 0-1 1z"/>',
    "check": '<path d="M3 8.5l3 3 7-7"/>',
    "warning": '<path d="M8 1.8l6.5 11.7h-13z"/><path d="M8 6v3.5M8 11.6v.1"/>',
    "drive": '<rect x="1.5" y="4" width="13" height="8" rx="1.5"/><path d="M11.5 8h.1M4 8h4"/>',
}


def _svg(name: str, color: str) -> bytes:
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" '
        f'stroke="{color}" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">'
        f"{_PATHS[name]}</svg>"
    ).encode("utf-8")


def icon(name: str, color: str = TEXT_MUTED, size: int = 16) -> QIcon:
    renderer = QSvgRenderer(QByteArray(_svg(name, color)))
    scale = 2
    pixmap = QPixmap(size * scale, size * scale)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    renderer.render(painter)
    painter.end()
    pixmap.setDevicePixelRatio(scale)
    return QIcon(pixmap)


def app_icon() -> QIcon:
    return icon("drive", BRAND_GREEN, 32)
```

- [ ] **Step 6: Create `diskguard/ui/widgets.py`**

```python
from dataclasses import dataclass, field
import os
from pathlib import Path
import subprocess

from PySide6.QtCore import QSize, Qt, QTimer
from PySide6.QtGui import QColor, QGuiApplication
from PySide6.QtWidgets import (
    QFrame,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QSizePolicy,
    QStackedWidget,
    QTableWidget,
    QTableWidgetItem,
    QToolButton,
    QVBoxLayout,
    QWidget,
)

from diskguard.ui.icons import icon


def icon_button(name: str, tooltip: str, checkable: bool = False) -> QToolButton:
    btn = QToolButton()
    btn.setObjectName("IconButton")
    btn.setIcon(icon(name))
    btn.setIconSize(QSize(16, 16))
    btn.setToolTip(tooltip)
    btn.setCheckable(checkable)
    btn.setAutoRaise(True)
    btn.setCursor(Qt.CursorShape.PointingHandCursor)
    btn.setFixedSize(26, 26)
    return btn


def _label(text: str, name: str) -> QLabel:
    lbl = QLabel(text)
    lbl.setObjectName(name)
    return lbl


def page_title(text: str) -> QLabel:
    return _label(text, "PageTitle")


def card_title(text: str) -> QLabel:
    return _label(text, "CardTitle")


def muted_label(text: str, wrap: bool = True) -> QLabel:
    lbl = _label(text, "Muted")
    lbl.setWordWrap(wrap)
    return lbl


def make_card() -> QFrame:
    card = QFrame()
    card.setObjectName("Card")
    return card


def make_table(headers: list[str], stretch_col: int, right_cols: tuple[int, ...] = ()) -> QTableWidget:
    table = QTableWidget(0, len(headers))
    table.setHorizontalHeaderLabels(headers)
    table.verticalHeader().setVisible(False)
    table.setShowGrid(False)
    table.setFocusPolicy(Qt.FocusPolicy.NoFocus)
    table.setSelectionMode(QTableWidget.SelectionMode.NoSelection)
    table.setEditTriggers(QTableWidget.EditTrigger.NoEditTriggers)
    table.setVerticalScrollMode(QTableWidget.ScrollMode.ScrollPerPixel)
    header = table.horizontalHeader()
    header.setHighlightSections(False)
    for col in range(len(headers)):
        mode = QHeaderView.ResizeMode.Stretch if col == stretch_col else QHeaderView.ResizeMode.ResizeToContents
        header.setSectionResizeMode(col, mode)
        if col in right_cols:
            item = table.horizontalHeaderItem(col)
            item.setTextAlignment(int(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter))
    table.verticalHeader().setDefaultSectionSize(36)
    return table


def fit_table_height(table: QTableWidget, max_rows: int = 12) -> None:
    rows = min(table.rowCount(), max_rows)
    header_h = table.horizontalHeader().height() if table.horizontalHeader().isVisible() else 0
    height = header_h + sum(table.rowHeight(r) for r in range(rows)) + 4
    table.setFixedHeight(max(height, header_h + 8))


def open_in_explorer(path: str) -> bool:
    p = Path(path)
    try:
        if p.is_file():
            subprocess.Popen(["explorer", "/select,", str(p)])
            return True
        target = p if p.is_dir() else p.parent
        if target.is_dir():
            os.startfile(str(target))
            return True
    except OSError:
        pass
    return False


class Toast(QLabel):
    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self.setObjectName("Toast")
        self.hide()

    @staticmethod
    def show_message(anchor: QWidget, text: str, ms: int = 1200) -> None:
        win = anchor.window()
        toast = win.findChild(Toast) or Toast(win)
        toast.setText(text)
        toast.adjustSize()
        toast.move(max(8, win.width() - toast.width() - 24), max(8, win.height() - toast.height() - 24))
        toast.raise_()
        toast.show()
        QTimer.singleShot(ms, toast.hide)


class PathCell(QWidget):
    """Elided path + copy + open-in-Explorer buttons; full path in the tooltip."""

    def __init__(self, path: str, parent: QWidget | None = None):
        super().__init__(parent)
        self.path = path
        lay = QHBoxLayout(self)
        lay.setContentsMargins(8, 0, 4, 0)
        lay.setSpacing(2)
        self.label = QLabel()
        self.label.setObjectName("PathText")
        self.label.setToolTip(path)
        self.label.setMinimumWidth(40)
        self.label.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        self.copy_button = icon_button("copy", "Sao chép đường dẫn")
        self.copy_button.clicked.connect(self.copy_to_clipboard)
        self.open_button = icon_button("folder", "Mở trong Explorer")
        self.open_button.clicked.connect(lambda: open_in_explorer(self.path))
        lay.addWidget(self.label, 1)
        lay.addWidget(self.copy_button)
        lay.addWidget(self.open_button)
        self._update_elide()

    def copy_to_clipboard(self) -> None:
        QGuiApplication.clipboard().setText(self.path)
        Toast.show_message(self, "Đã sao chép đường dẫn")

    def resizeEvent(self, event) -> None:  # noqa: N802
        super().resizeEvent(event)
        self._update_elide()

    def _update_elide(self) -> None:
        width = max(40, self.label.width())
        self.label.setText(self.label.fontMetrics().elidedText(self.path, Qt.TextElideMode.ElideMiddle, width))


@dataclass
class PathValue:
    path: str


@dataclass
class TagValue:
    text: str
    warning: bool = False
    tooltip: str = ""


@dataclass
class TableRow:
    simple: tuple[str, str, str]  # name, summary, size
    detail: list = field(default_factory=list)  # str | PathValue | TagValue | QWidget
    sort_key: int = 0


def _tag_widget(value: TagValue) -> QWidget:
    wrap = QWidget()
    lay = QHBoxLayout(wrap)
    lay.setContentsMargins(8, 0, 8, 0)
    tag = QLabel(value.text)
    tag.setObjectName("TagWarning" if value.warning else "Tag")
    if value.tooltip:
        tag.setToolTip(value.tooltip)
    lay.addWidget(tag)
    lay.addStretch(1)
    return wrap


def set_cell(table: QTableWidget, row: int, col: int, value, right: bool = False) -> None:
    if isinstance(value, PathValue):
        item = QTableWidgetItem(value.path)  # keeps text for sorting/tests; hidden behind the widget
        item.setForeground(QColor(0, 0, 0, 0))
        table.setItem(row, col, item)
        table.setCellWidget(row, col, PathCell(value.path))
    elif isinstance(value, TagValue):
        table.setItem(row, col, QTableWidgetItem(""))
        table.setCellWidget(row, col, _tag_widget(value))
    elif isinstance(value, QWidget):
        table.setCellWidget(row, col, value)
    else:
        item = QTableWidgetItem(str(value))
        if right:
            item.setTextAlignment(int(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter))
        table.setItem(row, col, item)


class ExpandableTableCard(QFrame):
    """Card with a compact list (default) and a detailed table behind an eye toggle."""

    def __init__(self, title: str, detail_headers: list[str], right_detail_cols: tuple[int, ...] = (), parent=None):
        super().__init__(parent)
        self.setObjectName("Card")
        self._right_cols = right_detail_cols
        lay = QVBoxLayout(self)
        lay.setContentsMargins(14, 10, 14, 10)
        lay.setSpacing(6)

        head = QHBoxLayout()
        self.title_label = card_title(title)
        head.addWidget(self.title_label)
        head.addStretch(1)
        self.header_actions = QHBoxLayout()
        self.header_actions.setSpacing(6)
        head.addLayout(self.header_actions)
        self.eye_button = icon_button("eye", "Xem chi tiết", checkable=True)
        self.eye_button.toggled.connect(self.set_expanded)
        head.addWidget(self.eye_button)
        lay.addLayout(head)

        self.empty_label = muted_label("")
        lay.addWidget(self.empty_label)

        self.stack = QStackedWidget()
        self.simple_table = make_table(["", "", ""], stretch_col=1, right_cols=(2,))
        self.simple_table.horizontalHeader().setVisible(False)
        self.detail_table = make_table(detail_headers, stretch_col=1, right_cols=right_detail_cols)
        self.stack.addWidget(self.simple_table)
        self.stack.addWidget(self.detail_table)
        lay.addWidget(self.stack)

    def set_empty_text(self, text: str) -> None:
        self.empty_label.setText(text)

    def set_rows(self, rows: list[TableRow]) -> None:
        empty = not rows
        self.empty_label.setHidden(not empty)
        self.stack.setHidden(empty)
        self.eye_button.setHidden(empty)
        self.simple_table.setRowCount(len(rows))
        self.detail_table.setRowCount(len(rows))
        for r, row in enumerate(rows):
            name, summary, size = row.simple
            set_cell(self.simple_table, r, 0, name)
            item = QTableWidgetItem(summary)
            item.setForeground(QColor("#616161"))
            self.simple_table.setItem(r, 1, item)
            set_cell(self.simple_table, r, 2, size, right=True)
            for c, value in enumerate(row.detail):
                set_cell(self.detail_table, r, c, value, right=c in self._right_cols)
        fit_table_height(self.simple_table)
        fit_table_height(self.detail_table)

    def is_expanded(self) -> bool:
        return self.stack.currentIndex() == 1

    def set_expanded(self, on: bool) -> None:
        self.stack.setCurrentIndex(1 if on else 0)
        if self.eye_button.isChecked() != on:
            self.eye_button.blockSignals(True)
            self.eye_button.setChecked(on)
            self.eye_button.blockSignals(False)
        self.eye_button.setToolTip("Ẩn chi tiết" if on else "Xem chi tiết")
```

- [ ] **Step 7: Run tests** — `pytest tests/test_ui_widgets.py -q` → PASS.

- [ ] **Step 8: Commit**

```bash
git add diskguard/ui/theme.py diskguard/ui/icons.py diskguard/ui/widgets.py diskguard/ui/__init__.py tests/test_ui_widgets.py
git commit -m "feat: Fluent theme, line icons, PathCell and expandable table card"
```

---

### Task 10: Data location widget and first-run dialog

**Files:**
- Create: `diskguard/ui/data_location.py`
- Test: `tests/test_ui_first_run.py`

**Interfaces:**
- Consumes: Task 2 `DriveInfo`, `recommend_drive`, `data_root_for`; Task 1 `AppSettings`; Task 9 widgets/theme.
- Produces: `DataLocationWidget(drives, current_root, retention_days)` (`.options: list[DriveOption]`, `.retention_spin`, `.path_label`, `.selected_drive()`, `.data_root()`, `.retention_days()`, signal `changed`); `FirstRunDialog(drives, current_root=None, retention_days=7, parent=None, title="Chọn nơi lưu dữ liệu", allow_later=True)` (`.location`, `.btn_ok`, `.btn_later`, `.chosen_root()`, `.chosen_retention()`); `run_first_run(settings, drives, parent=None, exec_fn=None) -> Path | None`.

- [ ] **Step 1: Write the failing test** — `tests/test_ui_first_run.py`

```python
import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from diskguard.core.drives import DriveInfo
from diskguard.core.settings import AppSettings
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.data_location import FirstRunDialog, run_first_run

GB = 1024**3
DRIVES = [
    DriveInfo("C:\\", "Windows", "NTFS", 237 * GB, 18 * GB, True, True),
    DriveInfo("D:\\", "Data", "NTFS", 931 * GB, 612 * GB, False, True),
    DriveInfo("E:\\", "USB", "FAT32", 64 * GB, 60 * GB, False, False, "Ổ di động hoặc ổ mạng"),
]


def _app():
    return QApplication.instance() or QApplication([])


def test_dialog_preselects_recommended_and_disables_ineligible():
    _app()
    dlg = FirstRunDialog(DRIVES)
    loc = dlg.location
    assert loc.selected_drive().root == "D:\\"
    assert loc.data_root() == Path("D:\\Green C")
    assert "D:\\Green C" in loc.path_label.text()
    assert not loc.options[2].isEnabled()
    assert loc.retention_spin.value() == 7
    loc.options[0].radio.setChecked(True)
    assert loc.data_root() == Path("C:\\Green C")


def test_run_first_run_accept_saves_choice(tmp_path):
    _app()
    settings = AppSettings(DiskGuardDB(tmp_path / "f.db"))
    root = run_first_run(settings, DRIVES, exec_fn=lambda dlg: (dlg.location.retention_spin.setValue(14), True)[1])
    assert root == Path("D:\\Green C")
    assert settings.data_root == Path("D:\\Green C")
    assert settings.retention_days_default == 14


def test_run_first_run_later_uses_recommended(tmp_path):
    _app()
    settings = AppSettings(DiskGuardDB(tmp_path / "f.db"))
    assert run_first_run(settings, DRIVES, exec_fn=lambda dlg: False) == Path("D:\\Green C")
    assert settings.data_root == Path("D:\\Green C")


def test_current_root_is_preselected():
    _app()
    dlg = FirstRunDialog(DRIVES, current_root=Path("C:\\Green C"), allow_later=False)
    assert dlg.location.selected_drive().root == "C:\\"
    assert dlg.btn_later.text() == "Hủy"
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_ui_first_run.py -q` → `ModuleNotFoundError`.

- [ ] **Step 3: Create `diskguard/ui/data_location.py`**

```python
from pathlib import Path
from typing import Callable

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QButtonGroup,
    QDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QProgressBar,
    QPushButton,
    QRadioButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import APP_NAME, DEFAULT_RETENTION_DAYS, MAX_RETENTION_DAYS, MIN_RETENTION_DAYS
from diskguard.core.drives import DriveInfo, data_root_for, drive_of, recommend_drive
from diskguard.core.settings import AppSettings
from diskguard.ui import format_size
from diskguard.ui.theme import QSS, repolish
from diskguard.ui.widgets import muted_label, page_title


class DriveOption(QFrame):
    def __init__(self, drive: DriveInfo, recommended: bool, parent: QWidget | None = None):
        super().__init__(parent)
        self.drive = drive
        self.setObjectName("DriveOption")
        self.setProperty("selected", False)
        self.setCursor(Qt.CursorShape.PointingHandCursor)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(12, 10, 12, 10)
        lay.setSpacing(10)
        self.radio = QRadioButton()
        lay.addWidget(self.radio, 0, Qt.AlignmentFlag.AlignTop)

        col = QVBoxLayout()
        col.setSpacing(4)
        top = QHBoxLayout()
        name = QLabel(f"<b>{drive.letter}</b>&nbsp;&nbsp;{drive.label or 'Ổ đĩa cục bộ'}")
        top.addWidget(name)
        top.addStretch(1)
        tag_text, tag_name = "", "Tag"
        if not drive.eligible:
            tag_text, tag_name = "Không dùng được", "TagNeutral"
        elif recommended:
            tag_text = "Đề xuất"
        elif drive.is_system:
            tag_text, tag_name = "Ổ hệ thống", "TagNeutral"
        if tag_text:
            tag = QLabel(tag_text)
            tag.setObjectName(tag_name)
            top.addWidget(tag)
        col.addLayout(top)

        bar = QProgressBar()
        bar.setRange(0, 100)
        pct = int(drive.used * 100 / max(1, drive.total))
        bar.setValue(pct)
        bar.setTextVisible(False)
        if pct >= 90:
            bar.setObjectName("DangerBar")
        col.addWidget(bar)
        detail = (
            f"Còn trống {format_size(drive.free)} / {format_size(drive.total)}"
            if drive.eligible
            else drive.reason
        )
        col.addWidget(muted_label(detail))
        lay.addLayout(col, 1)
        self.setEnabled(drive.eligible)
        self.radio.toggled.connect(self._on_toggled)

    def mousePressEvent(self, event) -> None:  # noqa: N802
        if self.isEnabled():
            self.radio.setChecked(True)
        super().mousePressEvent(event)

    def _on_toggled(self, on: bool) -> None:
        self.setProperty("selected", on)
        repolish(self)


class DataLocationWidget(QWidget):
    changed = Signal()

    def __init__(
        self,
        drives: list[DriveInfo],
        current_root: Path | None = None,
        retention_days: int = DEFAULT_RETENTION_DAYS,
        parent: QWidget | None = None,
    ):
        super().__init__(parent)
        self.drives = list(drives)
        recommended = recommend_drive(self.drives)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(8)

        self.group = QButtonGroup(self)
        self.group.setExclusive(True)
        self.options: list[DriveOption] = []
        for idx, drive in enumerate(self.drives):
            opt = DriveOption(drive, recommended is not None and drive.root == recommended.root)
            self.group.addButton(opt.radio, idx)
            opt.radio.toggled.connect(lambda _on: self._refresh_path())
            self.options.append(opt)
            lay.addWidget(opt)

        preselect = None
        if current_root is not None:
            cur_drive = drive_of(current_root)
            preselect = next((o for o in self.options if o.drive.root == cur_drive and o.drive.eligible), None)
        if preselect is None and recommended is not None:
            preselect = next(o for o in self.options if o.drive.root == recommended.root)
        if preselect is not None:
            preselect.radio.setChecked(True)

        row = QHBoxLayout()
        row.setSpacing(8)
        row.addWidget(QLabel("Giữ bản dự phòng trong"))
        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
        self.retention_spin.setValue(retention_days)
        self.retention_spin.setFixedWidth(70)
        self.retention_spin.valueChanged.connect(lambda _v: self.changed.emit())
        row.addWidget(self.retention_spin)
        row.addWidget(QLabel("ngày trước khi xóa vĩnh viễn"))
        row.addStretch(1)
        lay.addSpacing(6)
        lay.addLayout(row)

        self.path_label = muted_label("")
        lay.addWidget(self.path_label)
        self._refresh_path()

    def selected_drive(self) -> DriveInfo | None:
        idx = self.group.checkedId()
        return self.drives[idx] if idx >= 0 else None

    def data_root(self) -> Path | None:
        drive = self.selected_drive()
        return data_root_for(drive.root) if drive else None

    def retention_days(self) -> int:
        return self.retention_spin.value()

    def _refresh_path(self) -> None:
        root = self.data_root()
        self.path_label.setText(f"Thư mục: {root}\\" if root else "Chưa chọn ổ đĩa")
        self.changed.emit()


class FirstRunDialog(QDialog):
    def __init__(
        self,
        drives: list[DriveInfo],
        current_root: Path | None = None,
        retention_days: int = DEFAULT_RETENTION_DAYS,
        parent: QWidget | None = None,
        title: str = "Chọn nơi lưu dữ liệu",
        allow_later: bool = True,
    ):
        super().__init__(parent)
        self.setWindowTitle(APP_NAME)
        self.setStyleSheet(QSS)
        self.setMinimumWidth(540)
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.setSpacing(0)

        body = QWidget()
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 22, 24, 18)
        lay.setSpacing(8)
        lay.addWidget(page_title(title))
        lay.addWidget(muted_label(
            "Green C:\\ chuyển dữ liệu ứng dụng và Kho dự phòng sang ổ này để giải phóng ổ C:. "
            "Có thể đổi lại trong Cài đặt."
        ))
        lay.addSpacing(4)
        self.location = DataLocationWidget(drives, current_root, retention_days)
        lay.addWidget(self.location)
        outer.addWidget(body)

        footer = QFrame()
        footer.setObjectName("DialogFooter")
        f = QHBoxLayout(footer)
        f.setContentsMargins(24, 12, 24, 12)
        f.addStretch(1)
        self.btn_later = QPushButton("Để sau" if allow_later else "Hủy")
        self.btn_later.clicked.connect(self.reject)
        self.btn_ok = QPushButton("Tiếp tục" if allow_later else "Lưu")
        self.btn_ok.setObjectName("PrimaryButton")
        self.btn_ok.setDefault(True)
        self.btn_ok.clicked.connect(self.accept)
        f.addWidget(self.btn_later)
        f.addWidget(self.btn_ok)
        outer.addWidget(footer)

        self.location.changed.connect(self._sync_ok)
        self._sync_ok()

    def _sync_ok(self) -> None:
        self.btn_ok.setEnabled(self.location.data_root() is not None)

    def chosen_root(self) -> Path | None:
        return self.location.data_root()

    def chosen_retention(self) -> int:
        return self.location.retention_days()


def run_first_run(
    settings: AppSettings,
    drives: list[DriveInfo],
    parent: QWidget | None = None,
    exec_fn: Callable[[FirstRunDialog], bool] | None = None,
) -> Path | None:
    """Show the first-run dialog and persist the result. "Để sau" keeps the recommended drive."""
    dlg = FirstRunDialog(drives, None, settings.retention_days_default, parent)
    accepted = bool(exec_fn(dlg)) if exec_fn else dlg.exec() == QDialog.DialogCode.Accepted
    settings.set_retention_days_default(dlg.chosen_retention())
    if accepted:
        root = dlg.chosen_root()
    else:
        rec = recommend_drive(drives)
        root = data_root_for(rec.root) if rec else None
    if root is not None:
        settings.set_data_root(root)
    return root
```

- [ ] **Step 4: Run tests** — `pytest tests/test_ui_first_run.py -q` → PASS.

- [ ] **Step 5: Commit**

```bash
git add diskguard/ui/data_location.py tests/test_ui_first_run.py
git commit -m "feat: data drive picker and first-run dialog"
```

---

### Task 11: Restyle existing pages and copy (daily, cleanup, dialogs, chart, analyzer)

**Files:**
- Modify: `diskguard/ui/daily_tab.py` (whole file), `diskguard/ui/smart_cleanup_tab.py` (whole file), `diskguard/ui/dialogs.py` (whole file), `diskguard/ui/chart_widget.py` (colors + background), `diskguard/ui/workers.py` (`TaskProgressCard` UI), `diskguard/core/smart_analyzer.py` (labels + `retention_days` param)
- Modify tests: `tests/test_whitebox_internals.py` (one assertion), `tests/test_ui_headless.py` (probe_strategy sample text)
- Test: `tests/test_ui_pages_restyled.py`

**Interfaces:**
- Consumes: Task 9 widgets/theme, `format_size`.
- Produces: `SmartCleanupTab.set_retention_days(days)`; `QuotaRuleDialog(..., default_retention_days: int = 7)`; `SmartAppAnalyzer.execute_one_click_cleanup(proposals, task_control=None, retention_days: int = 7)`; probe labels `"Đạt"`, `"Đạt · bỏ qua file đang mở"`, `"Chỉ dọn bộ đệm"`, `"Không đạt"`. Attributes preserved for existing tests: `DailyMonitorTab.chart/folder_table/actor_table/load_drive_data/requestCreateQuota`, `SmartCleanupTab.table/btn_clean_now/btn_scan_apps/proposal_summary_label/set_proposals/scanAppsRequested/executeCleanupRequested`, `TaskProgressCard.title_label/detail_label/pct_label/progress_bar/btn_pause/btn_stop/attach_worker/update_progress/startRequested`.

- [ ] **Step 1: Write the failing test** — `tests/test_ui_pages_restyled.py`

```python
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from diskguard.core.smart_analyzer import AppAnalysisProposal
from diskguard.ui.dialogs import QuotaRuleDialog
from diskguard.ui.smart_cleanup_tab import SmartCleanupTab
from diskguard.ui.widgets import PathCell

BANNED = ("(", "thông minh", "1-click", "Sandbox", "Rollback", "Verified")


def _app():
    return QApplication.instance() or QApplication([])


def _prop(name, reclaim):
    return AppAnalysisProposal(
        app_name=name, process_name=f"{name}.exe", service_name="", is_running=False,
        storage_summary="Đang nghỉ", target_paths=[rf"C:\Users\me\AppData\Roaming\{name}"],
        total_physical_bytes=reclaim * 2, reclaimable_bytes=reclaim, recommended_max_bytes=reclaim,
        probe_passed=True, probe_strategy="Đạt", probe_detail="Đã thử nén và khôi phục, dữ liệu khớp.",
    )


def test_cleanup_tab_copy_and_path_cells():
    _app()
    tab = SmartCleanupTab()
    tab.set_retention_days(14)
    assert "14 ngày" in tab.description.text()
    tab.set_proposals([_prop("Zalo", 3 * 1024**3), _prop("Teams", 1024**3)])
    assert tab.table.rowCount() == 2
    assert isinstance(tab.table.cellWidget(0, 2), PathCell)
    assert tab.btn_clean_now.isEnabled() and tab.btn_clean_now.text() == "Dọn dẹp 4,0 GB"
    for text in (tab.description.text(), tab.proposal_summary_label.text(), tab.btn_scan_apps.text()):
        assert not any(b in text for b in BANNED), text


def test_rule_dialog_uses_default_retention():
    _app()
    dlg = QuotaRuleDialog(default_retention_days=21)
    assert dlg.retention_spin.value() == 21
    assert dlg.windowTitle() == "Quy tắc dung lượng"
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_ui_pages_restyled.py -q` → FAIL (`AttributeError: 'SmartCleanupTab' object has no attribute 'set_retention_days'`).

- [ ] **Step 3: Analyzer copy + retention** — in `diskguard/core/smart_analyzer.py`:

Replace the three probe return tuples / fallback:

```python
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
```

```python
            if safe_subdirs:
                return (
                    True,
                    "Chỉ dọn bộ đệm",
                    "Thư mục chính có file quan trọng, chỉ dọn các thư mục Cache, Temp, Logs.",
                    safe_subdirs[:6],
                )
            return False, "Không đạt", "Không vượt qua kiểm tra an toàn.", target_dirs
```

Change progress copy in `scan_and_probe_apps`: step titles to `"Đang chuẩn bị phân tích"`, `f"Đang phân tích {app_name} ({idx}/{total_cands})"`, `f"Đang kiểm tra an toàn cho {app_name}"`, `f"Đã phân tích xong {len(proposals)} ứng dụng"`; details to `"Đang lấy danh sách ứng dụng và tiến trình."`, `target_dirs[0] if target_dirs else ""`, `"Nén thử một file cũ rồi khôi phục để kiểm tra dữ liệu."`, `"Chọn ứng dụng rồi bấm Dọn dẹp."`. Traits: `"Có liên kết cứng ({hardlink_count} nhóm)"`, `"Bỏ qua {locked_count} file đang mở"`, `"File cũ đều không bị khóa"`. The `("Bộ nhớ tạm Hệ thống & Ứng dụng (Temp)", ...)` label becomes `"Thư mục Temp của Windows"`.

Change the cleanup signature and rule creation:

```python
    def execute_one_click_cleanup(
        self,
        proposals: list[AppAnalysisProposal],
        task_control: TaskControl | None = None,
        retention_days: int = 7,
    ) -> dict:
```

use `retention_days=retention_days` in the `QuotaRule(...)` it builds, the per-app report becomes `f"Đang dọn {prop.app_name} ({idx}/{total})"` / `"Nén file cũ vào Kho dự phòng."`, and the final report:

```python
        detail = f"Đã giải phóng {freed_bytes / (1024**3):.1f} GB từ {archives_created} ứng dụng.".replace(".", ",", 1)
        if errors:
            detail += f" Lỗi ở {len(errors)} ứng dụng: " + "; ".join(errors)
        ctrl.report(100, "Đã dọn xong", detail)
```

Update `tests/test_whitebox_internals.py` line with `assert "Phương án dự phòng" in strategy` → `assert strategy == "Chỉ dọn bộ đệm"`, and in `tests/test_ui_headless.py` replace `probe_strategy="Chu trình chuẩn (Verified Safe)"` with `probe_strategy="Đạt"`.

- [ ] **Step 4: Replace `diskguard/ui/smart_cleanup_tab.py`**

```python
from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import QCheckBox, QHBoxLayout, QPushButton, QVBoxLayout, QWidget

from diskguard.core.smart_analyzer import AppAnalysisProposal
from diskguard.ui import format_size
from diskguard.ui.widgets import (
    PathValue,
    TagValue,
    card_title,
    make_card,
    make_table,
    muted_label,
    page_title,
    set_cell,
)


class SmartCleanupTab(QWidget):
    """Dọn dẹp: analysis results with checkboxes and one primary action."""

    scanAppsRequested = Signal()
    executeCleanupRequested = Signal(list)  # list[AppAnalysisProposal]

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self._proposals: list[AppAnalysisProposal] = []
        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        head = QHBoxLayout()
        head.addWidget(page_title("Dọn dẹp"))
        head.addStretch(1)
        self.btn_scan_apps = QPushButton("Phân tích")
        self.btn_scan_apps.clicked.connect(self.scanAppsRequested.emit)
        self.btn_clean_now = QPushButton("Dọn dẹp")
        self.btn_clean_now.setObjectName("PrimaryButton")
        self.btn_clean_now.setEnabled(False)
        self.btn_clean_now.clicked.connect(self._on_clean_clicked)
        head.addWidget(self.btn_scan_apps)
        head.addWidget(self.btn_clean_now)
        lay.addLayout(head)

        self.description = muted_label("")
        lay.addWidget(self.description)
        self.set_retention_days(7)

        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        self.proposal_summary_label = card_title("Chưa có kết quả phân tích")
        cl.addWidget(self.proposal_summary_label)
        self.table = make_table(
            ["", "Ứng dụng", "Vị trí", "Đang dùng", "Giải phóng", "Kiểm tra"], stretch_col=2, right_cols=(3, 4)
        )
        cl.addWidget(self.table)
        lay.addWidget(card, 1)

    def set_retention_days(self, days: int) -> None:
        self.description.setText(
            f"Nén file cũ không dùng tới vào Kho dự phòng. Sau {days} ngày gói nén tự xóa; "
            "trong thời gian đó có thể khôi phục bất cứ lúc nào."
        )

    def set_proposals(self, proposals: list[AppAnalysisProposal]) -> None:
        self._proposals = list(proposals)
        self.table.setRowCount(len(self._proposals))
        for row, prop in enumerate(self._proposals):
            chk = QCheckBox()
            chk.setChecked(prop.selected)
            chk.toggled.connect(lambda checked, idx=row: self._on_row_toggled(idx, checked))
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(10, 0, 4, 0)
            h.setAlignment(Qt.AlignmentFlag.AlignCenter)
            h.addWidget(chk)
            self.table.setCellWidget(row, 0, wrap)
            set_cell(self.table, row, 1, prop.app_name)
            set_cell(self.table, row, 2, PathValue(prop.target_paths[0] if prop.target_paths else ""))
            set_cell(self.table, row, 3, format_size(prop.total_physical_bytes), right=True)
            set_cell(self.table, row, 4, format_size(prop.reclaimable_bytes), right=True)
            set_cell(self.table, row, 5, TagValue(prop.probe_strategy, warning=not prop.probe_passed, tooltip=prop.probe_detail))
        self._refresh_summary()

    def _refresh_summary(self) -> None:
        total = sum(p.reclaimable_bytes for p in self._proposals if p.selected)
        if not self._proposals:
            self.proposal_summary_label.setText("Không có ứng dụng nào cần dọn")
        else:
            self.proposal_summary_label.setText(
                f"{len(self._proposals)} ứng dụng · có thể giải phóng {format_size(total)}"
            )
        self.btn_clean_now.setEnabled(total > 0)
        self.btn_clean_now.setText(f"Dọn dẹp {format_size(total)}" if total > 0 else "Dọn dẹp")

    def _on_row_toggled(self, row: int, checked: bool) -> None:
        if 0 <= row < len(self._proposals):
            self._proposals[row].selected = checked
            self._refresh_summary()

    def _on_clean_clicked(self) -> None:
        selected = [p for p in self._proposals if p.selected]
        if selected:
            self.executeCleanupRequested.emit(selected)
```

- [ ] **Step 5: Replace `diskguard/ui/daily_tab.py`**

```python
from datetime import date

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QHBoxLayout, QPushButton, QVBoxLayout, QWidget

from diskguard.models import DailySnapshot
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.chart_widget import DailyBarChartWidget
from diskguard.ui.widgets import PathValue, card_title, make_table, muted_label, set_cell


class DailyMonitorTab(QWidget):
    """Daily growth chart + what grew and who wrote it on the selected day."""

    requestCreateQuota = Signal(str, str, str, list)

    def __init__(self, db: DiskGuardDB, parent: QWidget | None = None):
        super().__init__(parent)
        self.db = db
        self._current_drive = "C:\\"
        self._selected_date = ""
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(10)

        self.chart = DailyBarChartWidget(self)
        self.chart.daySelected.connect(self._on_day_selected)
        lay.addWidget(self.chart)

        self.day_banner = muted_label("Chọn một cột để xem chi tiết ngày đó.")
        lay.addWidget(self.day_banner)

        cols = QHBoxLayout()
        cols.setSpacing(16)
        left = QVBoxLayout()
        left.addWidget(card_title("Thư mục tăng dung lượng"))
        self.folder_table = make_table(["Thư mục", "Tăng", "Nguyên nhân"], stretch_col=0, right_cols=(1,))
        self.folder_table.setMinimumHeight(220)
        left.addWidget(self.folder_table)
        right = QVBoxLayout()
        right.addWidget(card_title("Ứng dụng và dịch vụ"))
        self.actor_table = make_table(["Ứng dụng", "Tiến trình", "Đã ghi", ""], stretch_col=0, right_cols=(2,))
        self.actor_table.setMinimumHeight(220)
        right.addWidget(self.actor_table)
        cols.addLayout(left, 11)
        cols.addLayout(right, 10)
        lay.addLayout(cols)

    def load_drive_data(self, drive: str, snapshots: list[DailySnapshot] | None = None) -> None:
        self._current_drive = drive
        snaps = snapshots if snapshots is not None else self.db.get_daily_snapshots(drive, limit_days=14)
        selected = self._selected_date
        if snaps and (not selected or all(s.snapshot_date != selected for s in snaps)):
            active_days = [s.snapshot_date for s in snaps if s.net_change_bytes > 0]
            selected = active_days[-1] if active_days else snaps[-1].snapshot_date
        self._selected_date = selected
        self.chart.set_snapshots(snaps, selected_date=selected)
        if selected:
            self._populate_day_tables(selected)

    def _on_day_selected(self, date_str: str) -> None:
        self._selected_date = date_str
        self._populate_day_tables(date_str)

    def _populate_day_tables(self, date_str: str) -> None:
        folders = self.db.get_day_folder_breakdown(self._current_drive, date_str)
        actors = self.db.get_day_actor_breakdown(self._current_drive, date_str)
        total = sum(int(f["total_delta_bytes"]) for f in folders)
        try:
            day_label = date.fromisoformat(date_str).strftime("%d/%m/%Y")
        except ValueError:
            day_label = date_str
        self.day_banner.setText(
            f"{day_label} · ổ {self._current_drive[:2]} · tăng {format_size(total)} · "
            f"{len(folders)} thư mục · {len(actors)} ứng dụng"
        )

        self.folder_table.setRowCount(len(folders))
        for row, f in enumerate(folders):
            set_cell(self.folder_table, row, 0, PathValue(f["folder_path"]))
            set_cell(self.folder_table, row, 1, f"+{format_size(f['total_delta_bytes'])}", right=True)
            set_cell(self.folder_table, row, 2, f["reason_detail"])

        self.actor_table.setRowCount(len(actors))
        for row, a in enumerate(actors):
            proc = a["process_name"] or "Không rõ"
            svc = a["service_name"] or ""
            display = a["service_display_name"] or proc
            label = f"{display} · {svc}" if svc else display
            folders_list = [p.strip() for p in (a.get("folders") or "").split(",") if p.strip()]
            set_cell(self.actor_table, row, 0, label)
            set_cell(self.actor_table, row, 1, proc)
            set_cell(self.actor_table, row, 2, f"+{format_size(a['total_delta_bytes'])}", right=True)
            btn = QPushButton("Tạo quy tắc")
            btn.clicked.connect(
                lambda _=False, n=display, p=proc, s=svc, fl=folders_list: self.requestCreateQuota.emit(n, p, s, fl)
            )
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 4, 2)
            h.addWidget(btn)
            self.actor_table.setCellWidget(row, 3, wrap)
```

Note: `tests/test_e2e_app_testcases.py` asserts `"DingTalk" in win.daily_tab.folder_table.item(0, 0).text()` — `set_cell` with `PathValue` keeps the item text, so this still passes.

- [ ] **Step 6: `diskguard/ui/chart_widget.py`** — replace the background block at the start of `paintEvent`:

```python
        rect = self.rect()
        painter.fillRect(rect, QColor("#FFFFFF"))
```

replace the empty-state text with `"Chưa có dữ liệu. Bấm Làm mới để đọc lịch sử ổ đĩa."`, and map colors: `#0066CC`→`#0067C0`, `#4791FF`→`#2B88D8`, `#BFD9FF`→`#C7E0F4`, `#1D1D1F`→`#1B1B1B`, `#6E6E73`/`#8E8E93`→`#616161`, `#E5E5EA`/`#F0F0F5`→`#EDEDED`; spike colors `#D97706`→`#C42B1C`, `#F59E0B`→`#E0705E`, `#FDE68A`→`#F4CFC9`, `#B45309`→`#C42B1C`. Any title text drawn by the chart becomes `"Dung lượng theo ngày"`.

- [ ] **Step 7: Replace `diskguard/ui/dialogs.py`**

```python
from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QDoubleSpinBox,
    QFileDialog,
    QFormLayout,
    QLineEdit,
    QPushButton,
    QSpinBox,
    QTextEdit,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import DEFAULT_PROTECTED_PATTERNS, DEFAULT_RETENTION_DAYS
from diskguard.models import FileManifestItem, QuotaRule, VaultArchive
from diskguard.ui import format_size
from diskguard.ui.theme import QSS
from diskguard.ui.widgets import PathValue, make_table, muted_label, set_cell


class QuotaRuleDialog(QDialog):
    """Create or edit a background size rule."""

    def __init__(
        self,
        parent: QWidget | None = None,
        rule: QuotaRule | None = None,
        prefill_app_name: str = "",
        prefill_process_name: str = "",
        prefill_service_name: str = "",
        prefill_paths: list[str] | None = None,
        prefill_max_gb: float = 2.0,
        prefill_excludes: list[str] | None = None,
        default_retention_days: int = DEFAULT_RETENTION_DAYS,
    ):
        super().__init__(parent)
        self.setWindowTitle("Quy tắc dung lượng")
        self.setStyleSheet(QSS)
        self.resize(580, 440)
        self._rule = rule
        layout = QVBoxLayout(self)
        layout.setContentsMargins(20, 18, 20, 14)
        form = QFormLayout()
        form.setVerticalSpacing(10)

        self.app_name_edit = QLineEdit(rule.app_name if rule else (prefill_app_name or ""))
        self.process_edit = QLineEdit(rule.process_name if rule else prefill_process_name)
        self.service_edit = QLineEdit(rule.service_name if rule else prefill_service_name)
        self.paths_edit = QTextEdit()
        self.paths_edit.setPlainText("\n".join(rule.target_paths) if rule else "\n".join(prefill_paths or []))
        self.paths_edit.setPlaceholderText("Mỗi dòng một thư mục")
        self.paths_edit.setMaximumHeight(90)
        browse_btn = QPushButton("Chọn thư mục…")
        browse_btn.clicked.connect(self._on_browse_folder)

        self.max_gb_spin = QDoubleSpinBox()
        self.max_gb_spin.setRange(0.001, 10000.0)
        self.max_gb_spin.setDecimals(2)
        self.max_gb_spin.setSuffix(" GB")
        self.max_gb_spin.setValue(rule.max_bytes / (1024**3) if rule else prefill_max_gb)
        self.watermark_spin = QSpinBox()
        self.watermark_spin.setRange(50, 95)
        self.watermark_spin.setSuffix(" %")
        self.watermark_spin.setValue(int((rule.watermark_ratio if rule else 0.85) * 100))
        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(1, 365)
        self.retention_spin.setSuffix(" ngày")
        self.retention_spin.setValue(rule.retention_days if rule else default_retention_days)
        self.excludes_edit = QLineEdit(
            ", ".join(rule.exclude_patterns) if rule else ", ".join(prefill_excludes or DEFAULT_PROTECTED_PATTERNS[:10])
        )
        self.enabled_chk = QCheckBox("Chạy nền")
        self.enabled_chk.setChecked(rule.enabled if rule else True)

        form.addRow("Tên ứng dụng", self.app_name_edit)
        form.addRow("Tiến trình", self.process_edit)
        form.addRow("Dịch vụ Windows", self.service_edit)
        paths_box = QVBoxLayout()
        paths_box.addWidget(self.paths_edit)
        paths_box.addWidget(browse_btn, alignment=Qt.AlignmentFlag.AlignLeft)
        form.addRow("Thư mục", paths_box)
        form.addRow("Giới hạn", self.max_gb_spin)
        form.addRow("Dọn về mức", self.watermark_spin)
        form.addRow("Giữ bản dự phòng", self.retention_spin)
        form.addRow("Không dọn các file", self.excludes_edit)
        form.addRow("", self.enabled_chk)
        layout.addLayout(form)
        layout.addWidget(muted_label(
            "Khi vượt giới hạn, các file cũ nhất không bị khóa được nén vào Kho dự phòng. "
            "Nếu ứng dụng cần lại file trong thời gian giữ, file được khôi phục tự động."
        ))
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Save | QDialogButtonBox.StandardButton.Cancel)
        buttons.button(QDialogButtonBox.StandardButton.Save).setText("Lưu")
        buttons.button(QDialogButtonBox.StandardButton.Save).setObjectName("PrimaryButton")
        buttons.button(QDialogButtonBox.StandardButton.Cancel).setText("Hủy")
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

    def _on_browse_folder(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, "Chọn thư mục")
        if folder:
            cur = self.paths_edit.toPlainText().strip()
            self.paths_edit.setPlainText(f"{cur}\n{folder}".strip())

    def get_rule(self) -> QuotaRule:
        paths = [ln.strip() for ln in self.paths_edit.toPlainText().splitlines() if ln.strip()]
        excludes = [x.strip() for x in self.excludes_edit.text().split(",") if x.strip()]
        return QuotaRule(
            id=self._rule.id if self._rule else None,
            app_name=self.app_name_edit.text().strip() or "Ứng dụng",
            process_name=self.process_edit.text().strip(),
            service_name=self.service_edit.text().strip(),
            target_paths=paths,
            max_bytes=int(self.max_gb_spin.value() * (1024**3)),
            watermark_ratio=self.watermark_spin.value() / 100.0,
            retention_days=self.retention_spin.value(),
            min_file_age_seconds=self._rule.min_file_age_seconds if self._rule else 60,
            exclude_patterns=excludes,
            protected_files=list(self._rule.protected_files) if self._rule else [],
            enabled=self.enabled_chk.isChecked(),
            created_at=self._rule.created_at if self._rule else "",
        )


class ArchiveDetailsDialog(QDialog):
    """Files inside one Vault archive."""

    def __init__(self, archive: VaultArchive, files: list[FileManifestItem], parent: QWidget | None = None):
        super().__init__(parent)
        self.setWindowTitle(f"Gói dự phòng · {archive.app_name}")
        self.setStyleSheet(QSS)
        self.resize(860, 460)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(20, 18, 20, 14)
        layout.addWidget(muted_label(
            f"{len(files)} tệp · {format_size(archive.original_bytes)} → {format_size(archive.compressed_bytes)} · "
            f"{archive.status_reason}"
        ))
        table = make_table(["Đường dẫn gốc", "Kích thước", "Liên kết", "SHA-256"], stretch_col=0, right_cols=(1,))
        table.setRowCount(len(files))
        for row, item in enumerate(files):
            links = len(item.hardlink_paths) if item.hardlink_paths else 1
            set_cell(table, row, 0, PathValue(item.original_path))
            set_cell(table, row, 1, format_size(item.size_bytes), right=True)
            set_cell(table, row, 2, f"{links} đường dẫn")
            set_cell(table, row, 3, item.sha256[:16] + "…")
        layout.addWidget(table)
        close = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        close.button(QDialogButtonBox.StandardButton.Close).setText("Đóng")
        close.rejected.connect(self.reject)
        layout.addWidget(close)
```

- [ ] **Step 8: Compact `TaskProgressCard` in `diskguard/ui/workers.py`** — replace the `TaskProgressCard.__init__` body and `_on_*` captions (keep `BackgroundTaskWorker` unchanged, keep all attribute names):

```python
class TaskProgressCard(QFrame):
    """Compact status bar at the bottom of the window; hidden while idle."""

    startRequested = Signal()  # kept for compatibility; no start button any more

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.setObjectName("StatusBar")
        self._worker: BackgroundTaskWorker | None = None
        lay = QHBoxLayout(self)
        lay.setContentsMargins(20, 8, 20, 8)
        lay.setSpacing(12)
        self.title_label = QLabel("Sẵn sàng")
        self.title_label.setStyleSheet("font-weight: 600;")
        self.detail_label = QLabel("")
        self.detail_label.setObjectName("Muted")
        self.detail_label.setMinimumWidth(80)
        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.progress_bar.setTextVisible(False)
        self.progress_bar.setFixedWidth(180)
        self.pct_label = QLabel("")
        self.pct_label.setObjectName("Muted")
        self.pct_label.setFixedWidth(40)
        self.btn_pause = QPushButton("Tạm dừng")
        self.btn_pause.setEnabled(False)
        self.btn_pause.clicked.connect(self._on_toggle_pause)
        self.btn_stop = QPushButton("Dừng")
        self.btn_stop.setEnabled(False)
        self.btn_stop.clicked.connect(self._on_stop)
        lay.addWidget(self.title_label)
        lay.addWidget(self.detail_label, 1)
        lay.addWidget(self.progress_bar)
        lay.addWidget(self.pct_label)
        lay.addWidget(self.btn_pause)
        lay.addWidget(self.btn_stop)
        self.setVisible(False)

    def attach_worker(self, worker: BackgroundTaskWorker, initial_title: str) -> None:
        self._worker = worker
        self.setVisible(True)
        self.title_label.setText(initial_title)
        self.detail_label.setText("")
        self.pct_label.setText("0%")
        self.progress_bar.setValue(0)
        self.btn_pause.setEnabled(True)
        self.btn_pause.setText("Tạm dừng")
        self.btn_stop.setEnabled(True)
        worker.progressChanged.connect(self.update_progress)
        worker.taskFinished.connect(self._on_finished)
        worker.taskFailed.connect(self._on_failed)

    def update_progress(self, percent: int, title: str, detail: str) -> None:
        self.progress_bar.setValue(percent)
        self.pct_label.setText(f"{percent}%")
        if title:
            self.title_label.setText(title)
        if detail:
            self.detail_label.setText(
                self.detail_label.fontMetrics().elidedText(detail, Qt.TextElideMode.ElideRight, max(80, self.detail_label.width()))
            )
            self.detail_label.setToolTip(detail)

    def _on_toggle_pause(self) -> None:
        if not self._worker:
            return
        ctrl = self._worker.control
        if ctrl.is_paused:
            ctrl.resume()
            self.btn_pause.setText("Tạm dừng")
        else:
            ctrl.pause()
            self.btn_pause.setText("Tiếp tục")
            self.title_label.setText("Đã tạm dừng")

    def _on_stop(self) -> None:
        if not self._worker:
            return
        self._worker.control.stop()
        self.btn_pause.setEnabled(False)
        self.btn_stop.setEnabled(False)
        self.title_label.setText("Đang dừng…")

    def _finish_ui(self) -> None:
        self.btn_pause.setEnabled(False)
        self.btn_pause.setText("Tạm dừng")
        self.btn_stop.setEnabled(False)
        QTimer.singleShot(5000, self._hide_if_idle)

    def _hide_if_idle(self) -> None:
        if self._worker is None or not self._worker.isRunning():
            self.setVisible(False)

    def _on_finished(self, _result: object, was_stopped: bool) -> None:
        if was_stopped:
            self.title_label.setText("Đã dừng")
            self.detail_label.setText("")
        else:
            self.progress_bar.setValue(100)
            self.pct_label.setText("100%")
        self._finish_ui()

    def _on_failed(self, err_msg: str) -> None:
        self.title_label.setText("Có lỗi")
        self.detail_label.setText(err_msg)
        self.detail_label.setToolTip(err_msg)
        self._finish_ui()
```

Add `QTimer` and `Qt` to the `PySide6.QtCore` import in `workers.py`.

- [ ] **Step 9: Run tests** — `pytest tests/test_ui_pages_restyled.py tests/test_e2e_app_testcases.py tests/test_ui_headless.py tests/test_whitebox_internals.py -q` → PASS; full suite → PASS.

- [ ] **Step 10: Commit**

```bash
git add diskguard/ui diskguard/core/smart_analyzer.py tests
git commit -m "feat: restyle cleanup, daily, dialogs and status bar with concise Vietnamese copy"
```

---

### Task 12: Vault page, Settings page, Overview page, new shell

**Files:**
- Create: `diskguard/ui/vault_page.py`, `diskguard/ui/overview_page.py`
- Modify: `diskguard/ui/settings_tab.py` (whole file), `diskguard/ui/main_window.py` (shell, pages, wiring)
- Modify tests: `tests/test_e2e_app_testcases.py` (settings index 2 → 4), `tests/test_ui_headless.py` (`stack.count() == 3` → `5`)
- Test: `tests/test_ui_shell.py`

**Interfaces:**
- Consumes: Tasks 1, 9, 10, 11.
- Produces: `VaultPage(db, watchdog, vault_dir_provider)` (`.vault_table`, `.info_label`, `.refresh()`, signal `dataChanged`); `OverviewPage(db, drives)` (`.drive_combo`, `.drive_usage_bar`, `.drive_stats_label`, `.freeable_card`, `.daily_tab`, `.btn_analyze`, `.btn_clean`, `.btn_refresh_history`, `.update_drive_usage(drive)`, `.set_freeable(proposals, candidates, target_label)`, signals `analyzeRequested`, `cleanupRequested`, `refreshHistoryRequested`, `driveChanged(str)`); `SettingsTab(db, enforcer, watchdog, simulator, parent=None, settings=None)` (`.rules_table`, `.retention_spin`, `.btn_change_root`, `.data_root_hint`, `.about_label`, `.refresh_all()`, `.refresh_rules_table()`, `.refresh_data_root()`, `.open_new_rule_dialog(...)`, signals `settingsDataChanged`, `dataRootChangeRequested`); `MainWindow(db=None, vault_dir=None, sandbox_dir=None, auto_scan_on_init=True, settings=None)` with `btn_nav_overview/cleanup/relocation/vault/settings`, `overview_page`, `daily_tab`, `cleanup_tab`, `relocation_page` (placeholder `QWidget` in this task, replaced in Task 13), `vault_page`, `settings_tab`, `brand_label`, `author_label`, `drive_combo`, `refresh_data_views()`.

- [ ] **Step 1: Write the failing test** — `tests/test_ui_shell.py`

```python
import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from diskguard.config import APP_NAME
from diskguard.core.settings import AppSettings
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.main_window import MainWindow


def _win(tmp_path: Path):
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "ui.db")
    settings = AppSettings(db)
    win = MainWindow(db=db, vault_dir=tmp_path / "vault", sandbox_dir=tmp_path / "sb",
                     auto_scan_on_init=False, settings=settings)
    win._bg_timer.stop()
    return app, win, settings


def test_brand_author_and_navigation(tmp_path: Path):
    app, win, _ = _win(tmp_path)
    assert win.windowTitle() == APP_NAME
    assert "Green" in win.brand_label.text() and "C:\\" in win.brand_label.text()
    assert "Cùng Chat" in win.author_label.text()
    assert 'href="https://cungchat.com"' in win.author_label.text()
    assert win.author_label.openExternalLinks()
    assert win.stack.count() == 5
    for btn, idx in ((win.btn_nav_overview, 0), (win.btn_nav_cleanup, 1), (win.btn_nav_relocation, 2),
                     (win.btn_nav_vault, 3), (win.btn_nav_settings, 4)):
        btn.click()
        app.processEvents()
        assert win.stack.currentIndex() == idx
    win.close()


def test_overview_freeable_card_has_eye_toggle(tmp_path: Path):
    app, win, _ = _win(tmp_path)
    card = win.overview_page.freeable_card
    assert not card.empty_label.isHidden()
    from diskguard.core.smart_analyzer import AppAnalysisProposal

    prop = AppAnalysisProposal("Zalo", "Zalo.exe", "", False, "", [r"C:\Zalo"], 2 * 1024**3,
                               1024**3, 1024**3, True, "Đạt", "")
    win.overview_page.set_freeable([prop], [], "D:")
    assert card.simple_table.rowCount() == 1
    card.eye_button.click()
    assert card.is_expanded() and card.detail_table.rowCount() == 1
    win.close()


def test_settings_retention_persists_and_feeds_cleanup(tmp_path: Path):
    app, win, settings = _win(tmp_path)
    win.settings_tab.retention_spin.setValue(30)
    app.processEvents()
    assert settings.retention_days_default == 30
    assert "30 ngày" in win.cleanup_tab.description.text()
    win.close()


def test_vault_page_lists_archives(tmp_path: Path):
    app, win, _ = _win(tmp_path)
    win.run_sandbox_demo(silent=True)
    win.refresh_data_views()
    assert win.vault_page.vault_table.rowCount() >= 1
    win.close()
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_ui_shell.py -q` → FAIL (`TypeError: MainWindow.__init__() got an unexpected keyword argument 'settings'`).

- [ ] **Step 3: Create `diskguard/ui/vault_page.py`**

```python
from datetime import datetime
from pathlib import Path
from typing import Callable

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QHBoxLayout, QMessageBox, QPushButton, QVBoxLayout, QWidget

from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.models import VaultArchive
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.dialogs import ArchiveDetailsDialog
from diskguard.ui.widgets import PathValue, TagValue, make_card, make_table, muted_label, page_title, set_cell

STATUS_TEXT = {"QUARANTINED": "Đang giữ", "ROLLED_BACK": "Đã khôi phục", "PURGED": "Đã xóa"}


class VaultPage(QWidget):
    dataChanged = Signal()

    def __init__(self, db: DiskGuardDB, watchdog: SafetyWatchdog, vault_dir_provider: Callable[[], Path], parent=None):
        super().__init__(parent)
        self.db = db
        self.watchdog = watchdog
        self._vault_dir = vault_dir_provider
        lay = QVBoxLayout(self)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)
        head = QHBoxLayout()
        head.addWidget(page_title("Kho dự phòng"))
        head.addStretch(1)
        self.btn_check = QPushButton("Kiểm tra ngay")
        self.btn_check.clicked.connect(self._check_now)
        head.addWidget(self.btn_check)
        lay.addLayout(head)
        self.info_label = muted_label("")
        lay.addWidget(self.info_label)
        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        self.vault_table = make_table(
            ["Ứng dụng", "Tạo lúc", "Còn lại", "Gốc → Nén", "Trạng thái", "Vị trí", ""], stretch_col=5
        )
        cl.addWidget(self.vault_table)
        lay.addWidget(card, 1)

    def refresh(self) -> None:
        archives = self.db.list_vault_archives()
        live = [a for a in archives if a.status == "QUARANTINED"]
        self.info_label.setText(
            f"{self._vault_dir()} · {len(live)} gói đang giữ · "
            f"{format_size(sum(a.compressed_bytes for a in live))}"
        )
        now = datetime.now()
        self.vault_table.setRowCount(len(archives))
        for row, arch in enumerate(archives):
            created = datetime.fromisoformat(arch.created_at)
            remaining = "—"
            if arch.status == "QUARANTINED":
                delta = datetime.fromisoformat(arch.expires_at) - now
                remaining = f"{max(0, delta.days)} ngày {max(0, delta.seconds // 3600)} giờ"
            set_cell(self.vault_table, row, 0, arch.app_name)
            set_cell(self.vault_table, row, 1, created.strftime("%d/%m %H:%M"))
            set_cell(self.vault_table, row, 2, remaining)
            set_cell(self.vault_table, row, 3, f"{format_size(arch.original_bytes)} → {format_size(arch.compressed_bytes)}")
            set_cell(self.vault_table, row, 4, TagValue(STATUS_TEXT.get(arch.status, arch.status), tooltip=arch.status_reason))
            set_cell(self.vault_table, row, 5, PathValue(arch.zip_path))
            self.vault_table.setCellWidget(row, 6, self._actions(arch))

    def _actions(self, arch: VaultArchive) -> QWidget:
        wrap = QWidget()
        h = QHBoxLayout(wrap)
        h.setContentsMargins(4, 2, 4, 2)
        h.setSpacing(6)
        btn_view = QPushButton("Chi tiết")
        btn_view.clicked.connect(lambda _=False, a=arch: self._view(a))
        h.addWidget(btn_view)
        if arch.status == "QUARANTINED":
            btn_restore = QPushButton("Khôi phục")
            btn_restore.clicked.connect(lambda _=False, aid=arch.id: self._rollback(aid))
            btn_purge = QPushButton("Xóa ngay")
            btn_purge.clicked.connect(lambda _=False, aid=arch.id: self._purge(aid))
            h.addWidget(btn_restore)
            h.addWidget(btn_purge)
        return wrap

    def _view(self, arch: VaultArchive) -> None:
        ArchiveDetailsDialog(arch, self.db.get_vault_files(arch.id or 0), self).exec()

    def _rollback(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        res = self.watchdog.rollback_archive(archive_id, reason="Khôi phục thủ công", auto_triggered=False)
        self.refresh()
        self.dataChanged.emit()
        QMessageBox.information(self, "Đã khôi phục", f"Đã khôi phục {res['restored_count']} tệp về vị trí cũ.")

    def _purge(self, archive_id: int | None) -> None:
        if archive_id is None:
            return
        answer = QMessageBox.question(self, "Xóa gói dự phòng", "Xóa vĩnh viễn gói này? Không thể hoàn tác.")
        if answer != QMessageBox.StandardButton.Yes:
            return
        self.watchdog.purge_archive_now(archive_id, "Người dùng xóa")
        self.refresh()
        self.dataChanged.emit()

    def _check_now(self) -> None:
        res = self.watchdog.run_safety_cycle()
        self.refresh()
        self.dataChanged.emit()
        QMessageBox.information(
            self, "Đã kiểm tra",
            f"Tự khôi phục: {len(res['auto_rolled_back'])} gói. Đã xóa gói hết hạn: {len(res['purged'])}.",
        )
```

- [ ] **Step 4: Replace `diskguard/ui/settings_tab.py`**

```python
from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import APP_NAME, APP_VERSION, AUTHOR, MAX_RETENTION_DAYS, MIN_RETENTION_DAYS, WEBSITE_LABEL, WEBSITE_URL
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.settings import AppSettings
from diskguard.core.simulator import SandboxSimulator
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.dialogs import QuotaRuleDialog
from diskguard.ui.widgets import (
    PathCell,
    PathValue,
    TagValue,
    card_title,
    make_card,
    make_table,
    muted_label,
    page_title,
    set_cell,
)


class SettingsTab(QWidget):
    settingsDataChanged = Signal()
    dataRootChangeRequested = Signal()

    def __init__(
        self,
        db: DiskGuardDB,
        enforcer: QuotaEnforcer,
        watchdog: SafetyWatchdog,
        simulator: SandboxSimulator,
        parent: QWidget | None = None,
        settings: AppSettings | None = None,
    ):
        super().__init__(parent)
        self.db = db
        self.enforcer = enforcer
        self.watchdog = watchdog
        self.simulator = simulator
        self.settings = settings or AppSettings(db)

        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        scroll = QScrollArea()
        scroll.setObjectName("PageScroll")
        scroll.setWidgetResizable(True)
        body = QWidget()
        body.setObjectName("PageBody")
        scroll.setWidget(body)
        outer.addWidget(scroll)
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)
        lay.addWidget(page_title("Cài đặt"))

        # Data location
        loc = make_card()
        ll = QVBoxLayout(loc)
        ll.setContentsMargins(14, 12, 14, 12)
        ll.addWidget(card_title("Vị trí lưu dữ liệu"))
        row = QHBoxLayout()
        self._root_holder = QHBoxLayout()
        row.addLayout(self._root_holder, 1)
        self.btn_change_root = QPushButton("Đổi…")
        self.btn_change_root.clicked.connect(self.dataRootChangeRequested.emit)
        row.addWidget(self.btn_change_root)
        ll.addLayout(row)
        self.data_root_hint = muted_label("")
        ll.addWidget(self.data_root_hint)
        lay.addWidget(loc)

        # Retention
        ret = make_card()
        rl = QVBoxLayout(ret)
        rl.setContentsMargins(14, 12, 14, 12)
        rl.addWidget(card_title("Thời gian giữ bản dự phòng"))
        rrow = QHBoxLayout()
        rrow.addWidget(QLabel("Giữ bản dự phòng trong"))
        self.retention_spin = QSpinBox()
        self.retention_spin.setRange(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS)
        self.retention_spin.setValue(self.settings.retention_days_default)
        self.retention_spin.setFixedWidth(70)
        self.retention_spin.valueChanged.connect(self._on_retention_changed)
        rrow.addWidget(self.retention_spin)
        rrow.addWidget(QLabel("ngày trước khi xóa vĩnh viễn"))
        rrow.addStretch(1)
        rl.addLayout(rrow)
        rl.addWidget(muted_label("Áp dụng cho quy tắc mới và lần dọn dẹp tiếp theo."))
        lay.addWidget(ret)

        # Rules
        rules = make_card()
        rul = QVBoxLayout(rules)
        rul.setContentsMargins(14, 12, 14, 12)
        rhead = QHBoxLayout()
        rhead.addWidget(card_title("Quy tắc chạy nền"))
        rhead.addStretch(1)
        btn_new = QPushButton("Thêm quy tắc")
        btn_new.clicked.connect(lambda: self.open_new_rule_dialog())
        rhead.addWidget(btn_new)
        rul.addLayout(rhead)
        self.rules_info_label = muted_label("")
        rul.addWidget(self.rules_info_label)
        self.rules_table = make_table(
            ["Ứng dụng", "Thư mục", "Đang dùng", "Giới hạn", "Trạng thái", "Giữ", ""], stretch_col=1, right_cols=(2, 3)
        )
        self.rules_table.setMinimumHeight(240)
        rul.addWidget(self.rules_table)
        lay.addWidget(rules)

        # About
        about = make_card()
        al = QVBoxLayout(about)
        al.setContentsMargins(14, 12, 14, 12)
        al.addWidget(card_title("Giới thiệu"))
        self.about_label = QLabel(
            f"{APP_NAME} {APP_VERSION}<br>Tác giả: <b>{AUTHOR}</b> · "
            f'<a href="{WEBSITE_URL}">{WEBSITE_LABEL}</a>'
        )
        self.about_label.setTextFormat(Qt.TextFormat.RichText)
        self.about_label.setOpenExternalLinks(True)
        al.addWidget(self.about_label)
        lay.addWidget(about)
        lay.addStretch(1)

        self.refresh_data_root()

    def refresh_all(self) -> None:
        self.refresh_rules_table()
        self.refresh_data_root()

    def refresh_data_root(self) -> None:
        while self._root_holder.count():
            w = self._root_holder.takeAt(0).widget()
            if w is not None:
                w.deleteLater()
        root = self.settings.data_root
        if root is None:
            self._root_holder.addWidget(QLabel("Chưa chọn"))
            self.data_root_hint.setText("Kho dự phòng đang ở thư mục tạm trên ổ C:. Chọn một ổ khác để giải phóng ổ C:.")
        else:
            self._root_holder.addWidget(PathCell(str(root)))
            self.data_root_hint.setText("Kho dự phòng và dữ liệu ứng dụng được chuyển vào thư mục này.")

    def _on_retention_changed(self, days: int) -> None:
        self.settings.set_retention_days_default(days)
        self.settingsDataChanged.emit()

    def refresh_rules_table(self) -> None:
        rules = self.db.list_quota_rules()
        self.rules_info_label.setText(f"{len(rules)} quy tắc · kiểm tra mỗi 45 giây")
        self.rules_table.setRowCount(len(rules))
        for row, rule in enumerate(rules):
            usage = self.enforcer.evaluate_rule_usage(rule)
            phys, pct = usage["physical_bytes"], usage["usage_pct"]
            over = phys > rule.max_bytes
            if not rule.enabled:
                status = TagValue("Tạm tắt", warning=False)
            elif over:
                status = TagValue(f"Vượt mức · {pct:.0f}%", warning=True)
            else:
                status = TagValue(f"Ổn định · {pct:.0f}%")
            set_cell(self.rules_table, row, 0, rule.app_name)
            set_cell(self.rules_table, row, 1, PathValue(rule.target_paths[0] if rule.target_paths else ""))
            set_cell(self.rules_table, row, 2, format_size(phys), right=True)
            set_cell(self.rules_table, row, 3, format_size(rule.max_bytes), right=True)
            set_cell(self.rules_table, row, 4, status)
            set_cell(self.rules_table, row, 5, f"{rule.retention_days} ngày")
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 4, 2)
            h.setSpacing(6)
            btn_edit = QPushButton("Sửa")
            btn_edit.clicked.connect(lambda _=False, r=rule: self._edit_rule(r))
            btn_del = QPushButton("Xóa")
            btn_del.clicked.connect(lambda _=False, r=rule: self._delete_rule(r))
            h.addWidget(btn_edit)
            h.addWidget(btn_del)
            self.rules_table.setCellWidget(row, 6, wrap)

    def open_new_rule_dialog(
        self, app_name: str = "", process_name: str = "", service_name: str = "", paths: list[str] | None = None
    ) -> None:
        dlg = QuotaRuleDialog(
            self, prefill_app_name=app_name, prefill_process_name=process_name,
            prefill_service_name=service_name, prefill_paths=paths,
            default_retention_days=self.settings.retention_days_default,
        )
        if dlg.exec():
            self.db.save_quota_rule(dlg.get_rule())
            self.refresh_rules_table()
            self.settingsDataChanged.emit()

    def _edit_rule(self, rule: QuotaRule) -> None:
        dlg = QuotaRuleDialog(self, rule=rule)
        if dlg.exec():
            self.db.save_quota_rule(dlg.get_rule())
            self.refresh_rules_table()
            self.settingsDataChanged.emit()

    def _delete_rule(self, rule: QuotaRule) -> None:
        if rule.id is None:
            return
        answer = QMessageBox.question(self, "Xóa quy tắc", f"Xóa quy tắc của {rule.app_name}?")
        if answer != QMessageBox.StandardButton.Yes:
            return
        self.db.delete_quota_rule(rule.id)
        self.refresh_rules_table()
        self.settingsDataChanged.emit()
```

- [ ] **Step 5: Create `diskguard/ui/overview_page.py`**

```python
import shutil

from PySide6.QtCore import Signal
from PySide6.QtWidgets import (
    QComboBox,
    QHBoxLayout,
    QProgressBar,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.daily_tab import DailyMonitorTab
from diskguard.ui.theme import repolish
from diskguard.ui.widgets import (
    ExpandableTableCard,
    PathValue,
    TableRow,
    TagValue,
    card_title,
    make_card,
    muted_label,
    page_title,
)


class OverviewPage(QWidget):
    analyzeRequested = Signal()
    cleanupRequested = Signal()
    refreshHistoryRequested = Signal()
    driveChanged = Signal(str)

    def __init__(self, db: DiskGuardDB, drives: list[str], parent: QWidget | None = None):
        super().__init__(parent)
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        scroll = QScrollArea()
        scroll.setObjectName("PageScroll")
        scroll.setWidgetResizable(True)
        body = QWidget()
        body.setObjectName("PageBody")
        scroll.setWidget(body)
        outer.addWidget(scroll)
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        head = QHBoxLayout()
        head.addWidget(page_title("Tổng quan"))
        head.addStretch(1)
        self.btn_analyze = QPushButton("Phân tích")
        self.btn_analyze.clicked.connect(self.analyzeRequested.emit)
        self.btn_clean = QPushButton("Dọn dẹp")
        self.btn_clean.setObjectName("PrimaryButton")
        self.btn_clean.clicked.connect(self.cleanupRequested.emit)
        head.addWidget(self.btn_analyze)
        head.addWidget(self.btn_clean)
        lay.addLayout(head)

        drive_card = make_card()
        dl = QVBoxLayout(drive_card)
        dl.setContentsMargins(14, 12, 14, 12)
        dl.setSpacing(8)
        top = QHBoxLayout()
        top.addWidget(card_title("Ổ đĩa"))
        self.drive_combo = QComboBox()
        self.drive_combo.addItems(drives)
        self.drive_combo.setMinimumWidth(80)
        self.drive_combo.currentTextChanged.connect(self._on_drive_changed)
        top.addWidget(self.drive_combo)
        top.addStretch(1)
        self.drive_stats_label = muted_label("", wrap=False)
        top.addWidget(self.drive_stats_label)
        dl.addLayout(top)
        self.drive_usage_bar = QProgressBar()
        self.drive_usage_bar.setRange(0, 100)
        self.drive_usage_bar.setTextVisible(False)
        dl.addWidget(self.drive_usage_bar)
        lay.addWidget(drive_card)

        self.freeable_card = ExpandableTableCard(
            "Có thể giải phóng", ["Ứng dụng", "Vị trí", "Dung lượng", "Giải phóng", "Đề xuất"], right_detail_cols=(2, 3)
        )
        self.freeable_card.set_empty_text("Chưa phân tích. Bấm Phân tích để tìm dữ liệu có thể dọn hoặc chuyển sang ổ khác.")
        self.freeable_card.set_rows([])
        lay.addWidget(self.freeable_card)

        hist = make_card()
        hl = QVBoxLayout(hist)
        hl.setContentsMargins(14, 12, 14, 12)
        hh = QHBoxLayout()
        hh.addWidget(card_title("Biến động theo ngày"))
        hh.addStretch(1)
        self.btn_refresh_history = QPushButton("Làm mới")
        self.btn_refresh_history.clicked.connect(self.refreshHistoryRequested.emit)
        hh.addWidget(self.btn_refresh_history)
        hl.addLayout(hh)
        self.daily_tab = DailyMonitorTab(db, hist)
        hl.addWidget(self.daily_tab)
        lay.addWidget(hist)
        lay.addStretch(1)

        self.update_drive_usage(self.drive_combo.currentText())

    def _on_drive_changed(self, drive: str) -> None:
        self.update_drive_usage(drive)
        self.driveChanged.emit(drive)

    def update_drive_usage(self, drive: str) -> None:
        if not drive:
            return
        try:
            u = shutil.disk_usage(drive)
        except OSError:
            return
        pct = int(u.used * 100 / max(1, u.total))
        self.drive_usage_bar.setValue(pct)
        self.drive_usage_bar.setObjectName("DangerBar" if pct >= 90 else "")
        repolish(self.drive_usage_bar)
        self.drive_stats_label.setText(f"Còn trống {format_size(u.free)} / {format_size(u.total)}")

    def set_freeable(self, proposals, candidates, target_label: str) -> None:
        rows: list[TableRow] = []
        for p in proposals:
            rows.append(TableRow(
                (p.app_name, "Dọn file cũ", format_size(p.reclaimable_bytes)),
                [p.app_name, PathValue(p.target_paths[0] if p.target_paths else ""),
                 format_size(p.total_physical_bytes), format_size(p.reclaimable_bytes), TagValue("Dọn file cũ")],
                p.reclaimable_bytes,
            ))
        for c in candidates:
            if c.advanced:
                continue
            action = f"Chuyển sang {target_label}"
            rows.append(TableRow(
                (c.name, action, format_size(c.size_bytes)),
                [c.name, PathValue(c.source_path), format_size(c.size_bytes), format_size(c.size_bytes), TagValue(action)],
                c.size_bytes,
            ))
        rows.sort(key=lambda r: r.sort_key, reverse=True)
        self.freeable_card.set_rows(rows)
```

- [ ] **Step 6: Rebuild the shell in `diskguard/ui/main_window.py`**

Replace imports at the top with:

```python
from pathlib import Path

from PySide6.QtCore import Qt, QTimer
from PySide6.QtGui import QAction
from PySide6.QtWidgets import (
    QApplication,
    QButtonGroup,
    QFrame,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMenu,
    QPushButton,
    QStackedWidget,
    QSystemTrayIcon,
    QVBoxLayout,
    QWidget,
)

from diskguard.config import APP_NAME, AUTHOR, SANDBOX_DEMO_DIR, WEBSITE_LABEL, WEBSITE_URL, DEFAULT_DB_PATH
from diskguard.core.app_profiles import discover_installed_heavy_apps
from diskguard.core.disk_tracker import DiskTracker
from diskguard.core.quota_enforcer import QuotaEnforcer
from diskguard.core.safety_watchdog import SafetyWatchdog
from diskguard.core.settings import AppSettings
from diskguard.core.simulator import SandboxSimulator
from diskguard.core.smart_analyzer import AppAnalysisProposal, SmartAppAnalyzer
from diskguard.core.task_controller import TaskControl
from diskguard.models import QuotaRule
from diskguard.storage.database import DiskGuardDB
from diskguard.ui import format_size
from diskguard.ui.icons import app_icon
from diskguard.ui.overview_page import OverviewPage
from diskguard.ui.settings_tab import SettingsTab
from diskguard.ui.smart_cleanup_tab import SmartCleanupTab
from diskguard.ui.theme import QSS
from diskguard.ui.vault_page import VaultPage
from diskguard.ui.workers import BackgroundTaskWorker, TaskProgressCard
```

Replace `__init__` from its signature down to (and including) the `if auto_scan_on_init: ... else: ...` block with:

```python
    def __init__(
        self,
        db: DiskGuardDB | None = None,
        vault_dir: Path | str | None = None,
        sandbox_dir: Path | str | None = None,
        auto_scan_on_init: bool = True,
        settings: AppSettings | None = None,
    ):
        super().__init__()
        self.db = db or DiskGuardDB(DEFAULT_DB_PATH)
        self.settings = settings or AppSettings(self.db)
        self.vault_dir = Path(vault_dir) if vault_dir else self.settings.vault_dir()
        self.sandbox_dir = Path(sandbox_dir) if sandbox_dir else SANDBOX_DEMO_DIR

        self.tracker = DiskTracker(self.db)
        self.enforcer = QuotaEnforcer(self.db, self.vault_dir)
        self.watchdog = SafetyWatchdog(self.db)
        self.simulator = SandboxSimulator(self.db, self.sandbox_dir, self.vault_dir)
        self.analyzer = SmartAppAnalyzer(self.db, self.vault_dir, self.sandbox_dir / "canary_probes")
        self._active_worker: BackgroundTaskWorker | None = None
        # Silent 45s cycle runs on its own QThread (never on the GUI thread) and yields to
        # user-started tasks: they ask it to stop, then start as soon as it has finished.
        self._silent_worker: BackgroundTaskWorker | None = None
        self._pending_launch: tuple | None = None
        self._last_proposals: list[AppAnalysisProposal] = []
        self._last_candidates: list = []

        self.setWindowTitle(APP_NAME)
        self.setWindowIcon(app_icon())
        self.resize(1280, 820)
        self.setStyleSheet(QSS)

        central = QWidget(self)
        central.setObjectName("Content")
        self.setCentralWidget(central)
        root_h = QHBoxLayout(central)
        root_h.setContentsMargins(0, 0, 0, 0)
        root_h.setSpacing(0)

        sidebar = QFrame()
        sidebar.setObjectName("Sidebar")
        sidebar.setFixedWidth(208)
        sb = QVBoxLayout(sidebar)
        sb.setContentsMargins(8, 18, 8, 12)
        sb.setSpacing(2)
        self.brand_label = QLabel('Green <span style="color:#107C10">C:\\</span>')
        self.brand_label.setObjectName("Brand")
        self.brand_label.setTextFormat(Qt.TextFormat.RichText)
        sb.addWidget(self.brand_label)

        self.nav_group = QButtonGroup(self)
        self.nav_group.setExclusive(True)
        self.btn_nav_overview = self._create_nav_button("Tổng quan", 0, checked=True)
        self.btn_nav_cleanup = self._create_nav_button("Dọn dẹp", 1)
        self.btn_nav_relocation = self._create_nav_button("Di chuyển dữ liệu", 2)
        self.btn_nav_vault = self._create_nav_button("Kho dự phòng", 3)
        self.btn_nav_settings = self._create_nav_button("Cài đặt", 4)
        for btn in (self.btn_nav_overview, self.btn_nav_cleanup, self.btn_nav_relocation,
                    self.btn_nav_vault, self.btn_nav_settings):
            sb.addWidget(btn)
        sb.addStretch(1)
        self.author_label = QLabel(f'Tác giả: <b>{AUTHOR}</b><br><a href="{WEBSITE_URL}">{WEBSITE_LABEL}</a>')
        self.author_label.setObjectName("AuthorBlock")
        self.author_label.setTextFormat(Qt.TextFormat.RichText)
        self.author_label.setOpenExternalLinks(True)
        sb.addWidget(self.author_label)
        root_h.addWidget(sidebar)

        content = QWidget()
        content.setObjectName("Content")
        cl = QVBoxLayout(content)
        cl.setContentsMargins(0, 0, 0, 0)
        cl.setSpacing(0)

        self.stack = QStackedWidget()
        self.overview_page = OverviewPage(self.db, self.tracker.list_available_drives(), self)
        self.daily_tab = self.overview_page.daily_tab
        self.drive_combo = self.overview_page.drive_combo
        self.cleanup_tab = SmartCleanupTab(self)
        self.cleanup_tab.set_retention_days(self.settings.retention_days_default)
        self.relocation_page = self._build_relocation_page()
        self.vault_page = VaultPage(self.db, self.watchdog, lambda: self.vault_dir, self)
        self.settings_tab = SettingsTab(self.db, self.enforcer, self.watchdog, self.simulator, self, settings=self.settings)
        for page in (self.overview_page, self.cleanup_tab, self.relocation_page, self.vault_page, self.settings_tab):
            self.stack.addWidget(page)
        cl.addWidget(self.stack, 1)

        self.progress_card = TaskProgressCard(self)
        cl.addWidget(self.progress_card)
        root_h.addWidget(content, 1)

        self.overview_page.analyzeRequested.connect(self.start_smart_app_analysis_async)
        self.overview_page.cleanupRequested.connect(self.btn_nav_cleanup.click)
        self.overview_page.refreshHistoryRequested.connect(self.start_drive_scan_async)
        self.overview_page.driveChanged.connect(self._on_drive_changed)
        self.daily_tab.requestCreateQuota.connect(self._on_request_create_quota)
        self.cleanup_tab.scanAppsRequested.connect(self.start_smart_app_analysis_async)
        self.cleanup_tab.executeCleanupRequested.connect(self.start_one_click_cleanup_async)
        self.settings_tab.settingsDataChanged.connect(self._on_settings_changed)
        self.settings_tab.dataRootChangeRequested.connect(self._on_change_data_root)
        self.vault_page.dataChanged.connect(self.refresh_data_views)

        self._setup_system_tray()

        self._bg_timer = QTimer(self)
        self._bg_timer.timeout.connect(self._run_silent_background_cycle)
        self._bg_timer.start(45_000)

        if auto_scan_on_init:
            self._seed_default_silent_rules()
            cur_drv = self.drive_combo.currentText() or "C:\\"
            existing = self.db.get_daily_snapshots(cur_drv, limit_days=14)
            if existing:
                self.daily_tab.load_drive_data(cur_drv, existing)
                self.refresh_data_views()
            else:
                self.start_drive_scan_async()
        else:
            self.refresh_data_views()
```

Add these methods (and delete `_apply_clean_apple_google_theme` and `_update_drive_header`):

```python
    def _build_relocation_page(self) -> QWidget:
        # Replaced by RelocationPage in Task 13.
        return QWidget()

    def refresh_data_views(self) -> None:
        self.settings_tab.refresh_all()
        self.vault_page.refresh()

    def _target_label(self) -> str:
        root = self.settings.data_root
        return root.drive if root else "ổ khác"

    def _apply_vault_dir(self, vault_dir: Path) -> None:
        self.vault_dir = vault_dir
        self.enforcer.vault_dir = vault_dir
        self.analyzer.vault_dir = vault_dir
        vault_dir.mkdir(parents=True, exist_ok=True)

    def _on_change_data_root(self) -> None:
        # Implemented in Task 13 (needs the drive dialog + Vault move worker).
        pass
```

Rewrite the existing handlers:

```python
    def _on_drive_changed(self, drive: str) -> None:
        if not drive:
            return
        existing = self.db.get_daily_snapshots(drive, limit_days=14)
        if existing:
            self.daily_tab.load_drive_data(drive, existing)
        else:
            self.start_drive_scan_async()

    def start_drive_scan_async(self) -> None:
        drive = self.drive_combo.currentText() or "C:\\"
        self.overview_page.update_drive_usage(drive)

        def _work(ctrl: TaskControl):
            return self.tracker.scan_drive_history_by_day(drive, days_back=14, task_control=ctrl)

        def _done(snaps, stopped: bool):
            if snaps:
                self.daily_tab.load_drive_data(drive, snaps)
            self.refresh_data_views()

        self._launch_background_worker(f"Đang đọc lịch sử ổ {drive[:2]}", _work, _done)

    def start_smart_app_analysis_async(self) -> None:
        def _work(ctrl: TaskControl):
            return {"proposals": self.analyzer.scan_and_probe_apps(task_control=ctrl)}

        def _done(res, stopped: bool):
            self._last_proposals = (res or {}).get("proposals", [])
            self.cleanup_tab.set_proposals(self._last_proposals)
            self.overview_page.set_freeable(self._last_proposals, self._last_candidates, self._target_label())

        self._launch_background_worker("Đang phân tích ứng dụng", _work, _done)

    def start_one_click_cleanup_async(self, proposals: list[AppAnalysisProposal]) -> None:
        retention = self.settings.retention_days_default

        def _work(ctrl: TaskControl):
            return self.analyzer.execute_one_click_cleanup(proposals, task_control=ctrl, retention_days=retention)

        def _done(res: dict | None, stopped: bool):
            self.overview_page.update_drive_usage(self.drive_combo.currentText())
            self.refresh_data_views()
            if res and not stopped:
                msg = f"Đã giải phóng {format_size(res['freed_bytes'])}, giữ dự phòng {retention} ngày."
                if res.get("errors"):
                    msg += " Lỗi: " + "; ".join(res["errors"])
                self.progress_card.update_progress(100, "Đã dọn xong", msg)
                self.cleanup_tab.proposal_summary_label.setText(msg)

        self._launch_background_worker("Đang dọn dẹp", _work, _done)

    def _on_request_create_quota(self, app_name: str, process_name: str, service_name: str, paths: list[str]) -> None:
        self.btn_nav_settings.click()
        self.settings_tab.open_new_rule_dialog(app_name=app_name, process_name=process_name,
                                               service_name=service_name, paths=paths)

    def _on_settings_changed(self) -> None:
        self.cleanup_tab.set_retention_days(self.settings.retention_days_default)
        self.overview_page.update_drive_usage(self.drive_combo.currentText())
```

In `_create_nav_button` keep the body; in `_seed_default_silent_rules` use `retention_days=self.settings.retention_days_default`. In `_launch_background_worker` change the waiting caption to `"Đang chờ tác vụ nền dừng…"`. In `_on_silent_cycle_finished` replace `self.settings_tab.refresh_all()` with `self.refresh_data_views()`; tray messages become title `APP_NAME` with bodies `f"Đã tự khôi phục {n} gói dữ liệu cho ứng dụng."` and `"Không dọn được: " + "; ".join(errors)`. In `run_sandbox_demo` / `simulate_auto_rollback_for_archive` replace `self.settings_tab.refresh_all()` with `self.refresh_data_views()` and drop the `self.daily_tab.load_drive_data(...)` call's drive fallback change (keep the call). In `_setup_system_tray`:

```python
    def _setup_system_tray(self) -> None:
        self.tray_icon = QSystemTrayIcon(self)
        self.tray_icon.setIcon(app_icon())
        self.tray_icon.setToolTip(f"{APP_NAME} — đang chạy nền")
        menu = QMenu(self)
        act_show = QAction(f"Mở {APP_NAME}", self)
        act_show.triggered.connect(self.showNormal)
        act_quit = QAction("Thoát", self)
        act_quit.triggered.connect(QApplication.instance().quit)
        menu.addAction(act_show)
        menu.addSeparator()
        menu.addAction(act_quit)
        self.tray_icon.setContextMenu(menu)
        self.tray_icon.show()
```

Remove `shutil`, `QComboBox`, `QProgressBar`, `QStyle`, `DailyMonitorTab`, `DEFAULT_VAULT_DIR`, `ensure_app_dirs` imports if unused (run `ruff`-style check by reading the import list).

- [ ] **Step 7: Update existing tests**

`tests/test_e2e_app_testcases.py` TC-01: `assert win.stack.currentIndex() == 2` after `btn_nav_settings.click()` → `== 4`. `tests/test_ui_headless.py`: `assert win.stack.count() == 3` → `== 5` and update its comment to `# 0 Tổng quan, 1 Dọn dẹp, 2 Di chuyển dữ liệu, 3 Kho dự phòng, 4 Cài đặt`.

- [ ] **Step 8: Run tests** — `pytest tests/test_ui_shell.py tests/test_e2e_app_testcases.py tests/test_ui_headless.py tests/test_app_performance.py tests/test_ui_silent_cycle_threading.py -q` → PASS; full suite → PASS.

- [ ] **Step 9: Commit**

```bash
git add diskguard/ui tests
git commit -m "feat: Green C:\\ shell with Overview, Vault and Settings pages"
```

---

### Task 13: Relocation page, guided dialog, data-root change, startup

**Files:**
- Create: `diskguard/ui/relocation_page.py`
- Modify: `diskguard/ui/dialogs.py` (add `GuidedDialog`), `diskguard/ui/main_window.py` (relocation wiring, `_on_change_data_root`, analysis also discovers candidates), `main.py`
- Test: `tests/test_ui_relocation.py`

**Interfaces:**
- Consumes: Tasks 2, 3, 5–8, 10, 12.
- Produces: `RelocationPage()` (`.set_target(root, free_bytes)`, `.set_disabled_reason(text | None)`, `.set_candidates(cands)`, `.set_records(records)`, `.table`, `.advanced_table`, `.done_table`, `.advanced_toggle`, `.banner`, signals `relocateRequested(object)`, `guideRequested(object)`, `undoRequested(object)`, `refreshRequested()`); `GuidedDialog(cand, target_root, service, parent=None)` (`.btn_open`, `.btn_verify`, `.result_label`, `.record`); `MainWindow.relocation_service`; `main.main()`.

- [ ] **Step 1: Write the failing test** — `tests/test_ui_relocation.py`

```python
import json
import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication

from diskguard.core.relocation.base import RelocationCandidate
from diskguard.core.relocation.junction import JunctionStrategy
from diskguard.core.relocation.service import RelocationService
from diskguard.core.relocation.zalo import ZaloGuidedStrategy
from diskguard.core.settings import AppSettings
from diskguard.models import RELOCATION_DONE, RelocationRecord
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.dialogs import GuidedDialog
from diskguard.ui.relocation_page import RelocationPage
from diskguard.ui.widgets import PathCell


def _app():
    return QApplication.instance() or QApplication([])


def test_page_lists_candidates_and_emits_actions():
    _app()
    page = RelocationPage()
    page.set_target(Path("D:\\Green C"), 600 * 1024**3)
    tg = RelocationCandidate("telegram", "Telegram Desktop", r"C:\TG", 9 * 1024**3, JunctionStrategy([]), "Chuyển thư mục và tạo liên kết")
    adv = RelocationCandidate("dir-x", "X", r"C:\X", 1024**3, JunctionStrategy([]), "Nâng cao", advanced=True)
    zalo = RelocationCandidate("zalo", "Zalo", r"C:\Z", 30 * 1024**3, ZaloGuidedStrategy(), "Dùng tùy chọn của Zalo")
    page.set_candidates([zalo, tg, adv])
    assert page.table.rowCount() == 2 and page.advanced_table.rowCount() == 1
    assert page.advanced_table.isHidden()
    page.advanced_toggle.setChecked(True)
    assert not page.advanced_table.isHidden()
    got = []
    page.relocateRequested.connect(got.append)
    page.guideRequested.connect(got.append)
    page.action_buttons[0].click()  # Zalo → guide
    page.action_buttons[1].click()  # Telegram → relocate
    assert got == [zalo, tg]
    assert page.action_buttons[0].text() == "Hướng dẫn"


def test_records_and_disabled_banner():
    _app()
    page = RelocationPage()
    rec = RelocationRecord(5, "telegram", "Telegram Desktop", "junction", r"C:\TG", r"D:\Green C\Apps\telegram", 1, RELOCATION_DONE)
    page.set_records([rec])
    assert isinstance(page.done_table.cellWidget(0, 1), PathCell)
    undone = []
    page.undoRequested.connect(undone.append)
    page.undo_buttons[0].click()
    assert undone == [rec]
    page.set_disabled_reason("Chọn ổ dữ liệu khác ổ C:")
    assert not page.banner.isHidden()


def test_guided_dialog_verifies_zalo(tmp_path):
    _app()
    cfg = tmp_path / "config.json"
    cfg.write_text(json.dumps({"db_path": {"location": "C:\\ZaloPC\\Zalo Data\\message"}}), encoding="utf-8")
    db = DiskGuardDB(tmp_path / "g.db")
    svc = RelocationService(db, AppSettings(db))
    cand = RelocationCandidate("zalo", "Zalo", "C:\\ZaloPC\\Zalo Data", 10, ZaloGuidedStrategy(config_path=cfg, system_root="C:\\"), "how")
    dlg = GuidedDialog(cand, Path("D:\\Green C"), svc)
    assert "D:\\Green C\\Zalo" in dlg.steps_label.text()
    dlg.btn_verify.click()
    assert dlg.record is None and "vẫn ở" in dlg.result_label.text()
    cfg.write_text(json.dumps({"db_path": {"location": "D:\\Green C\\Zalo\\Zalo Data\\message"}}), encoding="utf-8")
    dlg.btn_verify.click()
    assert dlg.record is not None and dlg.record.status == RELOCATION_DONE


def test_main_window_shows_relocation_page(tmp_path):
    app = _app()
    from diskguard.ui.main_window import MainWindow

    db = DiskGuardDB(tmp_path / "w.db")
    settings = AppSettings(db)
    win = MainWindow(db=db, vault_dir=tmp_path / "v", sandbox_dir=tmp_path / "s", auto_scan_on_init=False, settings=settings)
    win._bg_timer.stop()
    assert isinstance(win.relocation_page, RelocationPage)
    assert not win.relocation_page.banner.isHidden()  # no data root chosen yet
    settings.set_data_root(tmp_path / "Green C")
    win._refresh_relocation_target()
    win.close()
```

- [ ] **Step 2: Run to verify it fails** — `pytest tests/test_ui_relocation.py -q` → `ModuleNotFoundError: diskguard.ui.relocation_page`.

- [ ] **Step 3: Create `diskguard/ui/relocation_page.py`**

```python
from pathlib import Path

from PySide6.QtCore import Signal
from PySide6.QtWidgets import (
    QCheckBox,
    QFrame,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from diskguard.core.relocation.base import RelocationCandidate
from diskguard.models import RelocationRecord
from diskguard.ui import format_size
from diskguard.ui.widgets import (
    PathValue,
    TagValue,
    card_title,
    fit_table_height,
    make_card,
    make_table,
    muted_label,
    page_title,
    set_cell,
)

STRATEGY_TEXT = {
    "known_folder": "Vị trí của Windows",
    "tool_config": "Cấu hình công cụ",
    "guided": "Tùy chọn của ứng dụng",
    "junction": "Liên kết thư mục",
}


def _name_cell(name: str, how: str) -> QWidget:
    w = QWidget()
    v = QVBoxLayout(w)
    v.setContentsMargins(8, 4, 8, 4)
    v.setSpacing(0)
    title = QLabel(name)
    title.setStyleSheet("font-weight: 600;")
    v.addWidget(title)
    v.addWidget(muted_label(how, wrap=False))
    return w


class RelocationPage(QWidget):
    relocateRequested = Signal(object)  # RelocationCandidate
    guideRequested = Signal(object)  # RelocationCandidate (guided strategy)
    undoRequested = Signal(object)  # RelocationRecord
    refreshRequested = Signal()

    def __init__(self, parent: QWidget | None = None):
        super().__init__(parent)
        self.action_buttons: list[QPushButton] = []
        self.undo_buttons: list[QPushButton] = []
        outer = QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        scroll = QScrollArea()
        scroll.setObjectName("PageScroll")
        scroll.setWidgetResizable(True)
        body = QWidget()
        body.setObjectName("PageBody")
        scroll.setWidget(body)
        outer.addWidget(scroll)
        lay = QVBoxLayout(body)
        lay.setContentsMargins(24, 20, 24, 20)
        lay.setSpacing(12)

        head = QHBoxLayout()
        head.addWidget(page_title("Di chuyển dữ liệu"))
        head.addStretch(1)
        self.target_label = muted_label("", wrap=False)
        head.addWidget(self.target_label)
        self.btn_refresh = QPushButton("Phân tích")
        self.btn_refresh.clicked.connect(self.refreshRequested.emit)
        head.addWidget(self.btn_refresh)
        lay.addLayout(head)

        self.banner = QFrame()
        self.banner.setObjectName("Banner")
        bl = QHBoxLayout(self.banner)
        bl.setContentsMargins(12, 8, 12, 8)
        self.banner_label = QLabel("")
        self.banner_label.setWordWrap(True)
        bl.addWidget(self.banner_label)
        self.banner.hide()
        lay.addWidget(self.banner)

        card = make_card()
        cl = QVBoxLayout(card)
        cl.setContentsMargins(14, 10, 14, 10)
        cl.addWidget(card_title("Có thể chuyển"))
        self.empty_label = muted_label("Bấm Phân tích để tìm dữ liệu ứng dụng đang nằm trên ổ C:.")
        cl.addWidget(self.empty_label)
        self.table = make_table(["Ứng dụng", "Vị trí", "Dung lượng", ""], stretch_col=1, right_cols=(2,))
        self.table.verticalHeader().setDefaultSectionSize(48)
        cl.addWidget(self.table)
        self.advanced_toggle = QCheckBox("Hiện thư mục nâng cao")
        self.advanced_toggle.toggled.connect(lambda on: self.advanced_table.setHidden(not on))
        cl.addWidget(self.advanced_toggle)
        self.advanced_table = make_table(["Thư mục", "Vị trí", "Dung lượng", ""], stretch_col=1, right_cols=(2,))
        self.advanced_table.verticalHeader().setDefaultSectionSize(48)
        self.advanced_table.hide()
        cl.addWidget(self.advanced_table)
        lay.addWidget(card)

        done = make_card()
        dl = QVBoxLayout(done)
        dl.setContentsMargins(14, 10, 14, 10)
        dl.addWidget(card_title("Đã chuyển"))
        self.done_table = make_table(["Ứng dụng", "Vị trí mới", "Cách chuyển", "Trạng thái", ""], stretch_col=1)
        dl.addWidget(self.done_table)
        lay.addWidget(done)
        lay.addStretch(1)

    def set_target(self, root: Path | None, free_bytes: int | None) -> None:
        if root is None:
            self.target_label.setText("")
        else:
            free = f" · Còn trống {format_size(free_bytes)}" if free_bytes is not None else ""
            self.target_label.setText(f"Đích: {root}\\{free}")

    def set_disabled_reason(self, text: str | None) -> None:
        self.banner_label.setText(text or "")
        self.banner.setHidden(not text)
        for btn in self.action_buttons:
            btn.setEnabled(not text)

    def _fill(self, table, cands: list[RelocationCandidate]) -> None:
        table.setRowCount(len(cands))
        for row, cand in enumerate(cands):
            table.setCellWidget(row, 0, _name_cell(cand.name, cand.how))
            set_cell(table, row, 1, PathValue(cand.source_path))
            set_cell(table, row, 2, format_size(cand.size_bytes), right=True)
            guided = cand.strategy.kind == "guided"
            btn = QPushButton("Hướng dẫn" if guided else "Chuyển")
            btn.setObjectName("PrimaryButton")
            btn.setEnabled(self.banner.isHidden())
            signal = self.guideRequested if guided else self.relocateRequested
            btn.clicked.connect(lambda _=False, c=cand, s=signal: s.emit(c))
            self.action_buttons.append(btn)
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 8, 2)
            h.addWidget(btn)
            table.setCellWidget(row, 3, wrap)
        fit_table_height(table, max_rows=10)

    def set_candidates(self, cands: list[RelocationCandidate]) -> None:
        self.action_buttons = []
        main = [c for c in cands if not c.advanced]
        adv = [c for c in cands if c.advanced]
        self._fill(self.table, main)
        self._fill(self.advanced_table, adv)
        self.empty_label.setText(
            "Không có dữ liệu ứng dụng lớn nào trên ổ C:." if not cands else ""
        )
        self.empty_label.setHidden(bool(cands))
        self.table.setHidden(not main)
        self.advanced_toggle.setHidden(not adv)

    def set_records(self, records: list[RelocationRecord]) -> None:
        self.undo_buttons = []
        self.done_table.setRowCount(len(records))
        for row, rec in enumerate(records):
            set_cell(self.done_table, row, 0, rec.app_name)
            set_cell(self.done_table, row, 1, PathValue(rec.target_path))
            set_cell(self.done_table, row, 2, STRATEGY_TEXT.get(rec.strategy, rec.strategy))
            set_cell(self.done_table, row, 3, TagValue("Hoạt động bình thường" if not rec.detail else rec.detail,
                                                       warning=bool(rec.detail)))
            btn = QPushButton("Hoàn tác")
            btn.setEnabled(rec.strategy != "guided")
            btn.setToolTip("Đổi lại vị trí lưu trong Cài đặt của Zalo" if rec.strategy == "guided" else "")
            btn.clicked.connect(lambda _=False, r=rec: self.undoRequested.emit(r))
            self.undo_buttons.append(btn)
            wrap = QWidget()
            h = QHBoxLayout(wrap)
            h.setContentsMargins(4, 2, 8, 2)
            h.addWidget(btn)
            self.done_table.setCellWidget(row, 4, wrap)
        fit_table_height(self.done_table, max_rows=10)
```

- [ ] **Step 4: Add `GuidedDialog` to `diskguard/ui/dialogs.py`**

Imports to add: `from pathlib import Path`, `from PySide6.QtWidgets import QHBoxLayout, QLabel`, `from diskguard.core.drives import is_on_system_drive`, `from diskguard.core.relocation.base import RelocationCandidate`, `from diskguard.core.relocation.service import RelocationService`, `from diskguard.ui.widgets import page_title`.

```python
class GuidedDialog(QDialog):
    """Step-by-step instructions for apps that move their own data (Zalo)."""

    def __init__(self, cand: RelocationCandidate, target_root: Path, service: RelocationService, parent=None):
        super().__init__(parent)
        self.cand = cand
        self.service = service
        self.record = None
        self.setWindowTitle(f"Chuyển dữ liệu {cand.name}")
        self.setStyleSheet(QSS)
        self.setMinimumWidth(520)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(22, 20, 22, 16)
        lay.setSpacing(10)
        lay.addWidget(page_title(f"Chuyển dữ liệu {cand.name}"))
        lay.addWidget(muted_label(
            f"{cand.name} tự chuyển dữ liệu của mình, Green C:\\ chỉ hướng dẫn và kiểm tra lại. "
            f"Hiện đang dùng {format_size(cand.size_bytes)} trên ổ C:."
        ))
        steps = cand.strategy.steps(target_root)
        self.steps_label = QLabel("<ol style='margin-left:-20px'>" + "".join(f"<li>{s}</li>" for s in steps) + "</ol>")
        self.steps_label.setTextFormat(Qt.TextFormat.RichText)
        self.steps_label.setWordWrap(True)
        lay.addWidget(self.steps_label)
        self.result_label = muted_label("")
        lay.addWidget(self.result_label)
        row = QHBoxLayout()
        self.btn_open = QPushButton(f"Mở {cand.name}")
        self.btn_open.clicked.connect(self._open)
        self.btn_verify = QPushButton("Kiểm tra lại")
        self.btn_verify.setObjectName("PrimaryButton")
        self.btn_verify.clicked.connect(self._verify)
        close = QPushButton("Đóng")
        close.clicked.connect(self.reject)
        row.addWidget(self.btn_open)
        row.addStretch(1)
        row.addWidget(close)
        row.addWidget(self.btn_verify)
        lay.addLayout(row)

    def _open(self) -> None:
        if not self.cand.strategy.open_app():
            self.result_label.setText(f"Không tìm thấy {self.cand.name} trên máy. Hãy mở bằng tay.")

    def _verify(self) -> None:
        rec = self.service.complete_guided(self.cand)
        if rec is None:
            current = self.cand.strategy.data_dir()
            self.result_label.setText(f"Chưa thấy thay đổi. Dữ liệu {self.cand.name} vẫn ở {current}.")
            return
        self.record = rec
        self.result_label.setText(f"Đã chuyển xong. Dữ liệu mới ở {rec.target_path}.")
        self.btn_verify.setEnabled(False)
```

The dialog never touches the disk; `MainWindow._on_guide` pre-creates `<root>\Zalo` so the user can pick it in Zalo's folder picker.

- [ ] **Step 5: Wire relocation into `MainWindow`**

Imports to add:

```python
from PySide6.QtWidgets import QMessageBox
from diskguard.core.drives import is_on_system_drive, list_drives
from diskguard.core.relocation.catalog import discover_candidates
from diskguard.core.relocation.fs_ops import RelocationError, RelocationStopped
from diskguard.core.relocation.service import RelocationService
from diskguard.core.storage_migration import move_vault, remove_dir_if_empty
from diskguard.ui.data_location import FirstRunDialog
from diskguard.ui.dialogs import GuidedDialog
from diskguard.ui.relocation_page import RelocationPage
```

In `__init__`, right after `self.analyzer = ...`, add `self.relocation_service = RelocationService(self.db, self.settings)`. After the signal wiring block add:

```python
        self.relocation_page.relocateRequested.connect(self._on_relocate)
        self.relocation_page.guideRequested.connect(self._on_guide)
        self.relocation_page.undoRequested.connect(self._on_undo_relocation)
        self.relocation_page.refreshRequested.connect(self.start_smart_app_analysis_async)
        self._refresh_relocation_target()
        self.relocation_page.set_records(self.relocation_service.records())
```

Replace `_build_relocation_page` and `_on_change_data_root`, and extend analysis:

```python
    def _build_relocation_page(self) -> QWidget:
        return RelocationPage(self)

    def _refresh_relocation_target(self) -> None:
        root = self.settings.data_root
        free = None
        if root is not None:
            try:
                free = shutil.disk_usage(root.anchor).free
            except OSError:
                free = None
        self.relocation_page.set_target(root, free)
        if root is None:
            self.relocation_page.set_disabled_reason("Chưa chọn ổ lưu dữ liệu. Chọn trong Cài đặt → Vị trí lưu dữ liệu.")
        elif is_on_system_drive(root):
            self.relocation_page.set_disabled_reason("Ổ lưu dữ liệu đang là ổ C:. Chọn một ổ khác trong Cài đặt để chuyển dữ liệu ứng dụng.")
        else:
            self.relocation_page.set_disabled_reason(None)

    def start_smart_app_analysis_async(self) -> None:
        def _work(ctrl: TaskControl):
            proposals = self.analyzer.scan_and_probe_apps(task_control=ctrl)
            candidates = []
            if not ctrl.is_stopped:
                ctrl.report(97, "Đang tìm dữ liệu có thể chuyển", "")
                candidates = discover_candidates(self.db, should_stop=lambda: ctrl.is_stopped)
            return {"proposals": proposals, "candidates": candidates}

        def _done(res, stopped: bool):
            res = res or {}
            self._last_proposals = res.get("proposals", [])
            self._last_candidates = res.get("candidates", [])
            self.cleanup_tab.set_proposals(self._last_proposals)
            self.relocation_page.set_candidates(self._last_candidates)
            self._refresh_relocation_target()
            self.overview_page.set_freeable(self._last_proposals, self._last_candidates, self._target_label())

        self._launch_background_worker("Đang phân tích ứng dụng", _work, _done)

    def _on_relocate(self, cand) -> None:
        check = self.relocation_service.check(cand)
        if not check.ok:
            QMessageBox.warning(self, "Chưa thể chuyển", check.reason)
            return
        target = self.settings.data_root
        answer = QMessageBox.question(
            self, f"Chuyển {cand.name}",
            f"Chuyển {cand.name} ({format_size(cand.size_bytes)}) sang {target}?\n\n"
            "Dữ liệu được sao chép và kiểm tra trước, sau đó mới xóa ở ổ C:. Có thể hoàn tác.",
        )
        if answer != QMessageBox.StandardButton.Yes:
            return

        def _work(ctrl: TaskControl):
            try:
                return {"record": self.relocation_service.apply(cand, ctrl)}
            except RelocationStopped:
                return {"error": "Đã dừng. Dữ liệu vẫn ở vị trí cũ."}
            except RelocationError as err:
                return {"error": str(err)}

        def _done(res, stopped: bool):
            res = res or {}
            if res.get("error"):
                QMessageBox.warning(self, f"Không chuyển được {cand.name}", res["error"])
            else:
                self._last_candidates = [c for c in self._last_candidates if c is not cand]
                self.relocation_page.set_candidates(self._last_candidates)
                self.overview_page.set_freeable(self._last_proposals, self._last_candidates, self._target_label())
                self.progress_card.update_progress(100, f"Đã chuyển {cand.name}", res["record"].target_path)
            self.relocation_page.set_records(self.relocation_service.records())
            self.overview_page.update_drive_usage(self.drive_combo.currentText())
            self._refresh_relocation_target()

        self._launch_background_worker(f"Đang chuyển {cand.name}", _work, _done)

    def _on_guide(self, cand) -> None:
        root = self.relocation_service.target_root()
        try:
            (root / "Zalo").mkdir(parents=True, exist_ok=True)
        except OSError:
            pass
        dlg = GuidedDialog(cand, root, self.relocation_service, self)
        dlg.exec()
        if dlg.record is not None:
            self._last_candidates = [c for c in self._last_candidates if c is not cand]
            self.relocation_page.set_candidates(self._last_candidates)
            self.relocation_page.set_records(self.relocation_service.records())

    def _on_undo_relocation(self, rec) -> None:
        answer = QMessageBox.question(
            self, f"Hoàn tác {rec.app_name}",
            f"Trả {rec.app_name} về vị trí cũ trên ổ C: ({format_size(rec.size_bytes)})?",
        )
        if answer != QMessageBox.StandardButton.Yes:
            return

        def _work(ctrl: TaskControl):
            try:
                self.relocation_service.undo(rec, ctrl)
                return {}
            except RelocationError as err:
                return {"error": str(err)}

        def _done(res, stopped: bool):
            if (res or {}).get("error"):
                QMessageBox.warning(self, f"Không hoàn tác được {rec.app_name}", res["error"])
            self.relocation_page.set_records(self.relocation_service.records())
            self.overview_page.update_drive_usage(self.drive_combo.currentText())

        self._launch_background_worker(f"Đang hoàn tác {rec.app_name}", _work, _done)

    def _on_change_data_root(self) -> None:
        dlg = FirstRunDialog(
            list_drives(), self.settings.data_root, self.settings.retention_days_default, self,
            title="Đổi nơi lưu dữ liệu", allow_later=False,
        )
        if not dlg.exec():
            return
        new_root = dlg.chosen_root()
        self.settings.set_retention_days_default(dlg.chosen_retention())
        self.settings_tab.retention_spin.setValue(self.settings.retention_days_default)
        if new_root is None or new_root == self.settings.data_root:
            return
        new_vault = new_root / "Vault"
        old_vault = self.vault_dir

        def _work(ctrl: TaskControl):
            return move_vault(self.db, new_vault, ctrl)

        def _done(res, stopped: bool):
            res = res or {"moved": 0, "errors": []}
            if res["errors"]:
                QMessageBox.warning(self, "Chưa chuyển hết Kho dự phòng", "\n".join(res["errors"]))
                return
            self.settings.set_data_root(new_root)
            self._apply_vault_dir(new_vault)
            remove_dir_if_empty(old_vault)
            self.refresh_data_views()
            self._refresh_relocation_target()

        self._launch_background_worker("Đang chuyển Kho dự phòng", _work, _done)
```

Add `import shutil` at the top of `main_window.py`.

- [ ] **Step 6: Replace `main.py`**

```python
import sys

from PySide6.QtWidgets import QApplication

from diskguard.config import (
    APP_NAME,
    DEFAULT_DB_PATH,
    LEGACY_DB_PATH,
    LEGACY_VAULT_DIR,
    ensure_app_dirs,
)
from diskguard.core.drives import list_drives
from diskguard.core.relocation.recovery import recover_interrupted
from diskguard.core.settings import AppSettings
from diskguard.core.storage_migration import migrate_legacy_db, move_vault, remove_dir_if_empty
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.data_location import run_first_run
from diskguard.ui.icons import app_icon
from diskguard.ui.main_window import MainWindow
from diskguard.ui.theme import apply_theme


def main() -> int:
    app = QApplication(sys.argv)
    app.setApplicationName(APP_NAME)
    app.setWindowIcon(app_icon())
    apply_theme(app)

    ensure_app_dirs()
    migrate_legacy_db(LEGACY_DB_PATH, DEFAULT_DB_PATH)
    db = DiskGuardDB(DEFAULT_DB_PATH)
    settings = AppSettings(db)
    recover_interrupted(db)

    if settings.data_root is None:
        run_first_run(settings, list_drives())
    vault_dir = settings.vault_dir()
    vault_dir.mkdir(parents=True, exist_ok=True)
    if LEGACY_VAULT_DIR.exists():
        move_vault(db, vault_dir)
        remove_dir_if_empty(LEGACY_VAULT_DIR)

    window = MainWindow(db=db, vault_dir=vault_dir, auto_scan_on_init=True, settings=settings)
    window.show()
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 7: Run tests** — `pytest tests/test_ui_relocation.py -q` → PASS; full suite `pytest -q` → all pass.

- [ ] **Step 8: Visual check** — render every page offscreen with a temp DB (timer stopped) to PNG in the scratchpad and inspect them; fix spacing/copy issues found.

```bash
QT_QPA_PLATFORM=offscreen .venv/Scripts/python.exe - <<'EOF'
import tempfile
from pathlib import Path
from PySide6.QtWidgets import QApplication
app = QApplication([])
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.main_window import MainWindow
tmp = Path(tempfile.mkdtemp())
win = MainWindow(db=DiskGuardDB(tmp / "s.db"), vault_dir=tmp / "v", sandbox_dir=tmp / "sb", auto_scan_on_init=False)
win._bg_timer.stop(); win.resize(1280, 820); win.show(); app.processEvents()
for i in range(win.stack.count()):
    win.stack.setCurrentIndex(i); app.processEvents(); win.grab().save(str(tmp / f"page_{i}.png"))
print(tmp)
EOF
```

- [ ] **Step 9: Commit**

```bash
git add diskguard/ui main.py tests/test_ui_relocation.py
git commit -m "feat: data relocation page, Zalo guide, data drive change and first-run startup"
```

---

## Self-review notes

- Spec §1 branding → Tasks 1, 12. §2 storage/settings/eligibility/migration/drive change → Tasks 1–3, 13. §3 first run → Tasks 10, 13. §4 strategies, junction procedure, recovery, undo → Tasks 4–8, 13. §5 UI (theme, icons, widgets, pages, copy) → Tasks 9, 11–13. §6 error handling → worker wrappers in Task 13, rollback paths in Tasks 5–7. §7 testing → each task.
- Names cross-checked: `RelocationCandidate.strategy.kind` (`"guided"`) used by `RelocationPage`; `strategy_for_record` extended in Tasks 6 and 7; `TaskProgressCard` keeps `title_label/btn_pause/btn_stop/progress_bar/update_progress` used by existing tests; `set_cell` + `PathValue` keep item text for `folder_table.item(0, 0).text()`.
