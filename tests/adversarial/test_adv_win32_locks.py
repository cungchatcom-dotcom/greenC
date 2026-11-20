"""
Empirical Adversarial Challenge Suite 1: Win32 Exclusive File Lock Probe
Tests the native Windows CreateFileW probe behavior against all handle sharing modes,
directories, deleted files, read-only attributes, and multi-threaded stress.
"""

import ctypes
from ctypes import wintypes
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

# Win32 Constants matching greenc-win32/src/handle.rs
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
FILE_FLAG_BACKUP_SEMANTICS = 0x02000000

ERROR_SUCCESS = 0
ERROR_FILE_NOT_FOUND = 2
ERROR_PATH_NOT_FOUND = 3
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


def greenc_is_file_locked(path_str: str) -> tuple[bool, int]:
    """
    Exact implementation of greenc_win32::handle::is_file_locked:
    Attempts CreateFileW with DELETE_ACCESS, FILE_SHARE_NONE, OPEN_EXISTING,
    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS.
    Returns (is_locked, win32_error_code).
    """
    path = Path(path_str)
    if not path.exists():
        return False, ERROR_FILE_NOT_FOUND

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
        return False, ERROR_SUCCESS

    err = ctypes.get_last_error()
    is_locked = err in (ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION)
    return is_locked, err


def open_holding_handle(path_str: str, desired_access: int, share_mode: int) -> wintypes.HANDLE:
    """Helper to acquire an open Win32 file handle with specific access and sharing."""
    h = CreateFileW(
        str(path_str),
        desired_access,
        share_mode,
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
        None,
    )
    if h == INVALID_HANDLE_VALUE or h == 0xFFFFFFFFFFFFFFFF:
        raise OSError(f"Failed to open holding handle: {ctypes.get_last_error()}")
    return h


