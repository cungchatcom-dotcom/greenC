// Typed Tauri v2 IPC Command and Event Bindings (Task T024)

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface DriveInfo {
  root: string;
  drive_letter: string;
  label: string;
  filesystem: string;
  total_bytes: number;
  free_bytes: number;
  is_system: boolean;
  is_eligible: boolean;
  ineligible_reason?: string | null;
}

export interface BootstrapState {
  app_name: string;
  app_version: string;
  language: string;
  is_rtl: boolean;
  decimal_sep: string;
  theme_mode: string;
  needs_first_run: boolean;
  drives: DriveInfo[];
  recommended_drive: string | null;
  current_data_root: string | null;
  retention_days: number;
  autostart_enabled?: boolean;
  low_disk_shield_gb?: number;
  pause_on_battery_fullscreen?: boolean;
  translations?: Record<string, string>;
}

export interface PrimaryDataDriveDto {
  drive_letter: string;
  label: string;
  data_root: string;
  free_bytes: number;
  total_bytes: number;
}

export interface ReclaimableItemDto {
  app_name: string;
  path: string;
  size_bytes: number;
  category: string;
  recommendation: string;
  reason_code?: string | null;
}

export interface BurnRateForecast {
  daily_net_bps: number;
  daily_burn_rate_bytes: number;
  burn_rate_7d: number;
  burn_rate_14d: number;
  burn_rate_30d: number;
  days_until_90_pct: number | null;
  days_until_full: number | null;
  current_free_bytes: number;
  total_bytes: number;
  usage_ratio: number;
  is_critical: boolean;
  sample_days: number;
}

export interface OverviewSummary {
  drive_letter: string;
  total_bytes: number;
  free_bytes: number;
  used_bytes: number;
  usage_ratio: number;
  burn_rate_bps: number;
  days_until_full: number | null;
  days_until_90_pct: number | null;
  health_status: 'healthy' | 'warning' | 'critical' | string;
  primary_data_drive: PrimaryDataDriveDto | null;
  burn_rate_forecast?: BurnRateForecast;
  reclaimable_bytes: number;
  reclaimable_count: number;
  reclaimable_proposals: ReclaimableItemDto[];
}

export interface SpaceNode {
  name: string;
  path: string;
  physical_bytes: number;
  file_count: number;
  is_dir: boolean;
  reason_code?: string | null;
  children: SpaceNode[];
}

export interface TreemapRect {
  name: string;
  path: string;
  x: number;
  y: number;
  width: number;
  height: number;
  depth: number;
  percentage: number;
  reason_code?: string | null;
  children: TreemapRect[];
}

export interface TaskProgressEvent {
  task_id: string;
  kind: string;
  state: 'running' | 'completed' | 'failed' | 'cancelled' | string;
  percent: number;
  phase: string;
  title: string;
  detail: string;
  done_files: number;
  total_files: number;
  done_bytes: number;
  total_bytes: number;
  throughput_bps: number;
  eta_seconds: number;
}

export interface WatchdogAlertEvent {
  kind: 'auto_rollback' | 'auto_purge' | 'low_disk_warning' | 'junction_offline' | string;
  severity: 'info' | 'warning' | 'critical';
  title: string;
  detail: string;
  archive_id?: number | null;
}

// Typed Command Invokers

export async function getBootstrapState(): Promise<BootstrapState> {
  return await invoke<BootstrapState>('get_bootstrap_state');
}

export async function completeFirstRun(
  selectedDrive: string,
  retentionDays: number
): Promise<void> {
  return await invoke<void>('complete_first_run', {
    selectedDrive: selectedDrive.charAt(0),
    retentionDays,
  });
}

export async function getOverviewSummary(drive?: string): Promise<OverviewSummary> {
  return await invoke<OverviewSummary>('get_overview_summary', {
    drive: drive ? drive.charAt(0) : undefined,
  });
}

export async function scanSpaceLens(drive: string, maxDepth?: number): Promise<SpaceNode> {
  return await invoke<SpaceNode>('scan_space_lens', {
    drive: drive.charAt(0),
    maxDepth,
  });
}

