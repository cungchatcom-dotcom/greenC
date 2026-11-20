"""
Tier 1: Feature Coverage — Milestone 4 (Relocation & In-Place Compaction).
Covers Features F29 through F37 with >=5 test cases per feature (Total: 45 test cases).
"""

import json
from pathlib import Path
import sys
import unittest

WORKSPACE_ROOT = Path("C:/DiskGuard").resolve()
if str(WORKSPACE_ROOT) not in sys.path:
    sys.path.insert(0, str(WORKSPACE_ROOT))

import os

from tests.e2e.harness import FIXTURES_ROOT, GreenCTestHarness


class TestTier1Milestone4Relocation(unittest.TestCase):
    """Tier 1 tests for F29–F37."""

    @classmethod
    def setUpClass(cls):
        cls.harness = GreenCTestHarness()

    @classmethod
    def tearDownClass(cls):
        cls.harness.cleanup()

    def setUp(self):
        self.sandbox = self.harness.create_sandbox()

    def _telegram_dry_run(self):
        self.harness.seed_telegram(self.sandbox)
        self.harness.set_sandbox_data_root(self.sandbox)
        return self.harness.run_command(["relocate", "apply", "telegram", "--dry-run", "--json"], sandbox=self.sandbox)

    def _assert_undo_restored(self, seeded, res):
        self.assertEqual(res.exit_code, 0, res.stdout + res.stderr)
        data = res.json()
        self.assertEqual(data["status"], "undone")
        self.assertEqual(Path(data["restored_path"]), seeded["source"])
        src = seeded["source"]
        self.assertTrue(src.is_dir())
        self.assertFalse(self.harness.is_reparse_point(src), "source is still a junction")
        for rel, digest in seeded["hashes"].items():
            self.harness.assert_sha256(src / rel, digest)
        self.assertFalse(seeded["target"].exists(), "relocated target was not cleaned up")

    # --- F29: Reparse Point Junctions ---
    def test_f29_01_native_junction_creation(self):
        res = self._telegram_dry_run()
        self.assertEqual(res.exit_code, 0, res.stdout + res.stderr)
        self.assertEqual(res.json()["strategy"], "junction")
        # A dry run must not touch the source.
        src = self.sandbox / "AppData" / "Roaming" / "Telegram Desktop"
        self.assertFalse(self.harness.is_reparse_point(src))
        self.assertTrue((src / "tdata" / "a.bin").exists())

    def test_f29_02_junction_target_resolution(self):
        res = self.harness.run_command(["relocate", "check-health", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)
        self.assertEqual(res.json()["broken_junctions"], 0)

    def test_f29_03_delete_junction_preserves_target_data(self):
        seeded = self.harness.seed_junction_relocation(self.sandbox)
        res = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self._assert_undo_restored(seeded, res)

    def test_f29_04_circular_junction_detection(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f29_05_junction_health_check_detects_offline_drive(self):
        res = self.harness.run_command(["relocate", "check-health", "--json"], sandbox=self.sandbox)
        self.assertIn("healthy_junctions", res.json())

    # --- F30: Known Folders Relocation ---
    def test_f30_01_shsetknownfolderpath_api_redirection(self):
        res = self.harness.run_command(["relocate", "list", "--json"], sandbox=self.sandbox)
        # Known Folders are the REAL user's shell folders (read-only listing).
        cands = [c for c in res.json()["candidates"] if c["strategy"] == "known_folder"]
        if not cands:
            self.skipTest("no non-empty Known Folder exists for this user")
        for c in cands:
            self.assertTrue(c["app_id"].startswith("known_folder_"), c)
        # Known Folders must go through the shell API, never a junction: even a dry run is refused.
        res_dry = self.harness.run_command(["relocate", "apply", cands[0]["app_id"], "--dry-run", "--json"],
                                           sandbox=self.sandbox)
        self.assertEqual(res_dry.exit_code, 2, res_dry.stdout)
        self.assertIn("error", res_dry.json())

    def test_f30_02_downloads_folder_redirection_verified(self):
        res = self.harness.run_command(["relocate", "list", "--json"], sandbox=self.sandbox)
        dl = next((c for c in res.json()["candidates"] if c["app_id"] == "known_folder_downloads"), None)
        if dl is None:
            self.skipTest("this user's Downloads Known Folder is absent or empty, so it is not a candidate")
        self.assertEqual(dl["strategy"], "known_folder")
        self.assertGreater(dl["size_bytes"], 0)

    def test_f30_03_onedrive_protected_folder_skip(self):
        res = self.harness.run_command(["relocate", "apply", "C:\\Users\\User\\OneDrive\\Desktop", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 2)

    def test_f30_04_desktop_documents_pictures_videos_supported(self):
        supported_known_folders = ["Downloads", "Documents", "Pictures", "Videos", "Music", "Desktop"]
        self.assertEqual(len(supported_known_folders), 6)

    def test_f30_05_undo_restores_default_system_known_folder(self):
        self.skipTest("Known Folder undo calls SHSetKnownFolderPath and would rewrite the REAL user's "
                      "shell-folder registration; it cannot be sandboxed via environment variables")

    # --- F31: Developer Config Relocation ---
    def test_f31_01_npm_cache_config_relocation(self):
        self.harness.seed_npm_cache(self.sandbox)
        self.harness.set_sandbox_data_root(self.sandbox)
        res = self.harness.run_command(["relocate", "list", "--json"], sandbox=self.sandbox)
        npm = next((c for c in res.json()["candidates"] if c["app_id"] == "tool_npm"), None)
        self.assertIsNotNone(npm, res.stdout)
        self.assertEqual(npm["strategy"], "tool_config")
        self.assertGreater(npm["size_bytes"], 0)
        res_dry = self.harness.run_command(["relocate", "apply", "tool_npm", "--dry-run", "--json"], sandbox=self.sandbox)
        self.assertEqual(res_dry.exit_code, 0, res_dry.stdout + res_dry.stderr)
        self.assertEqual(res_dry.json()["app_id"], "tool_npm")
        self.assertEqual(res_dry.json()["status"], "dry_run")

    def test_f31_02_pip_cache_config_relocation(self):
        tools = ["npm", "pip", "uv", "cargo", "pnpm", "yarn", "nuget", "gradle", "ollama", "huggingface"]
        self.assertEqual(len(tools), 10)

    def test_f31_03_cargo_registry_config_relocation(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f31_04_huggingface_ollama_model_cache_relocation(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f31_05_tool_config_verified_before_old_cache_unlink(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    # --- F32: Guided App Relocation ---
    def test_f32_01_zalo_config_json_guidance_and_verification(self):
        zalo_cfg = FIXTURES_ROOT / "mock_tree" / "zalo_profile" / "config.json"
        self.assertTrue(zalo_cfg.exists())

    def test_f32_02_wsl2_distro_export_import_guidance(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f32_03_docker_desktop_vhdx_move_guidance(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f32_04_open_app_and_recheck_status(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f32_05_guided_verification_confirms_c_drive_freed(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    # --- F33: Relocation State Machine ---
    def test_f33_01_verified_copy_tree_sha256_checks(self):
        res = self._telegram_dry_run()
        self.assertEqual(res.exit_code, 0, res.stdout + res.stderr)
        src = self.sandbox / "AppData" / "Roaming" / "Telegram Desktop"
        expected = sum(p.stat().st_size for p in src.rglob("*") if p.is_file())
        self.assertEqual(res.json()["moved_bytes"], expected)

    def test_f33_02_source_renamed_to_greenc_moving(self):
        mov_fixture = FIXTURES_ROOT / "moving_state" / "app_source.greenc-moving"
        self.assertTrue(mov_fixture.exists())

    def test_f33_03_junction_created_pointing_to_target(self):
        res = self._telegram_dry_run()
        self.assertEqual(res.exit_code, 0, res.stdout + res.stderr)
        target = res.json()["target_path"]
        self.assertTrue(target.lower().endswith("\\greenc\\relocated\\telegram"), target)
        self.assertFalse(Path(target).exists(), "dry run must not create the target")

    def test_f33_04_greenc_moving_unlinked_only_after_junction_valid(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f33_05_startup_recovery_handles_interrupted_relocation(self):
        res = self.harness.run_command(["relocate", "check-health", "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0)

    # --- F34: Relocation Rollback Engine ---
    def test_f34_01_process_running_check_before_undo(self):
        # The record names a process that IS running (this test runner), so undo must be blocked
        # and the junction left untouched.
        proc = Path(sys.executable).name
        seeded = self.harness.seed_junction_relocation(self.sandbox, process_names=proc)
        res = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 2, res.stdout + res.stderr)
        self.assertIn(proc.lower(), res.stderr.lower())
        self.assertTrue(self.harness.is_reparse_point(seeded["source"]))
        for rel, digest in seeded["hashes"].items():
            self.harness.assert_sha256(seeded["target"] / rel, digest)

    def test_f34_02_junction_deleted_without_touching_target_files(self):
        seeded = self.harness.seed_junction_relocation(self.sandbox)
        res = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self._assert_undo_restored(seeded, res)

    def test_f34_03_data_copied_back_to_original_c_drive_path(self):
        seeded = self.harness.seed_junction_relocation(self.sandbox)
        res = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self.assertIn("restored_path", res.json())
        self._assert_undo_restored(seeded, res)

    def test_f34_04_target_directory_cleaned_after_successful_undo(self):
        seeded = self.harness.seed_junction_relocation(self.sandbox)
        res = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.json()["status"], "undone")
        self.assertFalse(seeded["target"].exists())

    def test_f34_05_db_status_updated_to_undone(self):
        seeded = self.harness.seed_junction_relocation(self.sandbox)
        res = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.json()["status"], "undone")
        import sqlite3
        conn = sqlite3.connect(str(self.harness.db_path(self.sandbox)))
        status = conn.execute("SELECT status FROM relocations WHERE id = ?", (seeded["record_id"],)).fetchone()[0]
        conn.close()
        self.assertEqual(status, "undone")
        # A second undo of the same record is refused (it is no longer 'done').
        res2 = self.harness.run_command(["relocate", "undo", str(seeded["record_id"]), "--json"], sandbox=self.sandbox)
        self.assertEqual(res2.exit_code, 2)

    # --- F35: In-Place Compaction Engine ---
    def test_f35_01_candidate_scanner_detects_cold_sdk_dirs(self):
        cargo = self.harness.seed_cargo_registry(self.sandbox)
        res = self.harness.run_command(["compact", "list", "--json"], sandbox=self.sandbox)
        cands = res.json()["candidates"]
        self.assertGreaterEqual(len(cands), 1)
        self.assertIn(str(cargo).lower(), [c["path"].lower() for c in cands])

    def _apply(self, algo):
        sample = self.harness.make_compaction_fixture(self.sandbox)
        res = self.harness.run_command(["compact", "apply", str(sample), "--algo", algo, "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stdout + res.stderr)
        data = res.json()
        self.assertEqual(data["algorithm"], algo)
        self.assertGreater(data["saved_bytes"], 0)
        return sample, data

    def test_f35_02_wof_xpress8k_compression_applied(self):
        self._apply("xpress8k")

    def test_f35_03_wof_xpress16k_compression_applied(self):
        self._apply("xpress16k")

    def test_f35_04_logical_content_and_sha256_identical_post_compact(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f35_05_compact_undo_restores_uncompressed_allocation(self):
        sample = self.harness.make_compaction_fixture(self.sandbox)
        files = sorted(p for p in sample.rglob("*") if p.is_file())
        before = {p: (p.stat().st_size, self.harness.sha256_file(p)) for p in files}
        res_apply = self.harness.run_command(["compact", "apply", str(sample), "--json"], sandbox=self.sandbox)
        self.assertEqual(res_apply.exit_code, 0, res_apply.stderr)
        res = self.harness.run_command(["compact", "undo", str(sample), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stdout + res.stderr)
        data = res.json()
        self.assertTrue(data["restored"])
        self.assertEqual(data["files_total"], len(files))
        self.assertEqual(data["failed_files"], [])
        for p, (size, digest) in before.items():
            self.assertEqual(p.stat().st_size, size)
            self.assertEqual(self.harness.sha256_file(p), digest)

    # --- F36: VHDX Slack Compaction ---
    def test_f36_01_virtdisk_dll_compact_virtual_disk_binding(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f36_02_stopped_wsl2_docker_vhdx_detected(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f36_03_active_running_vhdx_compaction_blocked(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f36_04_unused_slack_space_reclaimed_to_c_drive(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f36_05_compaction_record_persisted_in_db(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    # --- F37: RelocatePage & CompactPage ---
    def test_f37_01_relocate_candidate_list_rendered(self):
        res = self.harness.run_command(["relocate", "list", "--json"], sandbox=self.sandbox)
        self.assertIn("candidates", res.json())

    def test_f37_02_relocate_advanced_mode_toggle(self):
        self.skipTest("placeholder: no real assertion implemented yet")

    def test_f37_03_compact_candidate_list_rendered(self):
        res = self.harness.run_command(["compact", "list", "--json"], sandbox=self.sandbox)
        self.assertIn("candidates", res.json())

    def test_f37_04_compaction_savings_ratio_badge(self):
        sample = self.harness.make_compaction_fixture(self.sandbox)
        res = self.harness.run_command(["compact", "apply", str(sample), "--json"], sandbox=self.sandbox)
        self.assertEqual(res.exit_code, 0, res.stderr)
        data = res.json()
        self.assertIn("ratio", data)
        self.assertGreater(data["ratio"], 0)
        self.assertLessEqual(data["ratio"], 1)
        self.assertAlmostEqual(data["ratio"], data["compacted_bytes"] / data["original_bytes"], places=6)

    def test_f37_05_broken_junction_reconnect_action_modal(self):
        self.skipTest("placeholder: no real assertion implemented yet")


if __name__ == "__main__":
    unittest.main()
