use greenc_core::error::GreenCError;
use greenc_core::task::{ProgressReporter, TaskControl, TaskProgress};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn test_cancellation() {
    let ctrl = Arc::new(TaskControl::new());
    let ctrl_clone = Arc::clone(&ctrl);

    let handle = thread::spawn(move || {
        for _ in 0..100 {
            ctrl_clone.check_pause_or_cancel()?;
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    });

    thread::sleep(Duration::from_millis(15));
    ctrl.cancel();

    let result = handle.join().unwrap();
    assert!(matches!(result, Err(GreenCError::Cancelled)));
    assert!(ctrl.is_cancelled());
}

#[test]
fn test_pause_and_resume() {
    let ctrl = Arc::new(TaskControl::new());
    let ctrl_clone = Arc::clone(&ctrl);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);

    ctrl.pause();
    assert!(ctrl.is_paused());

    let handle = thread::spawn(move || {
        for _ in 0..10 {
            ctrl_clone.check_pause_or_cancel().unwrap();
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Wait and ensure counter hasn't incremented while paused
    thread::sleep(Duration::from_millis(30));
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Resume
    ctrl.resume();
    assert!(!ctrl.is_paused());

    handle.join().unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

#[test]
fn test_progress_throttling() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let callback = Arc::new(move |_progress: TaskProgress| {
        call_count_clone.fetch_add(1, Ordering::SeqCst);
    });

    let reporter = ProgressReporter::new("test_task".to_string(), 1000, 100, callback);

    let start = std::time::Instant::now();
    // Rapid reports without force_emit
    for i in 0..50 {
        reporter.report("scanning", "item.txt", i * 20, i * 2, false);
        thread::sleep(Duration::from_millis(2));
    }
    let elapsed_ms = start.elapsed().as_millis() as usize;

    // Reports should be throttled to at most ~1 emission per 100ms window (accounting for VM timer jitter)
    let max_expected = (elapsed_ms / 100) + 3;
    let count = call_count.load(Ordering::SeqCst);
    assert!(
        count <= max_expected && count < 50,
        "Expected throttled calls <= {} (and < 50) over {}ms, got {}",
        max_expected,
        elapsed_ms,
        count
    );

    // Forced emit should always fire
    reporter.report("scanning", "final.txt", 1000, 100, true);
    assert_eq!(call_count.load(Ordering::SeqCst), count + 1);
}

#[test]
fn read_only_scans_are_told_apart_from_work_on_files() {
    for id in [
        "space-lens-C-1",
        "cleanup-scan-1",
        "relocate-scan-1",
        "compact-scan-1",
    ] {
        assert!(greenc_core::task::is_read_only_scan(id), "{id}");
    }
    for id in [
        "cleanup-exec-1",
        "relocate-exec-1",
        "relocate-kf-1",
        "relocate-undo-1",
        "compact-exec-1",
        "uncompact-exec-1",
        "compact-vhdx-1",
        "zalo-clean-1",
        "zalo-move-1",
        "move-vault-1",
        // Rewrites the history database.
        "history-scan-C-1",
        // Anything unknown counts as work on files.
        "something-new-1",
    ] {
        assert!(!greenc_core::task::is_read_only_scan(id), "{id}");
    }
}
