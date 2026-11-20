---
description: "Actionable, dependency-ordered Rust implementation task list for GreenC v2"
---

# Tasks: GreenC v2 — Viết Lại Toàn Bộ Bằng Rust (`C:\DiskGuard\greenc-v2`)

**Input**: Design documents from `/specs/001-greenc-complete-platform/`
**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)
**Tests**: Included per Constitution Principle III (Test-First & Trait-Isolated OS Boundaries).
**Organization**: Tasks are grouped by User Story (`US1`–`US7`) to enable independent implementation, verification, and incremental delivery inside `C:\DiskGuard\greenc-v2`.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (touches distinct files/crates without incomplete dependencies)
- **[Story]**: Maps task to a specific user story in `spec.md` (`[US1]`..`[US7]`)
- All file paths are relative to workspace root `C:\DiskGuard\greenc-v2\`

---

## Phase 1: Setup (Workspace & Build Infrastructure)

**Purpose**: Initialize the Cargo multi-crate workspace, frontend build pipeline, and static quality gates in `C:\DiskGuard\greenc-v2`.

- [x] T001 Create Cargo workspace manifest with shared dependencies (`windows`, `rusqlite`, `tokio`, `rayon`, `serde`, `zip`, `zstd`, `sha2`, `clap`, `thiserror`, `tracing`) and strict Clippy lints in `Cargo.toml`
- [x] T002 [P] Scaffold library crate manifests and `src/lib.rs` stubs for `crates/greenc-core/Cargo.toml`, `crates/greenc-win32/Cargo.toml`, `crates/greenc-storage/Cargo.toml`, `crates/greenc-i18n/Cargo.toml`, `crates/greenc-scanner/Cargo.toml`, `crates/greenc-vault/Cargo.toml`, and `crates/greenc-relocate/Cargo.toml`
- [x] T003 [P] Scaffold binary crate manifests for `crates/greenc-cli/Cargo.toml` and Tauri v2 shell `crates/greenc-app/Cargo.toml` plus `crates/greenc-app/tauri.conf.json`
- [x] T004 [P] Initialize Svelte 5 + TypeScript + Vite frontend project in `ui/package.json`, `ui/tsconfig.json`, and `ui/vite.config.ts`
- [x] T005 [P] Copy and extend the 50 localization catalogs (`messages.json` + 49 `<locale>.json` files) from `C:\DiskGuard\greenc\i18n\` into `crates/greenc-i18n/locales/`
- [x] T006 [P] Create GitHub Actions CI/Release workflow for Rust fmt, clippy, workspace tests, i18n check, and Inno Setup packaging in `.github/workflows/ci-release.yml`

---

## Phase 2: Foundational (Core Domain, Win32 RAII Traits, SQLite WAL & i18n)

**Purpose**: Core infrastructure that MUST be complete before any User Story begins.

**⚠️ CRITICAL**: All User Stories depend on this phase completing cleanly with passing unit tests.

- [x] T007 Implement branding constants (`APP_NAME = "GreenC"`, `APP_ID = "GreenC"`, `DATA_FOLDER_NAME = "GreenC"`, `LEGACY_DATA_FOLDER_NAME = "Green C"`, `DEFAULT_PROTECTED_PATTERNS`, retention bounds `1..=90`, reserve `512 MiB`) in `crates/greenc-core/src/config.rs`
- [x] T008 [P] Define strongly-typed domain structs and enums (`DriveInfo`, `DailySnapshot`, `WriteEvent`, `WriteReasonCode`, `QuotaRule`, `VaultArchive`, `VaultStatus`, `FileManifestItem`, `RelocationRecord`, `RelocationStatus`, `CompactionRecord`) in `crates/greenc-core/src/models.rs`
- [x] T009 [P] Implement `GreenCError` hierarchy (`InsufficientVaultSpace`, `FileLocked`, `IntegrityMismatch`, `PreconditionBlocked`, `Cancelled`, `Sqlite`, `Win32`) in `crates/greenc-core/src/error.rs`
- [x] T010 Implement thread-safe `TaskControl` (`Pause`, `Resume`, `< 100 ms` `CancellationToken` Stop, and `100 ms`-throttled `ProgressCallback` with ETA calculation) plus unit tests in `crates/greenc-core/src/task.rs`
- [x] T011 [P] Define injectable OS boundary traits (`Win32VolumeApi`, `FileLockProbe`, `ShellFolderApi`, `ReparsePointApi`, `ProcessServiceProbe`, `CommandRunner`) and mock test implementations in `crates/greenc-win32/src/traits.rs`
- [x] T012 [P] Implement RAII `OwnedHandle` wrapper, `is_file_locked` (`CreateFileW` with `DELETE` access and `FILE_SHARE_NONE`), and `GreenCAppMutex` single-instance lock in `crates/greenc-win32/src/handle.rs` and `crates/greenc-win32/src/mutex.rs`
- [x] T013 Implement `GreenCDb` SQLite WAL connection manager and idempotent Schema v1→v2 migrations (`daily_snapshots`, `write_events`, `quota_rules`, `vault_archives`, `vault_files`, `app_settings`, `relocations`, `compaction_records`) in `crates/greenc-storage/src/db.rs` and `crates/greenc-storage/src/migrations.rs`
- [x] T014 Implement `AppSettings` typed wrapper (clamping `retention_days_default` to `1..=90`, validating `language` against `LANGUAGE_CODES`, `theme_mode`, `data_root`, `low_disk_shield_gb`) and unit tests in `crates/greenc-storage/src/settings.rs`
- [x] T015 Implement lossless legacy database and Vault migration (`migrate_legacy_db` from `%LOCALAPPDATA%\Green C\greenc.db` and `C:\DiskGuard\data\diskguard.db`, plus verified `move_vault`) with rollback-on-disk-full tests in `crates/greenc-storage/src/legacy_migration.rs`
- [x] T016 [P] Implement in-memory 50-language `Catalog` (`tr`, `is_rtl`, `match_system_language`, `decimal_point`, `format_size`, `format_eta`) and placeholder validator binary in `crates/greenc-i18n/src/catalog.rs`, `crates/greenc-i18n/src/format.rs`, and `crates/greenc-i18n/src/validator.rs`
- [x] T017 [P] Build Tauri `AppState` container, background task registry, and Windows 11 Fluent base CSS tokens + reusable `Sidebar`, `ExpandableTableCard`, `PathCell` (copy toast + Explorer reveal), and `StatusBar` components in `crates/greenc-app/src/state.rs` and `ui/src/lib/components/`

**Checkpoint**: `cargo test --workspace` passes for `greenc-core`, `greenc-win32`, `greenc-storage`, and `greenc-i18n`; existing Python `greenc.db` opens and migrates without data loss.

---

## Phase 3: User Story 1 - Thiết lập Ổ dữ liệu, Tổng quan Sức khỏe Ổ C:, Space Lens & Dự báo Đầy Đĩa (Priority: P1) 🎯 MVP

**Goal**: Deliver first-run data drive selection, drive eligibility checks, Overview health card, predictive "Days Until C: Full" burn-rate forecast, and interactive Space Lens Treemap + reclaimable summary card.

**Independent Test**: Run `cargo test -p greenc-win32 -p greenc-scanner` and launch `OverviewPage.svelte` against mocked/temp drives to verify drive eligibility, first-run persistence, burn-rate forecast math, and Space Lens treemap drill-down.

### Tests for User Story 1

- [x] T018 [P] [US1] Write unit tests for fixed NTFS drive filtering (rejecting USB, exFAT/FAT32, Recovery partitions, and `< 10 GiB` recommendation fallback) in `crates/greenc-win32/tests/volume_eligibility_test.rs`
- [x] T019 [P] [US1] Write unit tests for 128-bit NTFS `FileId` hardlink deduplication, nested reparse-point skipping (`0x400`), Space Lens tree aggregation, and burn-rate days-until-full calculation in `crates/greenc-scanner/tests/space_lens_and_forecast_test.rs`

### Implementation for User Story 1

- [x] T020 [P] [US1] Implement Win32 volume enumeration (`GetLogicalDrives`, `GetDriveTypeW`, `GetVolumeInformationW`, `SetThreadErrorMode(SEM_FAILCRITICALERRORS)`) and `recommend_drive` in `crates/greenc-win32/src/volume.rs`
- [x] T021 [P] [US1] Implement parallel `rayon` directory walker (`FindFirstFileExW` + `GetFileInformationByHandleEx(FileIdInfo)` hardlink dedup, skipping nested junctions and OneDrive cloud placeholders) in `crates/greenc-scanner/src/walker.rs`
- [x] T022 [US1] Implement optional elevated USN/MFT fast scanner fallback and hierarchical `SpaceNode` Treemap builder in `crates/greenc-scanner/src/usn_mft.rs` and `crates/greenc-scanner/src/space_lens.rs`
- [x] T023 [US1] Implement rolling 7/14/30-day burn-rate velocity and `days_until_90_pct` / `days_until_full` predictor in `crates/greenc-scanner/src/forecast.rs`
- [x] T024 [US1] Expose `get_bootstrap_state`, `complete_first_run`, `get_overview_summary`, `scan_space_lens`, and `reveal_in_explorer` IPC commands in `crates/greenc-app/src/commands/overview.rs`
- [x] T025 [P] [US1] Build `FirstRunModal.svelte` (drive radio cards, `Đề xuất` / `Ổ hệ thống` / `Không dùng được` tags, `1..=90` retention spinbox, `Để sau` / `Tiếp tục`) in `ui/src/lib/components/FirstRunModal.svelte`
- [x] T026 [US1] Build `OverviewPage.svelte` with Drive Usage Card (red danger bar `>= 90%` + Burn-Rate Forecast badge), interactive `SpaceTreemap.svelte`, and eye-toggleable `ExpandableTableCard` in `ui/src/pages/OverviewPage.svelte`

**Checkpoint**: User Story 1 works end-to-end as a standalone MVP (`greenc-cli status` + First-Run Dialog + Overview Dashboard + Space Lens).

---

## Phase 4: User Story 2 - Dọn dẹp Kiểm chứng Canary, Kho Dự phòng (Vault) & Tự động Khôi phục (Priority: P1)

**Goal**: Deliver Smart App Profiles, Orphaned Leftover Radar, 1-cycle Sandbox Canary Probe, parallel `ZIP64` (`STORED`/`DEFLATE`/`ZSTD`) archiving with `SHA-256` & `NTFS HardLink` preservation, Granular Single-File Restore, and `SafetyWatchdog` Auto-Rollback.

**Independent Test**: Run `cargo test -p greenc-vault` verifying active Win32 locked files and modified-in-flight files are skipped, hardlink siblings are archived once and restored with `CreateHardLinkW`, single-file restore extracts in `O(1)` time, and simulated service stop / missing-file log triggers atomic Auto-Rollback + permanent protection.

### Tests for User Story 2

- [x] T027 [P] [US2] Write integration tests for parallel `ZIP64` archiver (small-file prefetch pool, `> 4 GiB` streaming, pre-compressed magic header `STORED` bypass, `512 MiB` Vault free-space reserve, and mid-flight file modification skip) in `crates/greenc-vault/tests/archiver_pipeline_test.rs`
- [x] T028 [P] [US2] Write integration tests for `SafetyWatchdog` Auto-Rollback, `.restored_<timestamp>` conflict handling, NTFS hardlink group reconstruction, Granular Single-File Restore, and legacy Python `.zip` backward compatibility in `crates/greenc-vault/tests/watchdog_rollback_test.rs`
- [x] T029 [P] [US2] Write unit tests for Zalo/DingTalk multi-drive profiles, Canary Probe fallback to cache-only subdirectories, and Orphaned App Leftover detection against mocked Uninstall Registry entries in `crates/greenc-scanner/tests/profiles_and_orphans_test.rs`

### Implementation for User Story 2

- [x] T030 [P] [US2] Implement Zalo (`%APPDATA%\ZaloData\config.json`, inactive `Zalo-*` version folders, `fileNoise` hardlinks), DingTalk (`*_v2/EAppFiles`, `resource_cache`, `dumps`), browser, and dev-cache profiles in `crates/greenc-scanner/src/profiles.rs`
- [x] T031 [P] [US2] Implement `OrphanLeftoverRadar` cross-referencing `HKLM`/`HKCU` `Uninstall` registry keys, Start Menu `.lnk` targets, and running processes to detect leftover `%APPDATA%`/`%LOCALAPPDATA%` folders in `crates/greenc-scanner/src/orphans.rs`
- [x] T032 [P] [US2] Implement pre-compressed format sniffer (extension set + 15 magic byte signatures + 64 KiB sample compression ratio `< 0.95` check) in `crates/greenc-vault/src/sniff.rs`
- [x] T033 [US2] Implement parallel `ZIP64` Vault archiver (`16`-worker bounded prefetch channel, hardware `SHA-256`, `STORED`/`DEFLATE`/`ZSTD`, pre-delete second lock & `mtime`/`size` re-verification, atomic hardlink sibling unlink) in `crates/greenc-vault/src/archiver.rs`
- [x] T034 [US2] Implement `QuotaEnforcer` (`85%` watermark calculation, `DEFAULT_PROTECTED_PATTERNS` + `min_file_age_seconds >= 60` filter, oldest-`mtime`-first selection) and 1-cycle `CanaryProbe` analyzer in `crates/greenc-vault/src/enforcer.rs` and `crates/greenc-vault/src/canary.rs`
- [x] T035 [US2] Implement `SafetyWatchdog` (Windows Service status check, recent `.log`/`.err`/`.calf` missing-file error scanner, full `rollback_archive`, `restore_files_selective` single-file extractor, permanent `protected_files` insertion, and `purge_expired_archives`) in `crates/greenc-vault/src/watchdog.rs`
- [x] T036 [P] [US2] Implement `SandboxSimulator` (Canary + HardLink + simulated crash Auto-Rollback demo inside `%LOCALAPPDATA%\GreenC\sandbox`) in `crates/greenc-vault/src/simulator.rs`
- [x] T037 [US2] Expose `analyze_cleanup_candidates`, `execute_cleanup`, `list_vault_archives`, `inspect_vault_archive`, `restore_vault_archive`, `restore_vault_files_selective`, `purge_vault_archive`, and `run_sandbox_demo` IPC commands in `crates/greenc-app/src/commands/cleanup_vault.rs`
- [x] T038 [US2] Build `CleanupPage.svelte` (Canary probe badges, orphan tags, checkboxes, 1-click `Dọn dẹp {size}`) and `VaultPage.svelte` (archive table + modal file browser with search & selective file restore) in `ui/src/pages/CleanupPage.svelte` and `ui/src/pages/VaultPage.svelte`

**Checkpoint**: User Stories 1 and 2 work end-to-end; users can analyze, clean safely into Vault at `> 250 MB/s`, browse/restore individual files, and rely on automatic rollback.

---

## Phase 5: User Story 3 - Di chuyển Dữ liệu Đa Chiến lược & Tự Phục hồi Sau Sự cố (Priority: P1)

**Goal**: Deliver 4-tier safe data relocation off `C:` (`Known Folders`, `10+ Dev/AI Tool Configs`, `Guided Zalo/WSL2/Docker`, and `Verified NTFS Junctions`) with power-loss recovery (`recover_interrupted`), live junction health monitoring, and 1-click Undo.

**Independent Test**: Run `cargo test -p greenc-relocate` on temporary directories with injected `ShellFolderApi` and `CommandRunner` mocks (never touching real user Known Folders or global tool configs), including simulated mid-copy cancellation, hash mismatch abort, `.greenc-moving` power-loss recovery, broken-link health detection, and full Undo.

### Tests for User Story 3

- [x] T039 [P] [US3] Write unit and crash-recovery tests for `JunctionStrategy` (process-running block, `SHA-256` copy mismatch abort, `.greenc-moving` power-loss recovery in both directions, and Undo) in `crates/greenc-relocate/tests/junction_and_recovery_test.rs`
- [x] T040 [P] [US3] Write unit tests for `KnownFolderStrategy` (OneDrive path skip, `SHSetKnownFolderPath` rollback on failure), `ToolConfigStrategy` (`npm`, `pip`, `uv`, `cargo`, `pnpm`, `yarn`, `nuget`, `gradle`, `ollama`, `huggingface`), and `GuidedStrategy` (`Zalo`, `WSL2`, `Docker`) in `crates/greenc-relocate/tests/strategies_catalog_test.rs`

### Implementation for User Story 3

- [x] T041 [P] [US3] Implement direct Win32 `FSCTL_SET_REPARSE_POINT` / `FSCTL_GET_REPARSE_POINT` junction creation, target resolution, and `SHGetKnownFolderPath` / `SHSetKnownFolderPath` + OneDrive guard in `crates/greenc-win32/src/reparse.rs` and `crates/greenc-win32/src/known_folders.rs`
- [x] T042 [US3] Implement streaming `verified_copy_tree` (inline `SHA-256` source + destination verification, timestamp preservation, nested reparse skip, cooperative Stop + partial-target cleanup) in `crates/greenc-relocate/src/verified_copy.rs`
- [x] T043 [P] [US3] Implement `KnownFolderStrategy` (`Downloads`, `Documents`, `Pictures`, `Videos`, `Music`, `Desktop`) in `crates/greenc-relocate/src/known_folder.rs`
- [x] T044 [P] [US3] Implement `ToolConfigStrategy` catalog (`npm`, `pip`, `uv`, `cargo`, `pnpm`, `yarn`, `nuget`, `gradle`, `ollama`, `huggingface`) in `crates/greenc-relocate/src/tool_config.rs`
- [x] T045 [P] [US3] Implement `GuidedStrategy` (`Zalo` `config.json` verifier, `WSL2` distro move guide/verifier, `Docker Desktop` `.vhdx` move guide/verifier) in `crates/greenc-relocate/src/guided.rs`
- [x] T046 [US3] Implement `JunctionStrategy` (`Telegram`, `Discord`, `Slack`, `CapCut`, `JetBrains`, plus opt-in `Nâng cao` `%APPDATA%`/`%LOCALAPPDATA%` `>= 200 MiB` with `ADVANCED_SKIP` system guardrails), `recover_interrupted`, and live `check_junction_health` in `crates/greenc-relocate/src/junction.rs`, `crates/greenc-relocate/src/recovery.rs`, and `crates/greenc-relocate/src/catalog.rs`
- [x] T047 [US3] Expose `discover_relocation_candidates`, `list_relocation_records`, `apply_relocation`, `verify_guided_relocation`, `open_guided_app`, `undo_relocation`, and `repair_junction_target` IPC commands in `crates/greenc-app/src/commands/relocate.rs`
- [x] T048 [US3] Build `RelocatePage.svelte` and `GuidedModal.svelte` (primary candidates, collapsible `Hiện thư mục nâng cao`, `Đã chuyển` health badges, `Hoàn tác`, and target reconnect helper) in `ui/src/pages/RelocatePage.svelte` and `ui/src/lib/components/GuidedModal.svelte`

**Checkpoint**: All 4 relocation strategies, startup crash recovery, junction health checks, and 1-click Undo work end-to-end.

---

## Phase 6: User Story 4 - Thu gọn Tại chỗ ("Nén Lạnh" WOF) & Thu gọn Đĩa ảo `.vhdx` (Priority: P2)

**Goal**: Enable single-drive (`C:`-only) laptops and immovable SDK/toolchain directories to reclaim 35–60% physical space via transparent OS per-file compression (`WOF XPRESS8K/16K`) and reclaim unused slack inside `.vhdx` virtual disks without changing any file path.

**Independent Test**: Run `cargo test -p greenc-relocate --test compact_inplace_test` on a temp NTFS folder, verifying `GetCompressedFileSizeW` decreases while logical file content & `SHA-256` remain identical and decompression restores normal allocation.

### Tests for User Story 4

- [x] T049 [P] [US4] Write integration tests for `WOF` / `LZNT1` transparent folder compaction (skipping locked & pre-compressed files, measuring physical bytes before/after, and decompressing back) in `crates/greenc-relocate/tests/compact_inplace_test.rs`

### Implementation for User Story 4

- [x] T050 [P] [US4] Implement Win32 `WofSetFileDataLocation` (`XPRESS8K` / `XPRESS16K`), `FSCTL_SET_COMPRESSION`, `GetCompressedFileSizeW`, and `virtdisk.dll` `CompactVirtualDisk` bindings in `crates/greenc-win32/src/wof_compact.rs`
- [x] T051 [US4] Implement `InPlaceCompactor` candidate scanner (cold SDK/toolchain/`node_modules`/shader directories & stopped WSL2/Docker `.vhdx` images), compaction runner, and decompression undo in `crates/greenc-relocate/src/compact.rs`
- [x] T052 [US4] Expose `discover_compaction_candidates`, `apply_inplace_compaction`, and `undo_inplace_compaction` IPC commands in `crates/greenc-app/src/commands/compact.rs` and build `CompactPage.svelte` in `ui/src/pages/CompactPage.svelte`

**Checkpoint**: Users on single-drive laptops can compact cold directories and `.vhdx` virtual disks safely on `C:` and undo at any time.

---

## Phase 7: User Story 5 - Dòng Thời gian Biến động Theo Ngày & Truy vết Tác nhân Ghi Đĩa Thực (Priority: P2)

**Goal**: Deliver interactive 7/14/30-day daily delta bar charts, real-time `ReadDirectoryChangesW` growth + deletion accounting, hybrid Process/Windows Service attribution (Restart Manager + I/O deltas + path signatures), and 1-click Quota Rule prefill from any culprit row.

**Independent Test**: Run `cargo test -p greenc-scanner --test attribution_and_daily_delta_test` verifying all 11 `WriteReasonCode` classifications, PID/Service attribution, signed daily net changes, and per-day drill-down queries.

### Tests for User Story 5

- [x] T053 [P] [US5] Write unit and integration tests for `classify_write_reason` (all 11 reason codes), `AttributionEngine` PID/Service mapping, and `DiskTracker` daily snapshot + drill-down aggregation in `crates/greenc-scanner/tests/attribution_and_daily_delta_test.rs`

### Implementation for User Story 5

- [x] T054 [P] [US5] Implement Win32 Restart Manager (`RmStartSession`, `RmRegisterResources`, `RmGetList`) and Service Control Manager PID-to-service mapper in `crates/greenc-win32/src/restart_mgr.rs`
- [x] T055 [US5] Implement `AttributionEngine` and `DiskTracker` (combining fast directory history scan + `notify` `ReadDirectoryChangesW` live write/delete coalescer) in `crates/greenc-scanner/src/attribution.rs` and `crates/greenc-scanner/src/tracker.rs`
- [x] T056 [US5] Expose `scan_drive_history` and `get_daily_drilldown` IPC commands in `crates/greenc-app/src/commands/history.rs`
- [x] T057 [US5] Build interactive `DailyBarChart.svelte` (7/14/30-day selector, clickable day bars, signed `+GB / -GB` labels) and two-column drill-down tables (Folders/Reasons + Apps/Services with `Tạo quy tắc` action) in `ui/src/lib/components/DailyBarChart.svelte`

**Checkpoint**: Clicking any day bar immediately updates folder/reason and app/service culprits with 1-click Quota Rule creation.

---

## Phase 8: User Story 6 - Quy tắc Hạn mức Xoay vòng Tự động & Chạy ngầm Thông minh ở Khay Hệ thống (Priority: P2)

**Goal**: Deliver background `QuotaRule` management, the silent 45-second guardian loop that yields `< 100 ms` to foreground user actions, battery-saver / fullscreen-game deferral, Emergency Low-Disk Shield, Start-with-Windows toggle, and `< 15 MB` RAM WebView2 unload-on-tray-close.

**Independent Test**: Run `cargo test -p greenc-app` verifying silent guardian yields immediately when a foreground task starts, defers compression when battery/fullscreen flags are active, and destroys/recreates the WebView2 window cleanly on tray minimize/restore.

### Tests for User Story 6

- [x] T058 [P] [US6] Write unit tests for guardian scheduler preemption (silent worker yields to user task), battery/fullscreen deferral, and low-disk threshold trigger in `crates/greenc-app/tests/guardian_scheduler_test.rs`

### Implementation for User Story 6

- [x] T059 [P] [US6] Implement Win32 power status (`GetSystemPowerStatus`), fullscreen app detector (`SHQueryUserNotificationState`), and per-user Registry autostart (`HKCU\...\Run\GreenC`) in `crates/greenc-win32/src/power_session.rs`
- [x] T060 [US6] Implement `BackgroundGuardian` 45-second loop (`QuotaEnforcer` + `SafetyWatchdog` + `check_junction_health` + Low-Disk Shield, yielding immediately on foreground task request) in `crates/greenc-app/src/guardian.rs`
- [x] T061 [US6] Implement System Tray menu (`Mở GreenC` / `Thoát`), native toast notifications for Auto-Rollback/Low-Disk alerts, and WebView2 window unload-on-close / `< 120 ms` recreate-on-open in `crates/greenc-app/src/tray.rs`
- [x] T062 [US6] Expose `list_quota_rules`, `save_quota_rule`, `delete_quota_rule`, `change_data_root`, and `control_task` IPC commands in `crates/greenc-app/src/commands/rules_settings.rs` and build `QuotaRuleModal.svelte` in `ui/src/lib/components/QuotaRuleModal.svelte`

**Checkpoint**: Closing the window drops resident RAM below `15 MB` while keeping rolling quotas and Auto-Rollback active in the system tray.

---

## Phase 9: User Story 7 - 50 Ngôn ngữ Tức thì + RTL, Giao diện Sáng/Tối, CLI Quản trị & Bộ cài Inno Setup (Priority: P3)

**Goal**: Deliver zero-restart `< 100 ms` live switching across all 50 languages (including RTL mirroring for `ar`, `he`, `fa`, `ur`), Light/Dark/System theme sync, full `greenc-cli` with `--json` & policy export/import, and Inno Setup installer `< 10 MB` seamlessly upgrading existing Python GreenC installs.

**Independent Test**: Run `greenc-i18n check` across all 50 locales, execute `greenc-cli` contract tests for `--json` output and policy round-trip, and build `GreenC-Setup-<version>.exe` verifying clean upgrade over v2.0.0.

### Tests for User Story 7

- [x] T063 [P] [US7] Write CLI contract tests (`status --json`, `analyze`, `clean --dry-run`, `vault list`, `policy export/import`) in `crates/greenc-cli/tests/cli_contract_test.rs`
- [x] T064 [P] [US7] Write automated 50-locale completeness, `{placeholder}` parity, and RTL layout direction tests in `crates/greenc-i18n/tests/all_locales_test.rs` and `ui/src/__tests__/rtl_and_theme.test.ts`

### Implementation for User Story 7

- [x] T065 [P] [US7] Implement reactive `i18n.svelte.ts` store (`dir="rtl"` / `dir="ltr"` root attribute switching, LTR spinbox/path isolation, locale decimal formatting) and Light/Dark/System theme switcher in `ui/src/lib/i18n.svelte.ts` and `ui/src/lib/theme.css`
- [x] T066 [US7] Build `SettingsPage.svelte` (live Language selector without restart, Theme selector, Data Location changer, Retention spinbox, Autostart & Battery/Low-Disk toggles, Background Rules table, and About card with `Cùng Chat · cungchat.com`) in `ui/src/pages/SettingsPage.svelte`
- [x] T067 [P] [US7] Implement full `greenc-cli` binary (`status`, `scan`, `analyze`, `clean`, `vault`, `relocate`, `compact`, `policy export/import`) per `contracts/cli-contract.md` in `crates/greenc-cli/src/main.rs`
- [x] T068 [US7] Create Inno Setup installer script preserving `AppId={{6F1C2B7A-3E4D-4C5B-9A8E-7D2F1B0C9E51}` and `AppMutex=GreenCAppMutex` (cleaning up legacy PyInstaller `_internal` directory on upgrade) plus release build script in `packaging/greenc.iss` and `scripts/build.ps1`

**Checkpoint**: All 50 locales, live RTL/Theme switching, `greenc-cli`, and `< 10 MB` Windows installer pass verification.

---

## Phase 10: Polish & Cross-Cutting Verification

**Purpose**: Final performance benchmarking, security audit of Win32 `unsafe` blocks, and end-to-end validation against `quickstart.md` and `spec.md` Success Criteria (`SC-001`..`SC-008`).

- [x] T069 [P] Audit every `unsafe` block in `crates/greenc-win32/` for `SAFETY:` invariant comments, handle leak freedom, and path canonicalization against system-directory guardrails
- [x] T070 [P] Benchmark NVMe scan throughput (`500k` files `< 3s`), Vault parallel compression (`>= 250 MB/s`), and tray idle working-set memory (`< 15 MB` after WebView2 unload) in `crates/greenc-vault/benches/archive_bench.rs`
- [x] T071 Execute all 5 end-to-end validation scenarios in `specs/001-greenc-complete-platform/quickstart.md` (including real upgrade test from Python GreenC v2.0 `greenc.db` + `Vault/*.zip`)

---

## Dependencies & Execution Order

### Phase Dependencies

1. **Phase 1 (Setup)** → starts immediately.
2. **Phase 2 (Foundational)** → depends on Phase 1; **BLOCKS** all User Stories (`US1`–`US7`).
3. **User Story Phases (Phase 3–9)** → once Phase 2 finishes:
   - **MVP Track (P1)**: `US1` (Phase 3), `US2` (Phase 4), and `US3` (Phase 5) can be developed **in parallel** across separate crates (`greenc-scanner`, `greenc-vault`, `greenc-relocate`)!
   - **Depth Track (P2)**: `US4` (Phase 6: In-Place Compaction) extends `greenc-relocate`; `US5` (Phase 7: Daily Delta & Attribution) extends `greenc-scanner`; `US6` (Phase 8: Tray Guardian) integrates `US2` + `US3` into `greenc-app`.
   - **Global & Admin Track (P3)**: `US7` (Phase 9: Live 50-Locale + CLI + Installer) wraps all core crates for release.
4. **Phase 10 (Polish)** → runs after all targeted User Stories complete.

### Parallel Opportunities

- **Within Phase 2**: `T008`/`T009` (`greenc-core`), `T011`/`T012` (`greenc-win32`), `T016` (`greenc-i18n`), and `T017` (`ui` base components) touch completely separate directories and run in parallel.
- **Across P1 Stories**: Three engineers/subagents can simultaneously implement `US1` (`greenc-scanner::walker` + `OverviewPage`), `US2` (`greenc-vault` + `CleanupPage`/`VaultPage`), and `US3` (`greenc-relocate` + `RelocatePage`).

---

## Implementation Strategy

### Incremental Delivery Milestones

1. **Milestone 1 — Foundation + US1 (MVP)**: Fast scanner, Space Lens Treemap, Burn-Rate Forecast, and First-Run Data Drive setup.
2. **Milestone 2 — US2 (Safe Cleanup & Vault)**: Parallel ZIP64/ZSTD Vault, Canary Probe, Orphan Radar, Granular Single-File Restore, and Auto-Rollback Watchdog.
3. **Milestone 3 — US3 + US4 (Relocation & Cold Compaction)**: 25+ relocation profiles, crash-safe NTFS junctions, WOF XPRESS cold compression, and `.vhdx` compaction.
4. **Milestone 4 — US5 + US6 + US7 (Autonomous Guardian & Release)**: Daily attribution timeline, `< 15 MB` Tray Guardian, instant 50-language RTL UI, `greenc-cli`, and `< 10 MB` `GreenC-Setup-2.1.0.exe` drop-in upgrade installer.