export async function revealInExplorer(path: string): Promise<boolean> {
  return await invoke<boolean>('reveal_in_explorer', { path });
}

export async function controlTask(
  taskId: string,
  action: 'pause' | 'resume' | 'cancel'
): Promise<boolean> {
  return await invoke<boolean>('control_task', { taskId, action });
}

// Event Listeners

export async function listenTaskProgress(
  callback: (event: TaskProgressEvent) => void
): Promise<UnlistenFn> {
  return await listen<TaskProgressEvent>('greenc://task-progress', (e) => {
    callback(e.payload);
  });
}

export async function listenWatchdogAlert(
  callback: (event: WatchdogAlertEvent) => void
): Promise<UnlistenFn> {
  return await listen<WatchdogAlertEvent>('greenc://watchdog-alert', (e) => {
    callback(e.payload);
  });
}

// =========================================================================
// Milestone 3: Cleanup & Canary Vault Interfaces and Invokers (Tasks T036-T038)
// =========================================================================

export interface CleanupCandidateDto {
  id: string;
  profile_id: string;
  app_name: string;
  category: string;
  path: string;
  target_paths: string[];
  total_physical_bytes: number;
  reclaimable_bytes: number;
  file_count: number;
  is_locked: boolean;
  locked_file_count: number;
  canary_probe_passed: boolean;
  canary_probe_detail: string;
  is_orphan: boolean;
  selected: boolean;
}

export interface SkippedItemDto {
  path: string;
  reason: string;
}

export interface CleanupReportDto {
  task_id: string;
  success: boolean;
  total_candidates: number;
  processed_candidates: number;
  total_reclaimed_bytes: number;
  total_files_deleted: number;
  vault_archive_id?: string | null;
  vault_archive_path?: string | null;
  compressed_vault_bytes: number;
  skipped_locked_files_count: number;
  skipped_items: SkippedItemDto[];
  error_message?: string | null;
}

export interface VaultArchiveDto {
  id: number;
  archive_id: string;
  rule_id: number;
  app_name: string;
  zip_path: string;
  manifest_path: string;
  file_count: number;
  original_bytes: number;
  compressed_bytes: number;
  created_at: string;
  expires_at: string;
  status: 'QUARANTINED' | 'ROLLED_BACK' | 'PURGED' | string;
  status_reason: string;
  remaining_days: number;
  integrity_verified: boolean;
}

export interface VaultFileItemDto {
  id: number;
  original_path: string;
  rel_arcname: string;
  size_bytes: number;
  sha256: string;
  mtime: number;
  hardlink_count: number;
  restored_individually: boolean;
}

export interface VaultArchiveDetailDto {
  archive: VaultArchiveDto;
  files: VaultFileItemDto[];
  total_files: number;
}

export interface VaultRestoreReportDto {
  archive_id: string;
  success: boolean;
  restored_count: number;
  restored_bytes: number;
  restored_paths: string[];
  message: string;
}

// Cleanup Commands
export async function scanCleanupCandidates(
  profileIds?: string[]
): Promise<CleanupCandidateDto[]> {
  return await invoke<CleanupCandidateDto[]>('scan_cleanup_candidates', {
    profileIds: profileIds ?? null,
  });
}

export async function executeCleanup(
  candidateIds: string[],
  enableVault: boolean,
  permanent: boolean
): Promise<CleanupReportDto> {
  return await invoke<CleanupReportDto>('execute_cleanup', {
    candidateIds,
    enableVault,
    permanent,
  });
}

// Vault Commands
export async function getVaultArchives(): Promise<VaultArchiveDto[]> {
  return await invoke<VaultArchiveDto[]>('get_vault_archives');
}

export async function restoreVaultArchive(
  archiveId: string
): Promise<VaultRestoreReportDto> {
  return await invoke<VaultRestoreReportDto>('restore_vault_archive', { archiveId });
}

export async function deleteVaultArchive(archiveId: string): Promise<boolean> {
  return await invoke<boolean>('delete_vault_archive', { archiveId });
}

export async function inspectVaultArchive(
  archiveId: string
): Promise<VaultArchiveDetailDto> {
  return await invoke<VaultArchiveDetailDto>('inspect_vault_archive', { archiveId });
}

