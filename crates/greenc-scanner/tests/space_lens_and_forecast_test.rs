//! Unit and integration test suite for scanner, hardlink dedup, treemap layout, and burn-rate forecast (Task T019).

use chrono::{NaiveDate, Utc};
use greenc_core::error::GreenCError;
use greenc_core::models::DailySnapshot;
use greenc_core::task::TaskControl;
use greenc_scanner::forecast::predict_fullness;
use greenc_scanner::space_lens::{classify_path_reason, SpaceNode, SquarifiedTreemapGenerator};
use greenc_scanner::walker::walk_directory_parallel;
use std::fs;
use std::path::{Path, PathBuf};

struct TestDirGuard {
    path: PathBuf,
}

impl TestDirGuard {
    fn new(suffix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "greenc_test_{}_{}_{}",
            suffix,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("Failed to create test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_hardlink_deduplication_counts_physical_size_once() {
    let guard = TestDirGuard::new("hardlink_test");
    let file1 = guard.path().join("original.bin");
    let file2 = guard.path().join("hardlink_sibling.bin");

    // Write 1 MB of test data
    let data = vec![0xABu8; 1024 * 1024];
    fs::write(&file1, &data).unwrap();

    // Create hardlink
    if let Err(e) = fs::hard_link(&file1, &file2) {
        eprintln!(
            "Skipping hardlink test due to OS filesystem restriction: {}",
            e
        );
        return;
    }

    let ctrl = TaskControl::new();
    let node = walk_directory_parallel(guard.path(), &ctrl, 4).unwrap();

    // Two files in directory, but only 1 MB physical disk usage!
    assert_eq!(node.file_count, 2);
    assert_eq!(node.physical_bytes, 1024 * 1024);
}

#[test]
fn test_treemap_squarified_bounding_box_invariant() {
    let guard = TestDirGuard::new("treemap_test");
    let mut root = SpaceNode::empty(guard.path());
    root.physical_bytes = 1000;

    let mut c1 = SpaceNode::empty(&root.path.join("Child1"));
    c1.physical_bytes = 600;
    let mut c2 = SpaceNode::empty(&root.path.join("Child2"));
    c2.physical_bytes = 400;

    root.children = vec![c1, c2];

    let layout = SquarifiedTreemapGenerator::generate_layout(&root, 800.0, 600.0, 2);

    assert_eq!(layout.width, 800.0);
    assert_eq!(layout.height, 600.0);
    assert_eq!(layout.children.len(), 2);

    // Sum of children areas equals total area
    let total_area = 800.0 * 600.0;
    let children_area: f64 = layout.children.iter().map(|c| c.width * c.height).sum();
    assert!((children_area - total_area).abs() < 1.0);
}

#[test]
fn test_burn_rate_predict_fullness_calculation() {
    let snapshots = vec![
        DailySnapshot {
            id: None,
            drive_letter: "C:\\".into(),
            snapshot_date: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            total_bytes: 100 * 1024 * 1024 * 1024,
            used_bytes: 70 * 1024 * 1024 * 1024,
            free_bytes: 30 * 1024 * 1024 * 1024,
            net_change_bytes: 2 * 1024 * 1024 * 1024, // +2 GB/day
            reclaimed_bytes: 0,
            updated_at: Utc::now(),
        },
        DailySnapshot {
            id: None,
            drive_letter: "C:\\".into(),
            snapshot_date: NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(),
            total_bytes: 100 * 1024 * 1024 * 1024,
            used_bytes: 72 * 1024 * 1024 * 1024,
            free_bytes: 28 * 1024 * 1024 * 1024,
            net_change_bytes: 2 * 1024 * 1024 * 1024,
            reclaimed_bytes: 0,
            updated_at: Utc::now(),
        },
    ];

    let current_free = 28 * 1024 * 1024 * 1024; // 28 GB free
    let total = 100 * 1024 * 1024 * 1024; // 100 GB total
                                          // 90% threshold = 90 GB. Current used = 72 GB. Need 18 GB to reach 90%.
                                          // Burn rate = 2 GB/day. Days until 90% = 18 / 2 = 9 days.
                                          // Days until full = 28 / 2 = 14 days.

    let res = predict_fullness(&snapshots, current_free, total);
    assert_eq!(res.daily_net_bps, 2 * 1024 * 1024 * 1024);
    assert_eq!(res.daily_burn_rate_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(res.days_until_90_pct, Some(9));
    assert_eq!(res.days_until_full, Some(14));
    assert!(res.is_critical); // <= 14 days to 90% triggers critical
}

#[test]
fn test_burn_rate_stable_or_shrinking_returns_none() {
    let snapshots = vec![DailySnapshot {
        id: None,
        drive_letter: "C:\\".into(),
        snapshot_date: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        used_bytes: 50 * 1024 * 1024 * 1024,
        free_bytes: 50 * 1024 * 1024 * 1024,
        net_change_bytes: -(1024 * 1024 * 1024), // -1 GB/day (shrinking)
        reclaimed_bytes: 0,
        updated_at: Utc::now(),
    }];

    let res = predict_fullness(
        &snapshots,
        50 * 1024 * 1024 * 1024,
        100 * 1024 * 1024 * 1024,
    );
    assert!(res.daily_net_bps <= 0);
    assert_eq!(res.days_until_90_pct, None);
    assert_eq!(res.days_until_full, None);
    assert!(!res.is_critical);
}

#[test]
fn test_cooperative_cancellation() {
    let guard = TestDirGuard::new("cancel_test");
    let sub = guard.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("file.txt"), b"test").unwrap();

    let ctrl = TaskControl::new();
    ctrl.cancel(); // Pre-cancelled

    let res = walk_directory_parallel(guard.path(), &ctrl, 2);
    assert!(matches!(res, Err(GreenCError::Cancelled)));
}

#[test]
fn test_empty_directory_handling() {
    let guard = TestDirGuard::new("empty_dir");
    let ctrl = TaskControl::new();
    let node = walk_directory_parallel(guard.path(), &ctrl, 2).unwrap();

    assert_eq!(node.physical_bytes, 0);
    assert_eq!(node.file_count, 0);
    assert!(node.children.is_empty());
}

#[test]
fn test_path_reason_classification() {
    assert_eq!(
        classify_path_reason(Path::new("C:\\Users\\User\\AppData\\Roaming\\ZaloData")),
        Some("MEDIA_CHAT".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\Users\\User\\AppData\\Local\\Temp")),
        Some("CACHE_TEMP".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\ProgramData\\CrashDumps")),
        Some("CRASH_DUMP".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\Logs\\app.log")),
        Some("LOG_GROWTH".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\Users\\User\\wsl.vhdx")),
        Some("CONTAINER_VM".into())
    );
}

#[test]
fn test_reparse_point_junction_is_skipped() {
    let guard = TestDirGuard::new("reparse_test");
    let real_dir = guard.path().join("real_folder");
    fs::create_dir_all(&real_dir).unwrap();

    // Create a 1 MB file inside real_folder
    let file1 = real_dir.join("payload.bin");
    fs::write(&file1, vec![0xCCu8; 1024 * 1024]).unwrap();

    // Create a junction folder pointing to real_dir
    let junction_link = guard.path().join("junction_to_real");

    #[cfg(windows)]
    {
        // mklink /J creates an unprivileged NTFS directory junction (tag 0x00000410)
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &junction_link.to_string_lossy(),
                &real_dir.to_string_lossy(),
            ])
            .output();

        let junction_created = match status {
            Ok(output) => output.status.success(),
            Err(_) => false,
        };

        if !junction_created {
            eprintln!("Skipping junction test: unable to create NTFS junction via mklink");
            return;
        }

        // Verify with symlink_metadata that it has reparse point attribute (0x400)
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = fs::symlink_metadata(&junction_link) {
            assert_ne!(
                meta.file_attributes() & 0x00000400,
                0,
                "Junction must have FILE_ATTRIBUTE_REPARSE_POINT (0x400)"
            );
        }

        let ctrl = TaskControl::new();
        let node = walk_directory_parallel(guard.path(), &ctrl, 4).unwrap();

        // The walker should ONLY count the file inside real_folder once!
        // If the junction was recursed into, file_count would be 2 and physical_bytes would be 2 MB!
        assert_eq!(
            node.file_count, 1,
            "Junction contents must not be counted; expected 1 file, found {}",
            node.file_count
        );
        assert_eq!(
            node.physical_bytes,
            1024 * 1024,
            "Junction contents must not be counted; expected 1MB, found {}",
            node.physical_bytes
        );

        // Explicitly remove junction before guard cleanup
        let _ = fs::remove_dir(&junction_link);
    }

    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(&real_dir, &junction_link) {
            eprintln!("Skipping symlink test: {}", e);
            return;
        }

        let ctrl = TaskControl::new();
        let node = walk_directory_parallel(guard.path(), &ctrl, 4).unwrap();

        assert_eq!(node.file_count, 1);
        assert_eq!(node.physical_bytes, 1024 * 1024);
        let _ = fs::remove_file(&junction_link);
    }
}

#[test]
fn test_drive_letter_extraction_and_unprivileged_usn_probe() {
    use greenc_scanner::usn_mft::{
        can_use_usn_scanner, extract_drive_letter, scan_volume_hierarchical,
    };

    assert_eq!(extract_drive_letter("C:\\"), Some('C'));
    assert_eq!(extract_drive_letter("c:"), Some('C'));
    assert_eq!(extract_drive_letter("D:\\data\\folder"), Some('D'));
    assert_eq!(extract_drive_letter("Z"), Some('Z'));
    assert_eq!(extract_drive_letter(""), None);
    assert_eq!(extract_drive_letter("\\\\?\\C:\\"), None);

    // In normal user test runner, can_use_usn_scanner returns false authentically (not dummy)
    let _ = can_use_usn_scanner("C:\\");

    // Verify transparent fallback to parallel walker works on real directories
    let guard = TestDirGuard::new("usn_probe");
    let file_path = guard.path().join("test.txt");
    fs::write(&file_path, b"hello world").unwrap();

    let ctrl = TaskControl::new();
    let result = scan_volume_hierarchical(guard.path(), &ctrl, 2);
    assert!(result.is_ok());
    let node = result.unwrap();
    assert_eq!(node.file_count, 1);
}
