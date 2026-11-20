# Test Infrastructure & Methodology: GreenC v2 Complete Platform

**Feature Branch**: `001-greenc-complete-platform`  
**Workspace Root**: `C:\DiskGuard\greenc-v2\`  
**Target Architecture**: Windows x64 (Rust 1.83+ MSVC, Tauri v2 shell, Svelte 5 frontend, Headless CLI)  
**Author**: E2E Test Engineering Team (`test_writer_e2e_1`)  
**Status**: APPROVED & READY FOR EXECUTION  

---

## 1. Test Philosophy & Opaque-Box Methodology

The GreenC v2 testing methodology enforces an **opaque-box (black-box / behavioral) standard**:
1. **Zero Coupling to Implementation Internals**: Test assertions evaluate observable behavior, CLI inputs/outputs (`--json`), exit codes, SQLite WAL database state, file system state, NTFS junction integrity, SHA-256 digests, and typed IPC event contracts. Tests never depend on private functions or crate-internal module structure.
2. **Authoritative Specification Derivation**: Every test case derives its expected values directly from:
   - `C:\DiskGuard\.agents\teamwork\ORIGINAL_REQUEST.md` (Constitutional requirements R1–R3, acceptance criteria)
   - `C:\DiskGuard\.agents\teamwork\orchestrator_1\PROJECT.md` (Feature Inventory F01–F54, 9-crate layout, interface contracts)
   - `C:\DiskGuard\greenc-v2\specs\001-greenc-complete-platform\spec.md` (Functional requirements FR-001–FR-035, Success criteria SC-001–SC-008, Edge cases)
   - `C:\DiskGuard\greenc-v2\specs\001-greenc-complete-platform\contracts\cli-contract.md` (CLI subcommands, options, exit codes: 0, 1, 2, 130)
   - `C:\DiskGuard\greenc-v2\specs\001-greenc-complete-platform\contracts\ipc-commands.md` (29 typed commands, 2 event streaming channels)
3. **Safety & Non-Destructive Execution**: Tests run strictly in isolated temporary sandboxes with injected mock paths and drives. Tests NEVER touch real user directories (`%USERPROFILE%`, `%APPDATA%`, real OneDrive, or production Windows Known Folders).
4. **Reproducibility & Determinism**: All test fixtures, legacy databases, file timestamps, mock drive metrics, and corrupt archives are synthetically generated and deterministic.

---

## 2. 4-Tier Test Suite Architecture

The test suite is structured into 4 distinct testing tiers:

```
                                 [ Tier 4 ]
                         Real-World Application Scenarios
                          (5 Complete End-to-End Flows)
                                     ▲
                                     │
                                 [ Tier 3 ]
                        Cross-Feature Combinations
                         (Pairwise Subsystem Tests)
                                     ▲
                                     │
                   ┌─────────────────┴─────────────────┐
                   │                                   │
               [ Tier 1 ]                          [ Tier 2 ]
            Feature Coverage                Boundary & Corner Cases
      (>=5 Tests per F01..F54)              (>=5 Tests per F01..F54)
         (270 Test Cases)                      (270 Test Cases)