export async function restoreVaultFilesSelective(
  archiveId: string,
  relArcnames: string[]
): Promise<VaultRestoreReportDto> {
  return await invoke<VaultRestoreReportDto>('restore_vault_files_selective', {
    archiveId,
    relArcnames,
  });
}

// =========================================================================
// Milestone 4: Multi-Strategy Relocation Interfaces and Invokers (T047)
// =========================================================================

export interface RelocationCandidateDto {
  id: string;
  name: string;
  strategy: 'known_folder' | 'tool_config' | 'guided' | 'junction' | string;
  source_path: string;
  target_path: string;
  size_bytes: number;
  file_count: number;
  is_locked: boolean;
  locked_processes: string[];
  how_description: string;
  is_advanced: boolean;
  is_eligible: boolean;
  ineligible_reason?: string | null;
}

export interface KnownFolderDto {
  folder_id: string;
  guid: string;
  name: string;
  current_path: string;
  target_path: string;
  size_bytes: number;
  file_count: number;
  is_redirected: boolean;
  is_onedrive: boolean;
  target_drive: string;
}

export interface RelocationResultDto {
  id?: number | null;
  candidate_id: string;
  app_name: string;
  strategy: string;
  source_path: string;
  target_path: string;
  size_bytes: number;
  status: 'moving' | 'done' | 'failed' | 'undone' | string;
  health_status: 'healthy' | 'target_offline' | 'link_broken' | string;
  detail?: string | null;
  created_at: string;
  success: boolean;
  error_message?: string | null;
}

export async function scanRelocationCandidates(
  targetDrive?: string
): Promise<RelocationCandidateDto[]> {
  return await invoke<RelocationCandidateDto[]>('scan_relocation_candidates', {
    targetDrive: targetDrive ?? null,
  });
}

export async function relocateDirectory(
  candidateId: string,
  targetDrive: string
): Promise<RelocationResultDto> {
  return await invoke<RelocationResultDto>('relocate_directory', {
    candidateId,
    targetDrive,
  });
}

export async function getKnownFolders(targetDrive?: string): Promise<KnownFolderDto[]> {
  return await invoke<KnownFolderDto[]>('get_known_folders', {
    targetDrive: targetDrive ?? null,
  });
}

export async function relocateKnownFolder(
  folderId: string,
  targetDrive: string
): Promise<RelocationResultDto> {
  return await invoke<RelocationResultDto>('relocate_known_folder', {
    folderId,
    targetDrive,
  });
}

export async function recoverInterruptedRelocations(): Promise<RelocationResultDto[]> {
  return await invoke<RelocationResultDto[]>('recover_interrupted_relocations');
}

export async function listRelocationHistory(): Promise<RelocationResultDto[]> {
  return await invoke<RelocationResultDto[]>('list_relocation_history');
}

export async function undoRelocation(recordId: number): Promise<RelocationResultDto> {
  return await invoke<RelocationResultDto>('undo_relocation', { recordId });
}

export async function repairJunctionTarget(
  recordId: number,
  newTargetPath: string
): Promise<RelocationResultDto> {
  return await invoke<RelocationResultDto>('repair_junction_target', {
    recordId,
    newTargetPath,
  });
}

// =========================================================================
// Milestone 4: In-Place Compaction Interfaces and Invokers (T052)
// =========================================================================

export interface CompactionCandidateDto {
  id: string;
  name: string;
  path: string;
  kind: 'wof_directory' | 'vhdx_trim' | string;
  logical_bytes: number;
  physical_bytes: number;
  estimated_reclaimable_bytes: number;
  file_count: number;
  recommended_algorithm: 'XPRESS4K' | 'XPRESS8K' | 'XPRESS16K' | 'LZX' | string;
  is_locked: boolean;
  locked_processes: string[];
  is_compacted: boolean;
  selected: boolean;
}

export interface CompactionResultDto {
  task_id: string;
  success: boolean;
  kind: 'wof_directory' | 'vhdx_trim' | string;
  target_path: string;
  algorithm: string;
  files_processed: number;
  files_compacted: number;
  files_skipped: number;
  logical_bytes: number;
  physical_before_bytes: number;
  physical_after_bytes: number;
  reclaimed_bytes: number;
  compression_ratio: number;
  error_message?: string | null;
}

