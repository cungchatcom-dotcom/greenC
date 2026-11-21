//! Silent 45-second autonomous `BackgroundGuardian` scheduler (Task T060).
//!
//! Responsibilities:
//! 1. Immediate foreground preemption (`< 100 ms` yield when `AppState::has_foreground_tasks()`).
//! 2. Battery-saver & D3D fullscreen game/presentation deferral (`PowerSessionApi`).
//! 3. Emergency Low-Disk Shield (`low_disk_shield_gb`) alert emission.
//! 4. Rolling `QuotaRule` enforcement down to `watermark_ratio` (`85%`) into Canary Vault.
//! 5. `RollbackEngine::run_watchdog_cycle` auto-rollback + expired archive purge.
//! 6. Live NTFS junction health verification (`RecoveryService::check_junction_health`).

use crate::state::{AppState, WatchdogAlertEvent};
use greenc_core::config::{default_vault_dir, is_protected_filename, MIN_VAULT_RESERVE_BYTES};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    CleanCandidate, DriveQuota, FileManifestItem, QuotaLevel, QuotaRule, RelocationHealthStatus,
};
use greenc_core::task::TaskControl;
use greenc_platform::handle::NativeFileLockProbe;
use greenc_platform::power_session::{NativePowerSessionApi, PowerSessionApi};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    FileLockProbe, NativeProcessServiceProbe, ProcessServiceProbe, ReparsePointApi, Win32VolumeApi,
};
use greenc_platform::volume::NativeWin32VolumeApi;
use greenc_relocate::recovery::RecoveryService;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::profiles::PresetProfiles;
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

/// Default interval between autonomous background guardian ticks (`45 seconds`).
pub const GUARDIAN_TICK_INTERVAL: Duration = Duration::from_secs(45);

/// Minimum cooldown between repeated low-disk shield alerts (`15 minutes`).
const LOW_DISK_ALERT_COOLDOWN: Duration = Duration::from_secs(15 * 60);

/// Telemetry returned by a single deterministic guardian cycle (used by unit/integration tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardianCycleReport {
    pub preempted_by_foreground: bool,
    pub deferred_for_power_or_fullscreen: bool,
    pub low_disk_triggered: bool,
    pub rules_evaluated: usize,
    pub rules_enforced: usize,
    pub bytes_reclaimed: u64,
    pub auto_rollbacks_triggered: usize,
    pub broken_junctions_detected: usize,
    pub drive_quotas_evaluated: usize,
    pub drive_quota_warnings: usize,
    pub drive_quota_cleanups: usize,
}

/// Injectable OS dependencies for `BackgroundGuardian`.
pub struct GuardianDeps {
    pub power_api: Arc<dyn PowerSessionApi>,
    pub volume_api: Arc<dyn Win32VolumeApi>,
    pub lock_probe: Arc<dyn FileLockProbe>,
    pub proc_probe: Arc<dyn ProcessServiceProbe>,
    pub reparse_api: Arc<dyn ReparsePointApi>,
}

impl std::fmt::Debug for GuardianDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardianDeps").finish()
    }
}

impl Default for GuardianDeps {
    fn default() -> Self {
        Self {
            power_api: Arc::new(NativePowerSessionApi),
            volume_api: Arc::new(NativeWin32VolumeApi),
            lock_probe: Arc::new(NativeFileLockProbe),
            proc_probe: Arc::new(NativeProcessServiceProbe),
            reparse_api: Arc::new(NativeReparsePointApi),
        }
    }
}

/// Silent autonomous background guardian service.
pub struct BackgroundGuardian {
    state: AppState,
    deps: Arc<GuardianDeps>,
    tick_interval: Duration,
    system_drive_root: PathBuf,
    last_low_disk_alert: Arc<Mutex<Option<Instant>>>,
    quota_alert_state: Arc<Mutex<HashMap<String, (QuotaLevel, Instant)>>>,
    /// Preset profile ids scanned by automatic drive-quota cleanup. Empty = never scan the host.
    auto_clean_profiles: Arc<Vec<String>>,
    alerts_log: Arc<Mutex<Vec<WatchdogAlertEvent>>>,
}

