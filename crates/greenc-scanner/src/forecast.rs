//! Rolling multi-window burn-rate velocity and disk capacity exhaustion forecasting (Task T023).

use greenc_core::models::DailySnapshot;
use serde::{Deserialize, Serialize};

/// Predictive burn-rate forecast metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BurnRateForecast {
    pub daily_net_bps: i64,
    pub daily_burn_rate_bytes: i64,
    pub burn_rate_7d: i64,
    pub burn_rate_14d: i64,
    pub burn_rate_30d: i64,
    pub days_until_90_pct: Option<u32>,
    pub days_until_full: Option<u32>,
    pub current_free_bytes: u64,
    pub total_bytes: u64,
    pub usage_ratio: f64,
    pub is_critical: bool,
    pub sample_days: usize,
}

pub type ForecastResult = BurnRateForecast;

/// Computes the average daily velocity over the last K snapshots.
fn calculate_rolling_velocity(snapshots: &[DailySnapshot], window_days: usize) -> i64 {
    if snapshots.is_empty() {
        return 0;
    }
    let take_count = snapshots.len().min(window_days);
    let slice = &snapshots[snapshots.len() - take_count..];
    let sum: i64 = slice.iter().map(|s| s.net_change_bytes).sum();
    sum / take_count as i64
}

/// Predicts disk capacity exhaustion timelines and fullness forecast according to FR-004.
pub fn predict_fullness(
    snapshots: &[DailySnapshot],
    current_free_bytes: u64,
    total_bytes: u64,
) -> BurnRateForecast {
    let current_used = total_bytes.saturating_sub(current_free_bytes);
    let usage_ratio = if total_bytes > 0 {
        current_used as f64 / total_bytes as f64
    } else {
        0.0
    };

    // Sort snapshots chronologically by snapshot_date
    let mut sorted_snapshots = snapshots.to_vec();
    sorted_snapshots.sort_by_key(|s| s.snapshot_date);

    let sample_days = sorted_snapshots.len();

    let rate_7d = calculate_rolling_velocity(&sorted_snapshots, 7);
    let rate_14d = calculate_rolling_velocity(&sorted_snapshots, 14);
    let rate_30d = calculate_rolling_velocity(&sorted_snapshots, 30);

    // Primary burn rate adopts the 14-day velocity, falling back to 7d or 30d
    let primary_burn_rate = if rate_14d != 0 {
        rate_14d
    } else if rate_7d != 0 {
        rate_7d
    } else {
        rate_30d
    };

    let target_90 = (total_bytes as f64 * 0.90) as u64;

    let (days_90, days_full) = if total_bytes == 0 {
        (None, None)
    } else if current_free_bytes == 0 {
        (Some(0), Some(0))
    } else if primary_burn_rate <= 0 {
        let d90 = if current_used >= target_90 {
            Some(0)
        } else {
            None
        };
        (d90, None)
    } else {
        let burn_f = primary_burn_rate as f64;

        let d90 = if current_used >= target_90 {
            Some(0)
        } else {
            let needed = target_90 - current_used;
            Some((needed as f64 / burn_f).ceil() as u32)
        };

        let dfull = Some((current_free_bytes as f64 / burn_f).ceil() as u32);

        (d90, dfull)
    };

    let is_critical = usage_ratio >= 0.90
        || days_full.is_some_and(|d| d < 14)
        || days_90.is_some_and(|d| d <= 14);

    BurnRateForecast {
        daily_net_bps: primary_burn_rate,
        daily_burn_rate_bytes: primary_burn_rate,
        burn_rate_7d: rate_7d,
        burn_rate_14d: rate_14d,
        burn_rate_30d: rate_30d,
        days_until_90_pct: days_90,
        days_until_full: days_full,
        current_free_bytes,
        total_bytes,
        usage_ratio,
        is_critical,
        sample_days,
    }
}
