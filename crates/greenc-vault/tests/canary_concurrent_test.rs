//! Two cleanup scans can run at once; their canary probes share one sandbox folder.

use greenc_core::models::CleanCandidate;
use greenc_vault::canary::CanaryVerifier;
use std::time::SystemTime;
use tempfile::tempdir;

#[test]
fn probes_running_at_the_same_time_do_not_disturb_each_other() {
    let root = tempdir().unwrap();
    let sandbox = root.path().join("sandbox");

    std::thread::scope(|scope| {
        for worker in 0..8u8 {
            let (root, sandbox) = (root.path(), &sandbox);
            scope.spawn(move || {
                let path = root.join(format!("file-{worker}.bin"));
                // Same bytes for pairs of workers, so their probes look alike too.
                let data = vec![worker / 2; 512 * 1024];
                std::fs::write(&path, &data).unwrap();
                let candidate = CleanCandidate {
                    path,
                    size_bytes: data.len() as u64,
                    mtime: SystemTime::now(),
                    is_locked: false,
                    profile_id: "test".to_string(),
                    is_canary: false,
                    is_orphan: false,
                    hardlink_siblings: Vec::new(),
                };
                for _ in 0..40 {
                    let result = CanaryVerifier::run_sandbox_probe(&candidate, sandbox).unwrap();
                    assert!(result.passed, "{:?}", result.error_detail);
                }
            });
        }
    });
}