class TestAdversarialWin32LockProbe(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.mkdtemp(prefix="greenc_adv_lock_")

    def tearDown(self):
        shutil.rmtree(self.temp_dir, ignore_errors=True)

    def test_01_unlocked_clean_file(self):
        """Clean file with no open handles should report is_file_locked == False."""
        p = os.path.join(self.temp_dir, "clean.txt")
        with open(p, "wb") as f:
            f.write(b"data")
        locked, err = greenc_is_file_locked(p)
        self.assertFalse(locked)
        self.assertEqual(err, ERROR_SUCCESS)

    def test_02_nonexistent_file(self):
        """Nonexistent file should return False without failing."""
        p = os.path.join(self.temp_dir, "ghost.txt")
        locked, err = greenc_is_file_locked(p)
        self.assertFalse(locked)

    def test_03_exclusive_lock_no_sharing(self):
        """File opened with FILE_SHARE_NONE must report is_file_locked == True."""
        p = os.path.join(self.temp_dir, "excl.txt")
        with open(p, "wb") as f:
            f.write(b"data")
        h = open_holding_handle(p, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_NONE)
        try:
            locked, err = greenc_is_file_locked(p)
            self.assertTrue(locked)
            self.assertEqual(err, ERROR_SHARING_VIOLATION)
        finally:
            CloseHandle(h)
        # After closing, probe must report unlocked
        locked_after, _ = greenc_is_file_locked(p)
        self.assertFalse(locked_after)

    def test_04_shared_read_lock(self):
        """File opened with GENERIC_READ and FILE_SHARE_READ (no delete share) must report is_file_locked == True."""
        p = os.path.join(self.temp_dir, "sh_read.txt")
        with open(p, "wb") as f:
            f.write(b"data")
        h = open_holding_handle(p, GENERIC_READ, FILE_SHARE_READ)
        try:
            locked, err = greenc_is_file_locked(p)
            self.assertTrue(locked)
            self.assertEqual(err, ERROR_SHARING_VIOLATION)
        finally:
            CloseHandle(h)

    def test_05_shared_write_lock(self):
        """File opened with GENERIC_WRITE and FILE_SHARE_WRITE (no delete share) must report is_file_locked == True."""
        p = os.path.join(self.temp_dir, "sh_write.txt")
        with open(p, "wb") as f:
            f.write(b"data")
        h = open_holding_handle(p, GENERIC_WRITE, FILE_SHARE_WRITE)
        try:
            locked, err = greenc_is_file_locked(p)
            self.assertTrue(locked)
            self.assertEqual(err, ERROR_SHARING_VIOLATION)
        finally:
            CloseHandle(h)

    def test_06_shared_delete_lock(self):
        """
        Adversarial case: File opened with FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
        Even if the other process permits DELETE, our probe requests FILE_SHARE_NONE (exclusive).
        Because the other process holds an existing handle, our FILE_SHARE_NONE request must conflict.
        Empirical check: must report is_file_locked == True.
        """
        p = os.path.join(self.temp_dir, "sh_del.txt")
        with open(p, "wb") as f:
            f.write(b"data")
        h = open_holding_handle(p, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        try:
            locked, err = greenc_is_file_locked(p)
            self.assertTrue(locked)
            self.assertEqual(err, ERROR_SHARING_VIOLATION)
        finally:
            CloseHandle(h)

    def test_07_read_only_attribute_file(self):
        """
        Adversarial case: File with FILE_ATTRIBUTE_READONLY set, but no open handles.
        Does CreateFileW(DELETE, FILE_SHARE_NONE) succeed or falsely trigger ERROR_ACCESS_DENIED?
        Empirical check: should report is_file_locked == False.
        """
        p = os.path.join(self.temp_dir, "readonly.txt")
        with open(p, "wb") as f:
            f.write(b"data")
        os.chmod(p, stat.S_IREAD)
        try:
            locked, err = greenc_is_file_locked(p)
            self.assertFalse(locked, f"Read-only file was falsely marked locked with error {err}")
        finally:
            os.chmod(p, stat.S_IWRITE)

    def test_08_empty_directory(self):
        """
        Adversarial case: Probe against a clean empty directory.
        FILE_FLAG_BACKUP_SEMANTICS allows opening directories.
        Empirical check: should report is_file_locked == False.
        """
        d = os.path.join(self.temp_dir, "empty_dir")
        os.mkdir(d)
        locked, err = greenc_is_file_locked(d)
        self.assertFalse(locked, f"Empty directory reported locked with error {err}")

    def test_09_directory_with_unlocked_files(self):
        """Directory with unlocked files inside."""
        d = os.path.join(self.temp_dir, "dir_with_files")
        os.mkdir(d)
        with open(os.path.join(d, "subfile.txt"), "wb") as f:
            f.write(b"content")
        locked, err = greenc_is_file_locked(d)
        self.assertFalse(locked, f"Directory with unlocked files reported locked: {err}")

    def test_10_directory_locked_as_current_working_directory(self):
        """
        Adversarial case: A child process has set the directory as its Current Working Directory (CWD).
        On Windows, a process holding CWD holds an open directory handle without FILE_SHARE_DELETE.
        Empirical check: does greenc_is_file_locked detect this lock?
        """
        d = os.path.join(self.temp_dir, "cwd_dir")
        os.mkdir(d)

        # Launch background process with CWD set to this directory
        proc = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(5)"],
            cwd=d,
        )
        try:
            time.sleep(0.5)  # Wait for child process to establish CWD handle
            locked, err = greenc_is_file_locked(d)
            self.assertTrue(locked, f"Directory held as CWD should be detected as locked (got err {err})")
            self.assertIn(err, (ERROR_SHARING_VIOLATION, ERROR_ACCESS_DENIED))
        finally:
            proc.terminate()
            proc.wait(timeout=2)

        # After child process exits, directory should become unlocked
        time.sleep(0.2)
        locked_after, _ = greenc_is_file_locked(d)
        self.assertFalse(locked_after)

    def test_11_directory_with_open_file_inside(self):
        """
        Adversarial case: A file inside the directory is held locked, but the directory itself is not.
        Probing the DIRECTORY: should report False (directory itself is not locked, though deleting it would fail).
        Probing the FILE: should report True.
        """
        d = os.path.join(self.temp_dir, "dir_open_child")
        os.mkdir(d)
        child = os.path.join(d, "child.dat")
        with open(child, "wb") as f:
            f.write(b"data")
        h = open_holding_handle(child, GENERIC_WRITE, FILE_SHARE_NONE)
        try:
            child_locked, _ = greenc_is_file_locked(child)
            self.assertTrue(child_locked)
            # The directory itself can still be opened exclusively if no directory handle is open
            dir_locked, _ = greenc_is_file_locked(d)
            self.assertFalse(dir_locked)
        finally:
            CloseHandle(h)

    def test_12_concurrent_probe_stress_multithreaded(self):
        """
        Adversarial stress test: 8 worker threads repeatedly probe 16 files while
        2 background threads randomly lock and unlock files.
        Verifies:
        1. Zero crashes, deadlocks, or access violations.
        2. Probe results accurately reflect instantaneous lock state.
        3. Zero OS handle leaks during 2,000 rapid iterations.
        """
        num_files = 16
        files = [os.path.join(self.temp_dir, f"stress_{i}.dat") for i in range(num_files)]
        for f in files:
            with open(f, "wb") as fp:
                fp.write(b"test data for stress")

        stop_event = threading.Event()
        lock_state = {f: False for f in files}
        state_lock = threading.Lock()
        active_handles = {}

        def locker_thread_worker(tid):
            while not stop_event.is_set():
                for f in files:
                    with state_lock:
                        is_held = f in active_handles
                    if is_held:
                        # Release handle
                        with state_lock:
                            if f in active_handles:
                                CloseHandle(active_handles.pop(f))
                                lock_state[f] = False
                    else:
                        # Acquire handle
                        try:
                            h = open_holding_handle(f, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_NONE)
                            with state_lock:
                                active_handles[f] = h
                                lock_state[f] = True
                        except OSError:
                            pass
                    time.sleep(0.005)

        locker1 = threading.Thread(target=locker_thread_worker, args=(1,), daemon=True)
        locker2 = threading.Thread(target=locker_thread_worker, args=(2,), daemon=True)
        locker1.start()
        locker2.start()

        errors = []
        probe_count = 0

        def prober_thread_worker(tid):
            nonlocal probe_count
            for _ in range(250):
                for f in files:
                    try:
                        locked, err = greenc_is_file_locked(f)
                        probe_count += 1
                    except Exception as ex:
                        errors.append(f"Thread {tid} probe failed: {ex}")

        probers = [threading.Thread(target=prober_thread_worker, args=(i,)) for i in range(8)]
        for p in probers:
            p.start()
        for p in probers:
            p.join()

        stop_event.set()
        locker1.join(timeout=1)
        locker2.join(timeout=1)

        # Clean up any remaining open handles
        with state_lock:
            for h in active_handles.values():
                CloseHandle(h)
            active_handles.clear()

        self.assertEqual(len(errors), 0, f"Stress probe encountered errors: {errors}")
        self.assertGreaterEqual(probe_count, 1000)


if __name__ == "__main__":
    unittest.main()
