//! Daily disk usage snapshots, 7/14/30-day delta reconstruction (`>= 64 KiB` bucket threshold),
//! per-day folder/actor drill-down, and `notify` live write/delete coalescing (Task T055).

use crate::attribution::AttributionEngine;
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{ActorWriteRow, DailySnapshot, FolderGrowthRow, WriteEvent};
use greenc_core::task::TaskControl;
use greenc_platform::traits::Win32VolumeApi;
use greenc_platform::volume::NativeWin32VolumeApi;
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

/// Minimum aggregated folder bucket size (`64 KiB`) recorded into `write_events`.
pub const MIN_BUCKET_DELTA_BYTES: i64 = 64 * 1024;

/// Drill-down payload returned when a user clicks a specific bar in the daily chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyDrilldown {
    pub drive_letter: String,
    pub date: String,
    pub folders: Vec<FolderGrowthRow>,
    pub actors: Vec<ActorWriteRow>,
}

#[derive(Default)]
struct FolderBucket {
    total_bytes: i64,
    file_count: u64,
    sample_file: Option<PathBuf>,
}

/// Tracks daily snapshots and attributes disk write/delete deltas to folders and processes.
pub struct DiskTracker {
    db: Arc<GreenCDb>,
    attribution: Mutex<AttributionEngine>,
    volume_api: Arc<dyn Win32VolumeApi>,
}

impl std::fmt::Debug for DiskTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskTracker")
            .field("db_path", &self.db.path())
            .finish()
    }
}

impl DiskTracker {
    pub fn new(db: Arc<GreenCDb>) -> Self {
        Self {
            db,
            attribution: Mutex::new(AttributionEngine::default()),
            volume_api: Arc::new(NativeWin32VolumeApi),
        }
    }

    pub fn with_dependencies(
        db: Arc<GreenCDb>,
        attribution: AttributionEngine,
        volume_api: Arc<dyn Win32VolumeApi>,
    ) -> Self {
        Self {
            db,
            attribution: Mutex::new(attribution),
            volume_api,
        }
    }

    /// Normalizes Windows drives (`"c"`, `"C:"`, or `"C:\\"`) into `"C:\\"`,
    /// and Unix mount points (`"/"`, `"/home/"`, `"/Volumes/Data"`) into canonical paths.
    pub fn normalize_drive(drive: &str) -> String {
        let trimmed = drive.trim();
        if trimmed.starts_with('/') {
            let stripped = trimmed.trim_end_matches('/');
            return if stripped.is_empty() {
                "/".to_string()
            } else {
                stripped.to_string()
            };
        }
        let clean = trimmed.trim_end_matches(['\\', '/']);
        if clean.is_empty() {
            return if cfg!(unix) {
                "/".to_string()
            } else {
                "C:\\".to_string()
            };
        }
        let letter = clean.chars().next().unwrap_or('C').to_ascii_uppercase();
        format!("{letter}:\\")
    }

    /// Captures and upserts today's `DailySnapshot` for `drive`.
    pub fn capture_current_snapshot(&self, drive: &str) -> Result<DailySnapshot> {
        let drive_norm = Self::normalize_drive(drive);
        let root_path = PathBuf::from(&drive_norm);
        let (total, free) = self.volume_api.get_disk_free_space(&root_path)?;
        let used = total.saturating_sub(free);
        let today = Local::now().date_naive();
        let today_str = today.format("%Y-%m-%d").to_string();

        let existing = self.db.get_daily_snapshots(&drive_norm, 30)?;
        let prev_used = existing
            .iter()
            .rev()
            .find(|s| s.snapshot_date < today)
            .map(|s| s.used_bytes);

        let net_change = prev_used.map(|prev| (used as i64) - (prev as i64));
        self.db
            .upsert_daily_snapshot(&drive_norm, &today_str, total, used, free, net_change)
    }

    /// Scans `scan_roots` on `drive`, deduplicates NTFS hardlinks, groups files modified within
    /// `days_back` (`7`, `14`, or `30` days), populates `write_events`, and rebuilds daily snapshots.
    pub fn scan_drive_history_with_roots(
        &self,
        drive: &str,
        days_back: u32,
        scan_roots: &[PathBuf],
        control: Option<&TaskControl>,
    ) -> Result<Vec<DailySnapshot>> {
        let days = days_back.clamp(1, 90);
        let drive_norm = Self::normalize_drive(drive);
        let root_path = PathBuf::from(&drive_norm);

        if let Some(ctrl) = control {
            if ctrl.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }
        }

        if let Ok(mut attr) = self.attribution.lock() {
            attr.refresh_service_map();
        }

        let (total_bytes, free_bytes) = self.volume_api.get_disk_free_space(&root_path)?;
        let current_used = total_bytes.saturating_sub(free_bytes);

