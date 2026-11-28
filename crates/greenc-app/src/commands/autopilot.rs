//! Goal-Driven 5-Tier "Make Green Now" Autopilot Orchestrator (`make_green_now`)
//! plus Tier 4 External USB Backup & Tier 5 App Uninstaller commands.
//!
//! Tier Pipeline (stops early as soon as drive goal is reached, unless `sweep_mode = "max_space"`):
//! - Tier 0 (`analyze`): Compute exact `bytes_needed` from the user's drive goal (`clean_ratio` / `target_ratio`).
//! - Tier 1 (`tier1`):   Pure disposable junk (`windows_temp`, `crash_dumps`, `app_logs`, `old_versions`, `orphan_radar`).
//! - Tier 2 (`tier2`):   Shallow, 100%-confident app caches ordered **least-recently-used first**.
//! - Tier 3 (`tier3`):   Deep per-app pass (WOF XPRESS8K compaction + curated relocate to backup drive), **least-recently-used first**.
//! - Tier 4 (`tier4`):   If still short of goal -> detect USB / external drives & heavy folders for verified ZIP backup (NEVER NTFS junctions).
//! - Tier 5 (`tier5`):   If still short of goal -> list installed apps ranked **least-recently-used & largest first** with 1-click official uninstaller.

use crate::state::{AppState, TaskProgressEvent};
use chrono::Utc;
use greenc_core::config::{default_vault_dir, MIN_VAULT_RESERVE_BYTES};
use greenc_core::models::{CleanCandidate, RegenerableCacheMode};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{AppLifecycleApi, AppShutdownSession, NativeAppLifecycleApi};
use greenc_platform::traits::{
    NativeCompactionApi, NativeFileLockProbe, NativeProcessServiceProbe, NativeReparsePointApi,
    NativeShellFolderApi, NativeWin32VolumeApi, ProcessServiceProbe, Win32VolumeApi, WofAlgorithm,
};
use greenc_platform::volume::{list_drives_native, recommend_drive_info};
use greenc_relocate::autopilot::{
    collect_folder_backup_candidates, estimate_last_used_epoch,
    filter_non_overlapping_compaction_dirs, find_external_backup_drives, inactive_days_from_epoch,
    select_safe_relocation_candidates, sort_least_used_first, tier1_pure_junk_profiles,
    tier2_shallow_app_profiles, ExternalBackupDrive, GoalBudget, LockedAppSummary, SweepMode,
    MIN_EXTERNAL_BACKUP_FREE_BYTES,
};
use greenc_relocate::candidate::CandidateScanner;
use greenc_relocate::compress::InPlaceCompactor;
use greenc_relocate::known_folders::KnownFolderService;
use greenc_relocate::mover::{MoveOptions, TransactionalMover};
use greenc_relocate::zalo_move::{is_zalo_auto_move_supported, ZaloMoveOptions, ZaloRelocator};
use greenc_scanner::installed_apps::{discover_unused_apps, InstalledAppSuggestion};
use greenc_scanner::zalo::{discover_zalo_installation, ZaloDiscoveryOptions, ZALO_PROCESS_NAMES};
use greenc_storage::SavingsEventInput;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use greenc_vault::profiles::{group_candidates_by_app, PresetProfiles};
use greenc_vault::zalo_cleanup::{execute_zalo_cleanup, ZaloCleanupOptions};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tauri::State;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MakeGreenOptionsDto {
    pub target_drive: Option<String>,
    /// `"direct_cache"` or `"local_vault"` when no secondary backup drive exists.
    pub single_drive_mode: Option<String>,
    /// `"goal"` (default — stop when goal reached), `"light"` (Tier 1 pure junk only),
    /// or `"max_space"` (run Tiers 1-3 without early-stop).
    pub sweep_mode: Option<String>,
    /// Optional app IDs from a previous report's `skipped_locked` explicitly confirmed for close & finish.
    #[serde(default)]
    pub close_app_ids: Vec<String>,
    /// Must be `true` to allow closing running/tray-resident apps in `close_app_ids`.
    pub confirm_close_running_apps: Option<bool>,
    /// Whether to include non-OneDrive Known Folders in auto-move (default `true`).
    pub include_known_folders: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeGreenActionItemDto {
    pub name: String,
    pub action_kind: String, // "clean" | "compact" | "relocate"
    pub bytes_freed: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalBackupCandidateDto {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub inactive_days: u64,
    pub kind: String, // "known_folder" | "app_data"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalBackupReportDto {
    pub task_id: String,
    pub target_root: String,
    pub backup_dir: String,
    pub backed_up_count: usize,
    pub freed_bytes: u64,
    pub archive_ids: Vec<String>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakeGreenReportDto {
    pub task_id: String,
    pub before_used_bytes: u64,
    pub after_used_bytes: u64,
    pub before_free_bytes: u64,
    pub after_free_bytes: u64,
    pub total_bytes: u64,
    pub cleaned_bytes: u64,
    pub compacted_bytes: u64,
    pub relocated_bytes: u64,
    pub total_freed_bytes: u64,
    pub backup_drive: Option<String>,
    pub single_drive_mode: Option<String>,
    pub sweep_mode: String,
    pub goal_target_bytes: u64,
    pub goal_reached: bool,
    pub remaining_bytes_needed: u64,
    pub stopped_at_tier: u8,
    pub external_drives: Vec<ExternalBackupDrive>,
    pub external_backup_candidates: Vec<ExternalBackupCandidateDto>,
    pub uninstall_suggestions: Vec<InstalledAppSuggestion>,
    pub actions: Vec<MakeGreenActionItemDto>,
    pub skipped_locked: Vec<LockedAppSummary>,
    pub reopened_apps: Vec<String>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

fn known_process_names_for_app_id(app_id: &str) -> Vec<String> {
    if app_id.eq_ignore_ascii_case("zalo") {
        return ZALO_PROCESS_NAMES.iter().map(|s| s.to_string()).collect();
    }
    for entry in greenc_relocate::catalog::get_app_catalog() {
        if entry.app_id.eq_ignore_ascii_case(app_id) {
            return entry.process_names;
        }
    }
    match app_id.to_lowercase().as_str() {
        "chrome" | "google chrome" => vec!["chrome.exe".to_string()],
        "edge" | "microsoft edge" => vec!["msedge.exe".to_string()],
        "brave" | "brave browser" => vec!["brave.exe".to_string()],
        "firefox" | "mozilla firefox" => vec!["firefox.exe".to_string()],
        "teams" | "microsoft teams" => vec!["ms-teams.exe".to_string(), "teams.exe".to_string()],
        "whatsapp" => vec!["whatsapp.exe".to_string()],
        _ => Vec::new(),
    }
}

struct CleanupTierContext<'a> {
    state_inner: &'a AppState,
    task_id: &'a str,
    task_ctrl: &'a TaskControl,
    vol_api: &'a NativeWin32VolumeApi,
    lock_probe: &'a NativeFileLockProbe,
    vault_dir: &'a Path,
    sys_letter: char,
    use_direct_cache: bool,
    budget: GoalBudget,
    phase_key: &'a str,
    pct_start: u32,
    pct_span: u32,
    detail_prefix: &'a str,
}

/// Archives (if not `use_direct_cache`) and safely unlinks grouped cleanup candidates,
/// checking `budget.is_reached(*freed_so_far)` after EVERY app group so we stop as soon as the goal is met.
#[allow(clippy::too_many_arguments)]
fn run_cleanup_groups(
    groups: Vec<(String, String, Vec<CleanCandidate>)>,
    ctx: &CleanupTierContext<'_>,
    freed_so_far: &mut u64,
    cleaned_bytes: &mut u64,
    actions: &mut Vec<MakeGreenActionItemDto>,
    warnings: &mut Vec<String>,
) {
    if groups.is_empty() || ctx.task_ctrl.is_cancelled() || ctx.budget.is_reached(*freed_so_far) {
        return;
    }

    let total_group_bytes: u64 = groups
        .iter()
        .flat_map(|(_, _, items)| items.iter())
        .map(|c| c.size_bytes)
        .fold(0u64, u64::saturating_add);
    let mut done_group_bytes = 0u64;

    let mut snapshots = HashMap::new();
    for (_, _, items) in &groups {
        for c in items {
            let snap = match std::fs::metadata(&c.path) {
                Ok(m) => (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                Err(_) => (c.size_bytes, c.mtime),
            };
            snapshots.insert(c.path.clone(), snap);
            for sib in &c.hardlink_siblings {
                snapshots.entry(sib.clone()).or_insert(snap);
            }
        }
    }

    let cleaner = SafeCleaner::new(
        ctx.lock_probe,
        ctx.vol_api,
        None,
        CleanupOptions {
            deletion_method: if ctx.use_direct_cache {
                DeletionMethod::Permanent
            } else {
                DeletionMethod::RecycleBin
            },
            min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
            prune_empty_directories: true,
        },
    );

    let pipeline = ArchiverPipeline::new(ArchiveOptions {
        vault_dir: ctx.vault_dir.to_path_buf(),
        min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
        ..Default::default()
    });

    for (app_name, category, group_items) in groups {
        if ctx.task_ctrl.is_cancelled() || ctx.budget.is_reached(*freed_so_far) {
            break;
        }
        let mut archive_ref = ctx.task_id.to_string();
        let mut source_kind = "cleanup_direct";
        let mut manifest_items_opt = None;

        if !ctx.use_direct_cache {
            let st_prog = ctx.state_inner.clone();
            let tid_clone = ctx.task_id.to_string();
            let phase_str = ctx.phase_key.to_string();
            let pct_start = ctx.pct_start;
            let pct_span = ctx.pct_span;
            let base_b = done_group_bytes;
            let app_label = app_name.clone();
            match pipeline.archive_candidates(
                0,
                &app_name,
                &group_items,
                ctx.vol_api,
                ctx.task_ctrl,
                move |p| {
                    let cur = base_b.saturating_add(p.done_bytes);
                    let pct = pct_start
                        + (((cur as f64 / total_group_bytes.max(1) as f64) * pct_span as f64)
                            as u32);
                    st_prog.emit_progress(&TaskProgressEvent {
                        task_id: tid_clone.clone(),
                        kind: "make_green".to_string(),
                        state: "running".to_string(),
                        percent: pct.min(pct_start + pct_span),
                        phase: phase_str.clone(),
                        title: format!("Đang dọn & sao lưu {app_label}"),
                        detail: p.current_file,
                        done_files: p.done_files,
                        total_files: p.total_files,
                        done_bytes: cur,
                        total_bytes: total_group_bytes,
                        throughput_bps: p.throughput_bps,
                        eta_seconds: p.eta_seconds,
                    });
                },
            ) {
                Ok(manifest) => {
                    let zip_p = ctx.vault_dir.join(format!("{}.zip", manifest.archive_id));
                    let man_p = ctx
                        .vault_dir
                        .join(format!("{}.manifest.json", manifest.archive_id));
                    let _ = manifest.persist_to_db(&ctx.state_inner.db, &zip_p, &man_p);
                    archive_ref = manifest.archive_id;
                    source_kind = "cleanup_vault";
                    manifest_items_opt = Some(manifest.files);
                }
                Err(e) => {
                    warnings.push(format!("Bỏ qua sao lưu {app_name}: {e}"));
                    continue;
                }
            }
        }

        let manifest_items: Vec<greenc_core::models::FileManifestItem> = manifest_items_opt
            .unwrap_or_else(|| {
                group_items
                    .iter()
                    .enumerate()
                    .map(|(i, c)| greenc_core::models::FileManifestItem {
                        id: None,
                        archive_id: None,
                        original_path: c.path.clone(),
                        rel_arcname: format!("item_{:05}", i + 1),
                        size_bytes: c.size_bytes,
                        sha256: String::new(),
                        mtime: 0.0,
                        atime: 0.0,
                        hardlink_paths: c.hardlink_siblings.clone(),
                        restored_individually: false,
                    })
                    .collect()
            });

        if let Ok(clean_res) = cleaner.unlink_archived_items(&manifest_items, &snapshots) {
            if clean_res.bytes_freed > 0 {
                *cleaned_bytes = cleaned_bytes.saturating_add(clean_res.bytes_freed);
                *freed_so_far = freed_so_far.saturating_add(clean_res.bytes_freed);
                let now_utc = Utc::now();
                let _ = ctx.state_inner.db.record_savings_event(&SavingsEventInput {
                    timestamp: now_utc.to_rfc3339(),
                    event_date: now_utc.format("%Y-%m-%d").to_string(),
                    source_kind: source_kind.to_string(),
                    source_ref: archive_ref,
                    app_name: app_name.clone(),
                    category,
                    bytes_freed: clean_res.bytes_freed,
                    drive_letter: ctx.sys_letter.to_string(),
                });
                actions.push(MakeGreenActionItemDto {
                    name: app_name,
                    action_kind: "clean".to_string(),
                    bytes_freed: clean_res.bytes_freed,
                    detail: Some(format!(
                        "{} · {} tệp",
                        ctx.detail_prefix, clean_res.files_deleted
                    )),
                });
            }
        }

        done_group_bytes =
            done_group_bytes.saturating_add(group_items.iter().map(|c| c.size_bytes).sum::<u64>());
    }
}

fn record_locked_candidates(
    locked_cands: &[CleanCandidate],
    skipped_locked: &mut Vec<LockedAppSummary>,
) {
    if locked_cands.is_empty() {
        return;
    }
    for (app_name, _cat, items) in group_candidates_by_app(locked_cands) {
        let bytes: u64 = items.iter().map(|c| c.size_bytes).sum();
        if bytes > 1024 * 1024 {
            let procs = known_process_names_for_app_id(&app_name);
            if !procs.is_empty()
                && !skipped_locked
                    .iter()
                    .any(|s| s.app_name.eq_ignore_ascii_case(&app_name))
            {
                skipped_locked.push(LockedAppSummary {
                    app_id: app_name.to_lowercase().replace(' ', "_"),
                    app_name: app_name.clone(),
                    size_bytes: bytes,
                    process_names: procs,
                    reason: format!("{app_name} đang mở nên một số tệp đệm chưa dọn được"),
                });
            }
        }
    }
}

/// Runs the Goal-Driven 5-Tier "Make Green Now" sweep in a single background-capable task.
#[tauri::command]
pub async fn make_green_now(
    options: Option<MakeGreenOptionsDto>,
    state: State<'_, AppState>,
) -> std::result::Result<MakeGreenReportDto, String> {
    let opts = options.unwrap_or_default();
    let task_id = format!("make-green-{}", Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let vol_api = NativeWin32VolumeApi;
        let shell_api = NativeShellFolderApi;
        let lock_probe = NativeFileLockProbe;
        let proc_probe = NativeProcessServiceProbe;
        let reparse_api = NativeReparsePointApi;
        let compaction_api = NativeCompactionApi;
        let lifecycle = NativeAppLifecycleApi;

        let emit = |percent: u32,
                    phase: &str,
                    title: &str,
                    detail: String,
                    done_b: u64,
                    total_b: u64| {
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_worker.clone(),
                kind: "make_green".to_string(),
                state: "running".to_string(),
                percent,
                phase: phase.to_string(),
                title: title.to_string(),
                detail,
                done_files: 0,
                total_files: 0,
                done_bytes: done_b,
                total_bytes: total_b,
                throughput_bps: 0,
                eta_seconds: 0,
            });
        };

        // ── Tier 0: Analyze & Compute Goal Budget ────────────────────────
        emit(
            3,
            "analyze",
            "Đang phân tích mục tiêu dung lượng ổ C:...",
            "Tính toán lượng dung lượng cần giải phóng...".to_string(),
            0,
            0,
        );

        let drives = list_drives_native().unwrap_or_default();
        let sys_drive = drives
            .iter()
            .find(|d| d.is_system)
            .or_else(|| drives.first())
            .cloned();
        let sys_letter = sys_drive.as_ref().map(|d| d.drive_letter).unwrap_or('C');
        let sys_root = sys_drive
            .as_ref()
            .map(|d| d.root.clone())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    PathBuf::from(format!("{sys_letter}:\\"))
                } else {
                    PathBuf::from("/")
                }
            });
        let (total_bytes, before_free_bytes) = vol_api
            .get_disk_free_space(&sys_root)
            .unwrap_or_else(|_| {
                sys_drive
                    .as_ref()
                    .map(|d| (d.total_bytes, d.free_bytes))
                    .unwrap_or((0, 0))
            });
        let before_used_bytes = total_bytes.saturating_sub(before_free_bytes);

        // Read user's configured drive goal (default clean_ratio=0.80, target_ratio=0.75)
        let quotas = state_inner.db.list_drive_quotas(false).unwrap_or_default();
        let active_quota = quotas
            .iter()
            .find(|q| {
                q.enabled
                    && q.drive_root
                        .chars()
                        .next()
                        .is_some_and(|c| c.eq_ignore_ascii_case(&sys_letter))
            })
            .or_else(|| quotas.iter().find(|q| q.enabled))
            .or_else(|| quotas.first());
        let clean_ratio = active_quota.map(|q| q.clean_ratio).unwrap_or(0.80);
        let target_ratio = active_quota.map(|q| q.target_ratio).unwrap_or(0.75);
        let sweep_mode = SweepMode::parse(opts.sweep_mode.as_deref());
        let budget = GoalBudget::compute(
            total_bytes,
            before_used_bytes,
            clean_ratio,
            target_ratio,
            sweep_mode,
        );

        // Resolve backup drive (explicit > configured data_root > recommended fixed NTFS)
        let configured_root = state_inner.settings().get_data_root().ok().flatten();
        let configured_drive = configured_root
            .as_ref()
            .and_then(|p| p.to_string_lossy().chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| {
                drives
                    .iter()
                    .any(|d| d.drive_letter == *c && d.is_eligible && !d.is_system)
            });

        let explicit_drive = opts
            .target_drive
            .as_deref()
            .and_then(|s| s.chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| {
                drives
                    .iter()
                    .any(|d| d.drive_letter == *c && d.is_eligible && !d.is_system)
            });

        let backup_drive: Option<char> = explicit_drive.or(configured_drive).or_else(|| {
            recommend_drive_info(&drives)
                .filter(|d| d.is_eligible && !d.is_system)
                .map(|d| d.drive_letter)
        });

        let mut warnings: Vec<String> = Vec::new();
        let mut actions: Vec<MakeGreenActionItemDto> = Vec::new();
        let mut skipped_locked: Vec<LockedAppSummary> = Vec::new();
        let mut closed_sessions: Vec<(String, AppShutdownSession)> = Vec::new();

        let vault_dir = if let Some(bdrv) = backup_drive {
            let root = configured_root
                .filter(|p| {
                    p.to_string_lossy()
                        .chars()
                        .next()
                        .is_some_and(|c| c.to_ascii_uppercase() == bdrv)
                })
                .unwrap_or_else(|| {
                    if cfg!(windows) {
                        PathBuf::from(format!("{bdrv}:\\GreenC"))
                    } else {
                        drives
                            .iter()
                            .find(|d| d.drive_letter == bdrv)
                            .map(|d| d.root.join(greenc_core::config::DATA_FOLDER_NAME))
                            .unwrap_or_else(greenc_core::config::app_home)
                    }
                });
            let _ = std::fs::create_dir_all(root.join("Vault"));
            let _ = std::fs::create_dir_all(root.join("Apps"));
            let _ = std::fs::create_dir_all(root.join("Moving"));
            let _ = state_inner.settings().set_data_root(&root);
            root.join("Vault")
        } else {
            state_inner
                .settings()
                .vault_dir(&default_vault_dir())
                .unwrap_or_else(|_| default_vault_dir())
        };

        // Close explicitly confirmed running apps from a previous report (never silent!)
        let confirmed_close = opts.confirm_close_running_apps.unwrap_or(false);
        let close_set: HashSet<String> = opts
            .close_app_ids
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        if confirmed_close && !close_set.is_empty() {
            for app_id in &opts.close_app_ids {
                if app_id.eq_ignore_ascii_case("zalo")
                    && lifecycle.has_visible_window(&["ZaloCall.exe"])
                {
                    warnings.push(
                        "Zalo đang có cuộc gọi nên GreenC không đóng để tránh gián đoạn."
                            .to_string(),
                    );
                    continue;
                }
                let procs = known_process_names_for_app_id(app_id);
                if procs.is_empty() {
                    continue;
                }
                let refs: Vec<&str> = procs.iter().map(String::as_str).collect();
                emit(
                    6,
                    "analyze",
                    "Đang đóng ứng dụng đã xác nhận...",
                    app_id.clone(),
                    0,
                    0,
                );
                match lifecycle.shutdown_confirmed_apps(&refs, Duration::from_secs(10)) {
                    Ok(session) if !session.captured_exes.is_empty() => {
                        closed_sessions.push((app_id.clone(), session));
                    }
                    Ok(_) => {}
                    Err(e) => warnings.push(format!("Không thể tự đóng {app_id}: {e}")),
                }
            }
        }

        let use_direct_cache =
            backup_drive.is_none() && opts.single_drive_mode.as_deref() == Some("direct_cache");

        let mut cleaned_bytes = 0u64;
        let mut compacted_bytes = 0u64;
        let mut relocated_bytes = 0u64;
        let mut freed_so_far = 0u64;
        let mut stopped_at_tier: u8 = 1;

        // ── Tier 1: Pure Junk (least important, genuinely disposable) ────
        if !task_ctrl.is_cancelled() {
            stopped_at_tier = 1;
            emit(
                10,
                "tier1",
                "Tầng 1: Đang dọn rác thuần túy (Temp, Crash Dumps, Log cũ)...",
                "Quét tệp rác ít quan trọng nhất...".to_string(),
                0,
                budget.bytes_needed,
            );

            let t1_profiles =
                tier1_pure_junk_profiles(backup_drive.is_some(), opts.single_drive_mode.as_deref());
            let mut t1_cands = Vec::new();
            for pid in &t1_profiles {
                if task_ctrl.is_cancelled() {
                    break;
                }
                t1_cands.extend(PresetProfiles::scan_profile_candidates(
                    pid,
                    &task_ctrl,
                    &lock_probe,
                ));
            }
            let (t1_unlocked, t1_locked): (Vec<_>, Vec<_>) =
                t1_cands.into_iter().partition(|c| !c.is_locked);
            record_locked_candidates(&t1_locked, &mut skipped_locked);

            let mut t1_groups = group_candidates_by_app(&t1_unlocked);
            t1_groups.sort_by_key(|(_, _, items)| {
                std::cmp::Reverse(items.iter().map(|c| c.size_bytes).sum::<u64>())
            });

            let t1_ctx = CleanupTierContext {
                state_inner: &state_inner,
                task_id: &task_id_worker,
                task_ctrl: &task_ctrl,
                vol_api: &vol_api,
                lock_probe: &lock_probe,
                vault_dir: &vault_dir,
                sys_letter,
                use_direct_cache,
                budget,
                phase_key: "tier1",
                pct_start: 12,
                pct_span: 16,
                detail_prefix: "Tầng 1 · Rác thuần túy",
            };
            run_cleanup_groups(
                t1_groups,
                &t1_ctx,
                &mut freed_so_far,
                &mut cleaned_bytes,
                &mut actions,
                &mut warnings,
            );
        }

        // ── Tier 2: Shallow High-Confidence App Caches (Least-Used First) ─
        if !task_ctrl.is_cancelled()
            && sweep_mode != SweepMode::Light
            && !budget.is_reached(freed_so_far)
        {
            stopped_at_tier = 2;
            emit(
                30,
                "tier2",
                "Tầng 2: Đang dọn bộ nhớ đệm từ ứng dụng ít dùng nhất...",
                "Xếp hạng ứng dụng ít sử dụng gần đây...".to_string(),
                freed_so_far,
                budget.bytes_needed,
            );

            let t2_profiles = tier2_shallow_app_profiles();
            let mut t2_cands = Vec::new();
            for pid in &t2_profiles {
                if task_ctrl.is_cancelled() {
                    break;
                }
                t2_cands.extend(PresetProfiles::scan_profile_candidates(
                    pid,
                    &task_ctrl,
                    &lock_probe,
                ));
            }
            let (t2_unlocked, t2_locked): (Vec<_>, Vec<_>) =
                t2_cands.into_iter().partition(|c| !c.is_locked);
            record_locked_candidates(&t2_locked, &mut skipped_locked);

            // Order Tier 2 app groups LEAST-RECENTLY-USED first (tie-break: largest first)
            type CleanupAppGroup = (String, String, Vec<CleanCandidate>);
            let mut keyed_t2: Vec<(u64, u64, CleanupAppGroup)> =
                group_candidates_by_app(&t2_unlocked)
                    .into_iter()
                    .map(|g| {
                        let sample_paths: Vec<PathBuf> =
                            g.2.iter().take(24).map(|c| c.path.clone()).collect();
                        let last_ts = estimate_last_used_epoch(&sample_paths);
                        let sz: u64 = g.2.iter().map(|c| c.size_bytes).sum();
                        (last_ts, sz, g)
                    })
                    .collect();
            sort_least_used_first(&mut keyed_t2, |(ts, sz, _)| (*ts, *sz));
            let t2_groups = keyed_t2.into_iter().map(|(_, _, g)| g).collect();

            let t2_ctx = CleanupTierContext {
                state_inner: &state_inner,
                task_id: &task_id_worker,
                task_ctrl: &task_ctrl,
                vol_api: &vol_api,
                lock_probe: &lock_probe,
                vault_dir: &vault_dir,
                sys_letter,
                use_direct_cache,
                budget,
                phase_key: "tier2",
                pct_start: 32,
                pct_span: 18,
                detail_prefix: "Tầng 2 · Cache an toàn",
            };
            run_cleanup_groups(
                t2_groups,
                &t2_ctx,
                &mut freed_so_far,
                &mut cleaned_bytes,
                &mut actions,
                &mut warnings,
            );

            // Zalo safe cache & old-versions cleanup (only if still short of goal)
            if !task_ctrl.is_cancelled() && !budget.is_reached(freed_so_far) {
                let running_exe_dirs = lifecycle.find_running_exe_dirs(ZALO_PROCESS_NAMES);
                let zalo_running = !running_exe_dirs.is_empty()
                    || ZALO_PROCESS_NAMES
                        .iter()
                        .any(|p| proc_probe.is_process_running(p));
                let known_relocated_sources: HashSet<PathBuf> = state_inner
                    .db
                    .list_relocations(Some("done"))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.app_id == "zalo")
                    .map(|r| r.source_path)
                    .collect();

                if let Some(disc) = discover_zalo_installation(&ZaloDiscoveryOptions {
                    running_exe_dirs,
                    known_relocated_sources,
                    ..Default::default()
                }) {
                    let retention_days = state_inner.settings().get_retention_days().unwrap_or(14);
                    let zmode = if use_direct_cache {
                        RegenerableCacheMode::Direct
                    } else {
                        RegenerableCacheMode::Vault
                    };
                    if let Ok(zrep) = execute_zalo_cleanup(
                        &disc,
                        &vault_dir,
                        &state_inner.db,
                        &vol_api,
                        &lock_probe,
                        zalo_running,
                        &ZaloCleanupOptions {
                            dry_run: false,
                            mode: zmode,
                            retention_days,
                            ..Default::default()
                        },
                        &task_ctrl,
                    ) {
                        if zrep.logical_freed_bytes > 0 {
                            cleaned_bytes =
                                cleaned_bytes.saturating_add(zrep.logical_freed_bytes);
                            freed_so_far = freed_so_far.saturating_add(zrep.logical_freed_bytes);
                            let now_utc = Utc::now();
                            let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                                timestamp: now_utc.to_rfc3339(),
                                event_date: now_utc.format("%Y-%m-%d").to_string(),
                                source_kind: if zrep.vault_archive_id.is_some() {
                                    "cleanup_vault".to_string()
                                } else {
                                    "cleanup_direct".to_string()
                                },
                                source_ref: zrep
                                    .vault_archive_id
                                    .unwrap_or_else(|| task_id_worker.clone()),
                                app_name: "Zalo PC".to_string(),
                                category: "chat".to_string(),
                                bytes_freed: zrep.logical_freed_bytes,
                                drive_letter: sys_letter.to_string(),
                            });
                            actions.push(MakeGreenActionItemDto {
                                name: "Zalo PC".to_string(),
                                action_kind: "clean".to_string(),
                                bytes_freed: zrep.logical_freed_bytes,
                                detail: Some("Tầng 2 · Bộ đệm & phiên bản cũ".to_string()),
                            });
                        }
                    }
                }
            }
        }

        // ── Tier 3: Deep Per-App Pass (WOF Compact + Relocate, LRU First) ──
        let mut relocated_paths_done: HashSet<String> = HashSet::new();
        if !task_ctrl.is_cancelled()
            && sweep_mode != SweepMode::Light
            && !budget.is_reached(freed_so_far)
        {
            stopped_at_tier = 3;

            // Pre-plan relocation targets (already LRU-ordered) so compaction skips overlapping dirs
            let include_kf = opts.include_known_folders.unwrap_or(true);
            let (mut planned_kf, planned_reloc, mut reloc_locked) = if let Some(bdrv) = backup_drive
            {
                let kf_list = if include_kf && !cfg!(feature = "store") {
                    KnownFolderService::list_known_folders(bdrv, &shell_api)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|kf| !kf.is_onedrive && !kf.is_redirected && kf.size_bytes > 0)
                        .collect()
                } else {
                    Vec::new()
                };
                let discovered = CandidateScanner::discover_all(
                    bdrv,
                    &vol_api,
                    &shell_api,
                    &lock_probe,
                    &proc_probe,
                );
                let (ready, locked) = select_safe_relocation_candidates(discovered);
                (kf_list, ready, locked)
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };
            skipped_locked.append(&mut reloc_locked);

            // Sort Known Folders least-recently-used first
            sort_least_used_first(&mut planned_kf, |kf| {
                (
                    estimate_last_used_epoch(std::slice::from_ref(&kf.current_path)),
                    kf.size_bytes,
                )
            });

            let planned_reloc_paths: Vec<PathBuf> = planned_kf
                .iter()
                .map(|k| k.current_path.clone())
                .chain(planned_reloc.iter().map(|c| c.source_path.clone()))
                .collect();

            // 3a. In-place WOF Compaction (non-overlapping, LRU first, early-stop on goal)
            emit(
                52,
                "tier3",
                "Tầng 3: Đang dọn sâu — nén gọn thư mục ít dùng...",
                "Rà soát các thư mục có thể nén không mất dữ liệu...".to_string(),
                freed_so_far,
                budget.bytes_needed,
            );
            let mut dirs = filter_non_overlapping_compaction_dirs(
                InPlaceCompactor::discover_directories_for_compaction(),
                &planned_reloc_paths,
            );
            sort_least_used_first(&mut dirs, |d| {
                (
                    estimate_last_used_epoch(std::slice::from_ref(&d.path)),
                    d.logical_bytes,
                )
            });

            let total_dirs = dirs.len().max(1);
            for (idx, d) in dirs.into_iter().enumerate() {
                if task_ctrl.is_cancelled() || budget.is_reached(freed_so_far) {
                    break;
                }
                let dir_name = d
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Folder")
                    .to_string();
                let pct = 54 + (((idx * 14) / total_dirs) as u32);
                emit(
                    pct.min(68),
                    "tier3",
                    "Tầng 3: Đang nén gọn thư mục ít dùng...",
                    dir_name.clone(),
                    freed_so_far,
                    budget.bytes_needed.max(d.logical_bytes),
                );
                if let Ok(rec) = InPlaceCompactor::compact_directory(
                    &d.path,
                    WofAlgorithm::Xpress8k,
                    &state_inner.db,
                    &compaction_api,
                    &lock_probe,
                    &task_ctrl,
                    |_, _| {},
                ) {
                    let saved = rec
                        .physical_before_bytes
                        .saturating_sub(rec.physical_after_bytes);
                    if saved > 0 {
                        compacted_bytes = compacted_bytes.saturating_add(saved);
                        freed_so_far = freed_so_far.saturating_add(saved);
                        let now_utc = Utc::now();
                        let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                            timestamp: now_utc.to_rfc3339(),
                            event_date: now_utc.format("%Y-%m-%d").to_string(),
                            source_kind: "compact".to_string(),
                            source_ref: task_id_worker.clone(),
                            app_name: dir_name.clone(),
                            category: "compact".to_string(),
                            bytes_freed: saved,
                            drive_letter: sys_letter.to_string(),
                        });
                        actions.push(MakeGreenActionItemDto {
                            name: dir_name,
                            action_kind: "compact".to_string(),
                            bytes_freed: saved,
                            detail: Some("Tầng 3 · Nén gọn tại chỗ".to_string()),
                        });
                    }
                }
            }

            // 3b. Safe Curated Auto-Move to Backup Drive (LRU first, early-stop on goal)
            if let Some(bdrv) = backup_drive {
                if !task_ctrl.is_cancelled() && !budget.is_reached(freed_so_far) {
                    let total_move_bytes: u64 = planned_kf
                        .iter()
                        .map(|k| k.size_bytes)
                        .chain(planned_reloc.iter().map(|c| c.size_bytes))
                        .fold(0u64, u64::saturating_add);
                    let mut moved_so_far = 0u64;

                    #[cfg(not(feature = "store"))]
                    for kf in planned_kf {
                        if task_ctrl.is_cancelled() || budget.is_reached(freed_so_far) {
                            break;
                        }
                        let st_prog = state_inner.clone();
                        let tid = task_id_worker.clone();
                        let kf_name = kf.name.clone();
                        let base_b = moved_so_far;
                        let mut last_emit = Instant::now()
                            .checked_sub(Duration::from_millis(200))
                            .unwrap_or_else(Instant::now);

                        match KnownFolderService::relocate_known_folder_with_file(
                            &kf.folder_id,
                            bdrv,
                            None,
                            &state_inner.db,
                            &shell_api,
                            &vol_api,
                            &task_ctrl,
                            move |done_b, done_f, rel_file| {
                                if last_emit.elapsed() < Duration::from_millis(100) {
                                    return;
                                }
                                last_emit = Instant::now();
                                let cur = base_b.saturating_add(done_b);
                                let pct = 70
                                    + (((cur as f64 / total_move_bytes.max(1) as f64) * 20.0)
                                        as u32);
                                st_prog.emit_progress(&TaskProgressEvent {
                                    task_id: tid.clone(),
                                    kind: "make_green".to_string(),
                                    state: "running".to_string(),
                                    percent: pct.min(90),
                                    phase: "tier3".to_string(),
                                    title: format!(
                                        "Tầng 3: Đang chuyển {kf_name} sang {bdrv}:\\"
                                    ),
                                    detail: rel_file.to_string(),
                                    done_files: done_f,
                                    total_files: kf.file_count,
                                    done_bytes: cur,
                                    total_bytes: total_move_bytes,
                                    throughput_bps: 0,
                                    eta_seconds: 0,
                                });
                            },
                        ) {
                            Ok(rec) => {
                                relocated_paths_done
                                    .insert(kf.current_path.to_string_lossy().to_lowercase());
                                relocated_bytes = relocated_bytes.saturating_add(rec.size_bytes);
                                freed_so_far = freed_so_far.saturating_add(rec.size_bytes);
                                if rec.size_bytes > 0 {
                                    let now_utc = Utc::now();
                                    let _ = state_inner.db.record_savings_event(
                                        &SavingsEventInput {
                                            timestamp: now_utc.to_rfc3339(),
                                            event_date: now_utc.format("%Y-%m-%d").to_string(),
                                            source_kind: "relocate".to_string(),
                                            source_ref: rec
                                                .id
                                                .map(|i| i.to_string())
                                                .unwrap_or_default(),
                                            app_name: rec.app_name.clone(),
                                            category: "relocate".to_string(),
                                            bytes_freed: rec.size_bytes,
                                            drive_letter: sys_letter.to_string(),
                                        },
                                    );
                                    actions.push(MakeGreenActionItemDto {
                                        name: kf.name,
                                        action_kind: "relocate".to_string(),
                                        bytes_freed: rec.size_bytes,
                                        detail: Some(format!("Tầng 3 · Chuyển sang {bdrv}:\\")),
                                    });
                                }
                            }
                            Err(e) => warnings.push(format!("Bỏ qua chuyển {}: {e}", kf.name)),
                        }
                        moved_so_far = moved_so_far.saturating_add(kf.size_bytes);
                    }
                    #[cfg(feature = "store")]
                    {
                        let _ = planned_kf;
                        warnings.push(
                            "Bản Microsoft Store giới hạn di chuyển thư mục hệ thống & AppData do hộp cát MSIX."
                                .to_string(),
                        );
                    }

                    for cand in planned_reloc {
                        if task_ctrl.is_cancelled() || budget.is_reached(freed_so_far) {
                            break;
                        }
                        let st_prog = state_inner.clone();
                        let tid = task_id_worker.clone();
                        let cand_name = cand.name.clone();
                        let base_b = moved_so_far;
                        let mut last_emit = Instant::now()
                            .checked_sub(Duration::from_millis(200))
                            .unwrap_or_else(Instant::now);

                        match TransactionalMover::relocate_junction_with_file(
                            &cand,
                            MoveOptions {
                                target_drive: bdrv,
                                custom_target_path: None,
                            },
                            &state_inner.db,
                            &vol_api,
                            &reparse_api,
                            &proc_probe,
                            &shell_api,
                            &task_ctrl,
                            move |done_b, done_f, rel_file| {
                                if last_emit.elapsed() < Duration::from_millis(100) {
                                    return;
                                }
                                last_emit = Instant::now();
                                let cur = base_b.saturating_add(done_b);
                                let pct = 70
                                    + (((cur as f64 / total_move_bytes.max(1) as f64) * 20.0)
                                        as u32);
                                st_prog.emit_progress(&TaskProgressEvent {
                                    task_id: tid.clone(),
                                    kind: "make_green".to_string(),
                                    state: "running".to_string(),
                                    percent: pct.min(90),
                                    phase: "tier3".to_string(),
                                    title: format!(
                                        "Tầng 3: Đang chuyển {cand_name} sang {bdrv}:\\"
                                    ),
                                    detail: rel_file.to_string(),
                                    done_files: done_f,
                                    total_files: cand.file_count,
                                    done_bytes: cur,
                                    total_bytes: total_move_bytes,
                                    throughput_bps: 0,
                                    eta_seconds: 0,
                                });
                            },
                        ) {
                            Ok(rec) => {
                                relocated_paths_done
                                    .insert(cand.source_path.to_string_lossy().to_lowercase());
                                relocated_bytes = relocated_bytes.saturating_add(rec.size_bytes);
                                freed_so_far = freed_so_far.saturating_add(rec.size_bytes);
                                if rec.size_bytes > 0 {
                                    let now_utc = Utc::now();
                                    let _ = state_inner.db.record_savings_event(
                                        &SavingsEventInput {
                                            timestamp: now_utc.to_rfc3339(),
                                            event_date: now_utc.format("%Y-%m-%d").to_string(),
                                            source_kind: "relocate".to_string(),
                                            source_ref: rec
                                                .id
                                                .map(|i| i.to_string())
                                                .unwrap_or_default(),
                                            app_name: rec.app_name.clone(),
                                            category: "relocate".to_string(),
                                            bytes_freed: rec.size_bytes,
                                            drive_letter: sys_letter.to_string(),
                                        },
                                    );
                                    actions.push(MakeGreenActionItemDto {
                                        name: cand.name,
                                        action_kind: "relocate".to_string(),
                                        bytes_freed: rec.size_bytes,
                                        detail: Some(format!("Tầng 3 · Chuyển sang {bdrv}:\\")),
                                    });
                                }
                            }
                            Err(e) => warnings.push(format!("Bỏ qua chuyển {}: {e}", cand.name)),
                        }
                        moved_so_far = moved_so_far.saturating_add(cand.size_bytes);
                    }

                    // Zalo PC chat data move (only if still short of goal)
                    if !task_ctrl.is_cancelled()
                        && !budget.is_reached(freed_so_far)
                        && is_zalo_auto_move_supported()
                    {
                        let running_exe_dirs = lifecycle.find_running_exe_dirs(ZALO_PROCESS_NAMES);
                        let zalo_running = !running_exe_dirs.is_empty()
                            || ZALO_PROCESS_NAMES
                                .iter()
                                .any(|p| proc_probe.is_process_running(p));
                        let known_relocated_sources: HashSet<PathBuf> = state_inner
                            .db
                            .list_relocations(Some("done"))
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|r| r.app_id == "zalo")
                            .map(|r| r.source_path)
                            .collect();

                        if let Some(disc) = discover_zalo_installation(&ZaloDiscoveryOptions {
                            running_exe_dirs,
                            known_relocated_sources,
                            ..Default::default()
                        }) {
                            if disc.data_dir.is_some()
                                && disc.data_on_system_drive
                                && !disc.data_is_known_relocation
                                && disc.chat_data_bytes > 0
                            {
                                let allow_zalo_move = !zalo_running
                                    || (confirmed_close && close_set.contains("zalo"));
                                if allow_zalo_move {
                                    let st_prog = state_inner.clone();
                                    let tid = task_id_worker.clone();
                                    match ZaloRelocator::move_zalo_data(
                                        &disc,
                                        &ZaloMoveOptions {
                                            target_drive: bdrv,
                                            reopen_zalo: confirmed_close
                                                && close_set.contains("zalo"),
                                            ..Default::default()
                                        },
                                        &state_inner.db,
                                        &vol_api,
                                        &reparse_api,
                                        &shell_api,
                                        &lock_probe,
                                        &proc_probe,
                                        &lifecycle,
                                        &task_ctrl,
                                        move |p| {
                                            st_prog.emit_progress(&TaskProgressEvent {
                                                task_id: tid.clone(),
                                                kind: "make_green".to_string(),
                                                state: "running".to_string(),
                                                percent: 88 + (p.percent / 20).min(4),
                                                phase: "tier3".to_string(),
                                                title: format!(
                                                    "Tầng 3: Đang chuyển Zalo PC sang {bdrv}:\\"
                                                ),
                                                detail: p.detail,
                                                done_files: p.done_files,
                                                total_files: p.total_files,
                                                done_bytes: p.done_bytes,
                                                total_bytes: p.total_bytes,
                                                throughput_bps: 0,
                                                eta_seconds: 0,
                                            });
                                        },
                                    ) {
                                        Ok(rep)
                                            if !rep.already_on_other_drive
                                                && rep.moved_bytes > 0 =>
                                        {
                                            relocated_bytes =
                                                relocated_bytes.saturating_add(rep.moved_bytes);
                                            let now_utc = Utc::now();
                                            let _ = state_inner.db.record_savings_event(
                                                &SavingsEventInput {
                                                    timestamp: now_utc.to_rfc3339(),
                                                    event_date: now_utc
                                                        .format("%Y-%m-%d")
                                                        .to_string(),
                                                    source_kind: "relocate".to_string(),
                                                    source_ref: rep
                                                        .record_id
                                                        .map(|i| i.to_string())
                                                        .unwrap_or_default(),
                                                    app_name: "Zalo PC".to_string(),
                                                    category: "chat".to_string(),
                                                    bytes_freed: rep.moved_bytes,
                                                    drive_letter: sys_letter.to_string(),
                                                },
                                            );
                                            actions.push(MakeGreenActionItemDto {
                                                name: "Zalo PC".to_string(),
                                                action_kind: "relocate".to_string(),
                                                bytes_freed: rep.moved_bytes,
                                                detail: Some(format!(
                                                    "Tầng 3 · Chuyển sang {bdrv}:\\"
                                                )),
                                            });
                                        }
                                        Ok(_) => {}
                                        Err(e) => {
                                            warnings.push(format!("Bỏ qua chuyển Zalo PC: {e}"))
                                        }
                                    }
                                } else {
                                    skipped_locked.push(LockedAppSummary {
                                        app_id: "zalo".to_string(),
                                        app_name: "Zalo PC".to_string(),
                                        size_bytes: disc.chat_data_bytes,
                                        process_names: ZALO_PROCESS_NAMES
                                            .iter()
                                            .map(|s| s.to_string())
                                            .collect(),
                                        reason: "Zalo PC đang mở nên tạm bỏ qua".to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // Reopen confirmed closed apps before measuring final space
        let mut reopened_apps = Vec::new();
        for (app_id, mut session) in closed_sessions {
            if lifecycle.restart_app(&mut session).unwrap_or(false) {
                reopened_apps.push(app_id);
            }
        }

        let (_, after_free_bytes) = vol_api
            .get_disk_free_space(&sys_root)
            .unwrap_or((total_bytes, before_free_bytes));
        let after_used_bytes = total_bytes.saturating_sub(after_free_bytes);
        let computed_total = cleaned_bytes
            .saturating_add(compacted_bytes)
            .saturating_add(relocated_bytes);
        let os_delta = after_free_bytes.saturating_sub(before_free_bytes);
        let total_freed_bytes = computed_total.max(os_delta);

        let cancelled = task_ctrl.is_cancelled();
        let after_ratio = if total_bytes > 0 {
            after_used_bytes as f64 / total_bytes as f64
        } else {
            0.0
        };
        let goal_reached = if sweep_mode == SweepMode::Light || budget.bytes_needed == 0 {
            true
        } else {
            total_freed_bytes >= budget.bytes_needed || after_ratio <= budget.clean_ratio
        };
        let remaining_bytes_needed = if goal_reached {
            0
        } else {
            budget.remaining(total_freed_bytes)
        };

        // ── Tier 4 & Tier 5: Escalation when still short of goal ─────────
        let mut external_drives = Vec::new();
        let mut external_backup_candidates = Vec::new();
        let mut uninstall_suggestions = Vec::new();

        if !goal_reached && !cancelled && sweep_mode != SweepMode::Light {
            emit(
                94,
                "tier4",
                "Chưa đủ mục tiêu — kiểm tra USB/ổ di động & ứng dụng ít dùng...",
                "Tìm phương án sao lưu ra ngoài hoặc gỡ ứng dụng cũ...".to_string(),
                total_freed_bytes,
                budget.bytes_needed,
            );

            let fresh_drives = list_drives_native().unwrap_or_else(|_| drives.clone());
            external_drives = find_external_backup_drives(&fresh_drives, backup_drive);

            // Build top heavy folders still on C: eligible for safe ZIP backup to USB
            let now = SystemTime::now();
            let mut seen_paths: HashSet<String> = relocated_paths_done;
            let probe_target = backup_drive.unwrap_or('D');
            let min_folder_bytes = 5 * 1024 * 1024;

            for kf in KnownFolderService::list_known_folders(probe_target, &shell_api)
                .unwrap_or_default()
            {
                if kf.is_redirected || kf.size_bytes < min_folder_bytes {
                    continue;
                }
                let key = kf.current_path.to_string_lossy().to_lowercase();
                if seen_paths.insert(key) {
                    let ts = estimate_last_used_epoch(std::slice::from_ref(&kf.current_path));
                    external_backup_candidates.push(ExternalBackupCandidateDto {
                        name: kf.name,
                        path: kf.current_path.to_string_lossy().to_string(),
                        size_bytes: kf.size_bytes,
                        inactive_days: inactive_days_from_epoch(ts, now),
                        kind: "known_folder".to_string(),
                    });
                }
            }
            for c in CandidateScanner::discover_all(
                probe_target,
                &vol_api,
                &shell_api,
                &lock_probe,
                &proc_probe,
            ) {
                if c.is_advanced || c.is_locked || c.size_bytes < min_folder_bytes {
                    continue;
                }
                let key = c.source_path.to_string_lossy().to_lowercase();
                if seen_paths.insert(key) {
                    let ts = estimate_last_used_epoch(std::slice::from_ref(&c.source_path));
                    external_backup_candidates.push(ExternalBackupCandidateDto {
                        name: c.name,
                        path: c.source_path.to_string_lossy().to_string(),
                        size_bytes: c.size_bytes,
                        inactive_days: inactive_days_from_epoch(ts, now),
                        kind: "app_data".to_string(),
                    });
                }
            }
            // Least-recently-used (highest inactive_days) first, then largest
            external_backup_candidates.sort_by(|a, b| {
                (b.inactive_days / 7)
                    .cmp(&(a.inactive_days / 7))
                    .then_with(|| b.size_bytes.cmp(&a.size_bytes))
            });
            external_backup_candidates.truncate(10);

            uninstall_suggestions = discover_unused_apps(10);
            stopped_at_tier = if !uninstall_suggestions.is_empty() {
                5
            } else {
                4
            };
        }

        // Update daily snapshot so Overview & History reflect the new C: state immediately.
        let now_utc = Utc::now();
        let date_str = now_utc.format("%Y-%m-%d").to_string();
        let drive_key =
            greenc_scanner::tracker::DiskTracker::normalize_drive(&sys_root.to_string_lossy());
        let _ = state_inner.db.upsert_daily_snapshot(
            &drive_key,
            &date_str,
            total_bytes,
            after_used_bytes,
            after_free_bytes,
            Some(-(total_freed_bytes as i64)),
        );
        if total_freed_bytes > 0 {
            let _ = state_inner
                .db
                .record_reclaimed_bytes(&drive_key, &date_str, total_freed_bytes);
        }

        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "make_green".to_string(),
            state: if cancelled { "cancelled" } else { "completed" }.to_string(),
            percent: 100,
            phase: "done".to_string(),
            title: "Đã hoàn tất làm xanh ổ đĩa!".to_string(),
            detail: format!("Hoàn tất {} tác vụ", actions.len()),
            done_files: actions.len() as u64,
            total_files: actions.len() as u64,
            done_bytes: total_freed_bytes,
            total_bytes: total_freed_bytes,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        Ok(MakeGreenReportDto {
            task_id: task_id_worker,
            before_used_bytes,
            after_used_bytes,
            before_free_bytes,
            after_free_bytes,
            total_bytes,
            cleaned_bytes,
            compacted_bytes,
            relocated_bytes,
            total_freed_bytes,
            backup_drive: backup_drive.map(|c| c.to_string()),
            single_drive_mode: opts.single_drive_mode,
            sweep_mode: sweep_mode.as_str().to_string(),
            goal_target_bytes: budget.bytes_needed,
            goal_reached,
            remaining_bytes_needed,
            stopped_at_tier,
            external_drives,
            external_backup_candidates,
            uninstall_suggestions,
            actions,
            skipped_locked,
            reopened_apps,
            warnings,
            cancelled,
        })
    })
    .await
    .map_err(|e| format!("Make green now join error: {e}"))?;

    state.unregister_task(&task_id);
    result
}

/// Lists attached USB / external drives suitable for Tier 4 ZIP backup (called when user clicks "Kiểm tra lại USB").
#[tauri::command]
pub async fn list_external_backup_drives(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<ExternalBackupDrive>, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let drives = list_drives_native().unwrap_or_default();
        let configured_fixed = state_inner
            .settings()
            .get_data_root()
            .ok()
            .flatten()
            .and_then(|p| p.to_string_lossy().chars().next())
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| {
                drives
                    .iter()
                    .any(|d| d.drive_letter == *c && d.is_eligible && !d.is_system)
            });
        Ok(find_external_backup_drives(&drives, configured_fixed))
    })
    .await
    .map_err(|e| format!("List external drives join error: {e}"))?
}

/// Tier 4: Safely archives selected C: folders into verified SHA-256 ZIP backups on a USB/external
/// drive (`<USB>\GreenC-Backup`), registers them in Vault DB for 1-click Restore, and unlinks the
/// verified copies from C:. NEVER creates NTFS junctions onto removable drives.
#[tauri::command]
pub async fn backup_to_external_drive(
    target_root: String,
    source_paths: Vec<String>,
    state: State<'_, AppState>,
) -> std::result::Result<ExternalBackupReportDto, String> {
    if source_paths.is_empty() {
        return Err("Chưa chọn thư mục nào để sao lưu sang USB/ổ di động.".to_string());
    }

    let task_id = format!("ext-backup-{}", Utc::now().timestamp_millis());
    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let task_id_worker = task_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let vol_api = NativeWin32VolumeApi;
        let lock_probe = NativeFileLockProbe;
        let drives = list_drives_native().unwrap_or_default();

        let norm_target = target_root.trim().trim_end_matches(['\\', '/']).to_lowercase();
        let target_drive = drives
            .iter()
            .find(|d| {
                let dr = d
                    .root
                    .to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .to_lowercase();
                !d.is_system
                    && d.free_bytes >= MIN_EXTERNAL_BACKUP_FREE_BYTES
                    && (dr == norm_target
                        || format!("{}:", d.drive_letter.to_ascii_lowercase()) == norm_target)
            })
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Không tìm thấy ổ USB/di động hợp lệ tại '{target_root}' (hoặc dung lượng trống < 512 MB)."
                )
            })?;

        let sys_letter = drives
            .iter()
            .find(|d| d.is_system)
            .map(|d| d.drive_letter)
            .unwrap_or('C');

        let backup_dir = target_drive.root.join("GreenC-Backup");
        std::fs::create_dir_all(&backup_dir).map_err(|e| {
            format!(
                "Không thể tạo thư mục sao lưu trên {}: {e}",
                target_drive.root.display()
            )
        })?;

        // Collect & validate files for each selected folder
        let mut folders_plan: Vec<(String, PathBuf, Vec<CleanCandidate>, u64)> = Vec::new();
        let mut total_bytes = 0u64;
        let mut warnings = Vec::new();

        for raw_src in &source_paths {
            if task_ctrl.is_cancelled() {
                break;
            }
            let src = PathBuf::from(raw_src);
            if src
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&target_drive.root.to_string_lossy().to_lowercase())
            {
                warnings.push(format!("Bỏ qua '{raw_src}' vì đã nằm trên ổ đích."));
                continue;
            }
            match collect_folder_backup_candidates(&src, &lock_probe, &task_ctrl) {
                Ok(cands) if !cands.is_empty() => {
                    let bytes: u64 = cands.iter().map(|c| c.size_bytes).fold(0, u64::saturating_add);
                    total_bytes = total_bytes.saturating_add(bytes);
                    let name = src
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Backup")
                        .to_string();
                    folders_plan.push((name, src, cands, bytes));
                }
                Ok(_) => warnings.push(format!("Thư mục '{raw_src}' không có tệp mở khóa để sao lưu.")),
                Err(e) => warnings.push(e),
            }
        }

        if folders_plan.is_empty() {
            return Err("Không có tệp hợp lệ nào có thể sao lưu sang USB/ổ di động.".to_string());
        }

        let required = total_bytes.saturating_add(MIN_VAULT_RESERVE_BYTES);
        if target_drive.free_bytes < required {
            return Err(format!(
                "Ổ {} chỉ còn trống {:.2} GB, cần ít nhất {:.2} GB (kèm dự phòng 512 MB).",
                target_drive.root.display(),
                target_drive.free_bytes as f64 / 1_073_741_824.0,
                required as f64 / 1_073_741_824.0
            ));
        }

        let pipeline = ArchiverPipeline::new(ArchiveOptions {
            vault_dir: backup_dir.clone(),
            min_vault_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
            ..Default::default()
        });
        let cleaner = SafeCleaner::new(
            &lock_probe,
            &vol_api,
            None,
            CleanupOptions {
                deletion_method: DeletionMethod::Permanent,
                min_reserve_bytes: MIN_VAULT_RESERVE_BYTES,
                prune_empty_directories: true,
            },
        );

        let mut done_bytes = 0u64;
        let mut freed_bytes = 0u64;
        let mut backed_up_count = 0usize;
        let mut archive_ids = Vec::new();

        for (folder_name, _src_path, cands, folder_bytes) in folders_plan {
            if task_ctrl.is_cancelled() {
                break;
            }
            let mut snapshots = HashMap::with_capacity(cands.len());
            for c in &cands {
                let snap = match std::fs::metadata(&c.path) {
                    Ok(m) => (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                    Err(_) => (c.size_bytes, c.mtime),
                };
                snapshots.insert(c.path.clone(), snap);
            }

            let st_prog = state_inner.clone();
            let tid = task_id_worker.clone();
            let base_b = done_bytes;
            let app_label = format!("USB Backup · {folder_name}");
            let label_prog = folder_name.clone();

            match pipeline.archive_candidates(
                0,
                &app_label,
                &cands,
                &vol_api,
                &task_ctrl,
                move |p| {
                    let cur = base_b.saturating_add(p.done_bytes);
                    let pct = ((cur as f64 / total_bytes.max(1) as f64) * 95.0) as u32;
                    st_prog.emit_progress(&TaskProgressEvent {
                        task_id: tid.clone(),
                        kind: "external_backup".to_string(),
                        state: "running".to_string(),
                        percent: pct.min(95),
                        phase: "archiving".to_string(),
                        title: format!("Đang nén sao lưu {label_prog} sang USB..."),
                        detail: p.current_file,
                        done_files: p.done_files,
                        total_files: p.total_files,
                        done_bytes: cur,
                        total_bytes,
                        throughput_bps: p.throughput_bps,
                        eta_seconds: p.eta_seconds,
                    });
                },
            ) {
                Ok(manifest) => {
                    let zip_p = backup_dir.join(format!("{}.zip", manifest.archive_id));
                    let man_p = backup_dir.join(format!("{}.manifest.json", manifest.archive_id));
                    let _ = manifest.persist_to_db(&state_inner.db, &zip_p, &man_p);
                    archive_ids.push(manifest.archive_id.clone());

                    // Only unlink files that were 100% verified and archived into the ZIP on USB
                    if let Ok(clean_res) =
                        cleaner.unlink_archived_items(&manifest.files, &snapshots)
                    {
                        if clean_res.bytes_freed > 0 {
                            freed_bytes = freed_bytes.saturating_add(clean_res.bytes_freed);
                            let now_utc = Utc::now();
                            let _ = state_inner.db.record_savings_event(&SavingsEventInput {
                                timestamp: now_utc.to_rfc3339(),
                                event_date: now_utc.format("%Y-%m-%d").to_string(),
                                source_kind: "external_backup".to_string(),
                                source_ref: manifest.archive_id,
                                app_name: folder_name,
                                category: "external_backup".to_string(),
                                bytes_freed: clean_res.bytes_freed,
                                drive_letter: sys_letter.to_string(),
                            });
                        }
                        backed_up_count += 1;
                    }
                }
                Err(e) => warnings.push(format!("Không thể sao lưu {folder_name}: {e}")),
            }
            done_bytes = done_bytes.saturating_add(folder_bytes);
        }

        let cancelled = task_ctrl.is_cancelled();
        state_inner.emit_progress(&TaskProgressEvent {
            task_id: task_id_worker.clone(),
            kind: "external_backup".to_string(),
            state: if cancelled { "cancelled" } else { "completed" }.to_string(),
            percent: 100,
            phase: "done".to_string(),
            title: "Đã sao lưu sang USB/ổ di động!".to_string(),
            detail: format!("Đã sao lưu {backed_up_count} thư mục"),
            done_files: backed_up_count as u64,
            total_files: backed_up_count as u64,
            done_bytes: freed_bytes,
            total_bytes: freed_bytes,
            throughput_bps: 0,
            eta_seconds: 0,
        });

        Ok(ExternalBackupReportDto {
            task_id: task_id_worker,
            target_root: target_drive.root.to_string_lossy().to_string(),
            backup_dir: backup_dir.to_string_lossy().to_string(),
            backed_up_count,
            freed_bytes,
            archive_ids,
            warnings,
            cancelled,
        })
    })
    .await
    .map_err(|e| format!("External backup join error: {e}"))?;

    state.unregister_task(&task_id);
    result
}

