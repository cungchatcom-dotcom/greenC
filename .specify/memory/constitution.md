<!--
Sync Impact Report
- Version change: template (unversioned) → 1.0.0
- Modified principles:
  - [PRINCIPLE_1_NAME] → I. Zero-Data-Loss & Reversible Operations (NON-NEGOTIABLE)
  - [PRINCIPLE_2_NAME] → II. Official-Mechanism-First Relocation & NTFS Reparse Safety
  - [PRINCIPLE_3_NAME] → III. Crate-First Modularity, Trait-Isolated OS Boundaries & Test-First
  - [PRINCIPLE_4_NAME] → IV. Sub-Second Responsiveness, Cooperative Cancellation & Low Footprint
  - [PRINCIPLE_5_NAME] → V. Per-User Least Privilege, Progressive Elevation & Global Accessibility
- Added sections:
  - Security & Data Safety Invariants
  - Engineering Workflow & Quality Gates
- Removed sections: None
- Templates requiring updates:
  - ✅ .specify/templates/spec-template.md (aligned)
  - ✅ .specify/templates/plan-template.md (aligned)
  - ✅ .specify/templates/tasks-template.md (aligned)
- Deferred TODOs: None
-->

# GreenC Constitution

## Core Principles

### I. Zero-Data-Loss & Reversible Operations (NON-NEGOTIABLE)

GreenC MUST NEVER destroy user or application data irreversibly without explicit, informed user
consent. Every automated or one-click cleanup action MUST quarantine candidate files inside a
verified backup package (Vault) for a configurable safety window (`1–90` days, default `7`) with
cryptographic integrity digests before deleting originals, except when switching pure reproducible
tool caches whose contents are regenerable by the tool itself. Immediately prior to archiving or
deleting any file, the system MUST verify exclusive file availability (zero active read/write/delete
handles) and confirm the file was not modified during archiving. Every directory relocation MUST
follow an atomic, crash-recoverable sequence (`Verified Copy → Atomic Source Rename → Link/Config
Switch → Target Verification → Source Cleanup`) and MUST run automatic crash recovery on startup.

### II. Official-Mechanism-First Relocation & NTFS Reparse Safety

Relocating data off the Windows system drive (`C:`) MUST strictly follow safety priority order:
1. Official Windows Shell APIs (Known Folders redirection, skipping cloud-synced OneDrive paths),
2. Native tool configuration or environment settings (`npm`, `pip`, `uv`, `cargo`, `wsl`, etc.),
3. Guided native application data migration when the app manages its own internal database paths,
4. Verified NTFS Junctions (`IO_REPARSE_TAG_MOUNT_POINT`) only when target processes are closed and
   the destination is a fixed, writable NTFS volume.

Directory scanners and archivers MUST NEVER follow nested reparse points (junctions, symlinks,
cloud placeholders) inside scanned roots to prevent accidental traversal into unrelated user
folders, and MUST deduplicate physical disk accounting and archival by NTFS File ID (`HardLink`
groups) so all sibling links are preserved and restored together.

### III. Crate-First Modularity, Trait-Isolated OS Boundaries & Test-First

Every subsystem (drive discovery, disk scanning, write attribution, quarantine vault, safety
watchdog, relocation strategies, i18n) MUST be implemented as a focused, independently testable
module behind explicit OS abstraction traits (filesystem, Win32 handles/volumes, process/service
inspection, shell folders, command execution). Automated tests MUST never mutate the developer's
real user profile, registry, Known Folders, or tool configurations. Every core algorithm (lock
detection, hardlink preservation, rollback conflict resolution, interrupted-move recovery) MUST
have deterministic unit and integration tests written and verified before release.

### IV. Sub-Second Responsiveness, Cooperative Cancellation & Low Footprint