impl std::fmt::Debug for BackgroundGuardian {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundGuardian")
            .field("tick_interval", &self.tick_interval)
            .field("system_drive_root", &self.system_drive_root)
            .finish()
    }
}

impl BackgroundGuardian {
    pub fn new(state: AppState) -> Self {
        Self::with_deps(state, GuardianDeps::default(), GUARDIAN_TICK_INTERVAL)
            .with_auto_clean_profiles(
                AUTO_CLEAN_PROFILE_IDS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            )
    }

    /// Chooses which preset profiles automatic drive-quota cleanup may scan.
    /// `with_deps` starts with none so tests and custom setups never scan the host by accident.
    pub fn with_auto_clean_profiles(mut self, ids: Vec<String>) -> Self {
        self.auto_clean_profiles = Arc::new(ids);
        self
    }

    pub fn with_deps(state: AppState, deps: GuardianDeps, tick_interval: Duration) -> Self {
        Self {
            state,
            deps: Arc::new(deps),
            tick_interval,
            system_drive_root: PathBuf::from(greenc_platform::volume::get_system_drive_root()),
            last_low_disk_alert: Arc::new(Mutex::new(None)),
            quota_alert_state: Arc::new(Mutex::new(HashMap::new())),
            auto_clean_profiles: Arc::new(Vec::new()),
            alerts_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_system_drive_root(mut self, root: PathBuf) -> Self {
        self.system_drive_root = root;
        self
    }

    /// Returns recorded watchdog alerts (useful for headless tests without a Tauri `AppHandle`).
    pub fn recorded_alerts(&self) -> Vec<WatchdogAlertEvent> {
        self.alerts_log.lock().clone()
    }

    /// Spawns the 45-second background loop onto the Tokio runtime, returning a `CancellationToken`
    /// that cleanly terminates the guardian on app exit.
    pub fn spawn(self: Arc<Self>) -> CancellationToken {
        let cancel_token = CancellationToken::new();
        let token_clone = cancel_token.clone();

        // Tauri `setup()` runs outside any Tokio context: use Tauri's global runtime, not `tokio::spawn`.
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(self.tick_interval);
            // Skip immediate t=0 burst so UI startup stays < 180 ms cold-start.
            interval.tick().await;

            loop {
                tokio::select! {
                    () = token_clone.cancelled() => {
                        tracing::info!("BackgroundGuardian stopped cleanly.");
                        break;
                    }
                    _ = interval.tick() => {
                        if let Err(err) = self.run_cycle().await {
                            tracing::warn!("BackgroundGuardian cycle error: {err}");
                        }
                    }
                }
            }
        });

        cancel_token
    }

