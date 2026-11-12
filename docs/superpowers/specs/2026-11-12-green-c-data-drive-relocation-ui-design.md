# Green C:\ — Data drive, app data relocation, UI redesign

Date: 2026-11-12 · Status: approved in brainstorming (visual mockups in `.superpowers/brainstorm/`)

## Goals

1. On first launch and in Settings, the user picks a **data drive**. Green C:\ stores its Vault
   there and moves app data there to free the system drive (C:).
2. Detect apps whose data lives on C: and relocate it, **official mechanism first, junction
   second**.
3. Rebrand to **Green C:\** (author **Cùng Chat**, **cungchat.com**) and redesign the UI: clean,
   professional, Windows 11 (Fluent) look, terse Vietnamese copy.

Non-goals: renaming the Python package `greenc`, cloud sync, moving apps installed in
`Program Files`, moving relocated apps again when the data drive changes.

## 1. Branding

- Window title, tray tooltip, sidebar header: `Green C:\` (the `C:\` part in accent green
  `#107C10`).
- Sidebar footer: `Tác giả: Cùng Chat` and a `cungchat.com` link (opens `https://cungchat.com`
  with `QDesktopServices.openUrl`). Same block in Settings → Giới thiệu, with the version.
- Internal package name stays `greenc`.

## 2. Storage layout and settings