/// Tier 5: Opens the app's official uninstaller (or OS Apps Settings). Never silently uninstalls.
#[tauri::command]
pub async fn launch_app_uninstaller(
    uninstall_command: Option<String>,
) -> std::result::Result<bool, String> {
    #[cfg(all(windows, not(feature = "store")))]
    {
        if let Some(raw) = uninstall_command.as_deref() {
            if let Some((exe, args)) =
                greenc_scanner::installed_apps::parse_safe_uninstall_command(raw)
            {
                if std::process::Command::new(&exe).args(&args).spawn().is_ok() {
                    return Ok(true);
                }
            }
        }
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:appsfeatures")
            .spawn()
            .map(|_| true)
            .map_err(|e| format!("Không thể mở Cài đặt Ứng dụng Windows: {e}"))
    }

    #[cfg(all(windows, feature = "store"))]
    {
        let _ = uninstall_command;
        std::process::Command::new("explorer.exe")
            .arg("ms-settings:appsfeatures")
            .spawn()
            .map(|_| true)
            .map_err(|e| format!("Không thể mở Cài đặt Ứng dụng Windows: {e}"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = uninstall_command;
        for bin in ["gnome-software", "plasma-discover"] {
            if std::process::Command::new(bin).spawn().is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = uninstall_command;
        std::process::Command::new("open")
            .arg("/Applications")
            .spawn()
            .map(|_| true)
            .map_err(|e| format!("Không thể mở thư mục Applications: {e}"))
    }
}
