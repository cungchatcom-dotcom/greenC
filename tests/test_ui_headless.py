import os
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QApplication
from diskguard.storage.database import DiskGuardDB
from diskguard.ui.main_window import MainWindow


def test_main_window_initializes_and_runs_sandbox_flow(tmp_path: Path):
    app = QApplication.instance() or QApplication([])
    db = DiskGuardDB(tmp_path / "ui_test.db")
    vault_dir = tmp_path / "vault"
    sandbox_dir = tmp_path / "sandbox"

    win = MainWindow(db=db, vault_dir=vault_dir, sandbox_dir=sandbox_dir, auto_scan_on_init=False)
    assert win.windowTitle().startswith("DiskGuard")

    # Trigger Sandbox Demo from UI controller method
    res = win.run_sandbox_demo(silent=True)
    assert res["archive"] is not None
    assert len(db.list_quota_rules()) >= 1
    assert len(db.list_vault_archives(status="QUARANTINED")) >= 1

    # Simulate unsafe file access -> verify automatic rollback updates Vault status
    arch_id = res["archive"].id
    rb_res = win.simulate_auto_rollback_for_archive(arch_id, silent=True)
    assert arch_id in rb_res["auto_rolled_back"]
    updated_arch = db.get_vault_archive(arch_id)
    assert updated_arch is not None
    assert updated_arch.status == "ROLLED_BACK"
    win.close()
    win.deleteLater()
    app.processEvents()
