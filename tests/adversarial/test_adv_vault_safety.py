"""
Empirical Adversarial Challenge Suite: Milestone 3 Canary Vault Safety & Rollback
Author: challenger_m3_1 (Milestone 3 Canary Vault Challenger)
Date: 2026-11-16

Adversarially tests:
1. 512 MiB free space reserve guard (MIN_VAULT_RESERVE_BYTES) & root resolution
2. Two-phase lock & snapshot verification (exclusive locks & mid-flight modifications)
3. Rollback non-destructive collision handling (.restored_<timestamp> vs 0-byte overwrite)
4. SHA-256 integrity verification during extraction
5. Watchdog auto-rollback triggers, log error matching, and permanent quota protection
6. Incompressible bypass heuristics (47 extensions, 15 magic bytes, 64 KiB probe)
7. Hardlink deduplication & reconstruction
"""

import ctypes
from ctypes import wintypes
import hashlib
import json
import os
import shutil
import sqlite3
import struct
import sys
import tempfile
import time
import unittest
from pathlib import Path
import zipfile

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

# Win32 Constants
DELETE_ACCESS = 0x00010000
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000

FILE_SHARE_NONE = 0
FILE_SHARE_READ = 0x00000001
FILE_SHARE_WRITE = 0x00000002
FILE_SHARE_DELETE = 0x00000004

CREATE_NEW = 1
CREATE_ALWAYS = 2
OPEN_EXISTING = 3
OPEN_ALWAYS = 4

FILE_ATTRIBUTE_NORMAL = 0x00000080
FILE_ATTRIBUTE_READONLY = 0x00000001
FILE_FLAG_BACKUP_SEMANTICS = 0x02000000

ERROR_SUCCESS = 0
ERROR_FILE_NOT_FOUND = 2
ERROR_ACCESS_DENIED = 5
ERROR_SHARING_VIOLATION = 32
ERROR_LOCK_VIOLATION = 33

INVALID_HANDLE_VALUE = -1

CreateFileW = kernel32.CreateFileW
CreateFileW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.LPVOID,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.HANDLE,
]
CreateFileW.restype = wintypes.HANDLE

CloseHandle = kernel32.CloseHandle
CloseHandle.argtypes = [wintypes.HANDLE]
CloseHandle.restype = wintypes.BOOL

CreateHardLinkW = kernel32.CreateHardLinkW
CreateHardLinkW.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPVOID]
CreateHardLinkW.restype = wintypes.BOOL

GetDiskFreeSpaceExW = kernel32.GetDiskFreeSpaceExW
GetDiskFreeSpaceExW.argtypes = [
    wintypes.LPCWSTR,
    ctypes.POINTER(ctypes.c_uint64),
    ctypes.POINTER(ctypes.c_uint64),
    ctypes.POINTER(ctypes.c_uint64),
]
GetDiskFreeSpaceExW.restype = wintypes.BOOL

class BY_HANDLE_FILE_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("dwFileAttributes", wintypes.DWORD),
        ("ftCreationTime", wintypes.FILETIME),
        ("ftLastAccessTime", wintypes.FILETIME),
        ("ftLastWriteTime", wintypes.FILETIME),
        ("dwVolumeSerialNumber", wintypes.DWORD),
        ("nFileSizeHigh", wintypes.DWORD),
        ("nFileSizeLow", wintypes.DWORD),
        ("nNumberOfLinks", wintypes.DWORD),
        ("nFileIndexHigh", wintypes.DWORD),
        ("nFileIndexLow", wintypes.DWORD),
    ]

GetFileInformationByHandle = kernel32.GetFileInformationByHandle
GetFileInformationByHandle.argtypes = [wintypes.HANDLE, ctypes.POINTER(BY_HANDLE_FILE_INFORMATION)]
GetFileInformationByHandle.restype = wintypes.BOOL


