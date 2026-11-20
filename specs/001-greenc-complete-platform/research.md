# Phase 0 Research: GreenC v2 (Rust Rewrite Architecture)

**Feature**: `001-greenc-complete-platform`
**Date**: 2026-11-16
**Target Source Root**: `C:\DiskGuard\greenc-v2`

---

## 1. Desktop UI & Application Shell

- **Decision**: **Tauri v2 (`tauri 2.2+`) + Svelte 5 (Runes) + TypeScript + Windows 11 Fluent Design CSS**, paired with **WebView2 Unload-on-Tray-Minimize**.
- **Rationale**:
  1. **50 Languages + 4 RTL Languages (`ar`, `he`, `fa`, `ur`)**: Constitution Principle V requires 100% localization parity and right-to-left layout mirroring with live instant switching. WebView2 (Chromium engine built into Windows 10/11) provides native HarfBuzz text shaping for Khmer, Lao, Burmese, Arabic, Indic scripts, and CSS Logical Properties (`margin-inline-start`, `dir="rtl"`) out of the box.
  2. **`< 25 MB` Idle Tray RAM (Constitution Principle IV)**: When the user closes the window to the System Tray, GreenC destroys the WebView2 window while keeping the pure-Rust background runtime (`tokio` + `tray-icon`) alive. Idle resident working set drops to **~7–12 MB RAM** and `~0.0%` CPU, while clicking the tray icon re-opens the window in `< 120 ms`.
  3. **Tiny Installer (`< 10 MB`)**: Because Windows 10 (1903+) and Windows 11 pre-install Edge WebView2 Evergreen, the Inno Setup installer ships a single stripped Rust `.exe` + embedded assets (~6–9 MB vs ~95 MB for PyInstaller + PySide6).
  4. **Rich Data Visualizations**: Interactive 60 FPS SVG/Canvas Space Treemap ("Space Lens") and 7/14/30-day Daily Delta Bar Charts require zero heavy native charting dependencies.
- **Alternatives Considered**:
  - `iced` / `egui`: Single binary without WebView2, but lack full bidirectional RTL layout mirroring and complex Indic/Southeast Asian script shaping required by GreenC's 50-language catalog.
  - `Slint`: Strong native performance, but GPLv3 / commercial / attribution licensing constraints complicate clean open/proprietary Windows distribution compared to MIT/Apache-2.0.
  - `WinUI 3` (`windows-rs`): Verbose XAML/COM bindings in Rust and Windows App SDK runtime redistribution overhead (~60 MB).

---

## 2. High-Speed Disk Scanning & NTFS HardLink / Reparse Handling

- **Decision**: **Dual-Tier Scanner in `greenc-scanner`** using `windows` crate (`v0.58+`):
  - **Tier 1 (Default, Per-User Unprivileged)**: Parallel work-stealing directory walker (`rayon`) calling `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` and `GetFileInformationByHandleEx(FileIdInfo)` (when `nNumberOfLinks > 1` or within known hardlink roots like Zalo `fileNoise`) to deduplicate physical bytes by 128-bit `(VolumeSerialNumber, FileId)`.
  - **Tier 2 (Optional Elevated Turbo Mode)**: Direct NTFS USN Data Enumeration (`FSCTL_ENUM_USN_DATA`) + incremental USN Change Journal (`FSCTL_READ_USN_JOURNAL`) for `< 600 ms` full-drive Space Lens indexing and exact deletion accounting.
- **Rationale**:
  - Satisfies Constitution Principle V (works 100% without Admin by default) while delivering 10–30x faster scans than Python `os.scandir` + `os.stat`.
  - Reparse-point safety: Any nested entry with `FILE_ATTRIBUTE_REPARSE_POINT` (`0x400`) — including NTFS junctions (`IO_REPARSE_TAG_MOUNT_POINT`), symlinks (`IO_REPARSE_TAG_SYMLINK`), and OneDrive cloud placeholders (`IO_REPARSE_TAG_CLOUD*`) — is skipped without opening or hydrating cloud files.
- **Alternatives Considered**:
  - `walkdir` / `jwalk` standard library `std::fs::metadata` per file: Issues an extra `CreateFileW` open per file on Windows to populate `file_index`, triggering Windows Defender (`WdFilter.sys`) overhead. Querying `FindExInfoBasic` first and only fetching `FileIdInfo` when needed cuts syscalls dramatically.

---

## 3. Win32 Exclusive File-Lock Check & Process/Service Attribution

- **Decision**:
  - **Lock Check**: `CreateFileW(path, DELETE, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None)` wrapped in RAII `OwnedHandle`. Returns locked (`true`) on `ERROR_SHARING_VIOLATION (32)`, `ERROR_LOCK_VIOLATION (33)`, or `ERROR_ACCESS_DENIED (5)`.
  - **Attribution**: Combine Windows **Restart Manager** (`Rstrtmgr.dll`: `RmStartSession` → `RmRegisterResources` → `RmGetList`), `ReadDirectoryChangesW` real-time filesystem watcher (`notify` crate + custom batch coalescer), process I/O counter deltas (`GetProcessIoCounters` / `sysinfo`), and Service Control Manager (`EnumServicesStatusExW`) PID-to-service mapping.