| What | Where |
|---|---|
| Config + SQLite DB (small) | `%LOCALAPPDATA%\Green C\` (`greenc.db`) |
| Vault (zip archives + manifests) | `<data drive>:\Green C\Vault\` |
| Relocated app data (junction strategy) | `<data drive>:\Green C\Apps\<app-id>\` |
| Relocated known folders | `<data drive>:\Green C\<Downloads/Documents/…>\` |
| Sandbox probes | `%LOCALAPPDATA%\Green C\sandbox\canary_probes\` |

`greenc/config.py` stops hard-coding `C:\Green C`; it exposes `app_home()` (LOCALAPPDATA
path) and default retention. Paths that depend on the data drive come from the settings store.

**Settings store** — new table `app_settings(key TEXT PRIMARY KEY, value TEXT)` in the DB,
wrapped by `greenc/core/settings.py` (`AppSettings`):

- `data_root` — e.g. `D:\Green C` (empty = not chosen yet → first-run dialog).
- `retention_days_default` — int, default **7**, range 1–90. Used for new rules and for the
  1-click cleanup (replaces the hard-coded `retention_days=7`). Existing rules keep their own
  value.

**Drive eligibility** (`greenc/core/drives.py`): fixed (`GetDriveTypeW == DRIVE_FIXED`),
file system NTFS (`GetVolumeInformationW`), writable. Removable, network, FAT/exFAT excluded
(junctions need NTFS; FAT32 cannot hold >4 GB archives). Recommended = non-system eligible
drive with most free space. If only the system drive is eligible: allowed, relocation features
disabled with an explanation.

**Legacy migration** (first run of the new version): if `C:\Green C\data\greenc.db`
exists and the new DB does not, copy the DB; after the user picks the drive, move
`C:\Green C\vault\*` into the new Vault and rewrite `zip_path`/`manifest_path` in
`vault_archives`. The old folder is removed only after every archive moved and verified (size).

**Changing the data drive later** (Settings): moves the Vault (same procedure as legacy
migration, with progress on a worker thread). Relocated app data stays where it is and is
listed under "Đã chuyển" with its real location.

## 3. First-run dialog

Single dialog (mockup `new-screens-v1.html`, left):

- Title `Chọn nơi lưu dữ liệu`, one-line explanation.
- Radio list of eligible drives: label, usage bar, `Còn trống X / Y`; `Đề xuất` tag on the
  recommended drive; system drive shown with `Ổ hệ thống` tag.
- `Giữ bản dự phòng trong [7] ngày trước khi xóa vĩnh viễn` (spin box 1–90).
- Shows the resulting folder (`D:\Green C\`).
- Buttons: `Để sau` (uses the recommended drive, Settings shows a reminder) / `Tiếp tục`.

Settings → `Vị trí lưu dữ liệu` reuses the same widget.

## 4. App data relocation

`greenc/core/relocation.py`. Each candidate is a `RelocationCandidate(app_id, name,
source_path, size_bytes, strategy, process_names, note)`; each strategy implements
`check() -> Ready | Blocked(reason)`, `apply(target_root, ctrl) -> RelocationRecord`,
`undo(record, ctrl)`. Every applied relocation is stored in a new table `relocations(id, app_id,
app_name, strategy, source_path, target_path, size_bytes, status, created_at, detail)`.

Only candidates whose source is on the system drive and larger than 200 MB are shown.

### 4.1 Strategies (in priority order)

1. **Known folders (Windows-supported)** — Downloads, Documents, Pictures, Videos, Music,
   Desktop. `SHGetKnownFolderPath` to read, move contents with the verified copy (§4.2), then
   `SHSetKnownFolderPath` to the new folder (`<root>\<Name>`). Skipped when the folder is
   redirected to OneDrive (path under the OneDrive root) or already off the system drive. Undo =
   set back to `%USERPROFILE%\<Name>` and move contents back.
2. **Tool configuration** — `npm` (`npm config get/set cache`, user `.npmrc`) and `pip`
   (`pip cache dir`, `pip config --user set global.cache-dir`). Only when the tool is on PATH.
   These are caches: after switching config, the old cache folder is deleted (no copy). Undo =
   `npm config delete cache` / `pip config --user unset global.cache-dir`.
3. **Guided (app's own setting)** — **Zalo**. Source = parent of `db_path.location` from
   `%APPDATA%\ZaloData\config.json` when it is on the system drive. The page shows the steps:
   *Zalo → Cài đặt → Dữ liệu → Quản lý dữ liệu → Thay đổi → chọn `<root>\Zalo` → Di chuyển*,
   a `Mở Zalo` button and `Kiểm tra lại`. Re-check reads `config.json`; when
   `db_path.location` is off the system drive, the record is stored as `done`. Green C:\ never
   writes Zalo's config (Zalo migrates its own DB).
4. **Junction (move + link)** — curated: **Telegram Desktop** (`%APPDATA%\Telegram Desktop`).
   Plus, in an `Nâng cao` section (unchecked by default), large app folders found by the
   analyzer under `%APPDATA%` / `%LOCALAPPDATA%` (never `Program Files`, `Windows`,
   `Packages`, `Microsoft`).

### 4.2 Junction move procedure

Preconditions (`check`): source is a real directory (not a reparse point), app processes not
running (`psutil`, by exe name), target drive eligible with `free > size + 512 MB`, target
folder absent or empty.

1. **Copy** source tree to `<root>\Apps\<app-id>` (skipping nested reparse points), preserving
   timestamps. Per file: SHA-256 computed while reading the source; destination re-read and
   hashed; mismatch → abort. Progress + Stop via `TaskControl`; on stop/error the partial target
   is deleted and the source is untouched.
2. **Rename** source → `<source>.greenc-moving` (same volume, atomic). Failure (file in use)
   → abort and delete the target.
3. **Link**: create junction `<source>` → target (`mklink /J`).
4. **Verify**: `os.path.realpath(source) == target` and the top-level listing matches.
   Failure → remove the junction, rename back, delete target.
5. **Cleanup**: delete `<source>.greenc-moving`; record `status=done`.

**Crash recovery** at startup: for every `*.greenc-moving` next to a recorded source — if the
junction exists and verifies, finish step 5; otherwise remove any junction and rename back.

**Undo** (`Hoàn tác`): app closed; copy target back to `<source>.greenc-restoring` (verified),
remove the junction (`os.rmdir`, link only), rename into place, delete the target, mark
`undone`.

Quota rules keep pointing at the original path; the scanner follows a root that is a junction
(see `compute_physical_usage`), so cleanup keeps working after relocation.

## 5. UI redesign

Direction approved: **A (Fluent) by default, B (dense table) on demand** —
mockup `combined-v2.html`.

- **Shell**: left navigation `Tổng quan · Dọn dẹp · Di chuyển dữ liệu · Kho dự phòng ·
  Cài đặt`, brand on top, author block at the bottom. Content area `#FAFAFA`, white cards with
  `#E5E5E5` border, radius 6, one accent `#0067C0`, green `#107C10` only for brand/success,
  red `#C42B1C` only for critical. Font: Segoe UI Variable / Segoe UI; numbers tabular.
