//! Tests for `wait_for_processes_to_exit` after `RmShutdown` (Item 11).

use greenc_platform::app_lifecycle::wait_for_processes_to_exit;
use std::time::Duration;

#[test]
fn item11_wait_for_processes_succeeds_when_app_exits_after_rm_shutdown_error() {
    let mut polls = 0usize;
    let exited =
        wait_for_processes_to_exit(Duration::from_millis(200), Duration::from_millis(5), || {
            polls += 1;
            polls >= 3
        });
    assert!(
        exited,
        "must wait and succeed when target processes finish exiting before timeout"
    );
    assert_eq!(polls, 3);
}

#[test]
fn item11_wait_for_processes_returns_false_only_after_timeout() {
    let mut polls = 0usize;
    let exited =
        wait_for_processes_to_exit(Duration::from_millis(25), Duration::from_millis(5), || {
            polls += 1;
            false
        });
    assert!(
        !exited,
        "must return false if processes remain running past timeout"
    );
    assert!(polls >= 2);
}
