# Quickstart & End-to-End Validation Guide: GreenC v2 (Rust)

**Feature**: `001-greenc-complete-platform`
**Workspace Root**: `C:\DiskGuard\greenc-v2`

---

## 1. Prerequisites

- **OS**: Windows 10 (1903+) or Windows 11 x64
- **Rust Toolchain**: `rustup` stable (`1.83+`, `x86_64-pc-windows-msvc`) + Visual Studio C++ Build Tools (Windows 10/11 SDK)
- **Frontend Build**: `Node.js 22+` / `pnpm 9+` (or `bun`) for compiling Svelte 5 static assets into `ui/dist`
- **Installer (Optional)**: Inno Setup 6 (`ISCC.exe`) for building `GreenC-Setup-<version>.exe`

---

## 2. Workspace Build & Test Commands

```powershell
Set-Location C:\DiskGuard\greenc-v2

# 1. Verify formatting, static lints, and 50-locale catalog integrity
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p greenc-i18n -- check

# 2. Run all unit, contract, and sandboxed integration tests (never touches real user profile)
cargo test --workspace

# 3. Smoke-test core engine via CLI (read-only status + dry-run analysis)
cargo run -p greenc-cli -- status --json
cargo run -p greenc-cli -- analyze --include-orphans

# 4. Launch GreenC Desktop App in development mode
pnpm --dir ui install
cargo tauri dev

# 5. Build production release installer (dist\GreenC-Setup-<version>.exe)
powershell -ExecutionPolicy Bypass -File scripts\build.ps1
```

---

## 3. End-to-End Validation Scenarios

1. **Legacy Upgrade & Zero-Loss State Migration (`SC-008`)**:
   - Place a copy of an existing Python v2.0 `greenc.db` and `Vault/vault_*.zip` in a temp `LOCALAPPDATA` sandbox.
   - Launch `GreenC.exe` → verify all existing quota rules, relocation records, settings, and `QUARANTINED` `.zip` packages load cleanly and can be inspected/restored.
2. **Active File Lock & HardLink Safety (`SC-002`, `SC-003`)**:
   - Run `cargo test -p greenc-vault --test hardlink_and_lock_safety` → opens an exclusive/shared Win32 file handle alongside multi-link NTFS files, runs `enforce_rule`, verifies open files remain untouched, hardlink siblings are archived once and unlinked together, and `rollback_archive` recreates every hardlink with matching `SHA-256` and `mtime`.
3. **Granular Single-File Restore (`US2`)**:
   - Archive 100 files into a Vault package → restore a single `file_id` via `greenc-cli vault restore <id> --file-id <fid>` → verify only that file (and its hardlink group) returns to disk while the other 99 remain `QUARANTINED`.
4. **Crash-Safe Junction Relocation & Recovery (`US3`)**:
   - Simulate a crash right after `.greenc-moving` rename → restart engine (`recover_interrupted`) → verify original directory on `C:` is atomically restored with zero lost bytes.
5. **50-Locale Live Switch & RTL Mirroring (`SC-007`)**:
   - Switch `vi` → `en` → `ar` → `he` in Settings → verify `< 100 ms` live re-render without app restart and proper RTL layout mirroring while paths/numbers remain LTR-readable.
