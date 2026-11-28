# GreenC Low-Tech Redesign & One-Click "Make Green Now" Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign GreenC for low-tech users with a full-width top-bar icon layout, a real end-to-end 1-click "Make green now" sweep, a Day/Week/Month/Year savings scorecard with top junk apps, a unified 1-click Restore screen by date/app, batch background-capable Relocate with 1-click WSL2 PowerShell, and internationalized actionable-only Chat & Social app support.

**Architecture:** Compose existing verified Rust engines (`PresetProfiles`, `ArchiverPipeline`, `SafeCleaner`, `InPlaceCompactor`, `TransactionalMover`, `KnownFolderService`, `ZaloRelocator`, `RollbackEngine`) under a new backend `make_green_now` orchestrator and `savings_events` analytics layer (SQLite Schema v6). On the Svelte 5 frontend, move all long-running task tracking into a singleton `tasks.svelte.ts` store with strict `kind` + `taskId` filtering and background minimization, drop the left sidebar for a compact top bar, and present Home, Restore, and Advanced cleanly.

**Tech Stack:** Rust (Tauri v2, Rusqlite, Tokio, Serde), Svelte 5 Runes + TypeScript + Tailwind CSS + Lucide icons, Vite.

**Spec:** `docs/superpowers/specs/2026-11-28-greenc-lowtech-redesign-design.md`

## Global Constraints
- `#![deny(unsafe_code)]` in `crates/greenc-app`.
- Preserve all Microsoft Store (`cfg!(feature = "store")`) MSIX guards: no Known Folder redirect, no AppData NTFS junctions, no `powershell.exe -ExecutionPolicy Bypass` auto-launch, no self-updater.
- Preserve `NativeAppLifecycleApi::graceful_shutdown` zero-force-kill guarantee; confirmed process close is a separate opt-in method called ONLY after explicit user confirmation.
- `test_all_50_locales_100_percent_parity` in `crates/greenc-i18n` must pass 100% (`messages.json` + all 50 locale JSONs in parity).
- Never run `wsl --unregister` if `wsl --export` failed or produced an empty `.tar`.
- Never auto-move `is_advanced` arbitrary AppData folders in `make_green_now`.
- Never show non-actionable chat data (e.g., Zalo already on D:) next to a Clean/Move action.

## Review Focus
1. **Event-stream race on task completion:** `relocate_scan` emitted by `onFilesChanged` right after relocation finishes must never overwrite or reopen the progress modal (pinned by `tasks.svelte.ts` strict `kind`+`taskId` filtering and pre-refresh clearing).
2. **WSL2 command injection & export failure:** Distro names containing PowerShell metacharacters (`;`, `&`, `|`, `$`, quotes) or invalid target drives must be rejected before generating any script; generated script must guard `wsl --unregister` with exit-code and non-zero file-size checks.
3. **Single-drive safety (`backup_drive == None`):** `direct_cache` mode must never delete `Windows.old` or personal folders; `local_vault` mode must keep archives restorable.
4. **Phase 3/4 overlap:** When a backup drive exists, folders queued for Phase 4 relocation must not be WOF-compacted first in Phase 3.
5. **Savings rollback honesty:** Restoring a Vault archive or undoing a relocation must mark matching `savings_events.reverted = 1` so freed totals never over-report.

---

### Task 1: SQLite Schema v6 (`savings_events`), Idempotent Backfill, & Analytics IPC (`get_savings_stats`)

**Files:**
- Modify: `crates/greenc-storage/src/migrations.rs`
- Modify: `crates/greenc-storage/src/db.rs`
- Create: `crates/greenc-app/src/commands/analytics.rs`
- Modify: `crates/greenc-app/src/commands/mod.rs`
- Modify: `crates/greenc-app/src/commands/vault.rs`
- Modify: `crates/greenc-app/src/commands/relocate.rs`
- Modify: `crates/greenc-app/src/lib.rs`

