# GreenC Low-Tech Redesign & One-Click "Make Green Now" Design

**Date:** 2026-11-28
**Status:** Ready for User Approval (builder-check completed)
**Target Repo:** `/home/duong/projects/greenc`

---

## 1. Intent & Why

GreenC targets **low-tech desktop users** (primarily Windows) whose system drive (`C:`) is running out of space. They do not want to learn what an NTFS junction, Canary Vault, SHA-256 checksum, WOF algorithm, or Space Lens treemap is—they want to open GreenC, see clearly how healthy `C:` is, click one button (or let it run in the background) to actually free space safely, see real proof of how much space GreenC has saved over time, and restore by date or app in one click if anything ever needs rolling back.

Today's UI works against that goal in several concrete ways confirmed in the codebase:
- **"Đưa về mức xanh ngay" (`Make green now`)** in `GoalContextStrip.svelte` only navigates to the Cleanup tab (`activeTab = "cleanup"`), doing no actual work.
- **6 technical tabs in a wide left sidebar** (`Tổng quan`, `Dọn dẹp`, `Di chuyển`, `Thu gọn`, `Kho dự phòng`, `Cài đặt`) waste horizontal space and expose overlapping technical concepts.
- **Relocate (`RelocatePage.svelte`)** forces row-by-row clicks, fires native browser `confirm()` popups every time, has no **Move All**, locks the entire UI with a Cancel-only modal (no background mode), omits the current file being copied, and gets stuck at 100% switching to *"Đang tìm dữ liệu có thể chuyển"* because its event listener accepts `relocate_scan` events triggered by `onFilesChanged` before `isRelocating` resets.
- **WSL2 guide (`GuidedModal.svelte`)** prints 5 raw CLI commands with hardcoded `Ubuntu` instead of a 1-click PowerShell action.
- **Zalo PC (`ZaloCard.svelte`)** is hardcoded prominently for all users (irrelevant to international users) and displays large chat data already on another drive (`Đã ở ổ D:, không cần chuyển`) right next to a `Dọn ngay` button, causing confusion and disappointment.
- **Overview (`OverviewPage.svelte`)** lacks a motivating savings scorecard (GB freed per day / cumulative by week, month, year, and top junk-generating apps).
- **Vault (`VaultPage.svelte`)** uses technical jargon (`Canary Vault`, `QUARANTINED`, `SHA-256`, `HardLink`) and separates cleaned backups from relocated folders, instead of offering one simple Restore screen filtered by **Date** or **App**.

---

## 2. Goals, Non-Goals, Anti-Goals

