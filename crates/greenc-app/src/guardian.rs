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
use greenc_platform::dev_tools::{DevToolProbe, NativeDevToolProbe};
use greenc_platform::handle::{query_file_identity, NativeFileLockProbe};
use greenc_platform::power_session::{NativePowerSessionApi, PowerSessionApi};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    FileLockProbe, NativeProcessServiceProbe, ProcessServiceProbe, ReparsePointApi, Win32VolumeApi,
};
use greenc_platform::volume::NativeWin32VolumeApi;
use greenc_relocate::recovery::RecoveryService;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::cache_units::{CleanUnit, CleanUnitKind};
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
pub use greenc_vault::guard::backup_vault_archive;
use greenc_vault::guard::{
    enforce_drive_quota_units, run_guard_for_quota, GuardRunOptions, GUARD_AUTO_CLEAN_PROFILE_IDS,
};
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

/// Minimum cooldown between heavy directory-walking `QuotaRule` sweeps (`5 minutes`).
const HEAVY_RULE_SCAN_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// Minimum cooldown between automatic drive-quota cleanup sweeps per drive (`15 minutes`).
const DRIVE_QUOTA_CLEAN_COOLDOWN: Duration = Duration::from_secs(15 * 60);

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
    pub dev_tool_probe: Arc<dyn DevToolProbe>,
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
            dev_tool_probe: Arc::new(NativeDevToolProbe),
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
    last_heavy_rule_scan: Arc<Mutex<Option<Instant>>>,
    quota_alert_state: Arc<Mutex<HashMap<String, (QuotaLevel, Instant)>>>,
    quota_clean_state: Arc<Mutex<HashMap<String, Instant>>>,
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
            last_heavy_rule_scan: Arc::new(Mutex::new(None)),
            quota_alert_state: Arc::new(Mutex::new(HashMap::new())),
            quota_clean_state: Arc::new(Mutex::new(HashMap::new())),
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
            // Never burst missed ticks back-to-back if a cycle takes longer than `tick_interval`.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
        let last_rule_scan_ref = Arc::clone(&self.last_heavy_rule_scan);
        let quota_alert_ref = Arc::clone(&self.quota_alert_state);
        let quota_clean_ref = Arc::clone(&self.quota_clean_state);
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
            let vault_dir = settings
                .vault_dir(&default_vault_dir())
                .unwrap_or_else(|_| default_vault_dir());
            let rollback_engine = RollbackEngine::new(
                &guardian_state.db,
                RestoreOptions {
                    vault_dir: Some(vault_dir.clone()),
                    ..Default::default()
                },
            );
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

            // Check 5: Rolling QuotaRule Enforcement (down to watermark_ratio = 85%), throttled
            // to `HEAVY_RULE_SCAN_COOLDOWN` so dozens of rules do not spin CPU every 45 seconds.
            let should_scan_rules = {
                let guard = last_rule_scan_ref.lock();
                match *guard {
                    Some(prev) => prev.elapsed() >= HEAVY_RULE_SCAN_COOLDOWN,
                    None => true,
                }
            };

            if should_scan_rules {
                let rules = guardian_state.db.list_quota_rules(true).unwrap_or_default();

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
                            tracing::debug!(
                                "QuotaRule '{}' skipped this tick: {err}",
                                rule.app_name
                            );
                        }
                    }
                }

                // Stamp cooldown even after a partial sweep so repeated foreground preemption
                // does not restart scanning from rule #1 every 45 seconds.
                *last_rule_scan_ref.lock() = Some(Instant::now());
            }

            // Check 6: Drive-quota auto-clean (+ auto-backup) via the shared `greenc_vault::guard`
            // core (DB-backed cooldown 30m -> 2h -> 12h, atomic dev cache units, TOCTOU, net bytes).
            for (quota, _total, _free, level) in quota_states {
                let key = quota.drive_root.to_ascii_uppercase();
                if level != QuotaLevel::Critical || !quota.auto_clean {
                    continue;
                }
                let on_clean_cooldown = {
                    let map = quota_clean_ref.lock();
                    map.get(&key)
                        .is_some_and(|at| at.elapsed() < DRIVE_QUOTA_CLEAN_COOLDOWN)
                };
                if on_clean_cooldown {
                    continue;
                }
                if ctrl_worker.is_cancelled() || guardian_state.has_foreground_tasks() {
                    report.preempted_by_foreground = true;
                    break;
                }
                let guard_opts = GuardRunOptions {
                    profile_ids: (*auto_clean_ids).clone(),
                    ..Default::default()
                };
                match run_guard_for_quota(
                    &quota,
                    &vault_dir,
                    &guardian_state.db,
                    deps.volume_api.as_ref(),
                    deps.lock_probe.as_ref(),
                    deps.dev_tool_probe.as_ref(),
                    &guard_opts,
                    &ctrl_worker,
                ) {
                    Ok(out) => {
                        quota_clean_ref.lock().insert(key, Instant::now());
                        if out.logical_bytes > 0 {
                            report.drive_quota_cleanups += 1;
                            report.bytes_reclaimed += out.logical_bytes;
                            let today = chrono::Local::now()
                                .date_naive()
                                .format("%Y-%m-%d")
                                .to_string();
                            let _ = guardian_state.db.record_reclaimed_bytes(
                                &quota.drive_root,
                                &today,
                                out.net_freed_bytes.max(0) as u64,
                            );
                            pending_alerts.push(WatchdogAlertEvent {
                                kind: "drive_quota_cleanup".to_string(),
                                severity: if out.target_reached {
                                    "info"
                                } else {
                                    "warning"
                                }
                                .to_string(),
                                title: format!(
                                    "Đã tự dọn {:.1} GB (thực lợi {:+.1} GB) trên ổ {}",
                                    out.logical_bytes as f64 / 1_073_741_824.0,
                                    out.net_freed_bytes as f64 / 1_073_741_824.0,
                                    quota.drive_root
                                ),
                                detail: if !out.target_reached && !out.reasons.is_empty() {
                                    out.reasons.join("; ")
                                } else {
                                    match (&out.backed_up_to, &out.backup_problem) {
                                        (Some(p), _) => {
                                            format!("Đã sao lưu gói Vault tới {}", p.display())
                                        }
                                        (None, Some(why)) => format!("Chưa sao lưu được: {why}"),
                                        _ => "Dữ liệu được giữ trong Vault, có thể khôi phục."
                                            .to_string(),
                                    }
                                },
                                archive_id: None,
                            });
                        } else if let Some(warn) = out.warning {
                            pending_alerts.push(WatchdogAlertEvent {
                                kind: "drive_quota_cleanup_warning".to_string(),
                                severity: "warning".to_string(),
                                title: format!("Chưa dọn đạt mục tiêu trên ổ {}", quota.drive_root),
                                detail: if out.reasons.is_empty() {
                                    warn
                                } else {
                                    format!("{warn} ({})", out.reasons.join("; "))
                                },
                                archive_id: None,
                            });
                        }
                    }
                    Err(GreenCError::Cancelled) => {
                        report.preempted_by_foreground = true;
                        break;
                    }
                    Err(err) => {
                        quota_clean_ref.lock().insert(key, Instant::now());
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
pub const AUTO_CLEAN_PROFILE_IDS: [&str; 7] = GUARD_AUTO_CLEAN_PROFILE_IDS;

/// Result of one automatic drive-quota enforcement pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriveQuotaOutcome {
    pub bytes_freed: u64,
    pub vault_bytes: u64,
    pub net_freed_bytes: i64,
    pub target_reached: bool,
    pub vault_zip: Option<PathBuf>,
    pub backed_up_to: Option<PathBuf>,
    /// Set when a requested backup could not be made (the cleanup itself still stands).
    pub backup_problem: Option<String>,
    pub reasons: Vec<String>,
}

fn same_drive(a: &Path, b: &Path) -> bool {
    greenc_core::models::is_same_volume(a, b)
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

/// Volume API adapter supplying explicit caller-provided `(total, free)` for a single drive root
/// while delegating all other calls to the inner `Win32VolumeApi`.
struct ExplicitDriveSpaceAdapter<'a> {
    drive_root: PathBuf,
    total_bytes: u64,
    free_bytes: u64,
    inner: &'a dyn Win32VolumeApi,
}

impl Win32VolumeApi for ExplicitDriveSpaceAdapter<'_> {
    fn list_drives(&self) -> Result<Vec<greenc_core::models::DriveInfo>> {
        self.inner.list_drives()
    }

    fn get_disk_free_space(&self, root: &Path) -> Result<(u64, u64)> {
        if same_drive(root, &self.drive_root) {
            Ok((self.total_bytes, self.free_bytes))
        } else {
            self.inner.get_disk_free_space(root)
        }
    }

    fn get_volume_info(&self, root: &Path) -> Result<(String, String, u32)> {
        self.inner.get_volume_info(root)
    }
}

/// Brings a drive back under its quota's `target_ratio` using the shared `greenc_vault::guard`
/// engine, moving eligible files of `pool` into the Canary Vault (or direct-deleting verified
/// regenerable dev caches when enabled), and optionally mirroring the archive to a backup folder.
#[allow(clippy::too_many_arguments)]
pub fn enforce_drive_quota(
    quota: &DriveQuota,
    total_bytes: u64,
    free_bytes: u64,
    pool: Vec<CleanCandidate>,
    vault_dir: &Path,
    state: &AppState,
    deps: &GuardianDeps,
    ctrl: &TaskControl,
) -> Result<DriveQuotaOutcome> {
    let units: Vec<CleanUnit> = pool
        .into_iter()
        .filter(|c| !c.is_locked && !c.is_orphan)
        .map(|c| {
            let root_path = c.path.clone();
            let allowed_parent = root_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(&quota.drive_root));
            let total_bytes = c.size_bytes;
            let last_used = c.mtime;
            let profile_id = c.profile_id.clone();
            let root_identity = query_file_identity(&root_path);
            CleanUnit {
                root_path,
                allowed_parent,
                files: vec![c],
                total_bytes,
                last_used,
                profile_id,
                kind: CleanUnitKind::StandardFile,
                dev_tool: None,
                unit_label: "standard_file".to_string(),
                root_identity,
                min_age: Duration::ZERO,
            }
        })
        .collect();

    let adapter = ExplicitDriveSpaceAdapter {
        drive_root: PathBuf::from(&quota.drive_root),
        total_bytes,
        free_bytes,
        inner: deps.volume_api.as_ref(),
    };
    let opts = GuardRunOptions {
        ignore_cooldown: true,
        ..Default::default()
    };
    let out = enforce_drive_quota_units(
        quota,
        units,
        vault_dir,
        &state.db,
        &adapter,
        deps.lock_probe.as_ref(),
        deps.dev_tool_probe.as_ref(),
        &opts,
        ctrl,
    )?;

    Ok(DriveQuotaOutcome {
        bytes_freed: out.logical_bytes,
        vault_bytes: out.vault_bytes,
        net_freed_bytes: out.net_freed_bytes,
        target_reached: out.target_reached,
        vault_zip: out.vault_zip,
        backed_up_to: out.backed_up_to,
        backup_problem: out.backup_problem,
        reasons: out.reasons,
    })
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