    /// Executes a single autonomous guardian tick, yielding within `< 100 ms` if a foreground
    /// user operation starts at any point before or during the cycle.
    pub async fn run_cycle(&self) -> Result<GuardianCycleReport> {
        // 1. Immediate check before spawning any worker
        if self.state.has_foreground_tasks() {
            return Ok(GuardianCycleReport {
                preempted_by_foreground: true,
                ..Default::default()
            });
        }

        // Cooperative TaskControl cancelled within <= 20ms if a foreground task registers mid-cycle!
        let cycle_ctrl = Arc::new(TaskControl::new());
        let watcher_stop = CancellationToken::new();
        {
            let state_for_watch = self.state.clone();
            let ctrl_for_watch = Arc::clone(&cycle_ctrl);
            let stop_for_watch = watcher_stop.clone();
            tokio::spawn(async move {
                let mut fast_poll = tokio::time::interval(Duration::from_millis(20));
                loop {
                    tokio::select! {
                        () = stop_for_watch.cancelled() => break,
                        _ = fast_poll.tick() => {
                            if state_for_watch.has_foreground_tasks() {
                                ctrl_for_watch.cancel();
                                break;
                            }
                        }
                    }
                }
            });
        }

        let guardian_state = self.state.clone();
        let deps = Arc::clone(&self.deps);
        let sys_root = self.system_drive_root.clone();
        let last_alert_ref = Arc::clone(&self.last_low_disk_alert);
        let quota_alert_ref = Arc::clone(&self.quota_alert_state);
        let auto_clean_ids = Arc::clone(&self.auto_clean_profiles);
        let alerts_log_ref = Arc::clone(&self.alerts_log);
        let ctrl_worker = Arc::clone(&cycle_ctrl);

        let cycle_res = tokio::task::spawn_blocking(move || -> Result<GuardianCycleReport> {
            let mut report = GuardianCycleReport::default();
            let mut pending_alerts: Vec<WatchdogAlertEvent> = Vec::new();
            let settings = guardian_state.settings();
            let locale = settings
                .get_language()
                .ok()
                .flatten()
                .unwrap_or_else(|| "vi".to_string());

            // Check 1: Emergency Low-Disk Shield (runs even on battery because it's a single O(1) Win32 query)
            let shield_gb = settings.get_low_disk_shield_gb().unwrap_or(10);
            let shield_bytes = u64::from(shield_gb) * 1024 * 1024 * 1024;
            if let Ok((_total, free_bytes)) = deps.volume_api.get_disk_free_space(&sys_root) {
                if free_bytes < shield_bytes {
                    report.low_disk_triggered = true;
                    let should_emit = {
                        let mut guard = last_alert_ref.lock();
                        match *guard {
                            Some(prev) if prev.elapsed() < LOW_DISK_ALERT_COOLDOWN => false,
                            _ => {
                                *guard = Some(Instant::now());
                                true
                            }
                        }
                    };
                    if should_emit {
                        let free_gb = (free_bytes as f64) / (1024.0 * 1024.0 * 1024.0);
                        pending_alerts.push(WatchdogAlertEvent {
                            kind: "low_disk_warning".to_string(),
                            severity: "critical".to_string(),
                            title: guardian_state.catalog.tr(
                                &locale,
                                "Cảnh báo dung lượng ổ hệ thống thấp",
                                &[],
                            ),
                            detail: format!(
                                "Ổ {} chỉ còn trống {:.1} GB (ngưỡng bảo vệ: {} GB).",
                                sys_root.display(),
                                free_gb,
                                shield_gb
                            ),
                            archive_id: None,
                        });
                    }
                }
            }

            // Check 1b: Per-drive quota evaluation -> warnings (cheap O(1) volume queries)
            let drive_quotas = guardian_state
                .db
                .list_drive_quotas(true)
                .unwrap_or_default();
            let mut quota_states: Vec<(DriveQuota, u64, u64, QuotaLevel)> = Vec::new();
            for quota in drive_quotas {
                let Ok((total, free)) = deps
                    .volume_api
                    .get_disk_free_space(Path::new(&quota.drive_root))
                else {
                    continue;
                };
                report.drive_quotas_evaluated += 1;
                let level = quota.level(total, free);
                let key = quota.drive_root.to_ascii_uppercase();
                if level >= QuotaLevel::Warning {
                    let should_emit = {
                        let mut map = quota_alert_ref.lock();
                        let emit = match map.get(&key) {
                            Some((prev, at)) => {
                                level > *prev || at.elapsed() >= LOW_DISK_ALERT_COOLDOWN
                            }
                            None => true,
                        };
                        if emit {
                            map.insert(key.clone(), (level, Instant::now()));
                        }
                        emit
                    };
                    if should_emit {
                        report.drive_quota_warnings += 1;
                        let pct = quota.usage_ratio(total, free) * 100.0;
                        pending_alerts.push(WatchdogAlertEvent {
                            kind: "drive_quota".to_string(),
                            severity: if level == QuotaLevel::Critical {
                                "critical"
                            } else {
                                "warning"
                            }
                            .to_string(),
                            title: format!("Ổ {} đã dùng {:.0}% hạn mức", quota.drive_root, pct),
                            detail: format!(
                                "Hạn mức: {:.1} GB, còn trống {:.1} GB.{}",
                                quota.effective_limit(total) as f64 / 1_073_741_824.0,
                                free as f64 / 1_073_741_824.0,
                                if level == QuotaLevel::Critical && quota.auto_clean {
                                    " GreenC sẽ tự dọn dẹp."
                                } else {
                                    ""
                                }
                            ),
                            archive_id: None,
                        });
                    }
                } else {
                    quota_alert_ref.lock().remove(&key);
                }
                quota_states.push((quota, total, free, level));
            }

            if ctrl_worker.is_cancelled() || guardian_state.has_foreground_tasks() {
                report.preempted_by_foreground = true;
                return Ok(report);
            }

            // Check 2: Safety Watchdog (service stop / missing-file log auto-rollback & expired purge)
            let rollback_engine =
                RollbackEngine::new(&guardian_state.db, RestoreOptions::default());
            if let Ok(restores) = rollback_engine.run_watchdog_cycle(deps.proc_probe.as_ref()) {
                for res in restores {
                    report.auto_rollbacks_triggered += 1;
                    pending_alerts.push(WatchdogAlertEvent {
                        kind: "auto_rollback".to_string(),
                        severity: "warning".to_string(),
                        title: guardian_state.catalog.tr(
                            &locale,
                            "Đã tự khôi phục 1 gói dữ liệu cho ứng dụng.",
                            &[],
                        ),
                        detail: res.reason,
                        archive_id: Some(res.archive_id),
                    });
                }
            }

            // Check 3: Live Junction Health Monitor
            if let Ok(records) = guardian_state.db.list_relocations(Some("done")) {
                let healths =
                    RecoveryService::check_junction_health(&records, deps.reparse_api.as_ref());
                for (rec, live_health) in records.into_iter().zip(healths) {
                    if live_health != RelocationHealthStatus::Healthy {
                        report.broken_junctions_detected += 1;
                        if rec.health_status == RelocationHealthStatus::Healthy {
                            pending_alerts.push(WatchdogAlertEvent {
                                kind: "junction_offline".to_string(),
                                severity: "warning".to_string(),
                                title: format!("Liên kết {} đang ngoại tuyến", rec.app_name),
                                detail: format!(
                                    "Đích lưu trữ {} hiện không truy cập được ({}).",
                                    rec.target_path.display(),
                                    live_health.as_str()
                                ),
                                archive_id: None,
                            });
                        }
                    }
                    if let Some(id) = rec.id {
                        if rec.health_status != live_health {
                            let _ = guardian_state
                                .db
                                .update_relocation_health(id, live_health.as_str());
                        }
                    }
                }
            }

            // Check 4: Battery-Saver / Fullscreen Game Deferral before heavy QuotaRule compression
            let pause_on_bat_fs = settings.get_pause_on_battery_fullscreen().unwrap_or(true);
            if pause_on_bat_fs {
                if let Ok(pstate) = deps.power_api.query_state() {
                    if pstate.should_defer_background_work() {
                        report.deferred_for_power_or_fullscreen = true;
                        flush_alerts(&guardian_state, &alerts_log_ref, pending_alerts);
                        return Ok(report);
                    }
                }
            }

            // Check 5: Rolling QuotaRule Enforcement (down to watermark_ratio = 85%)
            let rules = guardian_state.db.list_quota_rules(true).unwrap_or_default();
            let vault_dir = settings
                .vault_dir(&default_vault_dir())
                .unwrap_or_else(|_| default_vault_dir());

            for rule in rules {
                if ctrl_worker.is_cancelled() || guardian_state.has_foreground_tasks() {
                    report.preempted_by_foreground = true;
                    break;
                }
                report.rules_evaluated += 1;

                match enforce_single_quota_rule(
                    &rule,
                    &vault_dir,
                    &guardian_state,
                    &deps,
                    &ctrl_worker,
                ) {
                    Ok(freed) if freed > 0 => {
                        report.rules_enforced += 1;
                        report.bytes_reclaimed += freed;
                        let today = chrono::Local::now()
                            .date_naive()
                            .format("%Y-%m-%d")
                            .to_string();
                        let _ = guardian_state.db.record_reclaimed_bytes(
                            &sys_root.to_string_lossy(),
                            &today,
                            freed,
                        );
                    }
                    Ok(_) => {}
                    Err(GreenCError::Cancelled) => {
                        report.preempted_by_foreground = true;
                        break;
                    }
                    Err(err) => {
                        tracing::debug!("QuotaRule '{}' skipped this tick: {err}", rule.app_name);
                    }
                }
            }

            // Check 6: Drive-quota auto-clean (+ auto-backup) for drives at the critical level
            for (quota, total, free, level) in quota_states {
                if level != QuotaLevel::Critical || !quota.auto_clean {
                    continue;
                }
                if ctrl_worker.is_cancelled() || guardian_state.has_foreground_tasks() {
                    report.preempted_by_foreground = true;
                    break;
                }
                let pool = match scan_auto_clean_pool(
                    &auto_clean_ids,
                    Path::new(&quota.drive_root),
                    deps.lock_probe.as_ref(),
                    &ctrl_worker,
                ) {
                    Ok(p) => p,
                    Err(GreenCError::Cancelled) => {
                        report.preempted_by_foreground = true;
                        break;
                    }
                    Err(_) => continue,
                };
                match enforce_drive_quota(
                    &quota,
                    total,
                    free,
                    pool,
                    &vault_dir,
                    &guardian_state,
                    &deps,
                    &ctrl_worker,
                ) {
                    Ok(out) if out.bytes_freed > 0 => {
                        report.drive_quota_cleanups += 1;
                        report.bytes_reclaimed += out.bytes_freed;
                        let today = chrono::Local::now()
                            .date_naive()
                            .format("%Y-%m-%d")
                            .to_string();
                        let _ = guardian_state.db.record_reclaimed_bytes(
                            &quota.drive_root,
                            &today,
                            out.bytes_freed,
                        );
                        pending_alerts.push(WatchdogAlertEvent {
                            kind: "drive_quota_cleanup".to_string(),
                            severity: "info".to_string(),
                            title: format!(
                                "Đã tự dọn {:.1} GB trên ổ {}",
                                out.bytes_freed as f64 / 1_073_741_824.0,
                                quota.drive_root
                            ),
                            detail: match (&out.backed_up_to, &out.backup_problem) {
                                (Some(p), _) => format!("Đã sao lưu gói Vault tới {}", p.display()),
                                (None, Some(why)) => format!("Chưa sao lưu được: {why}"),
                                _ => "Dữ liệu được giữ trong Vault, có thể khôi phục.".to_string(),
                            },
                            archive_id: None,
                        });
                    }
                    Ok(_) => {}
                    Err(GreenCError::Cancelled) => {
                        report.preempted_by_foreground = true;
                        break;
                    }
                    Err(err) => {
                        tracing::debug!(
                            "Drive quota {} skipped this tick: {err}",
                            quota.drive_root
                        );
                    }
                }
            }

            flush_alerts(&guardian_state, &alerts_log_ref, pending_alerts);
            Ok(report)
        })
        .await
        .map_err(|e| GreenCError::Vault(format!("Guardian worker join error: {e}")))?;

        watcher_stop.cancel();
        cycle_res
    }
}