export interface CompactionHistoryDto {
  id?: number | null;
  target_path: string;
  kind: string;
  algorithm: string;
  logical_bytes: number;
  physical_before_bytes: number;
  physical_after_bytes: number;
  file_count: number;
  status: string;
  updated_at: string;
}

export async function scanCompactionCandidates(
  targetPath?: string
): Promise<CompactionCandidateDto[]> {
  return await invoke<CompactionCandidateDto[]>('scan_compaction_candidates', {
    targetPath: targetPath ?? null,
  });
}

export async function compactFiles(
  targetPaths: string[],
  algorithm?: string
): Promise<CompactionResultDto> {
  return await invoke<CompactionResultDto>('compact_files', {
    targetPaths,
    algorithm: algorithm ?? null,
  });
}

export async function uncompactFiles(
  targetPaths: string[]
): Promise<CompactionResultDto> {
  return await invoke<CompactionResultDto>('uncompact_files', {
    targetPaths,
  });
}

export async function compactVhdx(vhdxPath: string): Promise<CompactionResultDto> {
  return await invoke<CompactionResultDto>('compact_vhdx', {
    vhdxPath,
  });
}

export async function listCompactionHistory(): Promise<CompactionHistoryDto[]> {
  return await invoke<CompactionHistoryDto[]>('list_compaction_history');
}

// =========================================================================
// Milestone 5 & 6: Daily History, Attribution, Quota Rules & Settings (T056-T066)
// =========================================================================

export interface DailySnapshotDto {
  id: number;
  drive_letter: string;
  date: string;
  total_bytes: number;
  used_bytes: number;
  free_bytes: number;
  net_change_bytes: number;
  reclaimed_bytes: number;
  updated_at: string;
}

export interface FolderGrowthDto {
  folder_path: string;
  reason_code: string;
  reason_label: string;
  total_delta_bytes: number;
  file_count: number;
}

export interface ActorWriteDto {
  process_name: string;
  service_name: string;
  display_actor: string;
  total_delta_bytes: number;
  event_count: number;
}

export interface DailyDrilldownDto {
  drive_letter: string;
  date: string;
  folders: FolderGrowthDto[];
  actors: ActorWriteDto[];
}

export interface DriveHistoryDto {
  drive_letter: string;
  days_back: number;
  snapshots: DailySnapshotDto[];
  selected_date: string | null;
  latest_drilldown: DailyDrilldownDto | null;
}

export interface LanguageOptionDto {
  code: string;
  native_name: string;
  english_name: string;
  is_rtl: boolean;
  decimal_sep: string;
}

export interface AppSettingsDto {
  app_name: string;
  app_version: string;
  author: string;
  website_url: string;
  website_label: string;
  data_root: string | null;
  retention_days_default: number;
  language: string;
  is_rtl: boolean;
  decimal_sep: string;
  theme_mode: 'system' | 'light' | 'dark' | string;
  autostart_enabled: boolean;
  low_disk_shield_gb: number;
  pause_on_battery_fullscreen: boolean;
  translations: Record<string, string>;
  supported_languages: LanguageOptionDto[];
}

export interface SettingsPatchDto {
  language?: string;
  theme_mode?: string;
  retention_days_default?: number;
  autostart_enabled?: boolean;
  low_disk_shield_gb?: number;
  pause_on_battery_fullscreen?: boolean;
}

export interface QuotaRuleInputDto {
  id?: number | null;
  app_name: string;
  process_name?: string;
  service_name?: string;
  target_paths: string[];
  max_bytes: number;
  watermark_ratio?: number;
  retention_days?: number;
  min_file_age_seconds?: number;
  exclude_patterns?: string[];
  enabled?: boolean;
}

export interface QuotaRuleStatusDto {
  id: number;
  app_name: string;
  process_name: string;
  service_name: string;
  target_paths: string[];
  max_bytes: number;
  current_bytes: number;
  watermark_ratio: number;
  watermark_bytes: number;
  retention_days: number;
  min_file_age_seconds: number;
  exclude_patterns: string[];
  protected_files: string[];
  enabled: boolean;
  is_exceeded: boolean;
  created_at: string;
}