GreenC is a resident system utility and MUST NOT become the performance or storage problem it
solves. Long-running operations (scanning, archiving, relocating, vault migration) MUST execute off
the UI thread, emit throttled live progress (`phase`, item count, bytes, throughput, ETA), and
honor Pause, Resume, and `< 100 ms` cooperative Stop cancellation cleanly without leaving partial
artifacts. Silent background quota and watchdog cycles MUST automatically yield to user-initiated
actions. Resident idle memory in the system tray MUST remain under `25 MB`, and Vault/relocation
operations MUST always preserve a minimum `512 MiB` free-space reserve on the destination volume.

### V. Per-User Least Privilege, Progressive Elevation & Global Accessibility

GreenC MUST install and operate per-user (`PrivilegesRequired=lowest`) with zero mandatory
Administrator UAC prompts for everyday monitoring, cleanup, Vault management, and user-profile
relocations. Advanced capabilities that benefit from Administrator privileges (such as direct NTFS
MFT/USN Journal indexing or kernel ETW file-I/O tracing) MUST remain optional enhancements with
seamless unprivileged fallbacks. User-facing text MUST adhere to concise, factual Windows 11 Fluent
copy rules (no marketing hype, clear actionable reasons when an action is blocked) and MUST maintain
100% parity across all 50 supported locales, including right-to-left (RTL) layout mirroring (`ar`,
`he`, `fa`, `ur`) and locale-specific number formatting.

## Security & Data Safety Invariants

- **Protected Core Files**: Executables, libraries, configurations, active databases, WAL/journal
  files, and lockfiles (`*.exe`, `*.dll`, `*.sys`, `*.ini`, `*.cfg`, `*.config`, `*.json`,
  `*.yaml`, `*.yml`, `*.toml`, `*.db`, `*.db-wal`, `*.db-shm`, `*.sqlite*`, `*.mdf`, `*.ldf`,
  `*.rescache`, `lockfile`, `CURRENT`, `MANIFEST-*`) MUST be excluded from automated cleanup by
  default, and any file that triggers an automatic or manual rollback MUST be permanently added to
  that rule's protected allowlist.
- **System Directory Guardrails**: GreenC MUST NEVER offer junction relocation or automated deletion
  against `Windows`, `Program Files`, `Program Files (x86)`, `ProgramData\Microsoft`, `Packages`,
  `$Recycle.Bin`, `System Volume Information`, or Recovery partitions.
- **Single-Instance & Upgrade Safety**: Application runtime and installer MUST coordinate via the
  shared named mutex (`GreenCAppMutex`) to prevent concurrent database or Vault mutations, and
  uninstallation MUST preserve user Vault archives, settings database, and relocated directories.
- **Backward-Compatible Data Migration**: Upgrades from prior releases (`DiskGuard`, `Green C`,
  `GreenC` Python v2.0) MUST migrate SQLite state (`greenc.db`), rules, relocation records, and
  Vault archives losslessly without breaking existing NTFS junctions.

## Engineering Workflow & Quality Gates

- **Spec-Driven Delivery**: Major features and architectural rewrites MUST progress through
  `/speckit-specify` → `/speckit-plan` → `/speckit-tasks` before implementation begins.
- **Zero Warnings & Static Safety**: Rust workspace builds MUST pass `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. Every
  `unsafe` Win32 FFI call MUST encapsulate handle lifetime via RAII wrappers (`OwnedHandle` or
  dedicated drop guards) and document its safety invariants.
- **Localization Gate**: CI MUST fail if any user-visible string key is missing, orphaned, or has
  mismatched `{placeholder}` tokens in any of the 50 locale catalogs.

## Governance

This Constitution governs all architectural decisions, specifications, implementation plans, and
code reviews for GreenC (`greenc-v2` and maintenance of legacy `greenc`). Any deviation from a
Core Principle requires an explicit entry in the plan's Complexity Tracking table with documented
risk mitigation and human approval. Amendments follow Semantic Versioning:
- **MAJOR**: Removal or relaxation of any data-safety or least-privilege invariant.
- **MINOR**: Addition of a new principle, security invariant, or mandatory quality gate.
- **PATCH**: Editorial clarifications that do not alter normative obligations.

**Version**: 1.0.0 | **Ratified**: 2026-11-16 | **Last Amended**: 2026-11-16