**Interfaces:**
- Produces in `GreenCDb`:
  - `pub struct SavingsEventInput { pub timestamp: String, pub event_date: String, pub source_kind: String, pub source_ref: String, pub app_name: String, pub category: String, pub bytes_freed: u64, pub drive_letter: String }`
  - `pub fn record_savings_event(&self, event: &SavingsEventInput) -> Result<i64>`
  - `pub fn revert_savings_by_ref(&self, source_kind: &str, source_ref: &str) -> Result<usize>`
  - `pub fn query_savings_events(&self) -> Result<Vec<SavingsEventRow>>`
- Produces Tauri command:
  - `get_savings_stats(period: Option<String>) -> Result<SavingsStatsDto, String>` where `period` is `"day" | "week" | "month" | "year"`.

- [ ] **Step 1:** Write failing unit tests in `crates/greenc-storage` for v5→v6 migration + idempotent backfill (including inferring app names from legacy `"QuickCleanup"` `vault_files.original_path`) + `record_savings_event` + `revert_savings_by_ref`.
- [ ] **Step 2:** Run `cargo test -p greenc-storage` to verify new tests fail.
- [ ] **Step 3:** Bump `CURRENT_SCHEMA_VERSION` to `6`, add `savings_events` DDL/indexes + idempotent backfill in `migrations.rs`, and implement DB helpers in `db.rs`.
- [ ] **Step 4:** Write failing unit tests for `aggregate_savings_stats(events, period, today)` in `crates/greenc-app/src/commands/analytics.rs` covering `day`/`week`/`month`/`year` buckets, top junk apps ranking, and reverted-event exclusion.
- [ ] **Step 5:** Implement `analytics.rs`, register `get_savings_stats` in `lib.rs`, and wire `revert_savings_by_ref` into `restore_vault_archive` and `undo_relocation`.
- [ ] **Step 6:** Run `cargo test -p greenc-storage -p greenc-app` and verify green. Commit.

---

### Task 2: Internationalized Chat & Social Profiles + Per-App Cleanup Grouping

**Files:**
- Modify: `crates/greenc-vault/src/profiles.rs`
- Modify: `crates/greenc-relocate/src/catalog.rs`
- Modify: `crates/greenc-app/src/commands/cleanup.rs`
- Modify: `crates/greenc-app/src/commands/zalo.rs`

**Interfaces:**
- Produces in `crates/greenc-vault/src/profiles.rs`:
  - `pub fn infer_friendly_app_name(path: &Path, profile_id: &str) -> String` (maps Chrome, Edge, Brave, Firefox, Discord, Slack, Telegram Desktop, Microsoft Teams, WhatsApp, LINE, Viber, WeChat, DingTalk, Zalo PC, Windows Temp, npm, pip, cargo, gradle, etc.)
  - Expanded Windows `chat_media` safe cache paths + expanded Windows relocation catalog entries (`wechat`, `viber`, `line`, `dingtalk`).
- Produces in `crates/greenc-app/src/commands/cleanup.rs`:
  - Per-app archive creation in `execute_cleanup` (creating one Vault archive per friendly `app_name` group, recording a `savings_events` row per group, returning primary `vault_archive_id` plus `vault_archive_ids: Vec<String>`).

- [ ] **Step 1:** Write failing unit tests for `infer_friendly_app_name` (Chrome, Discord, Telegram, Teams, WhatsApp, Zalo, npm, Windows Temp) and expanded Windows `chat_media` / relocation catalog definitions.
- [ ] **Step 2:** Run `cargo test -p greenc-vault -p greenc-relocate` to verify failure.
- [ ] **Step 3:** Implement `infer_friendly_app_name`, expand `profiles.rs` (`chat_media` label + safe paths) and `catalog.rs` (`wechat`, `viber`, `line`, `dingtalk`), and update `execute_cleanup` + `clean_zalo_now` to archive per app and record `savings_events`.
- [ ] **Step 4:** Run `cargo test -p greenc-vault -p greenc-relocate -p greenc-app` and verify green. Commit.

---

