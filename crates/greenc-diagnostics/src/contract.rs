use crate::{Env, Event, Outcome};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const MAX_BATCH_ROWS: usize = 200;
pub const MAX_BATCH_BYTES: usize = 32_768;
pub const MAX_QUEUE_KEYS: usize = 2_000;
pub const MAX_ROW_COUNT: u32 = 1_000;

const SCAN_NAMES: &[&str] = &["space_lens", "cleanup", "relocate", "compact", "history"];

const SCREEN_NAMES: &[&str] = &[
    "overview", "cleanup", "relocate", "compact", "vault", "settings",
];

const ACTION_NAMES: &[&str] = &[
    "cleanup",
    "restore",
    "restore_selective",
    "vault_delete",
    "vault_move",
    "relocate",
    "known_folder",
    "relocate_undo",
    "link_repair",
    "recovery",
    "compact",
    "uncompact",
    "compact_vhdx",
    "zalo_clean",
    "zalo_move",
    "zalo_purge",
    "zalo_restore",
    "guard_clean",
    "guard_purge",
];

const ERROR_CODES: &[&str] = &[
    "InsufficientVaultSpace",
    "FileLocked",
    "IntegrityMismatch",
    "PreconditionBlocked",
    "Cancelled",
    "Database",
    "Sqlite",
    "Vault",
    "Relocation",
    "Scanner",
    "I18n",
    "NotFound",
    "InvalidPath",
    "ConfigError",
    "Io",
    "Json",
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CounterKey {
    pub day: String,
    pub version: String,
    pub os: String,
    pub event: String,
    pub name: String,
    pub outcome: String,
    pub code: String,
    pub duration: String,
    pub build: u32,
    pub disk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterRow {
    pub day: String,
    pub version: String,
    pub os: String,
    pub event: String,
    pub name: String,
    pub outcome: String,
    pub code: String,
    pub duration: String,
    pub build: u32,
    pub disk: String,
    pub count: u32,
}

impl CounterRow {
    pub fn from_key(key: CounterKey, count: u32) -> Self {
        Self {
            day: key.day,
            version: key.version,
            os: key.os,
            event: key.event,
            name: key.name,
            outcome: key.outcome,
            code: key.code,
            duration: key.duration,
            build: key.build,
            disk: key.disk,
            count,
        }
    }

    pub fn into_key_and_count(self) -> (CounterKey, u32) {
        let count = self.count;
        let key = CounterKey {
            day: self.day,
            version: self.version,
            os: self.os,
            event: self.event,
            name: self.name,
            outcome: self.outcome,
            code: self.code,
            duration: self.duration,
            build: self.build,
            disk: self.disk,
        };
        (key, count)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchPayload {
    pub schema: u32,
    pub batch: String,
    pub rows: Vec<CounterRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExamplePayload {
    pub example: bool,
    pub schema: u32,
    pub batch: String,
    pub rows: Vec<CounterRow>,
}

pub fn outcome_str(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Ok => "ok",
        Outcome::Partial => "partial",
        Outcome::Failed => "failed",
        Outcome::Cancelled => "cancelled",
    }
}

pub fn duration_bucket(ms: u64) -> &'static str {
    match ms {
        0..500 => "lt500ms",
        500..2_000 => "lt2s",
        2_000..5_000 => "lt5s",
        5_000..15_000 => "lt15s",
        15_000..60_000 => "lt60s",
        60_000..300_000 => "lt5m",
        _ => "ge5m",
    }
}

pub fn disk_bucket(bytes: Option<u64>) -> &'static str {
    const GIB: u64 = 1024 * 1024 * 1024;
    match bytes {
        None => "",
        Some(b) if b < 128 * GIB => "lt128",
        Some(b) if b < 256 * GIB => "lt256",
        Some(b) if b < 512 * GIB => "lt512",
        Some(b) if b < 1024 * GIB => "lt1024",
        Some(_) => "ge1024",
    }
}

pub fn is_valid_code(code: &str) -> bool {
    if code.is_empty() || code == "unknown" || ERROR_CODES.contains(&code) {
        return true;
    }
    if let Some(digits) = code.strip_prefix("Win32:") {
        let len = digits.len();
        return (1..=10).contains(&len) && digits.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// Validates `^crates/greenc-[a-z0-9]+/src/[A-Za-z0-9_/]{1,80}\.rs:\d{1,6}$`
/// or `"dependency"`.
pub fn is_valid_crash_name(name: &str) -> bool {
    if name == "dependency" {
        return true;
    }
    let Some((path, line_str)) = name.rsplit_once(':') else {
        return false;
    };
    if !(1..=6).contains(&line_str.len()) || !line_str.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    is_valid_crash_path(path)
}

fn is_valid_crash_path(path: &str) -> bool {
    let Some(after_prefix) = path.strip_prefix("crates/greenc-") else {
        return false;
    };
    let Some((crate_suffix, after_src)) = after_prefix.split_once("/src/") else {
        return false;
    };
    if crate_suffix.is_empty()
        || !crate_suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return false;
    }
    let Some(stem) = after_src.strip_suffix(".rs") else {
        return false;
    };
    (1..=80).contains(&stem.len())
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'/')
}

pub fn normalize_crash_location(file: &str, line: u32) -> String {
    let normalized = file.replace('\\', "/");
    let candidate = format!("{normalized}:{line}");
    if is_valid_crash_name(&candidate) && candidate != "dependency" {
        candidate
    } else {
        "dependency".to_string()
    }
}

pub struct NormalizedEvent {
    pub event: &'static str,
    pub log_name: String,
    pub outcome: &'static str,
    pub log_code: String,
    pub duration: &'static str,
    pub build: u32,
    pub disk: &'static str,
    pub countable: bool,
}

pub fn normalize_event(event: Event<'_>, env: &Env) -> NormalizedEvent {
    match event {
        Event::Active => NormalizedEvent {
            event: "active",
            log_name: String::new(),
            outcome: "",
            log_code: String::new(),
            duration: "",
            build: env.build.min(99_999),
            disk: disk_bucket(env.system_disk_bytes),
            countable: true,
        },
        Event::Start { ms } => NormalizedEvent {
            event: "start",
            log_name: String::new(),
            outcome: "",
            log_code: String::new(),
            duration: duration_bucket(ms),
            build: 0,
            disk: "",
            countable: true,
        },
        Event::Scan {
            name,
            outcome,
            code,
            ms,
        } => {
            let name_ok = SCAN_NAMES.contains(&name);
            let code_ok = is_valid_code(code);
            NormalizedEvent {
                event: "scan",
                log_name: if name_ok {
                    name.to_string()
                } else {
                    "unknown".to_string()
                },
                outcome: outcome_str(outcome),
                log_code: if code_ok {
                    code.to_string()
                } else {
                    "unknown".to_string()
                },
                duration: duration_bucket(ms),
                build: 0,
                disk: "",
                countable: name_ok && code_ok,
            }
        }
        Event::Screen { name, cached, ms } => {
            let name_ok = SCREEN_NAMES.contains(&name);
            NormalizedEvent {
                event: "screen",
                log_name: if name_ok {
                    name.to_string()
                } else {
                    "unknown".to_string()
                },
                outcome: if cached { "cached" } else { "fresh" },
                log_code: String::new(),
                duration: duration_bucket(ms),
                build: 0,
                disk: "",
                countable: name_ok,
            }
        }
        Event::Action {
            name,
            outcome,
            code,
            ms,
        } => {
            let name_ok = ACTION_NAMES.contains(&name);
            let code_ok = is_valid_code(code);
            NormalizedEvent {
                event: "action",
                log_name: if name_ok {
                    name.to_string()
                } else {
                    "unknown".to_string()
                },
                outcome: outcome_str(outcome),
                log_code: if code_ok {
                    code.to_string()
                } else {
                    "unknown".to_string()
                },
                duration: duration_bucket(ms),
                build: 0,
                disk: "",
                countable: name_ok && code_ok,
            }
        }
    }
}

pub fn is_day_in_window(day_str: &str, today: NaiveDate) -> bool {
    let Ok(day) = NaiveDate::parse_from_str(day_str, "%Y-%m-%d") else {
        return false;
    };
    let delta = (day - today).num_days();
    (-7..=1).contains(&delta)
}

/// Builds a batch JSON string from candidate rows, fitting at most `MAX_BATCH_ROWS`
/// and `MAX_BATCH_BYTES`. Returns `(taken_count, serialized_body)`.
pub fn pack_batch_rows(batch_id: &str, candidates: &[CounterRow]) -> Option<(usize, String)> {
    let mut chosen = Vec::new();
    let base_len = r#"{"schema":1,"batch":"","rows":[]}"#.len() + batch_id.len();
    let mut running_len = base_len;

    for row in candidates.iter().take(MAX_BATCH_ROWS) {
        let Ok(row_json) = serde_json::to_string(row) else {
            continue;
        };
        let sep = usize::from(!chosen.is_empty());
        if running_len + sep + row_json.len() > MAX_BATCH_BYTES {
            break;
        }
        running_len += sep + row_json.len();
        chosen.push(row.clone());
    }

    if chosen.is_empty() {
        return None;
    }

    let taken = chosen.len();
    let payload = BatchPayload {
        schema: 1,
        batch: batch_id.to_string(),
        rows: chosen,
    };
    let body = serde_json::to_string(&payload).ok()?;
    if body.len() > MAX_BATCH_BYTES {
        return None;
    }
    Some((taken, body))
}