        let today: NaiveDate = Local::now().date_naive();
        let cutoff: NaiveDate = today - Duration::days(i64::from(days.saturating_sub(1)));

        let mut seen_identities: HashSet<(u64, u64)> = HashSet::new();
        let mut grouped: HashMap<String, HashMap<PathBuf, FolderBucket>> = HashMap::new();
        let mut daily_totals: HashMap<String, i64> = HashMap::new();

        for root in scan_roots {
            if !root.exists() {
                continue;
            }
            self.walk_history_root(
                root,
                cutoff,
                today,
                control,
                &mut seen_identities,
                &mut grouped,
                &mut daily_totals,
            )?;
        }

        // Clear stale write_events for this drive within our scanned date window before inserting fresh rows.
        let cutoff_str = cutoff.format("%Y-%m-%d").to_string();
        self.db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM write_events WHERE drive_letter = ?1 AND event_date >= ?2;",
                rusqlite::params![drive_norm, cutoff_str],
            )
            .map_err(|e| GreenCError::Sqlite(e.to_string()))?;
            Ok(())
        })?;

        let mut events_to_insert: Vec<WriteEvent> = Vec::new();
        let attr_guard = self
            .attribution
            .lock()
            .map_err(|e| GreenCError::Scanner(format!("Attribution lock poisoned: {e}")))?;

        let mut sorted_dates: Vec<String> = grouped.keys().cloned().collect();
        sorted_dates.sort();

        for date_str in sorted_dates {
            if let Some(folder_map) = grouped.get(&date_str) {
                for (folder_path, bucket) in folder_map {
                    if bucket.total_bytes.abs() < MIN_BUCKET_DELTA_BYTES {
                        continue;
                    }
                    let sample = bucket
                        .sample_file
                        .as_deref()
                        .unwrap_or(folder_path.as_path());
                    let matched = attr_guard.attribute_path(sample, false);
                    events_to_insert.push(WriteEvent {
                        id: None,
                        event_date: date_str.clone(),
                        timestamp: Utc::now(),
                        drive_letter: drive_norm.clone(),
                        folder_path: folder_path.clone(),
                        file_path: sample.to_path_buf(),
                        delta_bytes: bucket.total_bytes,
                        process_name: matched.process_name,
                        pid: matched.pid,
                        service_name: matched.service_name,
                        service_display_name: matched.service_display_name,
                        reason_code: matched.reason_code,
                        reason_detail: format!(
                            "{} ({} files)",
                            matched.reason_detail, bucket.file_count
                        ),
                    });
                }
            }
        }
        drop(attr_guard);

        if !events_to_insert.is_empty() {
            self.db.record_write_events_batch(&events_to_insert)?;
        }

        // Reconstruct ascending date sequence [today - (days - 1) ..= today]
        let dates_asc: Vec<NaiveDate> = (0..days)
            .rev()
            .map(|offset| today - Duration::days(i64::from(offset)))
            .collect();

        let mut used_by_day: HashMap<String, u64> = HashMap::new();
        let mut running_used = current_used as i64;

        for d in dates_asc.iter().rev() {
            let d_str = d.format("%Y-%m-%d").to_string();
            used_by_day.insert(d_str.clone(), running_used.max(0) as u64);
            let delta = daily_totals.get(&d_str).copied().unwrap_or(0);
            running_used = (running_used - delta).max(0);
        }

        let mut snapshots = Vec::with_capacity(dates_asc.len());
        for d in &dates_asc {
            let d_str = d.format("%Y-%m-%d").to_string();
            let u_b = used_by_day.get(&d_str).copied().unwrap_or(current_used);
            let f_b = total_bytes.saturating_sub(u_b);
            let n_b = daily_totals.get(&d_str).copied().unwrap_or(0);
            let snap = self.db.upsert_daily_snapshot(
                &drive_norm,
                &d_str,
                total_bytes,
                u_b,
                f_b,
                Some(n_b),
            )?;
            snapshots.push(snap);
        }

        Ok(snapshots)
    }

    /// Scans default candidate roots on `drive`.
    pub fn scan_drive_history(
        &self,
        drive: &str,
        days_back: u32,
        control: Option<&TaskControl>,
    ) -> Result<Vec<DailySnapshot>> {
        let drive_norm = Self::normalize_drive(drive);
        let roots = Self::default_candidate_roots(&drive_norm);
        self.scan_drive_history_with_roots(&drive_norm, days_back, &roots, control)
    }

    /// Records a real-time file write (`delta_bytes > 0`) or deletion (`delta_bytes < 0`) event
    /// from a `ReadDirectoryChangesW` (`notify`) stream and updates today's `DailySnapshot`.
    pub fn record_live_delta(&self, drive: &str, file_path: &Path, delta_bytes: i64) -> Result<()> {
        if delta_bytes == 0 {
            return Ok(());
        }
        let drive_norm = Self::normalize_drive(drive);
        let today_str = Local::now().date_naive().format("%Y-%m-%d").to_string();
        let folder = file_path.parent().unwrap_or(file_path).to_path_buf();

        let matched = self
            .attribution
            .lock()
            .map_err(|e| GreenCError::Scanner(format!("Attribution lock poisoned: {e}")))?
            .attribute_path(file_path, delta_bytes > 0);

        let ev = WriteEvent {
            id: None,
            event_date: today_str.clone(),
            timestamp: Utc::now(),
            drive_letter: drive_norm.clone(),
            folder_path: folder,
            file_path: file_path.to_path_buf(),
            delta_bytes,
            process_name: matched.process_name,
            pid: matched.pid,
            service_name: matched.service_name,
            service_display_name: matched.service_display_name,
            reason_code: matched.reason_code,
            reason_detail: matched.reason_detail,
        };
        self.db.record_write_event(&ev)?;

        // Refresh today's snapshot net_change_bytes with signed sum if no prior day exists, or live volume delta.
        let root_path = PathBuf::from(&drive_norm);
        if let Ok((total, free)) = self.volume_api.get_disk_free_space(&root_path) {
            let used = total.saturating_sub(free);
            let existing = self.db.get_daily_snapshots(&drive_norm, 2)?;
            let prev_net = existing
                .iter()
                .find(|s| s.snapshot_date.format("%Y-%m-%d").to_string() == today_str)
                .map(|s| s.net_change_bytes + delta_bytes)
                .unwrap_or(delta_bytes);
            let _ = self.db.upsert_daily_snapshot(
                &drive_norm,
                &today_str,
                total,
                used,
                free,
                Some(prev_net),
            );
        }

        Ok(())
    }

    /// Queries the folder/reason and process/service breakdown for a single calendar date.
    pub fn get_daily_drilldown(&self, drive: &str, date: &str) -> Result<DailyDrilldown> {
        let drive_norm = Self::normalize_drive(drive);
        let folders = self.db.get_day_folder_breakdown(&drive_norm, date)?;
        let actors = self.db.get_day_actor_breakdown(&drive_norm, date)?;
        Ok(DailyDrilldown {
            drive_letter: drive_norm,
            date: date.to_string(),
            folders,
            actors,
        })
    }

    pub fn default_candidate_roots(drive_norm: &str) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if drive_norm.starts_with('/') {
            let mount_path = Path::new(drive_norm);
            if let Ok(home) = std::env::var("HOME") {
                let h = PathBuf::from(home);
                for sub in [
                    "Downloads",
                    ".cache",
                    ".local/share",
                    ".npm",
                    ".gradle",
                    ".cargo",
                    "Library/Caches",
                ] {
                    let cand = h.join(sub);
                    if cand.exists() && greenc_core::models::is_same_volume(&cand, mount_path) {
                        roots.push(cand);
                    }
                }
            }
            if drive_norm == "/" {
                for sys_tmp in ["/tmp", "/var/tmp"] {
                    let p = PathBuf::from(sys_tmp);
                    if p.exists() && greenc_core::models::is_same_volume(&p, mount_path) {
                        roots.push(p);
                    }
                }
            } else if roots.is_empty() {
                if let Ok(entries) = fs::read_dir(mount_path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        let name = entry.file_name().to_string_lossy().to_lowercase();
                        if p.is_dir()
                            && !matches!(
                                name.as_str(),
                                "lost+found"
                                    | ".trash"
                                    | ".trashes"
                                    | ".spotlight-v100"
                                    | ".fseventsd"
                                    | "greenc"
                            )
                        {
                            roots.push(p);
                        }
                    }
                }
            }
        } else if drive_norm.starts_with("C:") {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let p = PathBuf::from(appdata);
                for sub in ["ZaloData", "DingTalk", "Telegram Desktop", "npm-cache"] {
                    let cand = p.join(sub);
                    if cand.exists() {
                        roots.push(cand);
                    }
                }
            }
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let p = PathBuf::from(local);
                for sub in ["Temp", "DingTalk", "Zalo", "zalo-updater", "uv", "pip"] {
                    let cand = p.join(sub);
                    if cand.exists() {
                        roots.push(cand);
                    }
                }
                let zalo_prog = p.join("Programs").join("Zalo");
                if zalo_prog.exists() {
                    roots.push(zalo_prog);
                }
            }
            if let Ok(profile) = std::env::var("USERPROFILE") {
                let p = PathBuf::from(profile);
                let dl = p.join("Downloads");
                if dl.exists() {
                    roots.push(dl);
                }
                let zalo_docs = p.join("Documents").join("Zalo Received Files");
                if zalo_docs.exists() {
                    roots.push(zalo_docs);
                }
            }
        } else {
            let root = PathBuf::from(drive_norm);
            if let Ok(entries) = fs::read_dir(&root) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if p.is_dir()
                        && !matches!(
                            name.as_str(),
                            "$recycle.bin" | "system volume information" | "greenc"
                        )
                    {
                        roots.push(p);
                    }
                }
            }
        }
        roots
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_history_root(
        &self,
        root: &Path,
        cutoff: NaiveDate,
        today: NaiveDate,
        control: Option<&TaskControl>,
        seen_identities: &mut HashSet<(u64, u64)>,
        grouped: &mut HashMap<String, HashMap<PathBuf, FolderBucket>>,
        daily_totals: &mut HashMap<String, i64>,
    ) -> Result<()> {
        let mut stack = vec![root.to_path_buf()];
        let mut visited_dirs: usize = 0;

        while let Some(dir) = stack.pop() {
            visited_dirs += 1;
            if visited_dirs % 32 == 0 {
                if let Some(ctrl) = control {
                    ctrl.wait_if_paused()?;
                    if ctrl.is_cancelled() {
                        return Err(GreenCError::Cancelled);
                    }
                }
            }

            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(sym_meta) = fs::symlink_metadata(&path) else {
                    continue;
                };

                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
                    if (sym_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                        continue;
                    }
                }
                #[cfg(not(windows))]
                {
                    if sym_meta.file_type().is_symlink() {
                        continue;
                    }
                }

                if sym_meta.is_dir() {
                    let name_low = entry.file_name().to_string_lossy().to_lowercase();
                    if !matches!(
                        name_low.as_str(),
                        "$recycle.bin"
                            | "system volume information"
                            | "windows"
                            | ".git"
                            | ".venv"
                            | "vault"
                    ) {
                        stack.push(path);
                    }
                    continue;
                }

                if !sym_meta.is_file() || sym_meta.len() == 0 {
                    continue;
                }

                let dedup_key = file_dedup_identity(&path, sym_meta.len());
                if !seen_identities.insert(dedup_key) {
                    continue;
                }

                let Ok(modified) = sym_meta.modified() else {
                    continue;
                };
                let Ok(dur) = modified.duration_since(UNIX_EPOCH) else {
                    continue;
                };
                let Some(dt_utc) = DateTime::from_timestamp(dur.as_secs() as i64, 0) else {
                    continue;
                };
                let file_date = dt_utc.with_timezone(&Local).date_naive();
                if file_date < cutoff || file_date > today {
                    continue;
                }

                let date_str = file_date.format("%Y-%m-%d").to_string();
                let bucket_folder =
                    Self::summarize_folder_path(path.parent().unwrap_or(root), root);
                let sz = sym_meta.len() as i64;

                let bucket = grouped
                    .entry(date_str.clone())
                    .or_default()
                    .entry(bucket_folder)
                    .or_default();
                bucket.total_bytes += sz;
                bucket.file_count += 1;
                if bucket.sample_file.is_none() {
                    bucket.sample_file = Some(path);
                }

                *daily_totals.entry(date_str).or_insert(0) += sz;
            }
        }

        Ok(())
    }

    pub fn summarize_folder_path(folder: &Path, root: &Path) -> PathBuf {
        let f_str = folder.to_string_lossy().replace('/', "\\");
        let lower = f_str.to_lowercase();
        for marker in [
            "\\resource_cache\\",
            "\\zalodownloads\\resource\\",
            "\\zalodownloads\\filenoise",
            "\\eappfiles\\download",
            "\\eappfiles\\unziped",
            "\\avatarfiles",
            "\\database\\_production",
        ] {
            if let Some(idx) = lower.find(marker) {
                let cut = idx + marker.trim_end_matches('\\').len();
                return PathBuf::from(&f_str[..cut]);
            }
        }

        if let Ok(rel) = folder.strip_prefix(root) {
            let comps: Vec<_> = rel.components().take(2).collect();
            if comps.len() == 2 {
                let mut joined = root.to_path_buf();
                for c in comps {
                    joined.push(c.as_os_str());
                }
                return joined;
            }
        }
        folder.to_path_buf()
    }
}

fn file_dedup_identity(path: &Path, fallback_len: u64) -> (u64, u64) {
    if let Some(id) = crate::walker::FileIdentity::from_path(path) {
        if id.file_index != 0 {
            return (id.volume_serial, id.file_index);
        }
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut hasher);
    (fallback_len, hasher.finish())
}
