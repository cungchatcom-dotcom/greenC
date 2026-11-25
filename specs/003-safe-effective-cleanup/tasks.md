---
description: "Dependency-ordered TDD implementation tasks for GC-GUARD"
---

# Tasks: Safe & Effective Cleanup (`GC-GUARD`)

**Input**: [spec.md](./spec.md), [plan.md](./plan.md)

## Phase 1: Spec & Platform Tool Detection
- [x] T001 Write `specs/003-safe-effective-cleanup/{spec.md,plan.md,tasks.md}`
- [x] T002 [US1] TDD: Pure `detect_busy_dev_tools`, `DevTool`, `BusyToolsStatus`, `DevToolProbe` + Linux `/proc` (same UID) + Windows/macOS/fallback in `crates/greenc-platform/src/dev_tools.rs`
- [x] T003 [US1] Enhance & document `crates/greenc-platform/src/unix/handle.rs::is_file_locked` (`fcntl F_GETLK` check alongside `flock`) + tests

## Phase 2: Core Models & Storage Schema v4
- [x] T004 [US2] Add `RegenerableCacheMode`, `GuardCooldownState`, `DirectDeleteRecord` in `crates/greenc-core/src/models.rs` and update `DriveQuota`
- [x] T005 [US2] Add SQLite migration v3 → v4 (`drive_quotas.regenerable_cache_mode`, `guard_cooldowns`, `direct_delete_log`) and CRUD tests in `crates/greenc-storage`

## Phase 3: Atomic Cache Units & Safe Profiles
- [x] T006 [US1] TDD: Implement `crates/greenc-vault/src/cache_units.rs` (Gradle transforms + old Gradle versions, Cargo crates, npm index-v5 + refcounted content-v2, pip/uv; reliable `max(mtime, atime)` ≥ 7 days; forbidden files/control dirs)
- [x] T007 [US1] Update `crates/greenc-vault/src/profiles.rs`: `package_managers` uses atomic units & 7-day default; Linux `app_logs` restricted to old `*.log`/`*.log.*`

## Phase 4: Shared Quota Guard Core & Build-Output Suggester
- [x] T008 [US2] TDD: Implement `crates/greenc-vault/src/guard.rs` (`GuardEngine`, drive isolation, priority ordering, same-drive Vault net estimation, TOCTOU per-unit check + mid-run tool abort, safe no-symlink direct delete + DB audit log, before/after `net_freed_bytes`, `target_reached`, DB cooldown escalation 30m→2h→12h, Vietnamese reasons)
- [x] T009 [US3] TDD: Implement `crates/greenc-vault/src/suggest.rs` (read-only git-ignored `build`/`target`/`.gradle`/`node_modules` ≥ 3 days, idle tool check, depth/time/same-drive/no-symlink bounds, Vault size report)

## Phase 5: CLI, App Integration & Linux Script
- [x] T010 [US2][US3] Add `guard run`, `guard suggest`, `guard set-mode` and `quota set --regenerable-cache-mode` in `crates/greenc-cli/src/lib.rs` + contract & incident regression tests
- [x] T011 [US2] Wire `crates/greenc-app/src/guardian.rs` and `commands/drive_quota.rs` to shared `greenc-vault::guard` core and audit `greenc-app` call sites/tests
- [x] T012 [US3] Add `packaging/linux/greenc-disk-guard.sh` + `packaging/linux/README.md` and run shell syntax + stubbed end-to-end verification

## Phase 6: Verification, Review & Final Report
- [x] T013 Run `gc-cargo.sh fmt --check`, `clippy --all-targets -- -D warnings`, `test` across all changed crates; run `ocr doctor` (manual review fallback); audit Windows/macOS `cfg`
- [x] T014 Write `docs/reports/gc-guard.md` and ensure every change is cleanly committed