fn flush_alerts(
    state: &AppState,
    alerts_log: &Mutex<Vec<WatchdogAlertEvent>>,
    alerts: Vec<WatchdogAlertEvent>,
) {
    if alerts.is_empty() {
        return;
    }
    let mut log = alerts_log.lock();
    for alert in alerts {
        log.push(alert.clone());
        state.emit_alert(&alert);
        if let Ok(guard) = state.app_handle.read() {
            if let Some(ref handle) = *guard {
                crate::tray::notify_watchdog_alert(handle, &alert);
            }
        }
    }
}

/// Measures current physical usage across a rule's `target_paths` and, if `> rule.max_bytes`,
/// archives oldest unlocked eligible files into Canary Vault until usage drops `<= watermark`.
pub fn enforce_single_quota_rule(
    rule: &QuotaRule,
    vault_dir: &Path,
    state: &AppState,
    deps: &GuardianDeps,
    ctrl: &TaskControl,
) -> Result<u64> {
    if rule.max_bytes == 0 || rule.target_paths.is_empty() {
        return Ok(0);
    }

    let now = SystemTime::now();
    let min_age = Duration::from_secs(rule.min_file_age_seconds);
    let mut all_files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut total_folder_bytes: u64 = 0;

    for target in &rule.target_paths {
        if !target.exists() {
            continue;
        }
        collect_rule_files(target, rule, ctrl, &mut all_files, &mut total_folder_bytes)?;
    }

    if total_folder_bytes <= rule.max_bytes {
        return Ok(0);
    }

    let watermark_bytes = ((rule.max_bytes as f64) * rule.watermark_ratio.clamp(0.10, 0.99)) as u64;
    let bytes_to_reclaim = total_folder_bytes.saturating_sub(watermark_bytes);
    if bytes_to_reclaim == 0 {
        return Ok(0);
    }

    // Sort oldest modified first for FIFO rolling quota eviction
    all_files.sort_by_key(|(_, _, mtime)| *mtime);

    let mut selected_candidates = Vec::new();
    let mut accumulated: u64 = 0;

    for (path, size_bytes, mtime) in all_files {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        if accumulated >= bytes_to_reclaim {
            break;
        }
        // Respect minimum file age guard
        if let Ok(age) = now.duration_since(mtime) {
            if age < min_age {
                continue;
            }
        }
        // Skip locked files
        if deps.lock_probe.is_file_locked(&path) {
            continue;
        }

        selected_candidates.push(CleanCandidate {
            path,
            size_bytes,
            mtime,
            is_locked: false,
            profile_id: format!("quota_rule_{}", rule.id.unwrap_or(0)),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: Vec::new(),
        });
        accumulated = accumulated.saturating_add(size_bytes);
    }

    if selected_candidates.is_empty() {
        return Ok(0);
    }

    let (freed, _zip) = archive_and_unlink(
        rule.id.unwrap_or(0),
        &rule.app_name,
        &rule.process_name,
        &rule.service_name,
        rule.retention_days,
        selected_candidates,
        vault_dir,
        state,
        deps,
        ctrl,
    )?;
    Ok(freed)
}