### Task 3: Relocate Engine Upgrades — Filename Progress, Known Folder Progress, Batch Move, & Safe WSL2 1-Click PowerShell

**Files:**
- Modify: `crates/greenc-relocate/src/verified_copy.rs`
- Modify: `crates/greenc-relocate/src/mover.rs`
- Modify: `crates/greenc-relocate/src/known_folders.rs`
- Modify: `crates/greenc-app/src/commands/relocate.rs`
- Modify: `crates/greenc-app/src/lib.rs`

**Interfaces:**
- Produces in `verified_copy.rs`:
  - `pub fn verified_copy_tree_manifest_with_file<F>(..., on_progress: F) -> Result<(CopyStats, CopyManifest)> where F: FnMut(u64, u64, &str)` (existing `verified_copy_tree_manifest` delegates to it for backwards compatibility).
- Produces in `mover.rs` & `known_folders.rs`:
  - `relocate_junction_with_file` and `relocate_known_folder_with_file` streaming compact relative filename.
- Produces Tauri commands in `commands/relocate.rs`:
  - `relocate_batch(candidate_ids: Vec<String>, known_folder_ids: Vec<String>, target_drive: String) -> Result<BatchRelocateReportDto, String>`
  - `list_wsl_distros() -> Result<Vec<WslDistroDto>, String>`
  - `build_wsl_move_command(distro: String, target_drive: String) -> Result<WslMoveScriptDto, String>`
  - `launch_wsl_move_powershell(distro: String, target_drive: String) -> Result<bool, String>` (compile-gated off under `#[cfg(feature = "store")]`).

- [ ] **Step 1:** Write failing unit tests for: (a) `verified_copy_tree_manifest_with_file` emitting relative file paths; (b) `build_wsl_move_command` validating distro & drive (rejecting injection chars like `Ubuntu; calc.exe` or `C`), verifying export exit code & non-zero tar size before `wsl --unregister`, and keeping tar on import failure.
- [ ] **Step 2:** Run `cargo test -p greenc-relocate -p greenc-app` to verify failure.
- [ ] **Step 3:** Implement filename-aware progress copy + mover/known_folder wrappers, throttled filename progress in `relocate_directory` & `relocate_known_folder`, `relocate_batch`, `list_wsl_distros`, `build_wsl_move_command`, `launch_wsl_move_powershell`, and register commands in `lib.rs`. Also record `savings_events` on successful relocations.
- [ ] **Step 4:** Run `cargo test -p greenc-relocate -p greenc-app` and verify green. Commit.

---

### Task 4: Powerful `make_green_now` Backend Orchestrator + Confirmed App Close/Reopen

**Files:**
- Modify: `crates/greenc-platform/src/app_lifecycle.rs`
- Create: `crates/greenc-app/src/commands/autopilot.rs`
- Modify: `crates/greenc-app/src/commands/mod.rs`
- Modify: `crates/greenc-app/src/commands/compact.rs`
- Modify: `crates/greenc-app/src/lib.rs`

**Interfaces:**
- Produces in `crates/greenc-platform/src/app_lifecycle.rs`:
  - `AppLifecycleApi::shutdown_confirmed_apps(&self, process_names: &[&str], timeout: Duration) -> Result<AppShutdownSession>` (tries graceful shutdown first, then terminates matching tray-resident processes ONLY when called via explicit user confirmation; captures exe paths for `restart_app`).
- Produces Tauri command in `commands/autopilot.rs`:
  - `make_green_now(options: Option<MakeGreenOptionsDto>) -> Result<MakeGreenReportDto, String>`

- [ ] **Step 1:** Write failing unit tests in `autopilot.rs` covering:
  - Curated allowlist filtering (`!is_advanced` only, never arbitrary `is_advanced` AppData folders).
  - Phase 3/4 overlap exclusion (folders scheduled for Phase 4 relocation are excluded from Phase 3 compaction).
  - Single-drive `direct_cache` safety filter (never includes `old_versions` / `Windows.old`).
  - Locked app collection into `skipped_locked` when `confirm_close_running_apps` is false.
