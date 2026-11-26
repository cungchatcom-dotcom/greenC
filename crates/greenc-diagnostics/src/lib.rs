//! Privacy-first, opt-in diagnostics counter aggregator and local event log.

#[cfg(not(feature = "store"))]
mod contract;
#[cfg(not(feature = "store"))]
mod log_store;

#[cfg(feature = "store")]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "store")]
use std::path::{Path, PathBuf};

pub const ENDPOINT: &str = "https://greenc-log.cungchat.com/v1/counters";
pub const CONSENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub version: String,
    pub os: String,
    pub build: u32,
    pub system_disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Partial,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    Active,
    Start {
        ms: u64,
    },
    Scan {
        name: &'a str,
        outcome: Outcome,
        code: &'a str,
        ms: u64,
    },
    Screen {
        name: &'a str,
        cached: bool,
        ms: u64,
    },
    Action {
        name: &'a str,
        outcome: Outcome,
        code: &'a str,
        ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentState {
    pub available: bool,
    pub granted: bool,
    pub should_ask: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub id: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResult {
    Accepted,
    Rejected,
    Retry,
}

#[cfg(feature = "store")]
pub struct Diagnostics;

#[cfg(feature = "store")]
impl Diagnostics {
    pub fn open(_dir: PathBuf, _env: Env) -> Self {
        Self
    }

    pub fn consent(&self) -> ConsentState {
        ConsentState {
            available: false,
            granted: false,
            should_ask: false,
        }
    }

    pub fn prompt_dismissed(&self) {}

    pub fn set_consent(&self, _granted: bool) {}

    pub fn record(&self, _event: Event<'_>, _now: DateTime<Utc>) {}

    pub fn preview(&self, _now: DateTime<Utc>) -> String {
        String::new()
    }

    pub fn take_due_batch(&self, _now: DateTime<Utc>) -> Option<Batch> {
        None
    }

    pub fn finish_batch(&self, _id: &str, _result: SendResult, _now: DateTime<Utc>) {}

    pub fn record_panic(&self, _file: &str, _line: u32) {}

    pub fn export_log(&self, _dest: &Path) -> std::io::Result<()> {
        Ok(())
    }

    pub fn clear_log(&self) {}
}

#[cfg(not(feature = "store"))]
mod standard {
    use super::{Batch, ConsentState, Env, Event, SendResult, CONSENT_VERSION};
    use crate::contract::{
        disk_bucket, is_day_in_window, is_valid_crash_name, normalize_crash_location,
        normalize_event, pack_batch_rows, BatchPayload, CounterKey, CounterRow, ExamplePayload,
        MAX_QUEUE_KEYS, MAX_ROW_COUNT,
    };
    use crate::log_store::LogStore;
    use chrono::{DateTime, Duration, Utc};
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use uuid::Uuid;

    const MAX_PROMPT_DISMISSALS: u8 = 2;
    const SEND_INTERVAL_HOURS: i64 = 20;
    const STATE_FILE: &str = "state.json";
    const CRASH_MARKER_FILE: &str = "crash.marker";

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    enum ConsentRecord {
        Unanswered { dismiss_count: u8 },
        Granted { consent_version: u32 },
        Declined,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct PendingBatch {
        id: String,
        body: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct PersistedState {
        consent: ConsentRecord,
        #[serde(default)]
        last_active_day: Option<String>,
        #[serde(default)]
        last_attempt_at: Option<DateTime<Utc>>,
        #[serde(default)]
        crash_unretried: bool,
        #[serde(default)]
        queue: Vec<CounterRow>,
        #[serde(default)]
        pending: Option<PendingBatch>,
    }

    #[derive(Serialize)]
    struct CounterRowRef<'a> {
        day: &'a str,
        version: &'a str,
        os: &'a str,
        event: &'a str,
        name: &'a str,
        outcome: &'a str,
        code: &'a str,
        duration: &'a str,
        build: u32,
        disk: &'a str,
        count: u32,
    }

    #[derive(Serialize)]
    struct PersistedStateRef<'a> {
        consent: &'a ConsentRecord,
        last_active_day: Option<&'a str>,
        last_attempt_at: Option<DateTime<Utc>>,
        crash_unretried: bool,
        queue: Vec<CounterRowRef<'a>>,
        pending: Option<&'a PendingBatch>,
    }

    struct InnerState {
        consent: ConsentRecord,
        last_active_day: Option<String>,
        last_attempt_at: Option<DateTime<Utc>>,
        crash_unretried: bool,
        queue: BTreeMap<CounterKey, u32>,
        pending: Option<PendingBatch>,
        log_store: LogStore,
    }

    impl InnerState {
        fn fresh(dir: &Path) -> Self {
            Self {
                consent: ConsentRecord::Unanswered { dismiss_count: 0 },
                last_active_day: None,
                last_attempt_at: None,
                crash_unretried: false,
                queue: BTreeMap::new(),
                pending: None,
                log_store: LogStore::new(dir),
            }
        }

        fn is_granted(&self) -> bool {
            matches!(
                self.consent,
                ConsentRecord::Granted { consent_version } if consent_version >= CONSENT_VERSION
            )
        }

        fn consent_state(&self) -> ConsentState {
            match self.consent {
                ConsentRecord::Granted { consent_version }
                    if consent_version >= CONSENT_VERSION =>
                {
                    ConsentState {
                        available: true,
                        granted: true,
                        should_ask: false,
                    }
                }
                ConsentRecord::Granted { .. } => ConsentState {
                    available: true,
                    granted: false,
                    should_ask: true,
                },
                ConsentRecord::Unanswered { dismiss_count } => ConsentState {
                    available: true,
                    granted: false,
                    should_ask: dismiss_count < MAX_PROMPT_DISMISSALS,
                },
                ConsentRecord::Declined => ConsentState {
                    available: true,
                    granted: false,
                    should_ask: false,
                },
            }
        }

        fn to_persisted_ref(&self) -> PersistedStateRef<'_> {
            let queue = self
                .queue
                .iter()
                .map(|(k, c)| CounterRowRef {
                    day: &k.day,
                    version: &k.version,
                    os: &k.os,
                    event: &k.event,
                    name: &k.name,
                    outcome: &k.outcome,
                    code: &k.code,
                    duration: &k.duration,
                    build: k.build,
                    disk: &k.disk,
                    count: *c,
                })
                .collect();
            PersistedStateRef {
                consent: &self.consent,
                last_active_day: self.last_active_day.as_deref(),
                last_attempt_at: self.last_attempt_at,
                crash_unretried: self.crash_unretried,
                queue,
                pending: self.pending.as_ref(),
            }
        }

        fn enqueue_key(&mut self, key: CounterKey, now: DateTime<Utc>) -> bool {
            if let Some(count) = self.queue.get_mut(&key) {
                if *count >= MAX_ROW_COUNT {
                    return false;
                }
                *count = count.saturating_add(1).min(MAX_ROW_COUNT);
                return true;
            }
            if self.queue.len() >= MAX_QUEUE_KEYS {
                self.prune_queue_for(now);
            }
            if self.queue.len() >= MAX_QUEUE_KEYS {
                return false;
            }
            self.queue.insert(key, 1);
            true
        }

        fn prune_queue_for(&mut self, now: DateTime<Utc>) {
            let today = now.date_naive();
            self.queue.retain(|k, c| *c > 0 && is_day_in_window(&k.day, today));
        }

        fn is_due(&self, now: DateTime<Utc>) -> bool {
            if !self.is_granted() {
                return false;
            }
            if self.pending.is_none() && self.queue.is_empty() {
                return false;
            }
            if self.crash_unretried {
                return true;
            }
            match self.last_attempt_at {
                None => true,
                Some(last) => {
                    // Backward clock (last_attempt_at in the future) is treated as due once.
                    if last > now {
                        true
                    } else {
                        now.signed_duration_since(last) >= Duration::hours(SEND_INTERVAL_HOURS)
                    }
                }
            }
        }
    }

    pub struct Diagnostics {
        dir: PathBuf,
        env: Env,
        inner: Mutex<InnerState>,
    }

    impl Diagnostics {
        pub fn open(dir: PathBuf, env: Env) -> Self {
            let mut state = load_state(&dir);
            let now = Utc::now();
            state.log_store.prune_old(now);

            let marker_path = dir.join(CRASH_MARKER_FILE);
            if let Ok(raw_marker) = fs::read_to_string(&marker_path) {
                let _ = fs::remove_file(&marker_path);
                let trimmed = raw_marker.trim();
                let crash_name = if is_valid_crash_name(trimmed) {
                    trimmed.to_string()
                } else {
                    "dependency".to_string()
                };

                state
                    .log_store
                    .append_entry(now, "crash", &crash_name, "", "", "");

                if state.is_granted() {
                    let key = CounterKey {
                        day: now.format("%Y-%m-%d").to_string(),
                        version: env.version.clone(),
                        os: env.os.clone(),
                        event: "crash".to_string(),
                        name: crash_name,
                        outcome: String::new(),
                        code: String::new(),
                        duration: String::new(),
                        build: 0,
                        disk: String::new(),
                    };
                    state.enqueue_key(key, now);
                    state.crash_unretried = true;
                    save_state(&dir, &state);
                }
            }

            Self {
                dir,
                env,
                inner: Mutex::new(state),
            }
        }

        fn lock_inner(&self) -> std::sync::MutexGuard<'_, InnerState> {
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        pub fn consent(&self) -> ConsentState {
            let state = self.lock_inner();
            state.consent_state()
        }

        pub fn prompt_dismissed(&self) {
            let mut state = self.lock_inner();
            if let ConsentRecord::Unanswered { dismiss_count } = &mut state.consent {
                *dismiss_count = dismiss_count.saturating_add(1).min(MAX_PROMPT_DISMISSALS);
                save_state(&self.dir, &state);
            }
        }

        pub fn set_consent(&self, granted: bool) {
            let mut state = self.lock_inner();
            if granted {
                state.consent = ConsentRecord::Granted {
                    consent_version: CONSENT_VERSION,
                };
            } else {
                state.consent = ConsentRecord::Declined;
                state.queue.clear();
                state.pending = None;
                state.crash_unretried = false;
                state.last_active_day = None;
                let _ = fs::remove_file(self.dir.join(CRASH_MARKER_FILE));
            }
            save_state(&self.dir, &state);
        }

        pub fn record(&self, event: Event<'_>, now: DateTime<Utc>) {
            let norm = normalize_event(event, &self.env);
            let mut state = self.lock_inner();

            state.log_store.append_entry(
                now,
                norm.event,
                &norm.log_name,
                norm.outcome,
                &norm.log_code,
                norm.duration,
            );

            if !state.is_granted() || !norm.countable {
                return;
            }

            let day = now.format("%Y-%m-%d").to_string();
            if matches!(event, Event::Active) {
                if state.last_active_day.as_deref() == Some(day.as_str()) {
                    return;
                }
                state.last_active_day = Some(day.clone());
            }

            let key = CounterKey {
                day,
                version: self.env.version.clone(),
                os: self.env.os.clone(),
                event: norm.event.to_string(),
                name: norm.log_name,
                outcome: norm.outcome.to_string(),
                code: norm.log_code,
                duration: norm.duration.to_string(),
                build: norm.build,
                disk: norm.disk.to_string(),
            };
            if state.enqueue_key(key, now) {
                save_state(&self.dir, &state);
            }
        }

        pub fn preview(&self, now: DateTime<Utc>) -> String {
            let state = self.lock_inner();
            let today = now.date_naive();

            if state.is_granted() {
                if let Some(pending) = &state.pending {
                    if let Ok(mut parsed) = serde_json::from_str::<BatchPayload>(&pending.body) {
                        parsed.rows.retain(|r| is_day_in_window(&r.day, today));
                        if !parsed.rows.is_empty() {
                            if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
                                return pretty;
                            }
                        }
                    }
                }

                let rows: Vec<CounterRow> = state
                    .queue
                    .iter()
                    .filter(|(k, c)| **c > 0 && is_day_in_window(&k.day, today))
                    .map(|(k, c)| CounterRow::from_key(k.clone(), *c))
                    .collect();

                if !rows.is_empty() {
                    let preview_id = Uuid::new_v4().to_string();
                    if let Some((_, body)) = pack_batch_rows(&preview_id, &rows) {
                        if let Ok(parsed) = serde_json::from_str::<BatchPayload>(&body) {
                            if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
                                return pretty;
                            }
                        }
                    }
                }
            }

            let example = ExamplePayload {
                example: true,
                schema: 1,
                batch: "00000000-0000-4000-8000-000000000000".to_string(),
                rows: vec![
                    CounterRow {
                        day: now.format("%Y-%m-%d").to_string(),
                        version: self.env.version.clone(),
                        os: self.env.os.clone(),
                        event: "active".to_string(),
                        name: String::new(),
                        outcome: String::new(),
                        code: String::new(),
                        duration: String::new(),
                        build: self.env.build.min(99_999),
                        disk: disk_bucket(self.env.system_disk_bytes).to_string(),
                        count: 1,
                    },
                    CounterRow {
                        day: now.format("%Y-%m-%d").to_string(),
                        version: self.env.version.clone(),
                        os: self.env.os.clone(),
                        event: "scan".to_string(),
                        name: "cleanup".to_string(),
                        outcome: "ok".to_string(),
                        code: String::new(),
                        duration: "lt5s".to_string(),
                        build: 0,
                        disk: String::new(),
                        count: 1,
                    },
                ],
            };
            serde_json::to_string_pretty(&example).unwrap_or_default()
        }

        pub fn take_due_batch(&self, now: DateTime<Utc>) -> Option<Batch> {
            let mut state = self.lock_inner();
            if !state.is_granted() {
                return None;
            }

            // Ensure out-of-window rows are never sent even across clock jumps.
            state.prune_queue_for(now);
            let today = now.date_naive();
            if let Some(pending) = state.pending.clone() {
                if let Ok(mut parsed) = serde_json::from_str::<BatchPayload>(&pending.body) {
                    let before = parsed.rows.len();
                    parsed.rows.retain(|r| is_day_in_window(&r.day, today));
                    if parsed.rows.is_empty() {
                        state.pending = None;
                    } else if parsed.rows.len() != before {
                        if let Ok(updated_body) = serde_json::to_string(&parsed) {
                            state.pending = Some(PendingBatch {
                                id: pending.id,
                                body: updated_body,
                            });
                        } else {
                            state.pending = None;
                        }
                    }
                } else {
                    state.pending = None;
                }
            }

            if !state.is_due(now) {
                return None;
            }

            if let Some(pending) = state.pending.clone() {
                state.last_attempt_at = Some(now);
                state.crash_unretried = false;
                save_state(&self.dir, &state);
                return Some(Batch {
                    id: pending.id,
                    body: pending.body,
                });
            }

            let candidates: Vec<(CounterKey, CounterRow)> = state
                .queue
                .iter()
                .map(|(k, c)| (k.clone(), CounterRow::from_key(k.clone(), *c)))
                .collect();
            let candidate_rows: Vec<CounterRow> =
                candidates.iter().map(|(_, r)| r.clone()).collect();

            let batch_id = Uuid::new_v4().to_string();
            let (taken, body) = pack_batch_rows(&batch_id, &candidate_rows)?;

            for (key, _) in candidates.into_iter().take(taken) {
                state.queue.remove(&key);
            }
            state.pending = Some(PendingBatch {
                id: batch_id.clone(),
                body: body.clone(),
            });
            state.last_attempt_at = Some(now);
            state.crash_unretried = false;
            save_state(&self.dir, &state);

            Some(Batch { id: batch_id, body })
        }

        pub fn finish_batch(&self, id: &str, result: SendResult, now: DateTime<Utc>) {
            let mut state = self.lock_inner();
            if !state.is_granted() {
                return;
            }
            let Some(pending) = &state.pending else {
                return;
            };
            if pending.id != id {
                return;
            }

            match result {
                SendResult::Accepted | SendResult::Rejected => {
                    state.pending = None;
                }
                SendResult::Retry => {
                    // Keep pending batch with the exact same id and body.
                }
            }
            state.last_attempt_at = Some(now);
            save_state(&self.dir, &state);
        }

        /// Writes a sanitized crash marker to disk without acquiring any lock and without panicking.
        pub fn record_panic(&self, file: &str, line: u32) {
            let location = normalize_crash_location(file, line);
            if fs::create_dir_all(&self.dir).is_err() {
                return;
            }
            let tmp_path = self.dir.join("crash.marker.tmp");
            let marker_path = self.dir.join(CRASH_MARKER_FILE);
            if fs::write(&tmp_path, location.as_bytes()).is_ok()
                && fs::rename(&tmp_path, &marker_path).is_err()
            {
                let _ = fs::remove_file(&tmp_path);
                let _ = fs::write(&marker_path, location.as_bytes());
            }
        }

        pub fn export_log(&self, dest: &Path) -> std::io::Result<()> {
            let state = self.lock_inner();
            state.log_store.export_to(dest)
        }

        pub fn clear_log(&self) {
            let state = self.lock_inner();
            state.log_store.clear();
        }
    }

    fn load_state(dir: &Path) -> InnerState {
        let state_path = dir.join(STATE_FILE);
        let Ok(raw) = fs::read_to_string(&state_path) else {
            return InnerState::fresh(dir);
        };
        let Ok(persisted) = serde_json::from_str::<PersistedState>(&raw) else {
            // Corrupt state file -> unanswered, drop queue, never become granted.
            return InnerState::fresh(dir);
        };

        // Older consent version -> unanswered, drop queued/pending data.
        if let ConsentRecord::Granted { consent_version } = persisted.consent {
            if consent_version < CONSENT_VERSION {
                return InnerState::fresh(dir);
            }
        }

        let mut queue = BTreeMap::new();
        for row in persisted.queue {
            let (key, count) = row.into_key_and_count();
            if count > 0 && queue.len() < MAX_QUEUE_KEYS {
                queue.insert(key, count.min(MAX_ROW_COUNT));
            }
        }

        InnerState {
            consent: persisted.consent,
            last_active_day: persisted.last_active_day,
            last_attempt_at: persisted.last_attempt_at,
            crash_unretried: persisted.crash_unretried,
            queue,
            pending: persisted.pending,
            log_store: LogStore::new(dir),
        }
    }

    fn save_state(dir: &Path, state: &InnerState) {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let Ok(serialized) = serde_json::to_vec(&state.to_persisted_ref()) else {
            return;
        };
        let tmp_path = dir.join("state.json.tmp");
        let state_path = dir.join(STATE_FILE);
        if fs::write(&tmp_path, serialized).is_ok() && fs::rename(&tmp_path, &state_path).is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
    }
}

#[cfg(not(feature = "store"))]
pub use standard::Diagnostics;