export interface MoveVaultReportDto {
  task_id: string;
  new_data_root: string;
  moved_archives_count: number;
  errors: string[];
  success: boolean;
}

export interface SandboxDemoResultDto {
  sandbox_dir: string;
  canary_passed: boolean;
  archived_files_count: number;
  auto_rolled_back: boolean;
  restored_files_count: number;
  detail: string;
}

export async function scanDriveHistory(
  drive: string,
  daysBack: number = 14
): Promise<DriveHistoryDto> {
  return await invoke<DriveHistoryDto>('scan_drive_history', {
    drive,
    daysBack,
  });
}

export async function getDailyDrilldown(
  drive: string,
  date: string
): Promise<DailyDrilldownDto> {
  return await invoke<DailyDrilldownDto>('get_daily_drilldown', {
    drive,
    date,
  });
}

export async function getSupportedLanguages(): Promise<LanguageOptionDto[]> {
  return await invoke<LanguageOptionDto[]>('get_supported_languages');
}

export async function getAppSettings(): Promise<AppSettingsDto> {
  return await invoke<AppSettingsDto>('get_app_settings');
}

export async function updateSettings(patch: SettingsPatchDto): Promise<AppSettingsDto> {
  return await invoke<AppSettingsDto>('update_settings', { patch });
}

export async function changeDataRoot(newRoot: string): Promise<MoveVaultReportDto> {
  return await invoke<MoveVaultReportDto>('change_data_root', { newRoot });
}

export async function listQuotaRules(): Promise<QuotaRuleStatusDto[]> {
  return await invoke<QuotaRuleStatusDto[]>('list_quota_rules');
}

export async function saveQuotaRule(rule: QuotaRuleInputDto): Promise<QuotaRuleStatusDto> {
  return await invoke<QuotaRuleStatusDto>('save_quota_rule', { rule });
}

export async function deleteQuotaRule(ruleId: number): Promise<boolean> {
  return await invoke<boolean>('delete_quota_rule', { ruleId });
}

export async function runSandboxDemo(simulateCrash: boolean): Promise<SandboxDemoResultDto> {
  return await invoke<SandboxDemoResultDto>('run_sandbox_demo', { simulateCrash });
}

// Drive quotas: per-drive capacity budget driving warnings, backup and auto-clean
export interface DriveQuotaInputDto {
  id?: number | null;
  drive_root: string;
  /** 0 = use the whole drive capacity */
  limit_bytes: number;
  warn_ratio?: number;
  clean_ratio?: number;
  target_ratio?: number;
  auto_clean?: boolean;
  auto_backup?: boolean;
  backup_dir?: string | null;
  enabled?: boolean;
}

export type DriveQuotaLevel = 'ok' | 'warning' | 'critical';

export interface DriveQuotaStatusDto {
  id: number;
  drive_root: string;
  limit_bytes: number;
  effective_limit_bytes: number;
  total_bytes: number;
  free_bytes: number;
  used_bytes: number;
  usage_ratio: number;
  warn_ratio: number;
  clean_ratio: number;
  target_ratio: number;
  auto_clean: boolean;
  auto_backup: boolean;
  backup_dir: string | null;
  enabled: boolean;
  level: DriveQuotaLevel;
  bytes_to_reclaim: number;
  /** false when the drive is currently not mounted */
  available: boolean;
}

export async function listDriveQuotas(): Promise<DriveQuotaStatusDto[]> {
  return await invoke<DriveQuotaStatusDto[]>('list_drive_quotas');
}

export async function saveDriveQuota(quota: DriveQuotaInputDto): Promise<DriveQuotaStatusDto> {
  return await invoke<DriveQuotaStatusDto>('save_drive_quota', { quota });
}

export async function deleteDriveQuota(quotaId: number): Promise<boolean> {
  return await invoke<boolean>('delete_drive_quota', { quotaId });
}

export async function hideToTray(): Promise<boolean> {
  return await invoke<boolean>('hide_to_tray');
}

