//! Pure savings analytics aggregation (`day` / `week` / `month` / `year` buckets + top junk apps).

use crate::db::SavingsEventRow;
use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavingsBucket {
    pub label: String,
    pub date_key: String,
    pub bytes_freed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopJunkApp {
    pub app_name: String,
    pub category: String,
    pub bytes_freed: u64,
    pub event_count: u64,
    pub share_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavingsStats {
    pub period: String,
    pub today_freed_bytes: u64,
    pub period_freed_bytes: u64,
    pub all_time_freed_bytes: u64,
    pub total_events: u64,
    pub buckets: Vec<SavingsBucket>,
    pub top_apps: Vec<TopJunkApp>,
}

/// Aggregates `SavingsEventRow`s into daily/weekly/monthly/yearly buckets and top junk apps.
pub fn aggregate_savings_stats(
    events: &[SavingsEventRow],
    period: &str,
    today: NaiveDate,
) -> SavingsStats {
    let period_norm = match period.to_lowercase().as_str() {
        "week" => "week",
        "month" => "month",
        "year" => "year",
        _ => "day",
    };

    let active: Vec<&SavingsEventRow> = events
        .iter()
        .filter(|e| !e.reverted && e.bytes_freed > 0)
        .collect();

    let today_str = today.format("%Y-%m-%d").to_string();
    let mut today_freed_bytes = 0u64;
    let mut all_time_freed_bytes = 0u64;

    for e in &active {
        all_time_freed_bytes = all_time_freed_bytes.saturating_add(e.bytes_freed);
        if e.event_date == today_str {
            today_freed_bytes = today_freed_bytes.saturating_add(e.bytes_freed);
        }
    }

    let (buckets, cutoff_date): (Vec<SavingsBucket>, Option<NaiveDate>) = match period_norm {
        "day" => {
            let start = today - Duration::days(13);
            let mut map: BTreeMap<String, u64> = BTreeMap::new();
            for offset in 0..14 {
                let d = start + Duration::days(offset);
                map.insert(d.format("%Y-%m-%d").to_string(), 0);
            }
            for e in &active {
                if let Some(slot) = map.get_mut(&e.event_date) {
                    *slot = slot.saturating_add(e.bytes_freed);
                }
            }
            let list = map
                .into_iter()
                .map(|(k, v)| {
                    let label = NaiveDate::parse_from_str(&k, "%Y-%m-%d")
                        .map(|d| d.format("%d/%m").to_string())
                        .unwrap_or_else(|_| k.clone());
                    SavingsBucket {
                        label,
                        date_key: k,
                        bytes_freed: v,
                    }
                })
                .collect();
            (list, Some(start))
        }
        "week" => {
            let days_from_mon = today.weekday().num_days_from_monday() as i64;
            let this_monday = today - Duration::days(days_from_mon);
            let start = this_monday - Duration::weeks(7);
            let mut map: BTreeMap<String, (String, u64)> = BTreeMap::new();
            for w in 0..8 {
                let wk_start = start + Duration::weeks(w);
                let key = wk_start.format("%Y-%m-%d").to_string();
                let label = format!("W{}", wk_start.iso_week().week());
                map.insert(key, (label, 0));
            }
            for e in &active {
                if let Ok(d) = NaiveDate::parse_from_str(&e.event_date, "%Y-%m-%d") {
                    if d >= start && d <= today {
                        let mon = d - Duration::days(d.weekday().num_days_from_monday() as i64);
                        let key = mon.format("%Y-%m-%d").to_string();
                        if let Some((_, sum)) = map.get_mut(&key) {
                            *sum = sum.saturating_add(e.bytes_freed);
                        }
                    }
                }
            }
            let list = map
                .into_iter()
                .map(|(k, (label, v))| SavingsBucket {
                    label,
                    date_key: k,
                    bytes_freed: v,
                })
                .collect();
            (list, Some(start))
        }
        "month" => {
            let mut months: Vec<(i32, u32)> = Vec::with_capacity(12);
            let mut y = today.year();
            let mut m = today.month() as i32;
            for _ in 0..12 {
                months.push((y, m as u32));
                m -= 1;
                if m == 0 {
                    m = 12;
                    y -= 1;
                }
            }
            months.reverse();
            let start = NaiveDate::from_ymd_opt(months[0].0, months[0].1, 1).unwrap_or(today);
            let mut map: BTreeMap<String, (String, u64)> = BTreeMap::new();
            for (yr, mo) in &months {
                let key = format!("{yr:04}-{mo:02}");
                let label = format!("{mo:02}/{:02}", yr % 100);
                map.insert(key, (label, 0));
            }
            for e in &active {
                if e.event_date.len() >= 7 {
                    let key = &e.event_date[..7];
                    if let Some((_, sum)) = map.get_mut(key) {
                        *sum = sum.saturating_add(e.bytes_freed);
                    }
                }
            }
            let list = map
                .into_iter()
                .map(|(k, (label, v))| SavingsBucket {
                    label,
                    date_key: k,
                    bytes_freed: v,
                })
                .collect();
            (list, Some(start))
        }
        _ => {
            let min_year = active
                .iter()
                .filter_map(|e| e.event_date.get(..4)?.parse::<i32>().ok())
                .min()
                .unwrap_or(today.year())
                .min(today.year() - 2);
            let mut map: BTreeMap<String, u64> = BTreeMap::new();
            for yr in min_year..=today.year() {
                map.insert(format!("{yr:04}"), 0);
            }
            for e in &active {
                if let Some(yr) = e.event_date.get(..4) {
                    if let Some(slot) = map.get_mut(yr) {
                        *slot = slot.saturating_add(e.bytes_freed);
                    }
                }
            }
            let list = map
                .into_iter()
                .map(|(k, v)| SavingsBucket {
                    label: k.clone(),
                    date_key: k,
                    bytes_freed: v,
                })
                .collect();
            (list, None)
        }
    };

    let period_freed_bytes: u64 = buckets
        .iter()
        .map(|b| b.bytes_freed)
        .fold(0u64, u64::saturating_add);

    let mut app_totals: BTreeMap<String, (String, u64, u64)> = BTreeMap::new();
    let mut period_events = 0u64;
    for e in &active {
        let in_period = match cutoff_date {
            Some(cut) => NaiveDate::parse_from_str(&e.event_date, "%Y-%m-%d")
                .map(|d| d >= cut && d <= today)
                .unwrap_or(false),
            None => true,
        };
        if !in_period {
            continue;
        }
        period_events += 1;
        let entry = app_totals
            .entry(e.app_name.clone())
            .or_insert_with(|| (e.category.clone(), 0, 0));
        entry.1 = entry.1.saturating_add(e.bytes_freed);
        entry.2 += 1;
    }

    let mut ranked: Vec<TopJunkApp> = app_totals
        .into_iter()
        .map(|(app_name, (category, bytes_freed, event_count))| {
            let share_pct = if period_freed_bytes > 0 {
                ((bytes_freed as f64 / period_freed_bytes as f64) * 1000.0).round() / 10.0
            } else {
                0.0
            };
            TopJunkApp {
                app_name,
                category,
                bytes_freed,
                event_count,
                share_pct,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.bytes_freed
            .cmp(&a.bytes_freed)
            .then_with(|| a.app_name.cmp(&b.app_name))
    });
    ranked.truncate(6);

    SavingsStats {
        period: period_norm.to_string(),
        today_freed_bytes,
        period_freed_bytes,
        all_time_freed_bytes,
        total_events: period_events,
        buckets,
        top_apps: ranked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(date: &str, app: &str, bytes: u64, reverted: bool) -> SavingsEventRow {
        SavingsEventRow {
            id: 1,
            timestamp: format!("{date}T10:00:00Z"),
            event_date: date.to_string(),
            source_kind: "cleanup_vault".to_string(),
            source_ref: "r1".to_string(),
            app_name: app.to_string(),
            category: "browser".to_string(),
            bytes_freed: bytes,
            drive_letter: "C".to_string(),
            reverted,
        }
    }

    #[test]
    fn aggregates_day_week_month_year_and_excludes_reverted() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 28).unwrap();
        let events = vec![
            mk("2026-11-28", "Google Chrome", 3000, false),
            mk("2026-11-27", "Telegram Desktop", 2000, false),
            mk("2026-11-27", "Google Chrome", 1000, false),
            mk("2026-11-28", "Reverted App", 99999, true),
            mk("2026-09-10", "Windows Temp", 5000, false),
            mk("2025-05-01", "Discord", 4000, false),
        ];

        let day = aggregate_savings_stats(&events, "day", today);
        assert_eq!(day.buckets.len(), 14);
        assert_eq!(day.today_freed_bytes, 3000);
        assert_eq!(day.period_freed_bytes, 6000);
        assert_eq!(day.all_time_freed_bytes, 15000);
        assert_eq!(day.top_apps.len(), 2);
        assert_eq!(day.top_apps[0].app_name, "Google Chrome");
        assert_eq!(day.top_apps[0].bytes_freed, 4000);

        let week = aggregate_savings_stats(&events, "week", today);
        assert_eq!(week.buckets.len(), 8);
        assert_eq!(week.period_freed_bytes, 6000);

        let month = aggregate_savings_stats(&events, "month", today);
        assert_eq!(month.buckets.len(), 12);
        assert_eq!(month.period_freed_bytes, 11000);

        let year = aggregate_savings_stats(&events, "year", today);
        assert!(year.buckets.len() >= 3);
        assert_eq!(year.period_freed_bytes, 15000);
        assert_eq!(year.top_apps[0].app_name, "Windows Temp");
    }
}