```

### Tier 1 — Feature Coverage (Isolated Happy Path)
- **Requirement**: >= 5 test cases per feature across all 54 features (`F01`–`F54`) in `PROJECT.md § Feature Inventory`.
- **Target**: 270 comprehensive test cases covering normal execution, standard parameter combinations, schema validations, and contract compliance.
- **Organization**:
  - `tier1_features/test_t1_m1_foundation.py`: Features F01–F10 (Workspace, Models, Win32 Traits, File Locks, SQLite WAL, Legacy DB Migration, i18n Catalog, Token Formatter, App State, Svelte Base) — 50 tests.
  - `tier1_features/test_t1_m2_analytics.py`: Features F11–F18 (Drive Eligibility, Parallel Walker, Reparse Tag Guard, Space Lens Treemap, Burn-Rate Forecast, FirstRunModal, OverviewPage, Scanner IPC Streaming) — 40 tests.
  - `tier1_features/test_t1_m3_vault.py`: Features F19–F28 (Streaming ZIP64 Archiver, Incompressible Bypass, Small File Prefetch, 512 MiB Reserve Guard, 8 Preset Cleanup Profiles, Orphan Radar, Canary Safety Watchdog, Two-Phase Deletion, Granular Rollback, Cleanup & Vault UI) — 50 tests.
  - `tier1_features/test_t1_m4_relocation.py`: Features F29–F37 (Reparse Point Junctions, Known Folders Relocation, Dev Config Relocation, Guided App Relocation, Relocation State Machine, Relocation Rollback, In-Place Compaction, VHDX Slack Compaction, Relocate & Compact UI) — 45 tests.
  - `tier1_features/test_t1_m5_guardian.py`: Features F38–F45 (11 Write Reason Codes, Restart Manager Attribution, Daily Delta Timeline, 45s Guardian Loop, Low-Disk Auto Shield, Power & Fullscreen Awareness, WebView2 Unload-on-Tray, QuotaRuleModal) — 40 tests.
  - `tier1_features/test_t1_m6_cli_locales.py`: Features F46–F54 (Headless CLI Engine, CLI JSON & Exit Codes, 50-Locale Live Switching, Comma-Decimal Formatter, SettingsPage, Win32 Unsafe Audit, NVMe Benchmark Suite, Inno Setup Installer, End-to-End Validation Scenarios) — 45 tests.

### Tier 2 — Boundary, Negative & Stress Conditions
- **Requirement**: >= 5 boundary, negative, limit, and resource stress test cases per feature across all 54 features (`F01`–`F54`).
- **Target**: 270 adversarial test cases.
- **Focus Areas**:
  - Empty directories, 0-byte files, extreme path lengths (> 260 characters / `\\?\` prefix), Unicode surrogate pairs, RTL strings, and emoji in file paths.
  - File lock conflicts: `CreateFileW(DELETE)` + `FILE_SHARE_NONE` collisions, Defender file locks, mid-flight modification detection.
  - Disk full boundaries: enforcing the strict 512 MiB free space reserve threshold; simulated disk exhaustion during compression.
  - Corrupt artifacts: tampered SHA-256 checksums in `.manifest.json`, truncated ZIP64 streams, invalid SQLite WAL header, missing database tables.
  - Cooperative cancellation: `Ctrl+C` / SIGINT simulation verifying standard exit code `130` and source file preservation.
  - Precondition blocks: exit code `2` on OneDrive synchronization directories, active processes during relocation, non-NTFS volumes.

### Tier 3 — Cross-Feature Combinations (Pairwise Subsystem Tests)
- **Requirement**: Comprehensive pairwise and tripartite integration tests verifying that independently tested features interact safely.
- **Target**: >= 25 combination test cases.
- **Key Interactions**:
  1. *Parallel Scanning + Vault Archiving* under live file modifications.
  2. *Relocation State Machine (`.greenc-moving`) + Crash Recovery* during active Guardian shield loops.
  3. *In-Place WOF Compaction + Reparse Point Guard* (ensuring junctions are not double-compressed or traversed).
  4. *Legacy SQLite Database Migration + Granular Vault Archive Rollback*.
  5. *Emergency Low-Disk Auto Shield + Quota Rule Watermark* (85% safe target reduction).
  6. *Live 50-Language RTL Switching + Svelte 5 IPC Progress Stream*.
  7. *Canary Probe Failure + Fallback to Cache-Only Subdirectories*.
  8. *Single-File Restore Conflict Handling (`.restored_<timestamp>`) under Active Process Lock*.

### Tier 4 — Real-World Application Scenarios
- **Requirement**: >= 5 realistic, end-to-end, multi-stage application workflows matching `quickstart.md § 3` and `spec.md § User Stories`.
- **Target**: 5 extensive end-to-end workflow validations:
  - **Scenario 1: First-Run Onboarding, NVMe Space Lens Scan & 14-Day Exhaustion Forecasting** (`US1`, `SC-001`)
  - **Scenario 2: Smart App Profile Cleanup with Active File Lock, Canary Verification & Single-File Rollback** (`US2`, `SC-002`, `SC-003`)
  - **Scenario 3: Multi-Drive Junction Relocation with Mid-Copy Power-Loss Recovery & Health Checks** (`US3`, `SC-002`)
  - **Scenario 4: Cold In-Place WOF XPRESS8K Compaction & VHDX Slack Compaction on C: Drive** (`US4`)
  - **Scenario 5: Full Legacy Migration (Python v1 -> Rust v2) with Database and Vault Upgrade** (`US7`, `SC-008`)

---

## 3. Test Fixtures Architecture (`tests/fixtures/`)

The test suite uses realistic, pre-generated and on-demand mock fixtures:

| Directory | Contents | Purpose |
|-----------|----------|---------|
| `tests/fixtures/legacy_db/` | `greenc_v1.db`, `diskguard_v1.db` | Authentic SQLite databases with legacy v1 schemas, sample quota rules, snapshots, and archive records for lossless migration verification. |
| `tests/fixtures/mock_tree/` | `zalo_profile/`, `dingtalk_cache/`, `dev_caches/`, `downloads/`, `orphans/` | Realistic Windows file system hierarchies containing NTFS hardlinks, nested subfolders, dev configs (`.npmrc`, `pip.ini`, `Cargo.toml`), and uninstall registry stubs. |
| `tests/fixtures/corrupt_vault/` | `broken_crc.zip`, `tampered_manifest.zip`, `truncated.zip` | Adversarial archives testing checksum verification, CRC failures, and graceful error handling (exit code `1`). |
| `tests/fixtures/locales/` | `test_catalog.json`, `corrupted_token.json`, `rtl_sample.json` | Sample localization files for testing placeholder interpolation, RTL tags, and missing key fallbacks. |
| `tests/fixtures/moving_state/` | `.greenc-moving` folder + mock database state | Interrupted relocation simulation to verify crash recovery on startup. |

---

## 4. Test Runner & Invocation Guide

The test harness supports multiple execution modes for automated CI/CD and developer testing:

### Mode A: Python 3 Test Runner (Recommended for Comprehensive E2E Verification)
```powershell
Set-Location C:\DiskGuard\greenc-v2