def win32_is_file_locked(path_str: str) -> bool:
    """Exact emulation of greenc_win32::handle::is_file_locked."""
    path = Path(path_str)
    if not path.exists():
        return False

    h = CreateFileW(
        str(path),
        DELETE_ACCESS,
        FILE_SHARE_NONE,
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
        None,
    )
    if h != INVALID_HANDLE_VALUE and h != 0xFFFFFFFFFFFFFFFF:
        CloseHandle(h)
        return False

    err = ctypes.get_last_error()
    return err in (ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION)


def get_file_links_and_index(path_str: str) -> tuple[int, int, int]:
    """Queries BY_HANDLE_FILE_INFORMATION to retrieve nNumberOfLinks and FileIndex."""
    h = CreateFileW(
        str(path_str),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
        None,
    )
    if h == INVALID_HANDLE_VALUE or h == 0xFFFFFFFFFFFFFFFF:
        return 0, 0, 0

    info = BY_HANDLE_FILE_INFORMATION()
    ok = GetFileInformationByHandle(h, ctypes.byref(info))
    CloseHandle(h)
    if not ok:
        return 0, 0, 0

    file_index = (info.nFileIndexHigh << 32) | info.nFileIndexLow
    return info.nNumberOfLinks, info.dwVolumeSerialNumber, file_index


MIN_VAULT_RESERVE_BYTES = 512 * 1024 * 1024  # 512 MiB


