# Contract: Headless CLI (`greenc-cli` / `GreenC.exe --cli`)

**Feature**: `001-greenc-complete-platform`
**Purpose**: Satisfies Constitution Principle III (every core subsystem testable via CLI with human + `--json` output) and User Story 7 (power-user diagnostics & policy export/import).

---

## Subcommands

```text
greenc status [--drive C:\] [--json]
  Prints drive usage, burn-rate forecast, Vault summary, active quota rules, and relocation health.

greenc scan [--drive C:\] [--days 14] [--space-lens] [--json]
  Scans drive daily deltas, attribution ledger, and top physical space consumers.

greenc analyze [--include-orphans] [--json]
  Runs Smart App Profiles + Orphan Leftover Radar + Canary Probes without modifying user files.

greenc clean [--apps <id,id...>] [--all] [--dry-run] [--json]
  Executes Canary-verified cleanup into the Safe Vault (or previews candidate files with --dry-run).

greenc vault list [--status QUARANTINED] [--json]
greenc vault files <archive_id> [--filter <glob>] [--json]
greenc vault restore <archive_id> [--file-id <id>...] [--json]
greenc vault purge [--expired | <archive_id>] [--json]

greenc relocate list [--json]
greenc relocate apply <app_id> [--dry-run] [--json]
greenc relocate undo <record_id> [--json]
greenc relocate check-health [--json]

greenc compact list [--json]
greenc compact apply <path> [--algo xpress8k|xpress16k] [--dry-run] [--json]
greenc compact undo <path> [--json]

greenc policy export <output.json>
greenc policy import <input.json> [--merge]
```

## Exit Codes
- `0`: Success
- `1`: Operational error (e.g., InsufficientVaultSpace, SHA-256 mismatch)
- `2`: Precondition blocked (e.g., Target process running, OneDrive path, non-NTFS drive)
- `130`: Cancelled cooperatively (`Ctrl+C`) — source data guaranteed intact
