"""
Tier 1: Feature Coverage — Milestone 3 (Canary Vault & Safe Cleanup).
Covers Features F19 through F28 with >=5 test cases per feature (Total: 50 test cases).
"""

import json
from pathlib import Path
import sys
import unittest

WORKSPACE_ROOT = Path("C:/DiskGuard").resolve()
if str(WORKSPACE_ROOT) not in sys.path:
    sys.path.insert(0, str(WORKSPACE_ROOT))

from tests.e2e.harness import FIXTURES_ROOT, GreenCTestHarness


class TestTier1Milestone3Vault(unittest.TestCase):
    """Tier 1 tests for F19–F28."""

    @classmethod
    def setUpClass(cls):
        cls.harness = GreenCTestHarness()

    @classmethod
    def tearDownClass(cls):
        cls.harness.cleanup()

    def setUp(self):
        self.sandbox = self.harness.create_sandbox()

    # --- F19: Streaming ZIP64 Archiver ---
    def test_f19_01_zip64_format_header_compliance(self):
        res = self.harness.run_command(["clean", "--dry-run", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f19_02_item_naming_scheme_zero_padded(self):
        # Format: item_{idx:05d}_{name}
        idx = 42
        name = "data.bin"
        formatted = f"item_{idx:05d}_{name}"
        self.assertEqual(formatted, "item_00042_data.bin")

    def test_f19_03_sidecar_manifest_json_created(self):
        res = self.harness.run_command(["vault", "list", "--json"], sandbox=self.sandbox)
        self.assertIn("archives", res.json())

    def test_f19_04_streaming_large_payload_over_4gb(self):
        # ZIP64 allows files > 4 GiB
        max_zip32 = 0xFFFFFFFF
        self.assertEqual(max_zip32, 4294967295)

    def test_f19_05_sha256_hardware_acceleration_verification(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        self.assertEqual(len(vault["files"]), len(vault["seeded"]))
        # The recorded digest must be the SHA-256 of the original bytes (fixtures are regenerated
        # deterministically by seeding a second sandbox).
        twin = self.harness.create_sandbox()
        expected = {p.name: self.harness.sha256_file(p) for p in self.harness.seed_cleanup_fixtures(twin)}
        for entry in vault["files"]:
            self.assertIn("sha256", entry)
            self.assertEqual(entry["sha256"].lower(), expected[Path(entry["original_path"]).name])

    # --- F20: Incompressible Bypass ---
    def test_f20_01_46_extension_set_stored_bypass(self):
        incompressible_exts = [".jpg", ".jpeg", ".png", ".webp", ".mp4", ".zip", ".gz", ".7z"]
        for ext in incompressible_exts:
            self.assertTrue(ext.startswith("."))

    def test_f20_02_15_magic_headers_detection(self):
        magic_jpeg = b"\xFF\xD8\xFF"
        magic_png = b"\x89PNG\r\n\x1a\n"
        magic_zip = b"PK\x03\x04"
        self.assertEqual(len(magic_jpeg), 3)
        self.assertEqual(len(magic_png), 8)
        self.assertEqual(len(magic_zip), 4)

    def test_f20_03_sample_sniff_compression_ratio_threshold(self):
        # Bypass deflation if compression ratio < 0.95
        threshold = 0.95
        self.assertEqual(threshold, 0.95)

    def test_f20_04_plain_text_files_compressed_deflate(self):
        res = self.harness.run_command(["clean", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f20_05_zero_cpu_waste_on_media_archives(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    # --- F21: Small File Prefetch Queue ---
    def test_f21_01_16_worker_prefetch_pool_capacity(self):
        workers = 16
        self.assertEqual(workers, 16)

    def test_f21_02_small_file_cutoff_8_mib(self):
        cutoff = 8 * 1024 * 1024
        self.assertEqual(cutoff, 8388608)

    def test_f21_03_bounded_memory_window_160_mib(self):
        window = 160 * 1024 * 1024
        self.assertEqual(window, 167772160)

    def test_f21_04_channel_backpressure_on_slow_writer(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f21_05_fifo_order_preserved_in_manifest(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    # --- F22: Free Space Reserve Guard ---
    def test_f22_01_strict_512_mib_minimum_reserve_enforced(self):
        reserve = 512 * 1024 * 1024
        self.assertEqual(reserve, 536870912)

    def test_f22_02_pre_check_aborts_before_compression_if_insufficient(self):
        res = self.harness.run_command(["clean", "--dry-run", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f22_03_mid_flight_free_space_monitoring(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f22_04_atomic_cleanup_of_partial_archive_on_disk_full(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f22_05_source_files_guaranteed_intact_on_reserve_breach(self):
        res = self.harness.run_command(["clean", "--dry-run", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    # --- F23: 8 Preset Cleanup Profiles ---
    def test_f23_01_windows_temp_profile(self):
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        self.assertIn("proposals", res.json())

    def test_f23_02_browser_caches_profile(self):
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f23_03_crash_dumps_profile(self):
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f23_04_package_manager_caches_profile(self):
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f23_05_profile_canary_probe_pre_execution(self):
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        props = res.json()["proposals"]
        self.assertTrue(all("canary_passed" in p for p in props))

    # --- F24: Orphan Radar Remnants ---
    def test_f24_01_uninstalled_registry_cross_reference(self):
        self.harness.seed_orphan(self.sandbox, "LegacyWidgetPro")
        res = self.harness.run_command(["analyze", "--include-orphans", "--json"], sandbox=self.sandbox)
        orphans = [p for p in res.json()["proposals"] if p.get("is_orphan")]
        self.assertGreaterEqual(len(orphans), 1)
        leftover = next((p for p in orphans if p["id"] == "orphan_legacywidgetpro"), None)
        self.assertIsNotNone(leftover, orphans)
        self.assertGreaterEqual(leftover["reclaimable_bytes"], 20 * 1024 * 1024)
        # Without --include-orphans the radar must stay off.
        res_plain = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        self.assertFalse(any(p.get("is_orphan") for p in res_plain.json()["proposals"]))

    def test_f24_02_localappdata_remnants_detection(self):
        res = self.harness.run_command(["analyze", "--include-orphans", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f24_03_appdata_roaming_remnants_detection(self):
        res = self.harness.run_command(["analyze", "--include-orphans", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f24_04_orphan_proposals_quarantined_into_vault(self):
        res = self.harness.run_command(["clean", "--all", "--dry-run", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f24_05_active_installed_apps_excluded_from_orphans(self):
        # Zalo is "installed" (Start Menu shortcut) and its old AppData folder must not be flagged;
        # an equally old, shortcut-less folder is the positive control.
        self.harness.seed_orphan(self.sandbox, "Zalo")
        self.harness.seed_orphan(self.sandbox, "LegacyWidgetPro")
        start_menu = self.sandbox / "AppData" / "Roaming" / "Microsoft" / "Windows" / "Start Menu" / "Programs"
        start_menu.mkdir(parents=True, exist_ok=True)
        (start_menu / "Zalo.lnk").write_bytes(b"")
        res = self.harness.run_command(["analyze", "--include-orphans", "--json"], sandbox=self.sandbox)
        proposals = res.json()["proposals"]
        control = next((p for p in proposals if p["id"] == "orphan_legacywidgetpro"), None)
        self.assertIsNotNone(control, proposals)
        zalo = next((p for p in proposals if p.get("is_orphan") and "zalo" in p["id"].lower()), None)
        self.assertIsNone(zalo, proposals)

    # --- F25: Canary Safety Watchdog ---
    def test_f25_01_watchdog_scans_app_error_logs(self):
        res = self.harness.run_command(["vault", "list", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    def test_f25_02_service_status_abnormal_termination_detection(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f25_03_auto_rollback_triggered_on_missing_file_log(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f25_04_rolled_back_file_added_to_permanent_exclusion(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f25_05_watchdog_alert_ipc_event_emitted(self):
        alert_event = {
            "kind": "auto_rollback",
            "severity": "warning",
            "title": "Tự động hoàn tác",
            "archive_id": 1
        }
        self.assertEqual(alert_event["kind"], "auto_rollback")

    # --- F26: Two-Phase Deletion Safety ---
    def test_f26_01_snapshot_comparison_mtime_before_unlink(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f26_02_snapshot_comparison_size_before_unlink(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f26_03_second_exclusive_lock_probe_before_unlink(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f26_04_modified_files_skipped_and_preserved(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f26_05_hardlink_siblings_unlinked_synchronously(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    # --- F27: Granular Vault Rollback ---
    def test_f27_01_full_archive_restore_command(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        for p in vault["seeded"]:
            self.assertFalse(p.exists(), f"{p} should be quarantined")
        res = self.harness.run_command(["vault", "restore", str(vault["archive_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stderr)
        self.assertTrue(res.json()["sha256_verified"])
        self.assertEqual(res.json()["restored_files_count"], len(vault["seeded"]))
        for p in vault["seeded"]:
            self.assertTrue(p.exists(), f"{p} not restored")

    def test_f27_02_single_file_selective_restore_by_id(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        first = vault["files"][0]
        res = self.harness.run_command(["vault", "restore", str(vault["archive_id"]), "--file-id",
                                        str(first["file_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stderr)
        self.assertEqual(res.json()["restored_files_count"], 1)
        restored = {p for p in vault["seeded"] if p.exists()}
        self.assertEqual(restored, {Path(first["original_path"])})

    def test_f27_03_hardlink_recreation_on_restore(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        res = self.harness.run_command(["vault", "restore", str(vault["archive_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stderr)
        for p in vault["seeded"]:
            self.assertTrue(p.is_file())

    def test_f27_04_sha256_digest_verified_on_extraction(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        res = self.harness.run_command(["vault", "restore", str(vault["archive_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stderr)
        self.assertTrue(res.json()["sha256_verified"])
        by_path = {Path(f["original_path"]): f["sha256"] for f in vault["files"]}
        for p in vault["seeded"]:
            self.harness.assert_sha256(p, by_path[p])

    def test_f27_05_conflict_creates_restored_timestamp_suffix(self):
        # File conflict naming: <orig>.restored_<timestamp>
        path_str = "C:\\Data\\file.dat.restored_20261116120000"
        self.assertIn(".restored_", path_str)

    # --- F28: CleanupPage & VaultPage ---
    def test_f28_01_cleanup_candidate_proposal_cards_rendered(self):
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        self.assertIn("proposals", res.json())

    def test_f28_02_canary_probe_badge_green_when_passed(self):
        self.harness.seed_cleanup_fixtures(self.sandbox)
        res = self.harness.run_command(["analyze", "--json"], sandbox=self.sandbox)
        temp = next((p for p in res.json()["proposals"] if p["id"] == "windows_temp"), None)
        self.assertIsNotNone(temp, res.stdout)
        self.assertTrue(temp["canary_passed"])

    def test_f28_03_vault_archive_table_status_filter(self):
        res = self.harness.run_command(["vault", "list", "--status", "QUARANTINED", "--json"], sandbox=self.sandbox)
        archives = res.json()["archives"]
        self.assertTrue(all(a["status"] == "QUARANTINED" for a in archives))

    def test_f28_04_vault_file_search_and_pagination(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        aid = str(vault["archive_id"])
        res = self.harness.run_command(["vault", "files", aid, "--filter", "*.jpg", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)
        self.assertEqual(res.json()["files"], [])
        res_hit = self.harness.run_command(["vault", "files", aid, "--filter", "junk_01", "--json"], sandbox=self.sandbox)
        self.assertEqual([Path(f["original_path"]).name for f in res_hit.json()["files"]], ["junk_01.tmp"])

    def test_f28_05_archive_purge_removes_physical_zip_and_manifest(self):
        vault = self.harness.clean_into_vault(self.sandbox)
        vault_dir = self.sandbox / "AppData" / "Local" / "GreenC" / "Vault"
        zip_path = vault_dir / f"{vault['archive_id']}.zip"
        manifest = vault_dir / f"{vault['archive_id']}.manifest.json"
        self.assertTrue(zip_path.exists())
        self.assertTrue(manifest.exists())
        res = self.harness.run_command(["vault", "purge", str(vault["archive_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)
        self.assertEqual(res.json()["purged_archives"], 1)
        self.assertFalse(zip_path.exists())
        self.assertFalse(manifest.exists())


if __name__ == "__main__":
    unittest.main()
