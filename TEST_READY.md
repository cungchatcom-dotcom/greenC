# TEST_READY: GreenC v2 Opaque-Box 4-Tier E2E Test Suite

**Feature Branch**: `001-greenc-complete-platform`  
**Workspace**: `C:\DiskGuard\greenc-v2\`  
**Sign-Off Date**: 2026-11-16  
**Status**: APPROVED & PUBLISHED (Track B Ready for Implementation Track Verification)  
**Author**: E2E Test Specialist (`test_writer_e2e_1`)  

---

## 1. Overview & Readiness Declaration

The comprehensive, opaque-box 4-tier End-to-End (E2E) test suite for **GreenC v2** has been designed, implemented, and verified. The test suite is completely independent of internal implementation code, deriving 100% of its test assertions from `ORIGINAL_REQUEST.md`, `PROJECT.md § Feature Inventory`, and the formal contracts in `specs/001-greenc-complete-platform/contracts/`.

All test fixtures, mock directory trees, synthetic legacy SQLite databases, corrupt archives, and the test harness runner are in place.

---

## 2. Test Catalog & Metrics Summary

| Tier | Category | Scope | Test Files | Total Tests |
|---|---|---|---|---|
| **Tier 1** | **Feature Coverage** | >=5 isolated tests per feature across all 54 features (`F01`–`F54`) | 6 test modules in `tests/e2e/tier1_features/` | **270** |
| **Tier 2** | **Boundary & Stress** | >=5 boundary/negative/adversarial tests per feature (`F01`–`F54`) | 6 test modules in `tests/e2e/tier2_boundary/` | **270** |
| **Tier 3** | **Cross-Feature** | Complex pairwise and tripartite subsystem integrations | `tests/e2e/tier3_combinations/test_t3_cross_feature.py` | **25** |
| **Tier 4** | **Real-World Scenarios** | 5 complete application lifecycle workflows | `tests/e2e/tier4_scenarios/test_t4_real_world_workflows.py` | **5** |
| **Total** | **Full E2E Suite** | **100% Feature Inventory Coverage (F01–F54)** | **14 Test Modules + Runner** | **570** |

---

## 3. Feature-to-Test Mapping Matrix

Every feature in `PROJECT.md` is strictly covered by both Tier 1 and Tier 2 tests:

| Feature # | Feature Name | Tier 1 Tests | Tier 2 Boundary Tests | Subsystem Area |
|---|---|---|---|---|
| **F01** | Cargo 9-Crate Workspace | `test_f01_01..05` (5) | `test_t2_f01_01..05` (5) | M1: Workspace |
| **F02** | Domain Models & Errors | `test_f02_01..05` (5) | `test_t2_f02_01..05` (5) | M1: greenc-core |
| **F03** | Win32 Boundary Traits | `test_f03_01..05` (5) | `test_t2_f03_01..05` (5) | M1: greenc-win32 |
| **F04** | Exclusive File Lock Probe | `test_f04_01..05` (5) | `test_t2_f04_01..05` (5) | M1: greenc-win32 |
| **F05** | SQLite WAL Schema v2 | `test_f05_01..05` (5) | `test_t2_f05_01..05` (5) | M1: greenc-storage |
| **F06** | Legacy Database Migration | `test_f06_01..05` (5) | `test_t2_f06_01..05` (5) | M1: greenc-storage |
| **F07** | 50-Locale i18n Catalog | `test_f07_01..05` (5) | `test_t2_f07_01..05` (5) | M1: greenc-i18n |
| **F08** | i18n Validator & Formatter | `test_f08_01..05` (5) | `test_t2_f08_01..05` (5) | M1: greenc-i18n |
| **F09** | Tauri v2 App State Harness | `test_f09_01..05` (5) | `test_t2_f09_01..05` (5) | M1: greenc-app |
| **F10** | Svelte 5 UI Foundation | `test_f10_01..05` (5) | `test_t2_f10_01..05` (5) | M1: ui |
| **F11** | Drive Eligibility Engine | `test_f11_01..05` (5) | `test_t2_f11_01..05` (5) | M2: greenc-scanner |
| **F12** | Parallel Directory Walker | `test_f12_01..05` (5) | `test_t2_f12_01..05` (5) | M2: greenc-scanner |
| **F13** | Reparse Point Tag Guard | `test_f13_01..05` (5) | `test_t2_f13_01..05` (5) | M2: greenc-scanner |
| **F14** | Space Lens Treemap Engine | `test_f14_01..05` (5) | `test_t2_f14_01..05` (5) | M2: greenc-scanner |
| **F15** | Predictive Fullness Forecast | `test_f15_01..05` (5) | `test_t2_f15_01..05` (5) | M2: greenc-scanner |
| **F16** | FirstRunModal Component | `test_f16_01..05` (5) | `test_t2_f16_01..05` (5) | M2: ui |
| **F17** | OverviewPage Component | `test_f17_01..05` (5) | `test_t2_f17_01..05` (5) | M2: ui |
| **F18** | Scanner IPC Streaming | `test_f18_01..05` (5) | `test_t2_f18_01..05` (5) | M2: greenc-app |
| **F19** | Streaming ZIP64 Archiver | `test_f19_01..05` (5) | `test_t2_f19_01..05` (5) | M3: greenc-vault |
| **F20** | Incompressible Bypass | `test_f20_01..05` (5) | `test_t2_f20_01..05` (5) | M3: greenc-vault |
| **F21** | Small File Prefetch Queue | `test_f21_01..05` (5) | `test_t2_f21_01..05` (5) | M3: greenc-vault |
| **F22** | Free Space Reserve Guard | `test_f22_01..05` (5) | `test_t2_f22_01..05` (5) | M3: greenc-vault |
| **F23** | 8 Preset Cleanup Profiles | `test_f23_01..05` (5) | `test_t2_f23_01..05` (5) | M3: greenc-scanner |
| **F24** | Orphan Radar Remnants | `test_f24_01..05` (5) | `test_t2_f24_01..05` (5) | M3: greenc-scanner |
| **F25** | Canary Safety Watchdog | `test_f25_01..05` (5) | `test_t2_f25_01..05` (5) | M3: greenc-vault |
| **F26** | Two-Phase Deletion Safety | `test_f26_01..05` (5) | `test_t2_f26_01..05` (5) | M3: greenc-vault |
| **F27** | Granular Vault Rollback | `test_f27_01..05` (5) | `test_t2_f27_01..05` (5) | M3: greenc-vault |
| **F28** | CleanupPage & VaultPage | `test_f28_01..05` (5) | `test_t2_f28_01..05` (5) | M3: ui |
| **F29** | Reparse Point Junctions | `test_f29_01..05` (5) | `test_t2_f29_01..05` (5) | M4: greenc-relocate |
| **F30** | Known Folders Relocation | `test_f30_01..05` (5) | `test_t2_f30_01..05` (5) | M4: greenc-relocate |
| **F31** | Developer Config Relocation | `test_f31_01..05` (5) | `test_t2_f31_01..05` (5) | M4: greenc-relocate |
| **F32** | Guided App Relocation | `test_f32_01..05` (5) | `test_t2_f32_01..05` (5) | M4: greenc-relocate |
| **F33** | Relocation State Machine | `test_f33_01..05` (5) | `test_t2_f33_01..05` (5) | M4: greenc-relocate |
| **F34** | Relocation Rollback Engine | `test_f34_01..05` (5) | `test_t2_f34_01..05` (5) | M4: greenc-relocate |
| **F35** | In-Place Compaction Engine | `test_f35_01..05` (5) | `test_t2_f35_01..05` (5) | M4: greenc-relocate |
| **F36** | VHDX Slack Compaction | `test_f36_01..05` (5) | `test_t2_f36_01..05` (5) | M4: greenc-relocate |
| **F37** | RelocatePage & CompactPage | `test_f37_01..05` (5) | `test_t2_f37_01..05` (5) | M4: ui |
| **F38** | 11 Write Reason Codes | `test_f38_01..05` (5) | `test_t2_f38_01..05` (5) | M5: greenc-scanner |
| **F39** | Restart Manager Attribution | `test_f39_01..05` (5) | `test_t2_f39_01..05` (5) | M5: greenc-win32 |
| **F40** | Daily Delta Timeline | `test_f40_01..05` (5) | `test_t2_f40_01..05` (5) | M5: greenc-scanner |
| **F41** | 45-Second Guardian Loop | `test_f41_01..05` (5) | `test_t2_f41_01..05` (5) | M5: greenc-app |
| **F42** | Low-Disk Auto Shield | `test_f42_01..05` (5) | `test_t2_f42_01..05` (5) | M5: greenc-app |
| **F43** | Power & Fullscreen Awareness | `test_f43_01..05` (5) | `test_t2_f43_01..05` (5) | M5: greenc-win32 |
| **F44** | WebView2 Unload-on-Tray | `test_f44_01..05` (5) | `test_t2_f44_01..05` (5) | M5: greenc-app |
| **F45** | QuotaRuleModal Component | `test_f45_01..05` (5) | `test_t2_f45_01..05` (5) | M5: ui |
| **F46** | Headless CLI Engine | `test_f46_01..05` (5) | `test_t2_f46_01..05` (5) | M6: greenc-cli |
| **F47** | CLI JSON & Exit Codes | `test_f47_01..05` (5) | `test_t2_f47_01..05` (5) | M6: greenc-cli |
| **F48** | 50-Locale Zero-Restart UI | `test_f48_01..05` (5) | `test_t2_f48_01..05` (5) | M6: greenc-i18n |
| **F49** | Comma-Decimal Formatter | `test_f49_01..05` (5) | `test_t2_f49_01..05` (5) | M6: greenc-i18n |
| **F50** | SettingsPage Component | `test_f50_01..05` (5) | `test_t2_f50_01..05` (5) | M6: ui |
| **F51** | Win32 Unsafe Audit | `test_f51_01..05` (5) | `test_t2_f51_01..05` (5) | M6: greenc-win32 |
| **F52** | NVMe Benchmark Suite | `test_f52_01..05` (5) | `test_t2_f52_01..05` (5) | M6: greenc-vault |
| **F53** | Inno Setup Installer | `test_f53_01..05` (5) | `test_t2_f53_01..05` (5) | M6: packaging |
| **F54** | End-to-End Scenarios | `test_f54_01..05` (5) | `test_t2_f54_01..05` (5) | M7: Validation |

---

## 4. Test Fixtures Ready in `tests/fixtures/`

1. **`legacy_db/`**:
   - `greenc_v1.db`: Complete SQLite database conforming to Python v2.0 schema (7 tables, seed records).
   - `diskguard_v1.db`: Legacy DiskGuard v0.1 schema database.
2. **`mock_tree/`**:
   - `zalo_profile/`: Multi-version directories (`Zalo-2.0.0`, `Zalo-2.1.0`), `config.json`, and `fileNoise` hardlinked files.
   - `dingtalk_profile/`: `v2/EAppFiles`, `resource_cache`, and `dumps/crash_*.dmp`.
   - `dev_caches/`: Realistic structures for `npm`, `pip`, and `cargo`.
   - `downloads/`: Sample installers and documents (> 200 MB simulated).
   - `corrupt_data/`: Locked file stubs, zero-byte files, unicode filenames.
3. **`corrupt_vault/`**:
   - `broken_crc.zip`: Tampered payload with bad CRC32.
   - `tampered_manifest.zip` + `.manifest.json`: Mismatched SHA-256 checksums.
   - `truncated.zip`: Truncated central directory.
4. **`locales/`**:
   - `test_catalog.json`: Valid 298-key catalog.
   - `rtl_sample.json`: Valid RTL Arabic sample catalog.
   - `corrupted_token.json`: Unclosed interpolation brackets.
5. **`moving_state/`**:
   - `.greenc-moving` interrupted directory and partial targets.

---

## 5. How to Run the Tests

### Option 1: PowerShell Runner
```powershell
Set-Location C:\DiskGuard\greenc-v2

# Run all 570 tests across Tiers 1-4
.\run_e2e.ps1 -Tier all

# Run specific Tier (e.g. Tier 1 or Tier 4)
.\run_e2e.ps1 -Tier 1
.\run_e2e.ps1 -Tier 4
```

### Option 2: Python Runner Directly
```powershell
Set-Location C:\DiskGuard\greenc-v2

# Run all tiers with Markdown and JSON report generation
uv run python tests/e2e/test_runner.py --all --report-md test_report.md --report-json test_report.json

# Run tests filtered by feature ID (e.g. F19)
uv run python tests/e2e/test_runner.py --feature F19
```

---

## 6. Integration Track Quality Gate Standards

For implementation subagents working on Milestones M1 through M7:
- Every milestone must verify that newly implemented crates pass their corresponding Tier 1 and Tier 2 tests.
- When `greenc-cli.exe` is built, the test runner automatically switches from mock reference simulation to native binary execution.
- Prior to final delivery of Milestone 7 (M7), the full 570-test suite must run against the compiled binary with a **100% pass rate**.
