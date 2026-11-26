use greenc_core::error::Result;
use greenc_core::task::{is_read_only_scan, TaskControl};
use greenc_i18n::catalog::Catalog;
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::AppSettings;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgressEvent {
    pub task_id: String,
    pub kind: String,
    pub state: String,
    pub percent: u32,
    pub phase: String,
    pub title: String,
    pub detail: String,
    pub done_files: u64,
    pub total_files: u64,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub throughput_bps: u64,
    pub eta_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchdogAlertEvent {
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub archive_id: Option<i64>,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<GreenCDb>,
    pub catalog: Arc<Catalog>,
    pub active_tasks: Arc<RwLock<HashMap<String, Arc<TaskControl>>>>,
    pub app_handle: Arc<RwLock<Option<AppHandle>>>,
    pub app_dir: PathBuf,
    busy_marker: Arc<Mutex<Option<greenc_platform::BusyMarker>>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("app_dir", &self.app_dir)
            .field("has_foreground_tasks", &self.has_foreground_tasks())
            .finish()
    }
}

impl AppState {
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let db = Arc::new(GreenCDb::open(&db_path)?);
        let catalog = Arc::new(Catalog::load_embedded());
        let app_dir = db_path.parent().unwrap_or(&db_path).to_path_buf();

        Ok(Self {
            db,
            catalog,
            active_tasks: Arc::new(RwLock::new(HashMap::new())),
            app_handle: Arc::new(RwLock::new(None)),
            app_dir,
            busy_marker: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_app_handle(&self, handle: AppHandle) {
        if let Ok(mut h) = self.app_handle.write() {
            *h = Some(handle);
        }
    }

    pub fn settings(&self) -> AppSettings<'_> {
        AppSettings::new(&self.db)
    }

    pub fn register_task(&self, task_id: String, control: Arc<TaskControl>) {
        if let Ok(mut tasks) = self.active_tasks.write() {
            tasks.insert(task_id, control);
            if let Ok(mut marker) = self.busy_marker.lock() {
                if marker.is_none() {
                    *marker = greenc_platform::BusyMarker::hold();
                }
            }
        }
    }

    pub fn unregister_task(&self, task_id: &str) {
        if let Ok(mut tasks) = self.active_tasks.write() {
            tasks.remove(task_id);
            if tasks.is_empty() {
                if let Ok(mut marker) = self.busy_marker.lock() {
                    *marker = None;
                }
            }
        }
    }

    /// `true` while a task works on the user's files. Read-only scans, which screens start by
    /// themselves, do not count.
    pub fn has_foreground_tasks(&self) -> bool {
        if let Ok(tasks) = self.active_tasks.read() {
            tasks.keys().any(|id| !is_read_only_scan(id))
        } else {
            false
        }
    }

    pub fn control_task(&self, task_id: &str, action: &str) -> bool {
        if let Ok(tasks) = self.active_tasks.read() {
            if let Some(ctrl) = tasks.get(task_id) {
                match action {
                    "pause" => {
                        ctrl.pause();
                        true
                    }
                    "resume" => {
                        ctrl.resume();
                        true
                    }
                    "cancel" => {
                        ctrl.cancel();
                        true
                    }
                    _ => false,
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn emit_progress(&self, event: &TaskProgressEvent) {
        if let Ok(guard) = self.app_handle.read() {
            if let Some(handle) = guard.as_ref() {
                let _ = handle.emit("greenc://task-progress", event);
            }
        }
    }

    pub fn emit_alert(&self, alert: &WatchdogAlertEvent) {
        if let Ok(guard) = self.app_handle.read() {
            if let Some(handle) = guard.as_ref() {
                let _ = handle.emit("greenc://watchdog-alert", alert);
            }
        }
    }
}
