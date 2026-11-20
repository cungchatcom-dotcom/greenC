# GreenC v2 E2E Test Execution Report

- **Generated**: 2026-11-20T08:23:53.665123Z
- **Engine**: real binary `C:\DiskGuard\greenc-v2\target\release\greenc-cli.exe`
- **Total Tests Executed**: 359
- **Passed**: 359 (100.0%)
- **Failed / Errors**: 0
- **Skipped (not counted above)**: 211 (201 placeholders without real assertions, the rest environment limits)
- **Total Duration**: 37.31 seconds

## Summary by Tier

| Tier | Category | Tests | Passed | Failed |
|------|----------|-------|--------|--------|
| Tier 1 | Feature Coverage (F01–F54) | 203 | 203 | 0 |
| Tier 2 | Boundary & Stress Conditions | 128 | 128 | 0 |
| Tier 3 | Cross-Feature Combinations | 24 | 24 | 0 |
| Tier 4 | Real-World Scenarios | 4 | 4 | 0 |

## Failed Tests Detail

## Skipped for environment reasons

- `test_f11_03_usb_external_drive_ineligible_with_reason`: no removable/FAT volume attached; invariants checked on C:\, E:\, F:\
- `test_f11_04_fat32_exfat_ineligible_due_to_no_reparse_points`: no FAT32/exFAT/non-NTFS volume attached; invariants checked on C:\, E:\, F:\
- `test_f11_05_secondary_drive_with_max_free_space_recommended`: this machine has no eligible non-system drive with >= 10 GiB free
- `test_f30_02_downloads_folder_redirection_verified`: this user's Downloads Known Folder is absent or empty, so it is not a candidate
- `test_f30_05_undo_restores_default_system_known_folder`: Known Folder undo calls SHSetKnownFolderPath and would rewrite the REAL user's shell-folder registration; it cannot be sandboxed via environment variables
- `test_f54_03_scenario_3_junction_relocation_and_crash_recovery`: a real `relocate apply` writes to <drive>:\GreenC\Relocated\<id>, outside the sandbox; set GREENC_E2E_TARGET_DRIVE=<letter> (fixed NTFS drive) to opt in
- `test_t2_f03_01_mock_trait_injection_simulates_win32_failure`: not observable through the CLI; covered by Rust mock-trait tests
- `test_t2_f29_02_junction_creation_on_non_ntfs_volume_blocked`: no FAT32/exFAT/non-NTFS volume attached; invariants checked on C:\, E:\, F:\
- `test_t3_24_known_folders_relocation_undo_restores_shell_api_and_data`: Known Folder relocation/undo calls SHSetKnownFolderPath and would rewrite the REAL user's shell-folder registration; it cannot be sandboxed via environment variables
- `test_t4_scenario_3_junction_relocation_and_crash_recovery`: steps 3-5 run a real `relocate apply` whose target <drive>:\GreenC\Relocated\telegram is outside the sandbox; set GREENC_E2E_TARGET_DRIVE=<letter> to opt in

🎉 All test cases passed with zero discrepancies!