/// Archives `candidates` into the Canary Vault, records them in the DB, then removes the originals.
/// Returns `(bytes_freed, path_of_vault_zip)`.
#[allow(clippy::too_many_arguments)]
fn archive_and_unlink(
    rule_id: i64,
    app_name: &str,
    process_name: &str,
    service_name: &str,
    retention_days: u32,
    candidates: Vec<CleanCandidate>,
    vault_dir: &Path,
    state: &AppState,
    deps: &GuardianDeps,
    ctrl: &TaskControl,
) -> Result<(u64, PathBuf)> {
    let snapshots: HashMap<PathBuf, (u64, SystemTime)> = candidates
        .iter()
        .map(|c| (c.path.clone(), (c.size_bytes, c.mtime)))
        .collect();

    let arch_opts = ArchiveOptions {
        vault_dir: vault_dir.to_path_buf(),
        retention_days,
        min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
        ..Default::default()
    };
    let pipeline = ArchiverPipeline::new(arch_opts);
    let mut manifest = pipeline.archive_candidates(
        rule_id,
        app_name,
        &candidates,
        deps.volume_api.as_ref(),
        ctrl,
        |_| {},
    )?;
    manifest.process_name = process_name.to_string();
    manifest.service_name = service_name.to_string();

    let zip_p = vault_dir.join(format!("{}.zip", manifest.archive_id));
    let man_p = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
    let _ = manifest.write_to_file(&man_p);
    manifest.persist_to_db(&state.db, &zip_p, &man_p)?;

    let cleaner = SafeCleaner::new(
        deps.lock_probe.as_ref(),
        deps.volume_api.as_ref(),
        None,
        CleanupOptions {
            deletion_method: DeletionMethod::Permanent,
            min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
            prune_empty_directories: false,
        },
    );

    let manifest_items: Vec<FileManifestItem> = manifest.files.clone();
    let clean_res = cleaner.unlink_archived_items(&manifest_items, &snapshots)?;
    Ok((clean_res.bytes_freed, zip_p))
}