- [ ] **Step 2:** Run `cargo test -p greenc-app autopilot` to verify failure.
- [ ] **Step 3:** Implement `shutdown_confirmed_apps` in `app_lifecycle.rs` and full `make_green_now` pipeline in `autopilot.rs` (Analyze → Clean per-app → Non-overlapping Compact → Curated Auto-Move → Reopen confirmed closed apps → Record `savings_events` + `daily_snapshots` → Return `MakeGreenReportDto`). Also record `savings_events` in `compact_files`/`compact_vhdx`. Register `make_green_now` in `lib.rs`.
- [ ] **Step 4:** Run `cargo test -p greenc-platform -p greenc-app` and verify green. Commit.

---

### Task 5: Frontend API Bindings, Global Background Task Store (`tasks.svelte.ts`), & Top-Bar Navigation Shell (`App.svelte`)

**Files:**
- Modify: `ui/src/lib/api.ts`
- Create: `ui/src/lib/tasks.svelte.ts`
- Create: `ui/src/lib/components/TaskProgressOverlay.svelte`
- Create: `ui/src/pages/AdvancedPage.svelte`
- Modify: `ui/src/App.svelte`

**Interfaces:**
- Produces in `ui/src/lib/api.ts`:
  - `makeGreenNow`, `getSavingsStats`, `relocateBatch`, `listWslDistros`, `buildWslMoveCommand`, `launchWslMovePowershell` + TypeScript DTOs.
- Produces in `ui/src/lib/tasks.svelte.ts`:
  - `taskStore` singleton with `startTask(...)`, `minimize()`, `restore()`, `cancel()`, `clearResult()`, and global `initTaskListener()` enforcing strict `kind` + `taskId` matching and pre-refresh modal auto-close.

- [ ] **Step 1:** Add new DTO types and Tauri invoke wrappers to `ui/src/lib/api.ts`.
- [ ] **Step 2:** Implement `ui/src/lib/tasks.svelte.ts` and `ui/src/lib/components/TaskProgressOverlay.svelte` (compact 1-line truncated current filename, **"Ẩn chạy ngầm"**, **"Hủy"**, auto-close on finish).
- [ ] **Step 3:** Create `ui/src/pages/AdvancedPage.svelte` with clean sub-tabs (*Di chuyển*, *Dọn dẹp chi tiết*, *Thu gọn*, *Bản đồ ổ đĩa*).
- [ ] **Step 4:** Refactor `ui/src/App.svelte` to remove the `w-64` left sidebar and render the compact icon top bar (`Trang chủ`, `Khôi phục`, `Tùy chỉnh`, live Background Task Pill, `⚙`, `Chạy ngầm`) + mount `<TaskProgressOverlay />`.
- [ ] **Step 5:** Commit after wiring pages in Tasks 6–7 (or keep compiling via stubs).

---

### Task 6: New `HomePage.svelte` ("Make Green Now" Hero, Single-Drive 2-Option Dialog, Confirmed Close Dialog, & `SavingsScorecard.svelte`)

**Files:**
- Create: `ui/src/lib/components/SavingsScorecard.svelte`
- Create: `ui/src/pages/HomePage.svelte`

**Interfaces:**
- Consumes: `getOverviewSummary`, `getBootstrapState`, `makeGreenNow`, `getSavingsStats`, `taskStore`.
- Produces: Full-width low-tech Home view with:
  - Drive `C:` health bar + backup drive indicator + primary **⚡ Làm xanh ổ C: ngay** button.
  - 2-option modal when no secondary drive exists (`Giải phóng tối đa — Xóa thẳng cache tái tạo được` vs `An toàn — Nén giữ trong Kho trên ổ C:`).
  - Inline before→after Result Summary card with 1-click **"Đóng ứng dụng & giải phóng nốt"** button opening an explicit confirmation dialog listing open apps with checkboxes.
  - `SavingsScorecard.svelte` with Day/Week/Month/Year switcher, Today / Period / All-Time KPIs, SVG bar chart, and Top 6 junk-generating apps.

