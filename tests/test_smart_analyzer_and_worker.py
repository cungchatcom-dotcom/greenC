import ctypes
from ctypes import wintypes
import hashlib
import os
from pathlib import Path
import threading
import time

from diskguard.core.smart_analyzer import SmartAppAnalyzer
from diskguard.core.task_controller import TaskControl
from diskguard.storage.database import DiskGuardDB


def test_task_control_pause_resume_stop():
    ctrl = TaskControl()
    events = []
    ctrl.set_callback(lambda pct, title, detail: events.append((pct, title, detail)))

    ctrl.report(25, "Đang quét thư mục", "Kiểm tra AppData...")
    assert len(events) == 1
    assert events[0][0] == 25

    # Test pause and resume from another thread
    ctrl.pause()
    assert ctrl.is_paused is True

    resumed_at = []

    def worker():
        ok = ctrl.wait_if_paused()
        resumed_at.append(ok)

    t = threading.Thread(target=worker)
    t.start()
    time.sleep(0.05)
    assert len(resumed_at) == 0  # Still paused
    ctrl.resume()
    t.join(timeout=1.0)
    assert resumed_at == [True]

    # Test stop
    ctrl.stop()
    assert ctrl.is_stopped is True
    assert ctrl.wait_if_paused() is False


def test_smart_analyzer_sandbox_probe_and_one_click_cleanup(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "smart.db")
    vault_dir = tmp_path / "vault"
    probe_dir = tmp_path / "probe_sandbox"

    # Simulate an environment with 2 installed apps:
    # App 1: ChatApp (uses NTFS HardLinks like Zalo, unlocked old media + active DB)
    app1_dir = tmp_path / "Apps" / "ChatApp"
    (app1_dir / "fileNoise").mkdir(parents=True)
    (app1_dir / "resource" / "chat1").mkdir(parents=True)
    now = time.time()

    (app1_dir / "core.db").write_bytes(b"SQLITE_CORE" * 10_000)
    hl1 = app1_dir / "fileNoise" / "media_old_1.bin"
    hl2 = app1_dir / "resource" / "chat1" / "media_old_1_link.bin"
    hl1.write_bytes(b"CHAT_MEDIA_PAYLOAD_" * 120_000)  # ~2.28 MB
    os.utime(hl1, (now - 86400 * 15, now - 86400 * 15))
    os.link(hl1, hl2)
    orig_sha = hashlib.sha256(hl1.read_bytes()).hexdigest()

    # App 2: WorkApp (has a locked root data file, but an unlocked Cache subfolder -> fallback test!)
    app2_dir = tmp_path / "Apps" / "WorkApp"
    (app2_dir / "ActiveData").mkdir(parents=True)
    (app2_dir / "Cache").mkdir(parents=True)
    locked_file = app2_dir / "ActiveData" / "locked_session.dat"
    locked_file.write_bytes(b"LOCKED_SESSION_" * 100_000)
    os.utime(locked_file, (now - 86400 * 20, now - 86400 * 20))

    cache_file = app2_dir / "Cache" / "bundle_old.amr"
    cache_file.write_bytes(b"CACHE_BUNDLE_" * 180_000)  # ~2.34 MB
    os.utime(cache_file, (now - 86400 * 12, now - 86400 * 12))

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    h = kernel32.CreateFileW(str(locked_file), 0x80000000 | 0x40000000, 0, None, 3, 0, None)
    assert h != wintypes.HANDLE(-1).value
    try:
        analyzer = SmartAppAnalyzer(db=db, vault_dir=vault_dir, probe_dir=probe_dir)
        ctrl = TaskControl()
        steps_logged = []
        ctrl.set_callback(lambda pct, title, detail: steps_logged.append((pct, title, detail)))

        proposals = analyzer.scan_and_probe_apps(
            custom_app_dirs=[
                ("ChatApp", "ChatApp.exe", [str(app1_dir)]),
                ("WorkApp", "WorkApp.exe", [str(app2_dir)]),
            ],
            task_control=ctrl,
            min_reclaimable_bytes=512 * 1024,
        )

        # Verify step captions were emitted for the user
        assert len(steps_logged) >= 3

        # Verify the sandbox probe tested and restored the canary files 100% intact before cleanup!
        assert hl1.exists() and hl2.exists()
        assert hashlib.sha256(hl1.read_bytes()).hexdigest() == orig_sha
        assert os.stat(hl1).st_ino == os.stat(hl2).st_ino
        assert cache_file.exists()

        # Both apps should have verified proposals
        assert len(proposals) == 2
        assert all(p.probe_passed for p in proposals)

        # Now user clicks the single "Dọn dẹp" button!
        cleanup_res = analyzer.execute_one_click_cleanup(proposals, task_control=ctrl)
        assert cleanup_res["freed_bytes"] >= 4 * 1024 * 1024
        assert cleanup_res["rules_saved"] == 2
        assert len(db.list_quota_rules()) == 2
        assert len(db.list_vault_archives(status="QUARANTINED")) == 2
    finally:
        kernel32.CloseHandle(h)