/// Safe preset profiles used by automatic drive-quota cleanup (opt-in `orphan_radar` excluded).
pub const AUTO_CLEAN_PROFILE_IDS: [&str; 7] = [
    "windows_temp",
    "browser_cache",
    "chat_media",
    "crash_dumps",
    "package_managers",
    "app_logs",
    "old_versions",
];

/// Retention (days) for Vault archives produced by automatic drive-quota cleanup.
const DEFAULT_QUOTA_RETENTION_DAYS: u32 = 14;

/// Result of one automatic drive-quota enforcement pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriveQuotaOutcome {
    pub bytes_freed: u64,
    pub vault_zip: Option<PathBuf>,
    pub backed_up_to: Option<PathBuf>,
    /// Set when a requested backup could not be made (the cleanup itself still stands).
    pub backup_problem: Option<String>,
}

fn same_drive(a: &Path, b: &Path) -> bool {
    greenc_core::models::is_same_volume(a, b)
}

/// Copies a Vault archive (zip + manifest) into `backup_dir`. Refuses a backup folder that lives on
/// the very drive being protected, because it would eat the space we are trying to free.
pub fn backup_vault_archive(
    zip: &Path,
    protected_root: &Path,
    backup_dir: &Path,
) -> std::result::Result<PathBuf, String> {
    if same_drive(backup_dir, protected_root) {
        return Err(format!(
            "Thư mục sao lưu {} nằm trên chính ổ đang được bảo vệ",
            backup_dir.display()
        ));
    }
    std::fs::create_dir_all(backup_dir)
        .map_err(|e| format!("Không tạo được thư mục sao lưu: {e}"))?;
    let name = zip
        .file_name()
        .ok_or_else(|| "Đường dẫn gói Vault không hợp lệ".to_string())?;
    let dest = backup_dir.join(name);
    std::fs::copy(zip, &dest).map_err(|e| format!("Không sao chép được gói Vault: {e}"))?;

    // Manifest sits next to the zip as "<archive_id>.manifest.json".
    if let (Some(dir), Some(stem)) = (zip.parent(), zip.file_stem()) {
        let mname = format!("{}.manifest.json", stem.to_string_lossy());
        let manifest = dir.join(&mname);
        if manifest.exists() {
            let _ = std::fs::copy(&manifest, backup_dir.join(&mname));
        }
    }
    Ok(dest)
}