### Goals
1. **Top-bar icon navigation (no left sidebar):** Compact header with brand logo, 3 primary views (**Trang chủ / Home**, **Khôi phục / Restore**, **Tùy chỉnh / Advanced**), plus compact right-side icon controls (**Cài đặt / Settings**, **Chạy ngầm / Hide to tray**, and a live **Background Task Pill** when any task is minimized).
2. **Real, powerful "Làm xanh ổ C: ngay" (`Make green now`):** One backend-orchestrated pipeline (`make_green_now`) executing:
   `Analyze → Auto-select backup drive → Clean & archive safe junk/caches → In-place Compact eligible cold folders → Safe Auto-Move unlocked heavy data to backup drive → Before/after Report`.
   - **Locked/running app policy (User-selected Policy #1):** Automatically clean, compact, and move all eligible **unlocked** items. Never force-close running apps silently. Record any skipped locked items in the final report with a 1-click **"Đóng ứng dụng & giải phóng nốt"** follow-up button.
3. **Background-capable, transparent progress everywhere:**
   - Progress UI shows overall step, percentage, GB processed, a compact **1-line truncated current filename**, **"Ẩn chạy ngầm" (Run in background)**, and **"Hủy" (Cancel)**.
   - Minimizing collapses the progress modal into a top-bar pill (global reactive store outside page components) so users can browse other screens or hide to tray while the work continues.
   - Auto-closes the progress dialog immediately upon completion and transitions cleanly to the result summary (never hangs at 100%, never overwritten by `relocate_scan`).
4. **Savings & Impact Scorecard on Home:**
   - Displays GB freed **today**, **cumulative** totals with period switcher (**Ngày / Tuần / Tháng / Năm** — Day / Week / Month / Year), trend bar chart, and **Top apps/categories generating the most junk**.
   - Backed by persistent SQLite data (migrated/backfilled from existing history tables so current users immediately see real stats).
5. **Dead-simple Unified Restore ("Khôi phục"):**
   - Combines Vault archives, active Relocations, and retained safety copies into one plain-language screen.
   - Filter by **Date** (All / Today / 7 days / 30 days / specific date) or **App**, with a **1-click Restore** action per item (and 1-click "Restore all from this date"). Technical details (archive ID, file list) stay tucked inside an optional "Xem tệp" drawer without SHA-256/HardLink clutter.
6. **Relocate fixes (inside Tùy chỉnh / Advanced):**
   - Checkboxes + **"Chuyển tất cả" (Move All)** batch execution (single clean inline confirmation, zero native browser `confirm()`/`alert()` popups).
   - Compact current filename streaming + real progress for Known Folders too.
   - WSL2 **1-click PowerShell** execution (auto-detects installed WSL2 distros on `C:`, verifies export succeeded with non-empty archive before unregistering, verifies import before deleting temp `.tar`), plus a copyable 1-line PowerShell fallback.
7. **Internationalized Chat & Social Apps:**
   - Generalize detection & safe cache/data handling to global chat/social apps: **Telegram, Discord, Slack, WhatsApp, Microsoft Teams, WeChat, LINE, Viber, DingTalk, Zalo PC**.
   - **Strict Actionability Rule:** Only display a chat/social app if it is installed AND currently has actionable bytes on `C:` (cleanable cache/old versions > 0, OR movable data on `C:`, OR retained safety copies). Never display non-actionable chat data that is already on another drive.

### Non-Goals
- Rewriting core safety engines (`TransactionalMover`, `ZaloRelocator`, `ArchiverPipeline`, `RollbackEngine`, `InPlaceCompactor`, `BackgroundGuardian`). Compose and extend them.
- Removing power-user tools (Space Lens, Compact, manual Relocate, Quota Rules). Keep them accessible under **Tùy chỉnh (Advanced)** and **Cài đặt (Settings)**.
- Translating all 50 locale files manually in this pass. Update `vi.json`, `en.json`, and `messages.json` with full translations, and sync the new keys into the remaining 48 locale JSON files with English fallback strings so `test_all_50_locales_100_percent_parity` in `crates/greenc-i18n/tests/all_locales_test.rs` continues to pass 100% *(settled call)*.

### Anti-Goals
- Silently force-killing open user apps (browsers, chat apps) during the initial 1-click `Make green now` run.
- Displaying misleading/non-actionable sizes (e.g., showing 20 GB of Zalo chat data already on `D:` next to a `Clean now` button).
- Running `wsl --unregister` if `wsl --export` failed or produced a 0-byte archive.
- Breaking Microsoft Store (`cfg!(feature = "store")`) MSIX guards (Known Folder move, AppData junctions, self-updater).

---

## 3. Approaches Considered

| Approach | Summary | Tradeoffs | Decision |
|---|---|---|---|
| **A+ (Low-Tech Home-First + Top-Bar Icons)** | Drop left sidebar; compact top bar (`Trang chủ`, `Khôi phục`, `Tùy chỉnh`, `⚙`, `Chạy ngầm`). Home leads with 1-click `Make green now` + savings scorecard. Unified Restore by date/app. Internationalized Chat/Social apps. Fixed Relocate in Advanced. | Bigger frontend IA restructure and E2E selector updates, but eliminates clutter, maximizes content space, and aligns 100% with low-tech users. | **Selected** |
| **B (Keep 6-Tab Sidebar, Patch In-Place)** | Keep existing 6 tabs (`Tổng quan`, `Dọn dẹp`, `Di chuyển`, `Thu gọn`, `Kho dự phòng`, `Cài đặt`), wire `Make green now`, patch Relocate & Zalo, add stats card. | Smaller diff, fewer screenshot/nav changes, but preserves confusing multi-tab technical surface and sidebar space waste. | Rejected |

---

## 4. Detailed Architecture & Design

### 4.1 Top-Bar Navigation & Layout (`ui/src/App.svelte`)
- Remove the `w-64` `<aside>` sidebar completely.
- Single compact top bar (`h-14`, `border-b border-border bg-card`):
  - **Left:** `<BrandLogo size={24} />` + `GreenC`.
  - **Center:** 3 clean icon+label tabs:
    - `home` — **Trang chủ** (Home icon)
    - `restore` — **Khôi phục** (RotateCcw icon)
    - `advanced` — **Tùy chỉnh** (Sliders/Wrench icon — hosts sub-pills: *Di chuyển*, *Dọn dẹp chi tiết*, *Thu gọn*, *Bản đồ ổ đĩa*)
  - **Right:**
    - Active **Background Task Pill** (visible only while a task is running in background: spinner + short label + `%` — clicking re-opens the live progress modal).
    - **Settings** icon button (`⚙`, toggles `settings` view).
    - **Chạy ngầm** (Hide to tray) compact icon button.
- Autostart toggle moves cleanly into `SettingsPage.svelte` (already present there) so the top bar stays uncluttered.
- Update banner and Watchdog alert banner remain compact full-width strips below the top bar.
- Main container uses full width (`max-w-6xl mx-auto px-6 py-5`).

### 4.2 Global Background Task Manager (`ui/src/lib/tasks.svelte.ts`)
Replaces page-local `isRelocating` / `isExecuting` / `progressState` modal state that broke when navigating or when background scans fired:
- Singleton reactive store holding:
  - `activeTask`: `{ taskId: string | null; kind: 'make_green' | 'relocate' | 'relocate_batch' | 'cleanup' | 'compact' | 'zalo_move'; title: string; phase: string; percent: number; doneBytes: number; totalBytes: number; doneFiles: number; totalFiles: number; currentFile: string; minimized: boolean; stepIndex?: number; steps?: string[] } | null`
  - `lastResult`: structured completion summary for modal/banner display (auto-shown if not minimized, or shown as a completion toast/banner with "Xem báo cáo" if minimized).
- **Strict event filtering:** `listenTaskProgress` ONLY updates `activeTask` when `activeTask` is non-null AND `event.kind === activeTask.kind` AND (`activeTask.taskId === null || activeTask.taskId === '' || event.task_id === activeTask.taskId`). Events with `kind === 'relocate_scan'`, `'space_lens'`, `'compact_scan'` are **never** routed into the execution progress modal.
- **Auto-close & Completion:** The moment the awaited Tauri command settles (or emits `state === 'completed' | 'cancelled' | 'failed'`), `activeTask` is cleared immediately *before* triggering background refreshes (`onFilesChanged`), preventing the 100%-stuck or scan-hijack bug.
- **Compact file display:** Renders at most **1 single truncated line** (`text-xs font-mono truncate`) showing the current filename/subpath (e.g. `Telegram Desktop/tdata/user_data/cache/1a2b.bin`) purely for transparency, with no interactive clutter.
- **Controls on progress modal:**
  - **"Ẩn chạy ngầm"** (sets `activeTask.minimized = true` → modal closes, top-bar pill appears, work continues uninterrupted).
  - **"Hủy"** (calls `controlTask(taskId, 'cancel')`).

### 4.3 Powerful "Make Green Now" Pipeline

#### Backend: `crates/greenc-app/src/commands/autopilot.rs`
New Tauri command: `make_green_now(options: Option<MakeGreenOptionsDto>) -> Result<MakeGreenReportDto, String>`

```rust
pub struct MakeGreenOptionsDto {
    /// Preferred backup drive letter (e.g., "D"). If omitted, auto-picks configured data_root drive
    /// or the best eligible fixed NTFS drive with the most free space.
    pub target_drive: Option<String>,
    /// Single-drive mode when no secondary backup drive exists:
    /// - `"direct_cache"` — direct-delete strictly regenerable caches (never `Windows.old` / user files) for max immediate space.
    /// - `"local_vault"` — compress & store cleaned files in local Vault on `C:` so everything stays restorable.
    /// Required when `backup_drive` is `None` (prompted via 2-option modal before running).
    pub single_drive_mode: Option<String>,
    /// Optional app IDs (from a prior run's `skipped_locked`) that the user explicitly confirmed closing & finishing.
    pub close_app_ids: Vec<String>,
    /// Whether to allow escalating to forced process termination (after graceful shutdown fails on tray-resident apps)
    /// AND reopening those apps afterward. Only set to `true` after explicit user confirmation on the follow-up modal.
    pub confirm_close_running_apps: Option<bool>,
    /// Whether to include Known Folders (Downloads/Videos/Pictures/Documents/Music) in auto-move. Default: true.
    pub include_known_folders: Option<bool>,
}
```

**Execution sequence (all under 1 `task_id = "make-green-<ts>"`, `kind = "make_green"`):**
1. **Phase `analyze` (0–10%):**
   - Query `C:` (system drive) `used_bytes`, `free_bytes`, `total_bytes` before starting.
   - Resolve backup drive:
     1. Explicit `options.target_drive` if eligible & non-system.
     2. Configured `settings.get_data_root()` drive if non-system & eligible.
     3. `recommend_drive(&drives)` (best eligible fixed NTFS non-system drive).
     4. If none exists (single-drive PC), set `backup_drive = None` and skip Phase 4 cleanly (UI presents a clean 2-option choice dialog before starting: **Giải phóng tối đa — Xóa thẳng cache tái tạo được** vs. **An toàn — Nén giữ trong Kho trên ổ C: để khôi phục được**).
   - If `backup_drive` is newly auto-selected and `data_root` is not yet configured on a non-system drive, initialize `<drive>:\GreenC\{Vault,Apps,Moving}` and persist `data_root` so Vault archives immediately land off `C:`.
   - If `options.close_app_ids` is non-empty AND `options.confirm_close_running_apps == Some(true)` (follow-up confirmed by user), close those specific running apps (try graceful `RmShutdown` first; if tray-resident apps do not exit within timeout, escalate to process close for those confirmed processes only), remembering their launch paths to **reopen** after Phase 4 completes.
2. **Phase `clean` (10–40%):**
   - Scan safe cleanup profiles: `windows_temp`, `browser_cache`, `chat_media`, `crash_dumps`, `package_managers`, `app_logs`, plus `old_versions` (excluding `Windows.old` from direct deletion).
   - **Per-app Vault archiving *(settled call)*:** Instead of bundling everything into one monolithic `"QuickCleanup"` archive, group discovered files by detected app/source (e.g., `Chrome`, `Edge`, `Telegram Desktop`, `Discord`, `Windows Temp`, `npm Cache`) and create **one Vault archive per app/group** (same upgrade applied to `execute_cleanup`). This enables native 1-click per-app restore in `Khôi phục`, accurate per-app Top Junk rankings, and 1:1 `savings_events.source_ref = archive_id` rollback reconciliation.
   - When `backup_drive` exists: archive all groups into Canary Vault on the backup drive.
   - When `backup_drive == None` (single-drive PC):
     - If `single_drive_mode == "direct_cache"`: direct-delete **only** strictly regenerable caches (`browser_cache`, `chat_media`, `windows_temp`, `package_managers` + Zalo regenerable cache/old versions — **never** `Windows.old` or user personal files), and skip non-regenerable items (`old_versions` / `crash_dumps` / `app_logs` remain untouched or go to Recycle Bin only if explicitly chosen).
     - If `single_drive_mode == "local_vault"`: archive into local Vault on `C:` so all cleaned items stay restorable in `Khôi phục`.
   - If Zalo PC is installed and has reclaimable cache/old versions, run `execute_zalo_cleanup` under the same mode.
   - Attribute locked cleanup groups (by matching known browser/chat processes running against profiles with locked bytes) into `skipped_locked`.
   - Emit progress with compact current filename.
3. **Phase `compact` (40–65%):**
   - Discover eligible cold directories on `C:` via `InPlaceCompactor::discover_directories_for_compaction()`.
   - **No Phase 3/4 overlap *(settled call)*:** When `backup_drive` is available, exclude any directory that is already queued for Phase 4 relocation (e.g. `.cargo`, `.gradle`, `pip\cache`, `uv`) so GreenC never wastes time compressing data it is immediately moving off `C:`.
   - Compress remaining eligible unlocked directories with `WofAlgorithm::Xpress8k`, summing `reclaimed_bytes` (`physical_before - physical_after`).
   - Emit progress with current folder/file name.
4. **Phase `relocate` (65–95%):** *(Only if `backup_drive` is available)*
   - **Curated safe allowlist only *(settled call)*:** Auto-relocate **only** curated safe candidates (`!is_advanced`) + non-OneDrive Known Folders—NEVER arbitrary unvetted `is_advanced` AppData folders in the 1-click autopilot flow (those remain manual in `Tùy chỉnh → Di chuyển`).
   - Candidates included:
     - Non-Store builds: non-OneDrive, non-redirected Known Folders (`Downloads`, `Videos`, `Pictures`, `Music`, `Documents`) with `size_bytes > 0`.
     - Eligible, unlocked curated `Junction` / `ToolConfig` candidates on `C:` (`!is_advanced && is_eligible && size_bytes > 0`). On Microsoft Store builds (`cfg!(feature = "store")`), MSIX guards naturally skip Known Folders and AppData junctions while still moving eligible non-AppData tool caches (`%USERPROFILE%\.cargo`, `.gradle`, `Documents\WeChat Files`, etc.) and noting Store-edition limitations cleanly in `warnings` *(settled call)*.
     - If Zalo PC has chat data on `C:` and `is_zalo_auto_move_supported()`: move via `ZaloRelocator` **only if** Zalo is not running OR `"zalo"` is in `options.close_app_ids`; otherwise add Zalo to `skipped_locked`.
   - Before each move, re-check destination drive free space (`required + 512 MiB` reserve); if insufficient space for a large folder, skip it with a clear warning and continue smaller ones.
   - Any locked candidate (`is_locked == true`) is skipped and added to `skipped_locked` with its `app_id`, `app_name`, `size_bytes`, and `process_names`.
   - Emit progress with compact current filename.
5. **Phase `done` (95–100%):**
   - Reopen any apps that were closed via confirmed `close_app_ids` (best-effort, non-blocking).
   - Re-query `C:` `used_bytes`, `free_bytes`.
   - Record `savings_events` rows in SQLite for every freed/relocated/compacted item.
   - Update `daily_snapshots` for `C:`.
   - Return `MakeGreenReportDto`:
     - `before_used_bytes`, `after_used_bytes`, `before_free_bytes`, `after_free_bytes`, `total_bytes`
     - `cleaned_bytes`, `compacted_bytes`, `relocated_bytes`, `total_freed_bytes`
     - `backup_drive: Option<String>`
     - `actions: Vec<MakeGreenActionItemDto>` (plain-language itemized wins: app/folder name, action kind, bytes freed)
     - `skipped_locked: Vec<LockedAppSummaryDto>` (`app_id`, `app_name`, `size_bytes`, `process_names`, `reason`)
     - `reopened_apps: Vec<String>`
     - `warnings: Vec<String>`, `cancelled: bool`

#### Follow-up Action on Report (User-Confirmed Close & Finish)
If `skipped_locked` is non-empty, the final report shows a clean callout:
> *"Còn {size} đang được dùng bởi các ứng dụng đang mở ({appNames})."*
with a secondary button **"Đóng ứng dụng & giải phóng nốt"**.
- Clicking that button opens a clear **in-app confirmation dialog** listing each open app (with checkboxes, all selected by default) asking:
  > *"Bạn có muốn GreenC đóng các ứng dụng đang mở dưới đây để dọn/chuyển nốt {size}, sau đó tự mở lại không?"*
- Only after the user confirms does the frontend call `make_green_now({ close_app_ids: selectedIds, confirm_close_running_apps: true, ... })`. If the user declines, nothing is closed.

### 4.4 Home Screen & Savings Scorecard (`ui/src/pages/HomePage.svelte`)

Replaces `OverviewPage.svelte` as the default landing view (`Trang chủ`):
1. **Hero Card — Drive `C:` Health & Primary CTA:**
   - Clear plain-language status: e.g. *"Ổ C: đang dùng 86% (còn trống 34.2 GB)"* + target indicator (`< 80%`) + backup drive badge (`Ổ dự phòng: D: · còn trống 310 GB`, clickable to change).
   - Clean usage progress bar with threshold marker.
   - **One prominent primary button:** **⚡ Làm xanh ổ C: ngay** (`Make green now`) — always available (not hidden behind "over goal" only, so users can trigger a sweep whenever they want; when already green, label can read *"Làm xanh ổ C: ngay"* with subtext *"Quét & giải phóng thêm dung lượng"*).
   - When `Make green now` finishes, displays an inline dismissible **Result Summary Card** right below the Hero showing exact before → after (`88% → 69%`, `+42.4 GB đã giải phóng`: Dọn rác X GB · Nén Y GB · Di chuyển Z GB) + the optional 1-click follow-up for locked apps.
2. **Impact & Savings Scorecard (Hiệu quả GreenC):**
   - Period switcher pills: **Ngày** (last 14 days) · **Tuần** (last 8 weeks) · **Tháng** (last 12 months) · **Năm** (all years).
   - 3 concise KPI tiles:
     - **Hôm nay đã cứu** (`today_freed_bytes`)
     - **Trong kỳ này** (`period_freed_bytes`, e.g. 7 ngày / tháng này / năm nay)
     - **Tổng luỹ kế từ trước tới nay** (`all_time_freed_bytes`)
   - **2-column visual row:**
     - Left: Clean vertical bar chart of GB freed per day/week/month/year (built in SVG/CSS, no heavy chart dependency needed; hover tooltip shows exact date & GB).
     - Right: **Ứng dụng & mục sinh rác nhiều nhất** (Top 6 ranked horizontal progress bars showing app/category name, total GB freed, and `%` share, e.g. Chrome, Telegram, Windows Temp, Zalo PC, Discord, npm Cache).
   - Empty state (brand-new install with 0 history): encouraging plain-language prompt showing how much is currently reclaimable/movable on `C:` (from background quick scan) and pointing to **Làm xanh ổ C: ngay**.

#### Backend: Savings Analytics (`crates/greenc-app/src/commands/analytics.rs` + DB Schema v6)
- **Migration v5 → v6 (`crates/greenc-storage/src/migrations.rs`):**
  ```sql
  CREATE TABLE IF NOT EXISTS savings_events (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      timestamp TEXT NOT NULL,
      event_date TEXT NOT NULL,        -- YYYY-MM-DD (local or UTC ISO date)
      source_kind TEXT NOT NULL,       -- 'cleanup_vault' | 'cleanup_direct' | 'relocate' | 'compact'
      source_ref TEXT NOT NULL DEFAULT '', -- archive_id or relocation_id for rollback reconciliation
      app_name TEXT NOT NULL,
      category TEXT NOT NULL DEFAULT 'other',
      bytes_freed INTEGER NOT NULL,
      drive_letter TEXT NOT NULL DEFAULT 'C',
      reverted INTEGER NOT NULL DEFAULT 0
  );
  CREATE INDEX IF NOT EXISTS idx_savings_events_date ON savings_events(event_date);
  CREATE INDEX IF NOT EXISTS idx_savings_events_app ON savings_events(app_name);
  ```
- **Idempotent Backfill on v5 → v6 migration:**
  - From `vault_archives` (`status != 'ROLLED_BACK'`): for named per-app archives, insert `original_bytes` with `app_name`; for legacy monolithic `"QuickCleanup"` archives, infer per-app breakdown from `vault_files.original_path` *(settled call)* with `source_kind = 'cleanup_vault'`, `source_ref = archive_id`.
  - From `direct_delete_log`: insert `logical_bytes` with mapped friendly `app_name` from `profile_id`, `timestamp`, `source_kind = 'cleanup_direct'`.
  - From `relocations` (`status = 'done'`): insert `size_bytes` with `app_name`, `created_at`, `source_kind = 'relocate'`, `source_ref = id`.
  - From `compaction_records` (`physical_before_bytes > physical_after_bytes`): insert `physical_before_bytes - physical_after_bytes`, `source_kind = 'compact'`.
- **Reconciliation on Restore/Undo:** When a vault archive is restored or a relocation is undone, mark matching `savings_events.reverted = 1` so statistics always stay honest.
- **IPC Command:** `get_savings_stats(period: String) -> Result<SavingsStatsDto, String>`.

### 4.5 Unified Restore Screen (`ui/src/pages/RestorePage.svelte`)

Replaces `VaultPage.svelte` as the primary **Khôi phục** tab:
- **Plain-language mental model:** Everything GreenC cleaned into backup or moved to another drive appears in one unified list. No "Canary Vault", no "QUARANTINED / ROLLED_BACK / PURGED" enum pills, no SHA-256/HardLink columns.
- **Simple Controls Bar:**
  - **Lọc theo ngày:** Preset pills (`Tất cả`, `Hôm nay`, `7 ngày qua`, `30 ngày qua`) + `<input type="date">` picker for an exact date.
  - **Lọc theo ứng dụng:** Dropdown / selectable chips listing all apps present in restorable items (e.g., `Tất cả ứng dụng`, `Chrome`, `Telegram Desktop`, `Downloads`, `Zalo PC`...).
  - Toggle: *"Chỉ hiện mục có thể khôi phục"* (default ON; hides already-restored / expired entries unless user wants to see history).
- **Grouped by Date (newest first):**
  - Date header (e.g. **Hôm nay · 28/11/2026** — `3 mục · 6.4 GB`) with a 1-click **"Khôi phục tất cả ngày này"** button when multiple restorable items exist on that date.
  - Each item card shows:
    - Plain icon + **App / Folder name** (e.g. `Downloads`, `Chrome`, `Telegram Desktop`)
    - Simple badge: **Đã dọn vào kho** (with *"Tự xóa sau X ngày"*) or **Đã chuyển sang D:** or **Bản gốc giữ tạm**
    - Timestamp + Size (`2.4 GB · 142 tệp`)
    - Primary button: **Khôi phục** (1 click, inline confirmation popover/button—no browser `confirm()`/`alert()`!).
    - Secondary subtle link (only for Vault archives): **Xem tệp** (opens simplified file picker modal with search + checkboxes + "Khôi phục tệp đã chọn", hiding SHA-256 & HardLink columns behind nothing or an optional tooltip).
- **Backend support:** Reuses `get_vault_archives`, `restore_vault_archive`, `restore_vault_files_selective`, `list_relocation_history`, `undo_relocation`, and `restore_zalo_retained_original` (plus a small batch helper or sequential frontend loop with progress for "Khôi phục tất cả ngày này").

### 4.6 Relocate Fixes & WSL2 1-Click PowerShell (inside `Tùy chỉnh → Di chuyển`)

1. **Remove row-by-row `confirm()` / `alert()`:**
   - Add checkboxes to Known Folders and Application Candidates tables (eligible unlocked items selectable; top bar has **"Chọn tất cả"** and primary button **"Chuyển đã chọn ({count} · {size})"** / **"Chuyển tất cả ({size})"**).
   - Clicking **Chuyển tất cả** or a single row's **Chuyển** opens at most ONE clean in-app confirmation bar/popover summarizing destination drive and total GB (or starts immediately for single-row with 1-click Undo always available—decision: single clean summary confirmation for batch, direct start for single-row with Undo safety).
2. **Batch Move Command (`relocate_batch`):**
   - `relocate_batch(candidate_ids: Vec<String>, known_folder_ids: Vec<String>, target_drive: String) -> Result<BatchRelocateReportDto, String>`
   - Runs sequentially under one `task_id` (`kind: "relocate_batch"`), checking `TaskControl` cancellation between items and files, continuing on individual item errors and returning full per-item results.
3. **Compact Current File Streaming & Known Folder Progress:**
   - Update `verified_copy_tree_manifest` (in `crates/greenc-relocate/src/verified_copy.rs`) to pass the current relative path `&Path` (or filename) to the progress callback alongside `(done_bytes, done_files)`.
   - Propagate that filename in `TransactionalMover::relocate_junction`, `ZaloRelocator::move_zalo_data`, and `KnownFolderService::relocate_known_folder` (fixing the no-op `|_, _| {}` in `commands/relocate.rs:449`).
   - Throttle IPC progress emissions to ~100ms in `relocate.rs` so high-file-count moves stay fast while showing a live 1-line truncated file path.
4. **Fix 100%-stuck & `"Đang tìm dữ liệu có thể chuyển"` Hijack Bug:**
   - Route relocation progress through global `tasks.svelte.ts` filtering strictly on `event.kind === 'relocate' || event.kind === 'relocate_batch'` and matching `task_id`.
   - Clear modal immediately on command resolution before `onFilesChanged` fires `scanRelocationCandidates`.
5. **WSL2 1-Click PowerShell (`GuidedModal.svelte` + `commands/relocate.rs`):**
   - New Tauri commands:
     - `list_wsl_distros() -> Result<Vec<WslDistroDto>, String>` (on Windows: runs `wsl.exe -l -q` decoding UTF-16LE cleanly, filters out `docker-desktop*` internal distros or labels them clearly; returns empty vec on non-Windows).
     - `build_wsl_move_command(distro: String, target_drive: String) -> Result<WslMoveScriptDto, String>` returning a safe single-line PowerShell command and human-readable step preview.
     - `launch_wsl_move_powershell(distro: String, target_drive: String) -> Result<bool, String>` writes a temporary `.ps1` script in GreenC's temp dir and opens `powershell.exe -NoExit -ExecutionPolicy Bypass -File <script>` so the user clicks **1 button**, sees the live transfer in PowerShell, and never types a command. **Microsoft Store gate *(settled call)*:** Under `#[cfg(feature = "store")]`, `launch_wsl_move_powershell` is compile-gated to return an unsupported error and `GuidedModal.svelte` hides the auto-launch button, showing only the one-click copyable single-line PowerShell command to maintain Store certification compliance.
   - **Safety invariants inside generated PowerShell script:**
     - `$ErrorActionPreference = 'Stop'`
     - Validates `$distro` name against strict allowlist characters (`^[A-Za-z0-9._-]+$`) and `$targetDrive` (`^[D-Z]$`) to prevent command injection.
     - Checks destination drive free space before exporting.
     - Runs `wsl --shutdown`, then `wsl --export $distro $tarPath`.
     - Verifies `$LASTEXITCODE -eq 0` AND `(Test-Path $tarPath)` AND `(Get-Item $tarPath).Length -gt 0` BEFORE running `wsl --unregister $distro`.
     - Runs `wsl --import $distro $destDir $tarPath --version 2`.
     - Verifies `$LASTEXITCODE -eq 0` AND distro appears in `wsl -l -q` BEFORE deleting `$tarPath` (if import fails, keeps `$tarPath` intact and prints recovery instructions!).
   - UI in `GuidedModal.svelte` for `wsl2`:
     - Auto-lists detected WSL2 distros (dropdown if >1, auto-selected if 1).
     - Primary button: **"Chạy tự động bằng PowerShell"** (1 click!).
     - Secondary expandable/copyable: **"Sao chép 1 dòng lệnh PowerShell"** (for Store builds or users who prefer pasting manually).
     - Plain-language note explaining WSL will pause briefly while moving, and nothing is deleted until backup succeeds.

### 4.7 Internationalized Chat & Social Apps

1. **Cleanup Profile Expansion (`crates/greenc-vault/src/profiles.rs`):**
   - Rename `chat_media` display name from `"Bộ nhớ đệm Chat (Zalo & DingTalk)"` to `"Ứng dụng Chat & Mạng xã hội"` (`Chat & Social Apps`).
   - On Windows, add strict, regenerable-only cache paths (never touching databases, login tokens, or message stores):
     - **Discord:** `%APPDATA%\discord\Cache`, `Code Cache`, `GPUCache`
     - **Slack:** `%APPDATA%\Slack\Cache`, `Code Cache`, `GPUCache`
     - **Telegram Desktop:** `%APPDATA%\Telegram Desktop\tdata\user_data\cache`, `tdata\user_data\media_cache`
     - **Microsoft Teams (New & Classic):** `%LOCALAPPDATA%\Packages\MSTeams_8wekyb3d8bbwe\LocalCache\Microsoft\MSTeams\EBWebView\Default\Cache`, `%APPDATA%\Microsoft\Teams\Cache`, `Code Cache`, `GPUCache`
     - **WhatsApp Desktop:** safe cache subdirectories if present
     - **LINE:** `%LOCALAPPDATA%\LINE\Cache`
     - **Viber:** `%APPDATA%\ViberPC\Thumbnails` (regenerable thumbnails only)
     - **WeChat:** `%USERPROFILE%\Documents\WeChat Files\All Users\config` excluded; safe `Cache` / `Temp` subfolders only
     - **DingTalk:** `%LOCALAPPDATA%\DingTalk\dumps`, `%APPDATA%\DingTalk\Cache`
     - **Zalo PC:** existing strict allowlist via `scan_zalo_cleanup_units`.
2. **Relocation Catalog Expansion (`crates/greenc-relocate/src/catalog.rs`):**
   - Keep existing Telegram Desktop, Discord, Slack, CapCut, JetBrains, Zalo PC, WSL2, Docker.
   - Add safe Windows relocation entries (only surfaced when installed and > threshold on `C:`):
     - **WeChat Files** (`%USERPROFILE%\Documents\WeChat Files`, process `wechat.exe`)
     - **Viber** (`%APPDATA%\ViberPC`, process `viber.exe`)
     - **LINE** (`%LOCALAPPDATA%\LINE\Data`, process `line.exe`)
     - **DingTalk** (`%APPDATA%\DingTalk`, process `dingtalk.exe`)
   - Note: UWP/MSIX store apps (WhatsApp, New Teams) live inside `%LOCALAPPDATA%\Packages` which `TransactionalMover::preflight` rightly protects from junctions—so they are handled via safe cache cleanup only, never broken with junctions.
3. **Eliminating the Confusing Hardcoded `ZaloCard`:**
   - **On Cleanup (Advanced → Dọn dẹp chi tiết):** Remove the standalone `<ZaloCard />` that displayed `Dữ liệu chat (Đã ở ổ D:, không cần chuyển)`. Zalo's cleanable cache & old versions appear naturally as a cleanable row under `Ứng dụng Chat & Mạng xã hội` **only when `total_reclaimable_bytes > 0`** (or when locked bytes can be cleaned after closing Zalo).
   - **On Relocate (Advanced → Di chuyển):**
     - Render `<ZaloCard />` **only when** Zalo is installed AND (`data_on_system_drive` OR `active_relocation_id != null` OR `safety_copies.length > 0`).
     - When `already_on_other_drive` is true and there are no safety copies and no active GreenC relocation: **hide ZaloCard completely on Relocate**, because there is nothing to move!
     - Inside `ZaloCard` when visible: never show a non-actionable `Dữ liệu chat` metric next to `Dọn ngay` if chat data is on another drive; only show buttons that actually do something right now.

---

## 5. What's Left to the Builder

- Exact Tailwind styling, icon choices (`lucide-svelte`), responsive breakpoints, and animation polish for the top bar, Hero card, savings bar chart, and Restore timeline.
- Exact SVG implementation for the Day/Week/Month/Year bar chart on `HomePage.svelte`.
- Mapping technical `profile_id` / `app_name` strings cleanly to human-friendly labels in `analytics.rs` (e.g., `windows_temp` → `Windows Temp`, `browser_cache` → `Chrome / Edge` using actual detected app name where available).
- Updating/adding unit & integration tests (Rust `cargo test` for `autopilot`, `analytics`, DB migration v6 + backfill, `relocate_batch`, `verified_copy` filename callback, WSL2 script safety validation, chat cache profiles) and UI typecheck/build (`npm run check && npm run build`).
- Updating `vi.json`, `en.json`, and `messages.json` with all new UI strings.

---

## 6. Verification Criteria

1. `cargo test --workspace` passes (including new tests for `make_green_now` locked-app skipping & follow-up close, DB v6 migration + backfill + rollback reconciliation, `relocate_batch`, filename progress streaming, WSL2 PowerShell command validation & injection rejection, and international chat profile scanning).
2. `cd ui && npm run check && npm run build` passes with zero errors.
3. Manual/E2E checklist verified:
   - App opens directly to full-width Home with compact top-bar icons (no left sidebar).
   - Clicking **Làm xanh ổ C: ngay** runs Analyze → Clean → Compact → Move in one flow, supports **"Ẩn chạy ngầm"** to top-bar pill, shows 1-line current filename, auto-closes progress modal on completion, and shows real before/after GB freed + locked-app follow-up if any.
   - Home scorecard switches cleanly across Day / Week / Month / Year and ranks top junk-generating apps.
   - **Khôi phục** filters by Date and App and restores in 1 click without technical jargon.
   - **Tùy chỉnh → Di chuyển** supports **Chuyển tất cả**, no per-row browser `confirm()`, background mode, auto-close without switching to `"Đang tìm dữ liệu có thể chuyển"`, and 1-click PowerShell for WSL2.
   - Zalo PC never shows a dead-end "Đã ở ổ khác, không cần chuyển" card when there is nothing actionable on `C:`; international users see only their detected chat/social apps.
