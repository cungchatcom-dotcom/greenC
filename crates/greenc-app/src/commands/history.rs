//! Daily disk delta timeline and per-day folder/actor attribution IPC commands (Task T056).

use crate::commands::diagnostics::{record_scan_result, CommandFailure};
use crate::state::{AppState, TaskProgressEvent};
use greenc_core::models::WriteReasonCode;
use greenc_core::task::TaskControl;
use greenc_diagnostics::Outcome;
use greenc_scanner::attribution::reason_label_key;
use greenc_scanner::tracker::DiskTracker;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySnapshotDto {
    pub id: i64,
    pub drive_letter: String,
    pub date: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub net_change_bytes: i64,
    pub reclaimed_bytes: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderGrowthDto {
    pub folder_path: String,
    pub reason_code: String,
    pub reason_label: String,
    pub total_delta_bytes: i64,
    pub file_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorWriteDto {
    pub process_name: String,
    pub service_name: String,
    pub display_actor: String,
    pub total_delta_bytes: i64,
    pub event_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDrilldownDto {
    pub drive_letter: String,
    pub date: String,
    pub folders: Vec<FolderGrowthDto>,
    pub actors: Vec<ActorWriteDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveHistoryDto {
    pub drive_letter: String,
    pub days_back: u32,
    pub snapshots: Vec<DailySnapshotDto>,
    pub selected_date: Option<String>,
    pub latest_drilldown: Option<DailyDrilldownDto>,
}

fn current_locale(state: &AppState) -> String {
    state
        .settings()
        .get_language()
        .ok()
        .flatten()
        .unwrap_or_else(|| "vi".to_string())
}

fn map_drilldown_dto(
    state: &AppState,
    locale: &str,
    drilldown: greenc_scanner::tracker::DailyDrilldown,
) -> DailyDrilldownDto {
    let folders = drilldown
        .folders
        .into_iter()
        .map(|row| {
            let code_enum = WriteReasonCode::from_str_loose(&row.reason_code);
            let label_key = reason_label_key(code_enum);
            let reason_label = state.catalog.tr(locale, label_key, &[]);
            FolderGrowthDto {
                folder_path: row.folder_path.to_string_lossy().to_string(),
                reason_code: row.reason_code,
                reason_label,
                total_delta_bytes: row.total_delta_bytes,
                file_count: row.file_count,
            }
        })
        .collect();

    let actors = drilldown
        .actors
        .into_iter()
        .map(|row| {
            let display_actor = if !row.service_name.is_empty()
                && !row.process_name.is_empty()
                && row.process_name != "Unknown"
            {
                format!("{} (svc: {})", row.process_name, row.service_name)
            } else if !row.service_name.is_empty() {
                format!("Service: {}", row.service_name)
            } else if !row.process_name.is_empty() {
                row.process_name.clone()
            } else {
                state.catalog.tr(locale, "Tác nhân chưa xác định", &[])
            };

            ActorWriteDto {
                process_name: row.process_name,
                service_name: row.service_name,
                display_actor,
                total_delta_bytes: row.total_delta_bytes,
                event_count: row.event_count,
            }
        })
        .collect();

    DailyDrilldownDto {
        drive_letter: drilldown.drive_letter,
        date: drilldown.date,
        folders,
        actors,
    }
}

/// Scans `drive` over `days_back` (`7`, `14`, or `30` days), rebuilds daily delta snapshots
/// and attributed write events, and returns the bar chart series plus the latest day's drill-down.
#[tauri::command]
pub async fn scan_drive_history(
    drive: String,
    days_back: Option<u32>,
    state: State<'_, AppState>,
) -> std::result::Result<DriveHistoryDto, String> {
    let started = Instant::now();
    let days = days_back.unwrap_or(14).clamp(7, 90);
    let drive_norm = DiskTracker::normalize_drive(&drive);
    let task_id = format!(
        "history-scan-{}-{}",
        drive_norm.chars().next().unwrap_or('C'),
        chrono::Utc::now().timestamp_millis()
    );

    let task_ctrl = Arc::new(TaskControl::new());
    state.register_task(task_id.clone(), Arc::clone(&task_ctrl));

    let state_inner = state.inner().clone();
    let drive_for_worker = drive_norm.clone();
    let task_id_for_worker = task_id.clone();

    let join_res =
        tokio::task::spawn_blocking(
            move || -> std::result::Result<DriveHistoryDto, CommandFailure> {
            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_for_worker.clone(),
                kind: "scan_drive_history".to_string(),
                state: "running".to_string(),
                percent: 15,
                phase: "history_scan".to_string(),
                title: format!(
                    "Đang phân tích biến động {} ngày trên {}",
                    days, drive_for_worker
                ),
                detail: "Tổng hợp nhật ký thay đổi dung lượng và tác nhân ghi đĩa...".to_string(),
                done_files: 0,
                total_files: 0,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            let tracker = DiskTracker::new(Arc::clone(&state_inner.db));
            let snapshots = tracker
                .scan_drive_history(&drive_for_worker, days, Some(&task_ctrl))
                .map_err(|e| {
                    CommandFailure::from_err(&e, format!("Failed to scan drive history: {e}"))
                })?;

            let locale = current_locale(&state_inner);
            let snapshot_dtos: Vec<DailySnapshotDto> = snapshots
                .into_iter()
                .map(|s| DailySnapshotDto {
                    id: s.id.unwrap_or(0),
                    drive_letter: s.drive_letter,
                    date: s.snapshot_date.format("%Y-%m-%d").to_string(),
                    total_bytes: s.total_bytes,
                    used_bytes: s.used_bytes,
                    free_bytes: s.free_bytes,
                    net_change_bytes: s.net_change_bytes,
                    reclaimed_bytes: s.reclaimed_bytes,
                    updated_at: s.updated_at.to_rfc3339(),
                })
                .collect();

            let selected_date = snapshot_dtos.last().map(|s| s.date.clone());
            let latest_drilldown = if let Some(ref d) = selected_date {
                tracker
                    .get_daily_drilldown(&drive_for_worker, d)
                    .ok()
                    .map(|dd| map_drilldown_dto(&state_inner, &locale, dd))
            } else {
                None
            };

            state_inner.emit_progress(&TaskProgressEvent {
                task_id: task_id_for_worker,
                kind: "scan_drive_history".to_string(),
                state: "completed".to_string(),
                percent: 100,
                phase: "completed".to_string(),
                title: "Đã cập nhật biểu đồ biến động theo ngày".to_string(),
                detail: format!("{} mốc ngày trên {}", snapshot_dtos.len(), drive_for_worker),
                done_files: snapshot_dtos.len() as u64,
                total_files: snapshot_dtos.len() as u64,
                done_bytes: 0,
                total_bytes: 0,
                throughput_bps: 0,
                eta_seconds: 0,
            });

            Ok(DriveHistoryDto {
                drive_letter: drive_for_worker,
                days_back: days,
                snapshots: snapshot_dtos,
                selected_date,
                latest_drilldown,
            })
        })
        .await;

    state.unregister_task(&task_id);
    let diag_res = match join_res {
        Ok(inner) => inner,
        Err(e) => Err(CommandFailure::from_msg(
            format!("History scan join error: {e}"),
            "unknown",
        )),
    };
    record_scan_result(&state.diagnostics, "history", started, diag_res, |_| {
        Outcome::Ok
    })
}

/// Queries the instant folder/reason and process/service culprit breakdown for a clicked day bar.
#[tauri::command]
pub async fn get_daily_drilldown(
    drive: String,
    date: String,
    state: State<'_, AppState>,
) -> std::result::Result<DailyDrilldownDto, String> {
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let tracker = DiskTracker::new(Arc::clone(&state_inner.db));
        let drilldown = tracker
            .get_daily_drilldown(&drive, &date)
            .map_err(|e| format!("Failed to load daily drilldown: {e}"))?;
        let locale = current_locale(&state_inner);
        Ok(map_drilldown_dto(&state_inner, &locale, drilldown))
    })
    .await
    .map_err(|e| format!("Drilldown task join error: {e}"))?
}