# Run all test tiers (Tiers 1, 2, 3, 4)
python tests/e2e/test_runner.py --all

# Run specific tier
python tests/e2e/test_runner.py --tier 1
python tests/e2e/test_runner.py --tier 2
python tests/e2e/test_runner.py --tier 3
python tests/e2e/test_runner.py --tier 4

# Run specific feature tests
python tests/e2e/test_runner.py --feature F19

# Generate Markdown and JUnit XML reports
python tests/e2e/test_runner.py --all --report-md test_report.md --report-xml test_report.xml
```

### Mode B: PowerShell One-Click Runner
```powershell
Set-Location C:\DiskGuard\greenc-v2

# Run all tiers with colored console output
.\run_e2e.ps1 -Tier all

# Run individual tier
.\run_e2e.ps1 -Tier 1
.\run_e2e.ps1 -Tier 4 -Verbose
```

### Mode C: Cargo Integration Test Runner (When Rust Workspace is Built)
```powershell
Set-Location C:\DiskGuard\greenc-v2

# Run all Rust workspace unit & integration tests
cargo test --workspace

# Run CLI contract tests
cargo test -p greenc-cli --test cli_contract_test
```

---

## 5. Quality Gates & Pass/Fail Thresholds

To pass the E2E quality gate and achieve `TEST_READY.md` sign-off:

1. **Test Pass Rate**: 100% of test cases in Tiers 1–4 must execute and pass.
2. **Feature Coverage**: 54/54 features (`F01`–`F54`) covered with >= 5 Tier 1 and >= 5 Tier 2 tests each.
3. **Data Safety Standard (`SC-002`)**: Zero data loss across all rollback, relocation, and migration tests.
4. **Active Lock Protection (`SC-003`)**: Zero files locked by running processes touched or unlinked.
5. **Contract Parity**: 100% exit codes (`0`, `1`, `2`, `130`) match `cli-contract.md`.
6. **Zero Leaked Sandboxes**: All temporary test sandboxes must be deterministically cleaned up.