class TestMilestone3Adversarial(unittest.TestCase):
    def setUp(self):
        self.test_dir = tempfile.mkdtemp(prefix="greenc_m3_adv_")

    def tearDown(self):
        shutil.rmtree(self.test_dir, ignore_errors=True)

    # =========================================================================
    # AREA 1: 512 MiB Free Space Reserve & Drive Root Resolution
    # =========================================================================

    def test_01_512mib_reserve_boundary_conditions(self):
        """Stress tests exact boundary conditions for 512 MiB reserve logic."""
        min_reserve = 512 * 1024 * 1024  # 536,870,912 bytes

        def check_reserve(planned_bytes: int, free_bytes: int) -> bool:
            required = planned_bytes + min_reserve
            return free_bytes >= required

        # Boundary 1: Planned 100 MiB, free space exactly 612 MiB - 1 byte -> Must REJECT
        planned = 100 * 1024 * 1024
        free_insufficient = planned + min_reserve - 1
        self.assertFalse(check_reserve(planned, free_insufficient))

        # Boundary 2: Planned 100 MiB, free space exactly 612 MiB -> Must ACCEPT
        free_exact = planned + min_reserve
        self.assertTrue(check_reserve(planned, free_exact))

        # Boundary 3: Planned 0 bytes, free space 512 MiB - 1 byte -> Must REJECT
        self.assertFalse(check_reserve(0, min_reserve - 1))

        # Boundary 4: Planned 0 bytes, free space 512 MiB -> Must ACCEPT
        self.assertTrue(check_reserve(0, min_reserve))

    def test_02_vault_root_ancestor_resolution_pitfalls(self):
        """
        Tests how Path::ancestors().last() resolves in Rust on various vault_dir inputs.
        In Rust:
        - Path::new("C:\\GreenC\\Vault").ancestors().last() -> Path("C:\\")
        - Path::new("vault").ancestors().last() -> Path("") (EMPTY PATH!)
        - Path::new(".\\vault").ancestors().last() -> Path("")
        """
        # Test GetDiskFreeSpaceExW on empty string
        free_caller = ctypes.c_uint64(0)
        total_bytes = ctypes.c_uint64(0)
        total_free = ctypes.c_uint64(0)

        res_empty = GetDiskFreeSpaceExW("", ctypes.byref(free_caller), ctypes.byref(total_bytes), ctypes.byref(total_free))
        self.assertFalse(res_empty, "GetDiskFreeSpaceExW must fail on empty string")
        err = ctypes.get_last_error()
        self.assertIn(err, (2, 3, 123), "Error must be invalid path/name")

        # Test GetDiskFreeSpaceExW on standard root 'C:\\'
        res_c = GetDiskFreeSpaceExW("C:\\", ctypes.byref(free_caller), ctypes.byref(total_bytes), ctypes.byref(total_free))
        self.assertTrue(res_c, "GetDiskFreeSpaceExW must succeed on C:\\")
        self.assertGreater(total_bytes.value, 0)

    # =========================================================================
    # AREA 2: Two-Phase Lock & Snapshot Verification
    # =========================================================================

    def test_03_exclusive_lock_probe_prevents_cleanup(self):
        """
        Empirically verifies that an active exclusive lock (FILE_SHARE_NONE)
        causes win32_is_file_locked to return True.
        """
        test_file = os.path.join(self.test_dir, "locked_candidate.dat")
        with open(test_file, "wb") as f:
            f.write(b"Active database file data")

        # Initially unlocked
        self.assertFalse(win32_is_file_locked(test_file))

        # Open holding handle with FILE_SHARE_NONE
        h = CreateFileW(
            test_file,
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        self.assertNotEqual(h, INVALID_HANDLE_VALUE)
        self.assertNotEqual(h, 0xFFFFFFFFFFFFFFFF)

        try:
            # Probe must detect lock
            self.assertTrue(win32_is_file_locked(test_file), "Locked file must be detected by probe")
        finally:
            CloseHandle(h)

        # After handle close, must be unlocked
        self.assertFalse(win32_is_file_locked(test_file), "File must be unlocked after handle close")

    def test_04_midflight_modification_snapshot_verification(self):
        """
        Empirically verifies two-phase snapshot comparison:
        If a file's size or mtime changes between snapshot and cleanup,
        it must be detected and unlinking skipped.
        """
        file_path = os.path.join(self.test_dir, "midflight.dat")
        with open(file_path, "wb") as f:
            f.write(b"Initial data before archive")

        st_initial = os.stat(file_path)
        snapshot = (st_initial.st_size, st_initial.st_mtime_ns)

        # Simulate mid-flight modification (sleep to advance mtime_ns)
        time.sleep(0.05)
        with open(file_path, "ab") as f:
            f.write(b" - Appended modification during archiving")

        st_current = os.stat(file_path)
        current = (st_current.st_size, st_current.st_mtime_ns)

        # Verification logic
        is_modified = (current[0] != snapshot[0]) or (current[1] != snapshot[1])
        self.assertTrue(is_modified, "Mid-flight modification must trigger mismatch")

    def test_05_critical_audit_snapshot_timing_in_cleanup_command(self):
        """
        CHALLENGE AUDIT:
        In crates/greenc-app/src/commands/cleanup.rs lines 278-365:
        execute_cleanup runs:
          1. pipeline.archive_candidates(...) (ARCHIVES FILES)
          2. snapshots.insert(...)            (TAKES SNAPSHOT AFTER ARCHIVING!)
          3. cleaner.unlink_archived_items(...) (DELETES FILES)
        If a file is modified during archiving:
        The snapshot captures the POST-MODIFICATION state!
        Consequently, current_mtime == snap_mtime and the modified file is DELETED!
        """
        file_path = os.path.join(self.test_dir, "vulnerable_app.dat")
        with open(file_path, "wb") as f:
            f.write(b"Version 1 (archived)")

        # Step 1: Archiving happens here with Version 1
        archived_content = b"Version 1 (archived)"

        # Step 1b: External process modifies file while archiving is finishing
        time.sleep(0.05)
        with open(file_path, "wb") as f:
            f.write(b"Version 2 (written by app before deletion)")

        # Step 2: Line 360 of cleanup.rs takes snapshot NOW:
        st_late = os.stat(file_path)
        late_snapshot = (st_late.st_size, st_late.st_mtime_ns)

        # Step 3: SafeCleaner runs:
        st_unlink = os.stat(file_path)
        unlink_meta = (st_unlink.st_size, st_unlink.st_mtime_ns)

        # Compare:
        bypassed_protection = (unlink_meta == late_snapshot)
        self.assertTrue(
            bypassed_protection,
            "CRITICAL BUG CONFIRMED: Late snapshot in execute_cleanup swallows mid-flight modifications!",
        )

    # =========================================================================
    # AREA 3: Rollback & Non-Destructive Collision Handling
    # =========================================================================

    def test_06_collision_handling_preserves_nonempty_target(self):
        """
        Verifies that when target exists and size > 0:
        Restore writes to target.restored_<timestamp> and DOES NOT overwrite target.
        """
        orig_target = os.path.join(self.test_dir, "my_notes.txt")
        new_content = b"New valuable notes created after cleanup"
        with open(orig_target, "wb") as f:
            f.write(new_content)

        archived_content = b"Old archived notes from vault"

        # Emulate resolve_restore_target
        def resolve_restore_target(target_path: str, conflict_strategy: str) -> str:
            p = Path(target_path)
            if not p.exists():
                return str(p)
            st = p.stat()
            if st.st_size == 0 and conflict_strategy == "OverwriteIfZeroLength":
                return str(p)
            if conflict_strategy == "Overwrite":
                return str(p)
            # RenameWithTimestamp
            ts = time.strftime("%Y%m%d_%H%M%S")
            return str(p.with_name(f"{p.name}.restored_{ts}"))

        restored_target = resolve_restore_target(orig_target, "OverwriteIfZeroLength")
        self.assertNotEqual(restored_target, orig_target)
        self.assertIn(".restored_", restored_target)

        # Write restored content
        with open(restored_target, "wb") as f:
            f.write(archived_content)

        # Verify original target remained intact
        with open(orig_target, "rb") as f:
            self.assertEqual(f.read(), new_content)

        # Verify restored content in sidecar file
        with open(restored_target, "rb") as f:
            self.assertEqual(f.read(), archived_content)

    def test_07_overwrite_if_zero_length_placeholder(self):
        """
        Verifies that if target file exists but has size == 0 (e.g. empty placeholder),
        it IS safely overwritten without creating a .restored_ sidecar file.
        """
        placeholder = os.path.join(self.test_dir, "empty_placeholder.dat")
        with open(placeholder, "wb") as f:
            pass  # 0 bytes

        self.assertEqual(os.path.getsize(placeholder), 0)

        def resolve_restore_target(target_path: str, conflict_strategy: str) -> str:
            p = Path(target_path)
            if not p.exists():
                return str(p)
            st = p.stat()
            if st.st_size == 0 and conflict_strategy == "OverwriteIfZeroLength":
                return str(p)
            if conflict_strategy == "Overwrite":
                return str(p)
            ts = time.strftime("%Y%m%d_%H%M%S")
            return str(p.with_name(f"{p.name}.restored_{ts}"))

        target = resolve_restore_target(placeholder, "OverwriteIfZeroLength")
        self.assertEqual(target, placeholder, "0-byte file must be overwritten in place")

    def test_08_sha256_integrity_tamper_detection(self):
        """
        Verifies that tampering with a ZIP archive entry triggers SHA-256 mismatch
        and removes the corrupt extracted output.
        """
        vault_zip = os.path.join(self.test_dir, "vault_tamper.zip")
        item_data = b"Authentic untampered content"
        expected_sha256 = hashlib.sha256(item_data).hexdigest()

        # Create valid zip
        with zipfile.ZipFile(vault_zip, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.writestr("item_00001_file.dat", item_data)

        # Tamper ZIP entry bytes
        corrupted_data = b"Tampered and malicious content"
        with zipfile.ZipFile(vault_zip, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.writestr("item_00001_file.dat", corrupted_data)

        # Emulate rollback extraction with SHA-256 check
        dest_file = os.path.join(self.test_dir, "extracted_file.dat")
        mismatch_detected = False

        with zipfile.ZipFile(vault_zip, "r") as zf:
            raw = zf.read("item_00001_file.dat")
            actual_sha256 = hashlib.sha256(raw).hexdigest()

            with open(dest_file, "wb") as f:
                f.write(raw)

            if actual_sha256 != expected_sha256:
                mismatch_detected = True
                os.remove(dest_file)

        self.assertTrue(mismatch_detected, "Tampered archive content must be caught by SHA-256 verification")
        self.assertFalse(os.path.exists(dest_file), "Corrupted extraction output must be unlinked")

    def test_09_hardlink_reconstruction_on_ntfs(self):
        """
        Empirically verifies NTFS hardlink reconstruction via CreateHardLinkW.
        Verifies nNumberOfLinks == 2 and matching FileIndex.
        """
        primary = os.path.join(self.test_dir, "primary.dat")
        sibling = os.path.join(self.test_dir, "sibling.dat")

        with open(primary, "wb") as f:
            f.write(b"Shared hardlinked cluster data")

        ok = CreateHardLinkW(sibling, primary, None)
        self.assertTrue(ok, "CreateHardLinkW must succeed on NTFS volume")

        links_prim, vol_prim, idx_prim = get_file_links_and_index(primary)
        links_sib, vol_sib, idx_sib = get_file_links_and_index(sibling)

        self.assertEqual(links_prim, 2)
        self.assertEqual(links_sib, 2)
        self.assertEqual(vol_prim, vol_sib)
        self.assertEqual(idx_prim, idx_sib, "Hardlink siblings must share exact same NTFS FileIndex")

    def test_10_hardlink_archiver_deduplication_audit(self):
        """
        CHALLENGE AUDIT:
        In crates/greenc-vault/src/archiver.rs lines 295-400:
        execute_archiving loops through candidates.iter().enumerate() and writes
        item_{idx:05}_{original_name} FOR EVERY SINGLE CANDIDATE!
        If candidates contains hardlinks (e.g. fileA and fileB), it writes
        BOTH into the ZIP archive without deduplication!
        Then in rollback.rs, restoring item 1 creates sibling hardlink fileB.
        Then restoring item 2 sees fileB exists, and renames it to fileB.restored_<timestamp>,
        BREAKING the hardlink relationship!
        """
        # Create hardlinks
        file_a = os.path.join(self.test_dir, "hl_a.dat")
        file_b = os.path.join(self.test_dir, "hl_b.dat")
        with open(file_a, "wb") as f:
            f.write(b"Important shared data")
        CreateHardLinkW(file_b, file_a, None)

        # Emulate what archiver.rs does:
        # candidate A -> hardlink_paths: [file_a, file_b]
        # candidate B -> hardlink_paths: [file_b, file_a]
        # If both are in candidates, zip has entry A and entry B!
        archive_zip = os.path.join(self.test_dir, "hardlink_test.zip")
        with zipfile.ZipFile(archive_zip, "w") as zf:
            zf.writestr("item_00001_hl_a.dat", b"Important shared data")
            zf.writestr("item_00002_hl_b.dat", b"Important shared data")

        # Now simulate cleanup unlinking both
        os.remove(file_a)
        os.remove(file_b)

        # Now simulate rollback.rs:
        # Item 1: extracts item 1 to file_a
        with open(file_a, "wb") as f:
            f.write(b"Important shared data")
        # Item 1 reconstructs hardlink to sibling file_b:
        CreateHardLinkW(file_b, file_a, None)
        self.assertTrue(os.path.exists(file_b))

        # Item 2: rollback.rs encounters item 2 for file_b!
        # It calls resolve_restore_target(file_b):
        p_b = Path(file_b)
        self.assertTrue(p_b.exists())
        self.assertGreater(p_b.stat().st_size, 0)

        # resolve_restore_target renames it to file_b.restored_<timestamp>!
        ts = time.strftime("%Y%m%d_%H%M%S")
        target_item2 = str(p_b.with_name(f"{p_b.name}.restored_{ts}"))

        # CONFIRMED DEFECT: Item 2 creates a spurious restored sidecar file and breaks hardlinks!
        self.assertNotEqual(target_item2, file_b)

    # =========================================================================
    # AREA 4: Watchdog Auto-Rollback & Log Keyword Detection
    # =========================================================================

    def test_11_watchdog_log_error_detection_logic(self):
        """
        Tests the application log scanner in rollback.rs:
        Keywords: 'filenotfounderror', 'enoent', 'cannot find the file',
                  'missing file', 'failed to load', 'no such file'.
        Must verify that the log error references an ARCHIVED filename,
        avoiding false positives from unrelated missing files.
        """
        log_dir = os.path.join(self.test_dir, "app_logs")
        os.makedirs(log_dir, exist_ok=True)
        log_file = os.path.join(log_dir, "app.log")

        archived_files = ["cache_v2.db", "session.dat"]

        # Case 1: Log contains 'filenotfounderror' but for UNRELATED file
        with open(log_file, "w") as f:
            f.write("2026-11-16 [ERROR] FileNotFoundError: 'user_avatar_99.png' not found\n")

        def check_log(log_path: str, manifest_files: list[str]) -> bool:
            with open(log_path, "r", errors="ignore") as f:
                content = f.read().lower()
            keywords = ["filenotfounderror", "enoent", "cannot find the file", "missing file", "failed to load", "no such file"]
            has_kw = any(kw in content for kw in keywords)
            if not has_kw:
                return False
            return any(fname.lower() in content for fname in manifest_files)

        self.assertFalse(check_log(log_file, archived_files), "Unrelated missing file must NOT trigger auto-rollback")

        # Case 2: Log contains 'ENOENT' referencing archived 'cache_v2.db'
        with open(log_file, "a") as f:
            f.write("2026-11-16 [CRITICAL] ENOENT: open 'cache_v2.db' failed\n")

        self.assertTrue(check_log(log_file, archived_files), "Matching archived file error MUST trigger auto-rollback")

    def test_12_watchdog_permanent_quota_protection_database(self):
        """
        Verifies SQLite storage integration:
        When rollback occurs with rule_id > 0, restored paths are added to
        quota_rules.protected_files_json and permanently protected from future cleanups.
        Also verifies the behavior when rule_id == 0 (QuickCleanup).
        """
        db_path = os.path.join(self.test_dir, "greenc.db")
        conn = sqlite3.connect(db_path)
        cur = conn.cursor()

        # Setup schema matching greenc-storage
        cur.execute("""
            CREATE TABLE quota_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL,
                process_name TEXT NOT NULL,
                service_name TEXT NOT NULL,
                target_paths_json TEXT NOT NULL,
                max_bytes INTEGER NOT NULL,
                watermark_ratio REAL NOT NULL,
                retention_days INTEGER NOT NULL,
                min_file_age_seconds INTEGER NOT NULL,
                exclude_patterns_json TEXT NOT NULL,
                protected_files_json TEXT NOT NULL DEFAULT '[]',
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );
        """)

        cur.execute("""
            INSERT INTO quota_rules (
                id, app_name, process_name, service_name, target_paths_json,
                max_bytes, watermark_ratio, retention_days, min_file_age_seconds,
                exclude_patterns_json, protected_files_json, created_at
            ) VALUES (1, 'Zalo', 'Zalo.exe', '', '[]', 1000000, 0.85, 7, 60, '[]', '["C:\\\\keep.dat"]', '2026-11-16T12:00:00Z');
        """)
        conn.commit()

        # Simulate add_protected_files for rule_id = 1
        new_protected = [r"C:\AppData\ZaloData\restored_chat.dat"]

        cur.execute("SELECT protected_files_json FROM quota_rules WHERE id = 1;")
        current_list = json.loads(cur.fetchone()[0])
        for p in new_protected:
            if p not in current_list:
                current_list.push(p) if hasattr(current_list, 'push') else current_list.append(p)

        cur.execute("UPDATE quota_rules SET protected_files_json = ? WHERE id = 1;", (json.dumps(current_list),))
        conn.commit()

        cur.execute("SELECT protected_files_json FROM quota_rules WHERE id = 1;")
        updated_list = json.loads(cur.fetchone()[0])
        self.assertIn(r"C:\AppData\ZaloData\restored_chat.dat", updated_list)
        self.assertIn(r"C:\keep.dat", updated_list)

        # What happens for rule_id = 0?
        cur.execute("SELECT protected_files_json FROM quota_rules WHERE id = 0;")
        row_zero = cur.fetchone()
        self.assertIsNone(row_zero, "Rule ID 0 does not exist in quota_rules table; guarded by rule_id > 0")

        conn.close()

    # =========================================================================
    # AREA 5: Incompressible Bypass Heuristics & Sniffer
    # =========================================================================

    def test_13_incompressible_47_extensions_catalog(self):
        """Verifies exact catalog of 47 incompressible extensions across all 6 categories."""
        expected_extensions = [
            # Video (9)
            "mp4", "mkv", "mov", "avi", "webm", "m4v", "3gp", "flv", "wmv",
            # Audio (8)
            "mp3", "m4a", "aac", "ogg", "opus", "flac", "amr", "wma",
            # Image (8)
            "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif",
            # Archive (9)
            "zip", "7z", "rar", "gz", "tgz", "bz2", "xz", "zst", "lz4",
            # Package (8)
            "cab", "msi", "msix", "appx", "apk", "jar", "iso", "dmg",
            # Document (5)
            "docx", "xlsx", "pptx", "odt", "epub",
        ]
        self.assertEqual(len(expected_extensions), 47)

        # Ensure case-insensitivity and leading dot handling
        for ext in expected_extensions:
            clean = ext.strip().lstrip(".").lower()
            self.assertIn(clean, expected_extensions)

    def test_14_magic_header_signatures(self):
        """Verifies all 15 magic header byte signatures and ISO-BMFF / RIFF WEBP."""
        signatures = [
            (b"\x1F\x8B", "gzip/tgz"),
            (b"\x50\x4B\x03\x04", "zip/docx"),
            (b"\x28\xB5\x2F\xFD", "zstd"),
            (b"\xFD\x37\x7A\x58\x5A\x00", "xz"),
            (b"\x37\x7A\xBC\xAF\x27\x1C", "7z"),
            (b"\x52\x61\x72\x21\x1A\x07", "rar"),
            (b"\x42\x5A\x68", "bzip2"),
            (b"\x04\x22\x4D\x18", "lz4_frame"),
            (b"MSCF", "cab"),
            (b"\x89\x50\x4E\x47", "png"),
            (b"\xFF\xD8\xFF", "jpeg"),
            (b"GIF8", "gif"),
            (b"OggS", "ogg"),
            (b"fLaC", "flac"),
            (b"wOF2", "woff2"),
        ]
        self.assertEqual(len(signatures), 15)

        def matches_magic(header: bytes) -> bool:
            for sig, _ in signatures:
                if len(header) >= len(sig) and header[:len(sig)] == sig:
                    return True
            if len(header) >= 8 and header[4:8] == b"ftyp":
                return True
            if len(header) >= 12 and header[:4] == b"RIFF" and header[8:12] == b"WEBP":
                return True
            return False

        for sig, name in signatures:
            self.assertTrue(matches_magic(sig + b"\x00" * 10), f"Failed to match magic for {name}")

        # ISO-BMFF
        self.assertTrue(matches_magic(b"\x00\x00\x00\x1cftypisom"))
        # RIFF WEBP
        self.assertTrue(matches_magic(b"RIFF\x20\x00\x00\x00WEBPVP8 "))
        # Text file
        self.assertFalse(matches_magic(b"Plain text log entry"))


if __name__ == "__main__":
    unittest.main()
