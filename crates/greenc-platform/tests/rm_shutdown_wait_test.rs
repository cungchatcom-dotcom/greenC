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

#[test]
fn f10_session_filter_ignores_other_sessions_for_shutdown_and_liveness() {
    use greenc_platform::app_lifecycle::{
        has_live_process_in_session, select_rm_target_processes_in_session, RmProcessCandidate,
        SessionProcessCandidate,
    };
    use std::path::PathBuf;

    let candidates = vec![
        SessionProcessCandidate {
            process: RmProcessCandidate {
                pid: 1001,
                exe_path: PathBuf::from(r"C:\Users\Alice\AppData\Local\Programs\Zalo\Zalo.exe"),
                start_time_low: 1,
                start_time_high: 1,
            },
            session_id: Some(1),
        },
        SessionProcessCandidate {
            process: RmProcessCandidate {
                pid: 2002,
                exe_path: PathBuf::from(r"C:\Users\Bob\AppData\Local\Programs\Zalo\Zalo.exe"),
                start_time_low: 2,
                start_time_high: 2,
            },
            session_id: Some(2),
        },
        SessionProcessCandidate {
            process: RmProcessCandidate {
                pid: 3003,
                exe_path: PathBuf::from(r"C:\Users\Unknown\Zalo.exe"),
                start_time_low: 3,
                start_time_high: 3,
            },
            session_id: None,
        },
    ];

    // Session 1 sees ONLY PID 1001 (never Bob's session 2 or unknown session)
    let s1 = select_rm_target_processes_in_session(&candidates, &["Zalo.exe"], 1);
    assert_eq!(s1.len(), 1);
    assert_eq!(s1[0].pid, 1001);
    assert!(has_live_process_in_session(&candidates, &["Zalo.exe"], 1));

    // Session 3 has NO Zalo process — Bob's session 2 MUST NOT count as "alive"!
    let s3 = select_rm_target_processes_in_session(&candidates, &["Zalo.exe"], 3);
    assert!(s3.is_empty());
    assert!(
        !has_live_process_in_session(&candidates, &["Zalo.exe"], 3),
        "other session's process MUST NOT count as alive"
    );
}

#[test]
fn f10_shutdown_timeout_relaunches_already_closed_exes_before_returning_error() {
    use greenc_core::error::GreenCError;
    use greenc_platform::app_lifecycle::{
        select_exes_needing_relaunch, wait_or_relaunch_closed_on_timeout,
    };
    use std::path::PathBuf;

    let zalo_exe = PathBuf::from(r"C:\Users\Alice\AppData\Local\Programs\Zalo\Zalo.exe");
    let zalocap_exe = PathBuf::from(r"C:\Users\Alice\AppData\Local\Programs\Zalo\ZaloCap.exe");
    let captured = vec![zalo_exe.clone(), zalocap_exe.clone()];

    // Simulate: `Zalo.exe` exited cleanly during RmShutdown, while `ZaloCap.exe` hung until timeout!
    assert_eq!(
        select_exes_needing_relaunch(&captured, std::slice::from_ref(&zalocap_exe)),
        vec![zalo_exe.clone()]
    );

    let mut relaunched = Vec::new();
    let err = wait_or_relaunch_closed_on_timeout(
        Duration::from_millis(20),
        Duration::from_millis(5),
        &captured,
        PathBuf::from(r"C:\Zalo Data"),
        || vec![zalocap_exe.clone()],
        |exe| relaunched.push(exe.to_path_buf()),
    )
    .unwrap_err();

    assert!(matches!(err, GreenCError::FileLocked { .. }));
    assert_eq!(
        relaunched,
        vec![zalo_exe],
        "timeout MUST relaunch every captured exe that was already closed (and not double-launch still-running ones)"
    );
}