- [ ] **Step 1:** Build `ui/src/lib/components/SavingsScorecard.svelte`.
- [ ] **Step 2:** Build `ui/src/pages/HomePage.svelte` integrating `makeGreenNow`, single-drive 2-option modal, confirmed app-close follow-up modal, and `SavingsScorecard`.

---

### Task 7: Unified Plain-Language `RestorePage.svelte`, Relocate Page Overhaul, WSL2 1-Click Modal, & Actionable-Only Chat/Zalo UI

**Files:**
- Create: `ui/src/pages/RestorePage.svelte`
- Modify: `ui/src/pages/RelocatePage.svelte`
- Modify: `ui/src/lib/components/GuidedModal.svelte`
- Modify: `ui/src/lib/components/ZaloCard.svelte`
- Modify: `ui/src/pages/CleanupPage.svelte`

- [ ] **Step 1:** Create `ui/src/pages/RestorePage.svelte` merging Vault archives + Active Relocations + retained safety copies, with Date filter (All / Today / 7 days / 30 days / exact date), App filter, plain-language badges, 1-click Restore per item & per date group, and simplified file inspector (no SHA-256/HardLink clutter, no browser `confirm()`/`alert()`).
- [ ] **Step 2:** Overhaul `ui/src/pages/RelocatePage.svelte`:
  - Add checkboxes + **"Chọn tất cả"** + **"Chuyển tất cả / Chuyển đã chọn"** calling `relocateBatch`.
  - Remove every native `confirm()` / `alert()` popup (use inline feedback & one clean in-app summary confirm for batch).
  - Route all moves through global `taskStore` (fixing the 100%-stuck / `"Đang tìm dữ liệu có thể chuyển"` hijack bug, enabling **"Ẩn chạy ngầm"** and compact filename display).
- [ ] **Step 3:** Upgrade `ui/src/lib/components/GuidedModal.svelte` for `wsl2`: auto-list distros via `listWslDistros`, show 1-click **"Chạy tự động bằng PowerShell"** (hidden on Store builds) + copyable 1-line PowerShell command via `buildWslMoveCommand`.
- [ ] **Step 4:** Fix `ui/src/lib/components/ZaloCard.svelte` & `ui/src/pages/CleanupPage.svelte`:
  - Remove standalone `<ZaloCard />` from `CleanupPage.svelte` (Zalo cache appears naturally under `Ứng dụng Chat & Mạng xã hội` only when reclaimable > 0).
  - In `ZaloCard.svelte` (on Relocate), hide completely when `already_on_other_drive` and there is no active GreenC relocation and no safety copies; never display non-actionable chat data on another drive next to `Dọn ngay`.
  - Update `CleanupPage.svelte` to use `taskStore` (enabling background mode) and rename profile label `Zalo & DingTalk` → `Chat & Mạng xã hội`.

---

### Task 8: 50-Locale i18n Sync, Full Verification, `graphify update .`, & `ocr review`

**Files:**
- Modify: `crates/greenc-i18n/locales/messages.json`
- Modify: `crates/greenc-i18n/locales/vi.json`
- Modify: `crates/greenc-i18n/locales/en.json`
- Modify: remaining 48 locale JSON files in `crates/greenc-i18n/locales/*.json`

- [ ] **Step 1:** Sync all newly introduced `t('...')` keys across `messages.json`, `vi.json`, `en.json`, and the other 48 locale files (English fallback).
- [ ] **Step 2:** Run full verification:
  - `cargo test --workspace`
  - `cargo check --workspace --features greenc-app/store` (verify Store build compiles cleanly!)
  - `cd ui && npm run check && npm run build`
- [ ] **Step 3:** Run `graphify update .` to update the knowledge graph.
- [ ] **Step 4:** Run `ocr review` (and fix any real findings).
- [ ] **Step 5:** Commit cleanly and report verified results with evidence.
