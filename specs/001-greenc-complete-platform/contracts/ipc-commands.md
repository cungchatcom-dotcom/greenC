# Contract: Tauri IPC Commands & Event Streams (`greenc-app` ↔ `greenc-ui`)

**Feature**: `001-greenc-complete-platform`
**Transport**: Tauri v2 Typed IPC (`#[tauri::command]` + `tauri::ipc::Channel<TaskProgressEvent>`)

---

## 1. Real-Time Event Channels (`Backend -> Frontend`)

### `greenc://task-progress`
Emitted at most every `100 ms` (plus immediate emit on phase transition) during any long-running task.
```json
{
  "task_id": "cleanup-20261116-01",
  "kind": "scan_drive | analyze_apps | cleanup | relocate | undo_relocate | move_vault | compact_inplace",
  "state": "running | paused | cancelling | completed | failed",
  "percent": 64,
  "phase": "compress",
  "title": "Đang dọn npm cache · 1/2",
  "detail": "7.420/11.900 file · 910,4 MB / 1,4 GB · 340,2 MB/s · còn khoảng 0:02",
  "done_files": 7420,
  "total_files": 11900,
  "done_bytes": 954626867,
  "total_bytes": 1503238553,
  "throughput_bps": 356725555,
  "eta_seconds": 2
}
```

### `greenc://watchdog-alert`
Emitted whenever the silent background guardian auto-rolls back a Vault archive, purges expired packages, or detects a low-disk emergency / broken junction.
```json
{
  "kind": "auto_rollback | auto_purge | low_disk_warning | junction_offline",
  "severity": "info | warning | critical",
  "title": "Đã tự khôi phục 1 gói dữ liệu cho ứng dụng.",
  "detail": "Ứng dụng báo lỗi thiếu file 'session.dat' trong 'app.log' — kích hoạt Tự động Rollback",
  "archive_id": 12
}
```

---

## 2. IPC Commands (`Frontend -> Backend`)

### 2.1 Bootstrap, Settings & Localization
- `get_bootstrap_state() -> BootstrapState`
  - Returns `app_name ("GreenC")`, `app_version`, `language`, `is_rtl`, `decimal_sep`, `theme_mode`, `settings`, `needs_first_run`, `drives: Vec<DriveInfo>`, `recommended_drive: Option<String>`.
- `complete_first_run(data_root: Option<String>, retention_days: u32) -> AppSettingsDto`
- `update_settings(patch: SettingsPatch) -> AppSettingsDto`
  - Live-updates `language` (returns new catalog dictionary immediately for zero-restart switching), `theme_mode`, `retention_days_default`, `autostart_enabled`, `low_disk_shield_gb`, `pause_on_battery_fullscreen`.
- `change_data_root(new_root: String) -> TaskHandle`
  - Launches verified `move_vault` on background worker and updates `data_root`.

### 2.2 Overview, Space Lens & Daily History
- `get_overview_summary(drive: String) -> OverviewSummary`
  - Returns drive usage, `burn_rate_forecast` (`daily_net_bps`, `days_until_90_pct`, `days_until_full`), cached reclaimable proposals + relocation candidates summary.
- `scan_space_lens(drive: String, max_depth: u8) -> TaskHandle`
  - Streams hierarchical treemap nodes (`SpaceNode { name, path, physical_bytes, file_count, reason_code, children }`).
- `scan_drive_history(drive: String, days_back: u16) -> TaskHandle`
- `get_daily_drilldown(drive: String, date: String) -> DailyDrilldown`
  - Returns `folders: Vec<FolderGrowthRow>` and `actors: Vec<ActorWriteRow>` for the clicked bar.

### 2.3 Smart Analysis, Cleanup & Quarantine Vault
- `analyze_cleanup_candidates(include_orphans: bool) -> TaskHandle`
  - Runs app profiles + orphan leftover radar + 1-cycle Sandbox Canary Probe, returning `Vec<AppAnalysisProposal>`.
- `execute_cleanup(selected_app_ids: Vec<String>) -> TaskHandle`
  - Enforces cleanup into Vault and registers/updates background `QuotaRule` entries.
- `list_vault_archives(status_filter: Option<String>) -> Vec<VaultArchiveDto>`
- `inspect_vault_archive(archive_id: i64, search_query: Option<String>, limit: usize, offset: usize) -> VaultArchiveDetail`
- `restore_vault_archive(archive_id: i64) -> RollbackSummary`
- `restore_vault_files_selective(archive_id: i64, file_ids: Vec<i64>) -> RollbackSummary`
- `purge_vault_archive(archive_id: i64) -> bool`
- `run_sandbox_demo(simulate_crash: bool) -> SandboxDemoResult`

### 2.4 Multi-Strategy App Relocation
- `discover_relocation_candidates() -> TaskHandle`
- `list_relocation_records() -> Vec<RelocationRecordDto>` (includes live `health_status`)
- `apply_relocation(candidate_id: String) -> TaskHandle`
- `verify_guided_relocation(candidate_id: String) -> Option<RelocationRecordDto>`
- `open_guided_app(candidate_id: String) -> bool`
- `undo_relocation(record_id: i64) -> TaskHandle`
- `repair_junction_target(record_id: i64, new_target_path: String) -> RelocationRecordDto`

### 2.5 In-Place Cold Compression & VHDX Trim
- `discover_compaction_candidates() -> TaskHandle`
- `apply_inplace_compaction(target_path: String, algorithm: Option<String>) -> TaskHandle`
- `undo_inplace_compaction(record_id: i64) -> TaskHandle`

### 2.6 Background Quota Rules & Task Control
- `list_quota_rules() -> Vec<QuotaRuleStatusDto>`
- `save_quota_rule(rule: QuotaRuleInput) -> QuotaRuleStatusDto`
- `delete_quota_rule(rule_id: i64) -> bool`
- `control_task(task_id: String, action: "pause" | "resume" | "cancel") -> bool`
- `reveal_in_explorer(path: String) -> bool`
