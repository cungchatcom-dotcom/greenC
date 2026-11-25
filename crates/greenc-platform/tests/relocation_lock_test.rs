//! TDD tests for Item 1 (): machine-wide relocation lock across threads & processes.

use greenc_core::error::GreenCError;
use greenc_platform::RelocationLock;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tempfile::TempDir;

#[test]
fn relocation_lock_blocks_second_acquirer_and_releases_on_drop() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("greenc.db");

    let lock1 = RelocationLock::acquire_for_db(&db_path).expect("first acquire must succeed");

    // Non-blocking try_acquire must see the lock as busy
    let try2 = RelocationLock::try_acquire_for_db(&db_path).expect("try_acquire must not error");
    assert!(
        try2.is_none(),
        "second try_acquire must return None while lock1 is held"
    );

    // Blocking/failing acquire must return PreconditionBlocked
    let err = RelocationLock::acquire_for_db(&db_path).unwrap_err();
    assert!(
        matches!(err, GreenCError::PreconditionBlocked { .. }),
        "expected PreconditionBlocked, got {err:?}"
    );

    drop(lock1);

    let lock3 =
        RelocationLock::try_acquire_for_db(&db_path).expect("try_acquire after drop must not err");
    assert!(
        lock3.is_some(),
        "acquire must succeed once previous lock guard is dropped"
    );
}

/// Helper entry point invoked as a child process by `relocation_lock_blocks_across_os_processes`.
#[test]
fn helper_child_hold_relocation_lock() {
    let Ok(db_str) = std::env::var("GREENC_TEST_HOLD_LOCK_DB") else {
        return; // No-op during normal `cargo test` runs
    };
    let db_path = PathBuf::from(db_str);
    let _guard =
        RelocationLock::acquire_for_db(&db_path).expect("child must acquire relocation lock");
    println!("LOCK_HELD");
    let _ = std::io::stdout().flush();

    // Hold lock until parent closes stdin (or kills us to simulate crash)
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

#[test]
fn relocation_lock_blocks_across_os_processes_and_releases_on_crash() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("shared_greenc.db");

    let current_exe = std::env::current_exe().unwrap();
    let mut child = Command::new(&current_exe)
        .args([
            "--exact",
            "helper_child_hold_relocation_lock",
            "--nocapture",
        ])
        .env("GREENC_TEST_HOLD_LOCK_DB", &db_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn child test process");

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut ready = false;
    let mut buf = String::new();
    while reader.read_line(&mut buf).unwrap_or(0) > 0 {
        if buf.trim() == "LOCK_HELD" {
            ready = true;
            break;
        }
        buf.clear();
    }
    assert!(ready, "child process did not signal LOCK_HELD");

    // Parent process MUST observe lock held by child process
    assert!(
        RelocationLock::try_acquire_for_db(&db_path)
            .unwrap()
            .is_none(),
        "parent must NOT acquire relocation lock while child process holds it"
    );
    let err = RelocationLock::acquire_for_db(&db_path).unwrap_err();
    assert!(matches!(err, GreenCError::PreconditionBlocked { .. }));

    // Simulate abrupt crash of child process (SIGKILL / TerminateProcess)
    let _ = child.kill();
    let _ = child.wait();

    // OS must release the lock immediately on process termination
    let acquired = RelocationLock::try_acquire_for_db(&db_path).unwrap();
    assert!(
        acquired.is_some(),
        "parent must acquire relocation lock after child crashes"
    );
}
