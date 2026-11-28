//! Space-savings analytics scorecard IPC command (`get_savings_stats`).

use crate::state::AppState;
use chrono::Utc;
use greenc_storage::analytics::{aggregate_savings_stats, SavingsStats};
use tauri::State;

pub type SavingsStatsDto = SavingsStats;

/// Returns cumulative savings stats and top junk-generating apps for `"day" | "week" | "month" | "year"`.
#[tauri::command]
pub async fn get_savings_stats(
    period: Option<String>,
    state: State<'_, AppState>,
) -> std::result::Result<SavingsStatsDto, String> {
    let period_str = period.unwrap_or_else(|| "day".to_string());
    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let events = state_inner
            .db
            .query_savings_events()
            .map_err(|e| format!("Failed to load savings stats: {e}"))?;
        let today = Utc::now().date_naive();
        Ok(aggregate_savings_stats(&events, &period_str, today))
    })
    .await
    .map_err(|e| format!("Savings stats join error: {e}"))?
}
