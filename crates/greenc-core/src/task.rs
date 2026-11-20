use crate::error::{GreenCError, Result};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Real-time progress payload broadcasted to UI and CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task_id: String,
    pub phase: String,
    pub current_item: String,
    pub processed_bytes: u64,
    pub total_bytes: u64,
    pub processed_items: u64,
    pub total_items: u64,
    pub percentage: f32,
    pub bytes_per_sec: f64,
    pub eta_seconds: Option<u64>,
}

pub type ProgressCallback = Arc<dyn Fn(u32, &str) + Send + Sync>;
pub type WalkProgressCallback = Arc<dyn Fn(&std::path::Path, u64, u64) + Send + Sync>;

/// Cooperative task lifecycle controller (Pause, Resume, Stop).
pub struct TaskControl {
    cancelled: AtomicBool,
    paused: AtomicBool,
    pause_lock: Mutex<()>,
    pause_condvar: Condvar,
    progress_callback: Mutex<Option<ProgressCallback>>,
    walk_progress_callback: Mutex<Option<WalkProgressCallback>>,
}

impl std::fmt::Debug for TaskControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskControl")
            .field("cancelled", &self.is_cancelled())
            .field("paused", &self.is_paused())
            .finish()
    }
}

impl Default for TaskControl {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskControl {
    pub fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            pause_lock: Mutex::new(()),
            pause_condvar: Condvar::new(),
            progress_callback: Mutex::new(None),
            walk_progress_callback: Mutex::new(None),
        }
    }

    pub fn set_progress_callback(&self, cb: ProgressCallback) {
        let mut guard = self.progress_callback.lock();
        *guard = Some(cb);
    }

    pub fn report_progress(&self, percent: u32, message: &str) {
        let guard = self.progress_callback.lock();
        if let Some(cb) = guard.as_ref() {
            cb(percent, message);
        }
    }

    pub fn set_walk_progress_callback(&self, cb: WalkProgressCallback) {
        let mut guard = self.walk_progress_callback.lock();
        *guard = Some(cb);
    }

    pub fn report_walk_progress(&self, path: &std::path::Path, files: u64, bytes: u64) {
        let guard = self.walk_progress_callback.lock();
        if let Some(cb) = guard.as_ref() {
            cb(path, files, bytes);
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.resume(); // Unblock if currently paused
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.pause_condvar.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Waits while paused without returning an error (used inside loops that check `is_cancelled` separately).
    pub fn check_pause(&self) {
        if self.is_paused() {
            let mut guard = self.pause_lock.lock();
            while self.is_paused() && !self.is_cancelled() {
                self.pause_condvar.wait(&mut guard);
            }
        }
    }

    /// Alias for `check_pause_or_cancel` used by background/scanner workers.
    pub fn wait_if_paused(&self) -> Result<()> {
        self.check_pause_or_cancel()
    }

    /// Checks cancellation and pauses the current thread until resumed.
    /// Returns `Err(GreenCError::Cancelled)` if cancelled before or during pause.
    pub fn check_pause_or_cancel(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }

        if self.is_paused() {
            let mut guard = self.pause_lock.lock();
            while self.is_paused() && !self.is_cancelled() {
                self.pause_condvar.wait(&mut guard);
            }
            if self.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }
        }

        Ok(())
    }
}

/// Throttled progress callback manager preventing GUI event saturation.
pub struct ProgressReporter {
    task_id: String,
    total_bytes: u64,
    total_items: u64,
    start_time: Instant,
    last_emit_time: Mutex<Instant>,
    min_interval: Duration,
    callback: Arc<dyn Fn(TaskProgress) + Send + Sync>,
}

impl std::fmt::Debug for ProgressReporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressReporter")
            .field("task_id", &self.task_id)
            .field("total_bytes", &self.total_bytes)
            .field("total_items", &self.total_items)
            .finish()
    }
}

impl ProgressReporter {
    pub fn new(
        task_id: String,
        total_bytes: u64,
        total_items: u64,
        callback: Arc<dyn Fn(TaskProgress) + Send + Sync>,
    ) -> Self {
        let now = Instant::now();
        Self {
            task_id,
            total_bytes,
            total_items,
            start_time: now,
            last_emit_time: Mutex::new(now.checked_sub(Duration::from_secs(1)).unwrap_or(now)),
            min_interval: Duration::from_millis(100), // Throttled to 100 ms
            callback,
        }
    }

    pub fn report(
        &self,
        phase: &str,
        current_item: &str,
        processed_bytes: u64,
        processed_items: u64,
        force_emit: bool,
    ) {
        let now = Instant::now();
        let mut last_emit = self.last_emit_time.lock();

        if !force_emit && now.duration_since(*last_emit) < self.min_interval {
            return;
        }

        *last_emit = now;

        let elapsed = now.duration_since(self.start_time).as_secs_f64();
        let bytes_per_sec = if elapsed > 0.05 {
            processed_bytes as f64 / elapsed
        } else {
            0.0
        };

        let eta_seconds = if bytes_per_sec > 1024.0 && self.total_bytes > processed_bytes {
            let remaining_bytes = self.total_bytes - processed_bytes;
            Some((remaining_bytes as f64 / bytes_per_sec) as u64)
        } else {
            None
        };

        let percentage = if self.total_bytes > 0 {
            (processed_bytes as f32 / self.total_bytes as f32) * 100.0
        } else if self.total_items > 0 {
            (processed_items as f32 / self.total_items as f32) * 100.0
        } else {
            0.0
        };

        (self.callback)(TaskProgress {
            task_id: self.task_id.clone(),
            phase: phase.to_string(),
            current_item: current_item.to_string(),
            processed_bytes,
            total_bytes: self.total_bytes,
            processed_items,
            total_items: self.total_items,
            percentage: percentage.clamp(0.0, 100.0),
            bytes_per_sec,
            eta_seconds,
        });
    }
}
