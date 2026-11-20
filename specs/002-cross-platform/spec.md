# Feature Specification: macOS & Linux Cross-Platform Desktop Support

**Feature Branch**: `main` (Spec `002-cross-platform`)
**Status**: Approved for Implementation
**Platforms**: Windows (existing, zero regression), macOS (Apple Silicon & Intel, >= 11.0), Linux (x86_64/aarch64)

## 1. Summary & Goals

Extend GreenC v2 (Rust + Tauri v2 + Svelte) from Windows-only to native **macOS** and **Linux** desktop support by:
1. Refactoring `crates/greenc-win32` into `crates/greenc-platform` with `#[cfg(windows)]` (100% preserved Win32 backend) and `#[cfg(unix)]` (shared macOS + Linux native POSIX backend).
2. Replacing hardcoded Windows paths (`C:\`, `%LOCALAPPDATA%`) with OS-aware path resolution (`~/Library/Application Support/GreenC` on macOS, `$XDG_DATA_HOME/greenc` on Linux, `%LOCALAPPDATA%\GreenC` on Windows).
3. Providing macOS and Linux cleanup profiles (preserving all 8 profile IDs for UI/DB/i18n compatibility) and symlink-based relocation catalogs.
4. Adding multi-OS Tauri v2 icons/bundle configs and a 3-OS GitHub Actions CI & Release pipeline (`windows-latest`, `macos-latest` producing ad-hoc `.dmg`, `ubuntu-latest` producing `.deb` & `.AppImage`).

## 2. Functional Requirements

- **FR-001 (Platform Abstraction)**: Rename `greenc-win32` to `greenc-platform`. Gate the `windows` crate behind `[target.'cfg(windows)'.dependencies]` and add `libc = "0.2"` under `[target.'cfg(unix)'.dependencies]`. Preserve all public trait contracts (`Win32VolumeApi`, `FileLockProbe`, `ReparsePointApi`, `ShellFolderApi`, `ProcessServiceProbe`, `CommandRunner`, `CompactionApi`, `PowerSessionApi`, `RestartManagerApi`) and all `Mock*` implementations.
- **FR-002 (Unix Volume Enumeration & Free Space)**:
  - Use `libc::statvfs` for total/available disk space.
  - macOS: enumerate mounted volumes via `libc::getfsstat`, filtering to real local/external volumes (`/` and `/Volumes/*`), excluding virtual filesystems (`devfs`, `autofs`, `vm`, `fdesc`, Time Machine snapshots).
  - Linux: parse `/proc/mounts`, filtering to real block filesystems (`ext4`, `btrfs`, `xfs`, `zfs`, `apfs`, `ntfs`, `ntfs3`, `exfat`, `vfat`, `f2fs`) and excluding pseudo/loop/container mounts (`/proc`, `/sys`, `/dev`, `/run`, `tmpfs`, `squashfs`, `overlay`, `/snap/*`).
  - `is_process_elevated()` returns `libc::geteuid() == 0`.
  - `probe_usn_capability()` returns `UsnCapabilityStatus::NotNtfs` / unsupported so scanner falls back cleanly to parallel directory walker.
- **FR-003 (Unix Safety & System Protection)**:
  - `ensure_not_protected_system_path` blocks mutations on critical Unix OS trees (`/System`, `/Library/System`, `/usr`, `/bin`, `/sbin`, `/etc`, `/var/db`, `/proc`, `/sys`, `/dev`, `/boot`, `/lib`, `/lib64`, `/Applications`).
  - `is_file_locked` performs non-blocking advisory lock probe (`flock(LOCK_EX | LOCK_NB)`) without blocking or mutating file contents.
  - `query_file_identity` returns `(st_dev, st_ino)` for hardlink deduplication.
  - Symlink loop guards (`meta.file_type().is_symlink()`) active on Unix in scanner, tracker, guardian, and verified copy.
- **FR-004 (Unix Relocation via Symlinks)**:
  - `ReparsePointApi` on Unix uses `std::os::unix::fs::symlink`, `fs::read_link`, `symlink_metadata().file_type().is_symlink()`, and `fs::remove_file` (never `remove_dir_all` on symlinks).
- **FR-005 (Unix Single-Instance Lock)**:
  - `GreenCAppMutex` on Unix acquires an exclusive non-blocking `flock(LOCK_EX | LOCK_NB)` on a lockfile under `app_home()`, releasing deterministically on drop or process exit.
- **FR-006 (Unix Process, Service, Power & Autostart)**:
  - Process probe: `/proc/<pid>/comm` on Linux; `pgrep -ix` on macOS.
  - Service probe: `systemctl is-active` on Linux; `launchctl list` on macOS.
  - Power state: `/sys/class/power_supply/*/online` on Linux; `pmset -g batt` on macOS.
  - Autostart: macOS LaunchAgent plist (`~/Library/LaunchAgents/com.greenc.app.plist`); Linux XDG autostart desktop entry (`~/.config/autostart/greenc.desktop`).
- **FR-007 (Unix Compaction Graceful Unsupported)**:
  - `CompactionApi` on Unix returns a clear `GreenCError::PreconditionBlocked` explaining that WOF/VHDX compaction is Windows-only, without panicking.
- **FR-008 (OS-Specific Paths, Cleanup Profiles & Relocation Catalogs)**:
  - `greenc-core::config`: OS-aware `app_home()`, `default_db_path()`, `default_vault_dir()`, `sandbox_demo_dir()`, plus Unix protected filename patterns (`*.dylib`, `*.so`, `*.plist`, `*.framework`).
  - `greenc-vault::profiles`: OS-specific target roots for all 8 preset profile IDs on macOS and Linux. `PermissionDenied` (e.g., macOS TCC / Full Disk Access) is skipped gracefully with zero crash.
  - `greenc-relocate::catalog`: macOS and Linux app & dev-tool catalogs using symlink relocation.
- **FR-009 (Tauri v2 Icons, Packaging & 3-OS CI/CD)**:
  - Generate `icon.icns`, `icon.png`, `32x32.png`, `128x128.png`, `128x128@2x.png` in `crates/greenc-app/icons/`.
  - Configure `tauri.conf.json` for Windows, macOS (`minimumSystemVersion: "11.0"`, ad-hoc `.dmg`), and Linux (`.deb`, `.AppImage`).
  - Update `.github/workflows/ci-release.yml` to run quality gate on `windows-latest`, `macos-latest`, and `ubuntu-latest`, and build release bundles on all 3 OSes for `v*` tags.

## 3. Non-Functional Requirements & Invariants

- **Zero Windows Regression**: All existing Windows unit tests, integration tests, clippy lints (`-D warnings`), and i18n checks MUST pass unchanged.
- **Unsafe Discipline**: `unsafe_code = "deny"` remains enforced workspace-wide; only `greenc-platform` uses `#![allow(unsafe_code)]` with documented `// SAFETY:` comments on every FFI call.
- **Deterministic Tests**: Pure helpers (Linux `/proc/mounts` parser, macOS volume filter, Unix protected path guard, `pmset` output parser) must have unit tests runnable on ALL platforms (including Windows) so Unix parsing logic is verified even on Windows hosts.
