# Implementation Tasks: macOS & Linux Cross-Platform Support

## Phase 1: Rename `greenc-win32` -> `greenc-platform` & Platform Split
- [x] **T001** Rename `crates/greenc-win32` to `crates/greenc-platform`, update workspace `Cargo.toml` and all crate `Cargo.toml` & `use greenc_win32::` imports to `greenc_platform::`.
- [x] **T002** Gate `windows` dependency in `crates/greenc-platform/Cargo.toml` under `[target.'cfg(windows)'.dependencies]` and add `libc = "0.2"` under `[target.'cfg(unix)'.dependencies]`.
- [x] **T003** Move existing Windows implementations into `#[cfg(windows)]` modules and expose shared traits, DTOs, constants, and `Mock*` types unconditionally across all OSes.

## Phase 2: Native Unix Backend (macOS + Linux) in `greenc-platform`
- [x] **T004** Implement `unix/volume.rs`: `statvfs`, macOS `getfsstat` volume enumeration, Linux `/proc/mounts` parser (pure testable helper), `geteuid() == 0`, `get_system_drive_root()` (`/`), `probe_usn_capability()`.
- [x] **T005** Implement `unix/handle.rs`: `ensure_not_protected_system_path` (Unix system paths), non-blocking `flock` `is_file_locked`, `query_file_identity` (`st_dev`, `st_ino`), `NativeFileLockProbe`.
- [x] **T006** Implement `unix/reparse.rs`: `NativeReparsePointApi` using POSIX symlinks (`symlink`, `read_link`, `symlink_metadata`, `remove_file`).
- [x] **T007** Implement `unix/mutex.rs`: `GreenCAppMutex` using `flock(LOCK_EX | LOCK_NB)` on lockfile under `app_home()`.
- [x] **T008** Implement `unix/process.rs` & `unix/restart_mgr.rs`: `NativeProcessServiceProbe` & `NativeRestartManagerApi` (macOS `pgrep`/`launchctl`/`lsof`, Linux `/proc`/`systemctl`/`lsof`).
- [x] **T009** Implement `unix/power_session.rs`: `NativePowerSessionApi` (macOS `pmset` + LaunchAgent plist, Linux `/sys/class/power_supply` + XDG autostart `.desktop`, pure testable parsers).
- [x] **T010** Implement `unix/known_folders.rs` & `unix/wof_compact.rs`: standard `$HOME`/XDG user folder resolution and graceful `PreconditionBlocked` for Windows-only WOF/VHDX compaction.

## Phase 3: Cross-Platform Core, Scanner, Vault, Relocate, App & CLI
- [x] **T011** Update `crates/greenc-core/src/config.rs`: OS-aware `app_home()`, `legacy_green_c_home()`, `default_vault_dir()`, and Unix protected filename patterns (`*.dylib`, `*.so`, `*.plist`, `*.framework`).
- [x] **T012** Fix symlink guards & Unix warnings in `greenc-scanner/src/walker.rs`, `tracker.rs`, `greenc-relocate/src/verified_copy.rs`, `greenc-app/src/guardian.rs`, and `greenc-scanner/tests/space_lens_and_forecast_test.rs`.
- [x] **T013** Update `crates/greenc-vault/src/profiles.rs` with macOS and Linux cleanup target roots (gracefully skipping `PermissionDenied` / macOS TCC & symlinks).
- [x] **T014** Update `crates/greenc-relocate/src/catalog.rs` with macOS & Linux app/tool relocation catalogs (symlink strategy).
- [x] **T015** Audit `crates/greenc-app` (`overview.rs`, `guardian.rs`, `tray.rs`, `lib.rs`) for Windows-only assumptions (`reveal_in_explorer` -> Finder/`xdg-open`, system root lookup).

## Phase 4: Tauri v2 Icons, Packaging & 3-OS GitHub Actions CI/CD
- [x] **T016** Generate `icon.icns`, `icon.png`, `32x32.png`, `128x128.png`, `128x128@2x.png` in `crates/greenc-app/icons/` and update `crates/greenc-app/tauri.conf.json` for Windows, macOS, and Linux bundles.
- [x] **T017** Update `.github/workflows/ci-release.yml` with 3-OS matrix (`windows-latest`, `macos-latest`, `ubuntu-latest`) for quality gate and release packaging (`.exe`, `.dmg`, `.deb`, `.AppImage`).

## Phase 5: Verification & Code Review
- [x] **T018** Add cross-platform unit tests (`unix_parsers_test.rs`) and run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo run -p greenc-i18n -- check`.
- [ ] **T019** Run Open Code Review (`ocr`) on changes or push to trigger 3-OS GitHub Actions verification.