/// Scans the given preset profiles for unlocked cleanup candidates located on `drive_root`.
/// This reads real directories, so callers that must not touch the host (tests) pass no profile ids.
pub fn scan_auto_clean_pool(
    profile_ids: &[String],
    drive_root: &Path,
    lock_probe: &dyn FileLockProbe,
    ctrl: &TaskControl,
) -> Result<Vec<CleanCandidate>> {
    let mut pool: Vec<CleanCandidate> = Vec::new();
    for pid in profile_ids {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        pool.extend(
            PresetProfiles::scan_profile_candidates(pid, ctrl, lock_probe)
                .into_iter()
                .filter(|c| !c.is_locked && !c.is_orphan)
                // Only touch files on the protected drive.
                .filter(|c| same_drive(&c.path, drive_root)),
        );
    }
    Ok(pool)
}

/// Brings a drive back under its quota's `target_ratio` by moving the oldest files of `pool` into the
/// Canary Vault, optionally mirroring the archive to a backup folder.
#[allow(clippy::too_many_arguments)]
pub fn enforce_drive_quota(
    quota: &DriveQuota,
    total_bytes: u64,
    free_bytes: u64,
    mut pool: Vec<CleanCandidate>,
    vault_dir: &Path,
    state: &AppState,
    deps: &GuardianDeps,
    ctrl: &TaskControl,
) -> Result<DriveQuotaOutcome> {
    let need = quota.bytes_to_reclaim(total_bytes, free_bytes);
    if need == 0 {
        return Ok(DriveQuotaOutcome::default());
    }
    pool.retain(|c| !c.is_locked && !c.is_orphan);
    pool.sort_by_key(|c| c.mtime);

    let mut chosen = Vec::new();
    let mut acc = 0u64;
    for c in pool {
        if acc >= need {
            break;
        }
        acc = acc.saturating_add(c.size_bytes);
        chosen.push(c);
    }
    if chosen.is_empty() {
        return Ok(DriveQuotaOutcome::default());
    }

    let (freed, zip) = archive_and_unlink(
        quota.id.unwrap_or(0),
        &format!("Quota ổ {}", quota.drive_root),
        "",
        "",
        DEFAULT_QUOTA_RETENTION_DAYS,
        chosen,
        vault_dir,
        state,
        deps,
        ctrl,
    )?;

    let mut outcome = DriveQuotaOutcome {
        bytes_freed: freed,
        vault_zip: Some(zip.clone()),
        ..Default::default()
    };
    if quota.auto_backup {
        match quota.backup_dir.as_deref() {
            Some(dir) => match backup_vault_archive(&zip, Path::new(&quota.drive_root), dir) {
                Ok(dest) => outcome.backed_up_to = Some(dest),
                Err(e) => outcome.backup_problem = Some(e),
            },
            None => outcome.backup_problem = Some("Chưa chọn thư mục sao lưu".to_string()),
        }
    }
    Ok(outcome)
}