- **Theme** in one place: `greenc/ui/theme.py` (tokens + a single QSS). Inline
  `setStyleSheet` strings in widgets are removed in favour of object names / properties.
- **Icons**: `greenc/ui/icons.py` — small line SVGs (eye, copy, folder-open, check, warning)
  rendered via `QSvgRenderer` to `QIcon`, tinted from theme tokens.
- **Reusable widgets** (`greenc/ui/widgets.py`):
  - `ExpandableTableCard` — title + eye button; collapsed = simple 3-column list (name · action
    · size), expanded = detailed table (Ứng dụng · Vị trí · Dung lượng · Giải phóng · Đề xuất).
    The eye state is remembered per card for the session.
  - `PathCell` — elided monospace path, full path in tooltip, `copy` button (copies to
    clipboard, shows a 1.2 s toast `Đã sao chép đường dẫn`), `folder` button (opens Explorer
    with `explorer /select,` for files or the folder itself).
  - `StatusBar` — compact bottom bar replacing the large progress card: step title, progress
    bar, Pause/Stop; hidden when idle.
- **Pages**:
  - *Tổng quan*: drive card (usage bar, free space, drive selector), `Có thể giải phóng`
    `ExpandableTableCard` (cleanup + relocation suggestions), `Biến động theo ngày` card with
    the existing chart and day details (tables use `PathCell`).
  - *Dọn dẹp*: analysis results in `ExpandableTableCard` with checkboxes; primary button
    `Dọn dẹp`.
  - *Di chuyển dữ liệu*: mockup `new-screens-v1.html` (right): candidates with how-it-works
    line, size, action (`Chuyển` / `Hướng dẫn`), `Nâng cao` section, `Đã chuyển` list with
    status and `Hoàn tác`.
  - *Kho dự phòng*: archives table (app, created, expires, original → compressed, status) with
    `PathCell`, actions `Khôi phục` / `Xóa ngay`.
  - *Cài đặt*: `Vị trí lưu dữ liệu`, `Thời gian giữ bản dự phòng`, `Quy tắc chạy ngầm`
    (existing rules table), `Giới thiệu`.
- **Copy rules**: Vietnamese only (no English in parentheses), sentence case, labels ≤ 4 words,
  no marketing words (“thông minh”, “1-click”, “AI”), numbers with Vietnamese separators
  (`18,4 GB`), status as facts (`Đang chạy`, `Cần đóng Telegram`).

## 6. Error handling

- All long operations (scan, cleanup, relocation, vault move) run on `BackgroundTaskWorker`
  with Stop; the silent cycle yields to them (existing mechanism).
- Relocation failures never leave the source missing: every step either completes or is rolled
  back (§4.2); failures are shown inline on the row with the reason.
- `Blocked` reasons are shown instead of a disabled button with no explanation.

## 7. Testing

- `AppSettings`: defaults, persistence, retention bounds.
- `drives`: eligibility filtering with mocked Win32 results; recommendation choice.
- Legacy migration: tmp legacy dir → DB copied, archives moved, paths rewritten.
- Junction strategy on tmp dirs: success, process-running block, hash mismatch abort, stop
  mid-copy, rename failure, crash recovery from `.greenc-moving`, undo.
- Known-folder and tool-config strategies with the Win32/subprocess calls injected (tests never
  touch the real user profile, registry or npm config).
- Zalo guided: config on C: → candidate; config off C: → `done` on re-check.
- UI headless: first-run dialog shown when `data_root` empty and not after; eye toggles
  simple/detailed; `PathCell` copy puts the path on the clipboard; brand/author/link present.
- Existing 78 tests stay green (UI tests updated for the new page structure).
