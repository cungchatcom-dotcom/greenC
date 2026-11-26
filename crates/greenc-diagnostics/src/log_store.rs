use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_LOG_FILE_BYTES: u64 = 1_048_576; // 1 MiB
const MAX_LOG_FILES: usize = 5;
const LOG_RETENTION_DAYS: i64 = 7;

pub struct LogStore {
    log_dir: PathBuf,
    last_prune_day: Option<NaiveDate>,
}

impl LogStore {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            log_dir: base_dir.join("log"),
            last_prune_day: None,
        }
    }

    fn log_path(&self, index: usize) -> PathBuf {
        self.log_dir.join(format!("greenc-{index}.log"))
    }

    pub fn append_entry(
        &mut self,
        now: DateTime<Utc>,
        event: &str,
        name: &str,
        outcome: &str,
        code: &str,
        duration: &str,
    ) {
        let today = now.date_naive();
        if self.last_prune_day != Some(today) {
            self.prune_old(now);
            self.last_prune_day = Some(today);
        }

        if fs::create_dir_all(&self.log_dir).is_err() {
            return;
        }

        let line = format!(
            "{} {} {} {} {} {}\n",
            now.format("%Y-%m-%dT%H:%M:%SZ"),
            event,
            dash_if_empty(name),
            dash_if_empty(outcome),
            dash_if_empty(code),
            dash_if_empty(duration),
        );

        let active_path = self.log_path(1);
        let current_len = fs::metadata(&active_path).map(|m| m.len()).unwrap_or(0);
        if current_len > 0 && current_len.saturating_add(line.len() as u64) > MAX_LOG_FILE_BYTES {
            self.rotate();
        }

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn rotate(&self) {
        let oldest = self.log_path(MAX_LOG_FILES);
        let _ = fs::remove_file(&oldest);

        for idx in (1..MAX_LOG_FILES).rev() {
            let src = self.log_path(idx);
            let dst = self.log_path(idx + 1);
            if src.exists() {
                let _ = fs::rename(&src, &dst);
            }
        }
    }

    pub fn prune_old(&self, now: DateTime<Utc>) {
        let Ok(entries) = fs::read_dir(&self.log_dir) else {
            return;
        };
        let cutoff_ts = now - Duration::days(LOG_RETENTION_DAYS);
        let wall_cutoff =
            SystemTime::now().checked_sub(std::time::Duration::from_secs(7 * 24 * 3600));

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !file_name.starts_with("greenc-") || !file_name.ends_with(".log") {
                continue;
            }

            let expired_by_mtime = wall_cutoff
                .and_then(|cutoff| {
                    entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|modified| modified < cutoff)
                })
                .unwrap_or(false);

            let expired_by_content = last_line_timestamp(&path)
                .map(|last_ts| last_ts < cutoff_ts)
                .unwrap_or(false);

            if expired_by_mtime || expired_by_content {
                let _ = fs::remove_file(&path);
            }
        }
    }

    pub fn export_to(&self, dest: &Path) -> io::Result<()> {
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut out = File::create(dest)?;
        for idx in (1..=MAX_LOG_FILES).rev() {
            let path = self.log_path(idx);
            if let Ok(mut file) = File::open(&path) {
                io::copy(&mut file, &mut out)?;
            }
        }
        Ok(())
    }

    pub fn clear(&self) {
        let Ok(entries) = fs::read_dir(&self.log_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("greenc-") && name.ends_with(".log") {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
}

fn dash_if_empty(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

fn last_line_timestamp(path: &Path) -> Option<DateTime<Utc>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let read_size = len.min(512) as usize;
    use std::io::Seek;
    file.seek(io::SeekFrom::End(-(read_size as i64))).ok()?;
    let mut buf = vec![0u8; read_size];
    file.read_exact(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let last_line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let ts_token = last_line.split_whitespace().next()?;
    DateTime::parse_from_rfc3339(ts_token)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