- **Rationale**:
  - Opening with `DELETE` (`0x00010000`) and `dwShareMode = 0` forces `IoCheckShareAccess` in the Windows kernel to reject the call if *any* process holds Read, Write, or Delete access, without triggering Defender's post-write antivirus scan on `CloseHandle`.
  - Real-time `ReadDirectoryChangesW` captures both file creation/growth AND file unlink/shrinkage events so daily snapshots distinguish transient spikes from persistent growth.
- **Alternatives Considered**:
  - Kernel ETW (`Microsoft-Windows-Kernel-File`): Requires Administrator privileges; kept as an optional opt-in telemetry source rather than the mandatory baseline.

---

## 4. Quarantine Vault Format & High-Throughput Parallel Archiver

- **Decision**: **ZIP64 Container (`zip` crate + parallel `rayon` compression pipeline)** supporting **`STORED`**, **`DEFLATE` (`zlib-ng` via `flate2`)**, and **`ZSTD` (`zstd` crate, ZIP method 93)** + sidecar `.manifest.json` + full backward-compatible reader for Python v2.0 `.zip` archives.
- **Rationale**:
  - **Backward Compatibility**: Users upgrading from Python GreenC v2.0 keep full inspect/restore/auto-rollback functionality on existing `vault_*.zip` packages.
  - **Speed (`> 250 MB/s` on NVMe)**:
    1. Bounded prefetch pool (`16` threads) opens, reads, hashes (`SHA-256` with hardware SHA-NI intrinsics via `sha2` crate), and compresses files `<= 8 MiB` in parallel.
    2. Pre-compressed formats (by extension OR magic header bytes + 64 KiB compressibility probe `< 5%` gain) bypass compression via `ZIP_STORED`.
    3. Large files (`> 8 MiB`, including `> 4 GiB` via ZIP64) stream in `4 MiB` chunks with cooperative cancellation checks between chunks.
  - **Granular Single-File Restore**: Because ZIP64 stores a random-access Central Directory at EOF and `vault_files` indexes `rel_arcname` + `hardlink_paths_json`, restoring 1 file out of a 20 GB archive seeks directly to its byte offset in `O(1)` time.
- **Alternatives Considered**:
  - Custom `.tar.zst` stream: Requires sequential scan to extract a single file near the end of a multi-GB archive, making granular single-file restore slow unless an external seek table is maintained. ZIP64 with `ZSTD`/`DEFLATE`/`STORED` gives both random-access O(1) single-file restore AND standard 7-Zip/Explorer compatibility.

---

## 5. Native NTFS Junctions, In-Place Cold Compression & VHDX Compaction

- **Decision**:
  - **NTFS Junction Creation/Inspection**: Direct Win32 `DeviceIoControl` with `FSCTL_SET_REPARSE_POINT` / `FSCTL_GET_REPARSE_POINT` (`IO_REPARSE_TAG_MOUNT_POINT`, `\??\<target>`) in `greenc-win32` instead of spawning `cmd.exe /c mklink /J`.
  - **In-Place Cold Compression ("Freeze in Place")**: Win32 `WofSetFileDataLocation` (`WOF_PROVIDER_FILE` with `FILE_PROVIDER_COMPRESSION_XPRESS8K` / `XPRESS16K`) with fallback to `FSCTL_SET_COMPRESSION` (`COMPRESSION_FORMAT_LZNT1`), skipping locked and pre-compressed files. Query physical compressed disk allocation via `GetCompressedFileSizeW`.
  - **VHDX Compaction**: Detect `.vhdx` files (`ext4.vhdx` for WSL2 / Docker Desktop), verify `wsl --list --running` shows stopped state, and invoke `virtdisk.dll` `CompactVirtualDisk` (or guided elevation helper when Windows requires admin for virtual disk attachment).
- **Rationale**:
  - Direct `FSCTL_SET_REPARSE_POINT` eliminates console-spawn overhead/AV false positives and enables exact target path inspection (`verify_health`) even when the secondary drive is temporarily disconnected.
  - `WOF` (`XPRESS8K`/`XPRESS16K`) is Microsoft's modern Windows 10/11 transparent per-file compression engine (used by `compact.exe /exe:xpress8k`): fast multicore decompression (> 2 GB/s), zero path changes, and 100% unprivileged on user-owned files!
- **Alternatives Considered**:
  - Spawning `compact.exe` per file: Process creation overhead makes compacting 50,000 SDK files 50x slower than direct Win32 `DeviceIoControl` / `WofSetFileDataLocation` calls.

---

## 6. Database Schema Compatibility & State Persistence

- **Decision**: **`rusqlite` (with `bundled` feature compiling SQLite 3.46+ statically into the binary) + `r2d2` / dedicated writer actor**, preserving `%LOCALAPPDATA%\GreenC\greenc.db` (`WAL` mode) and migrating legacy `%LOCALAPPDATA%\Green C\greenc.db` and `C:\DiskGuard\data\diskguard.db`.
- **Rationale**:
  - Zero external DLL dependencies; 100% SQLite file compatibility with Python v2.0 `GreenCDB`.
  - Schema v2 uses idempotent `CREATE TABLE IF NOT EXISTS` + versioned `PRAGMA user_version` migrations to add new v2 columns/tables (`compaction_records`, `deleted_bytes` in `daily_snapshots`, `health_status` in `relocations`) without breaking existing data.
- **Alternatives Considered**:
  - `redb` / `sled`: Would require a one-way format conversion from `greenc.db` and lose standard SQLite inspectability.