/// Computes current physical bytes inside a `QuotaRule`'s `target_paths`.
pub fn measure_quota_rule_usage(rule: &QuotaRule) -> u64 {
    let dummy_ctrl = TaskControl::new();
    let mut files = Vec::new();
    let mut total = 0u64;
    for target in &rule.target_paths {
        if target.exists() {
            let _ = collect_rule_files(target, rule, &dummy_ctrl, &mut files, &mut total);
        }
    }
    total
}

fn collect_rule_files(
    root: &Path,
    rule: &QuotaRule,
    ctrl: &TaskControl,
    out_files: &mut Vec<(PathBuf, u64, SystemTime)>,
    total_bytes: &mut u64,
) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if ctrl.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };

            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
                if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                    continue;
                }
            }
            #[cfg(not(windows))]
            {
                if meta.file_type().is_symlink() {
                    continue;
                }
            }

            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }

            let len = meta.len();
            *total_bytes = total_bytes.saturating_add(len);

            let fname = entry.file_name().to_string_lossy().to_string();
            if is_protected_filename(&fname, Some(&rule.exclude_patterns)) {
                continue;
            }
            let path_norm = path.to_string_lossy().to_lowercase();
            if rule
                .protected_files
                .iter()
                .any(|pf| pf.to_lowercase() == path_norm || fname.eq_ignore_ascii_case(pf))
            {
                continue;
            }

            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out_files.push((path, len, mtime));
        }
    }
    Ok(())
}
