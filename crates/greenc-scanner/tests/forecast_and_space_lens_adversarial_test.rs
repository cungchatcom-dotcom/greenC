//! Empirical adversarial challenge and stress-test suite for forecast math and Space Lens treemap layout.
//! Authored by challenger_m2_2.

use chrono::{NaiveDate, Utc};
use greenc_core::models::DailySnapshot;
use greenc_scanner::forecast::predict_fullness;
use greenc_scanner::space_lens::{classify_path_reason, SpaceNode, SquarifiedTreemapGenerator};
use std::path::Path;

fn make_snapshot(date: (i32, u32, u32), net_change_bytes: i64) -> DailySnapshot {
    DailySnapshot {
        id: None,
        drive_letter: "C:\\".into(),
        snapshot_date: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        used_bytes: 50 * 1024 * 1024 * 1024,
        free_bytes: 50 * 1024 * 1024 * 1024,
        net_change_bytes,
        reclaimed_bytes: 0,
        updated_at: Utc::now(),
    }
}

// ==============================================================================
// 1. FORECAST MATH ADVERSARIAL CHALLENGES
// ==============================================================================

#[test]
fn test_zero_velocity_returns_none_and_not_critical_when_under_90() {
    let snapshots = vec![
        make_snapshot((2026, 11, 1), 0),
        make_snapshot((2026, 11, 2), 0),
        make_snapshot((2026, 11, 3), 0),
    ];
    let total = 100 * 1024 * 1024 * 1024;
    let free = 50 * 1024 * 1024 * 1024; // 50% used

    let res = predict_fullness(&snapshots, free, total);
    assert_eq!(res.daily_net_bps, 0);
    assert_eq!(res.days_until_full, None);
    assert_eq!(res.days_until_90_pct, None);
    assert!(!res.is_critical);
}

#[test]
fn test_shrinking_usage_returns_none_and_not_critical_when_under_90() {
    let snapshots = vec![
        make_snapshot((2026, 11, 1), -5 * 1024 * 1024 * 1024),
        make_snapshot((2026, 11, 2), -2 * 1024 * 1024 * 1024),
    ];
    let total = 100 * 1024 * 1024 * 1024;
    let free = 60 * 1024 * 1024 * 1024; // 40% used

    let res = predict_fullness(&snapshots, free, total);
    assert!(res.daily_net_bps < 0);
    assert_eq!(res.days_until_full, None);
    assert_eq!(res.days_until_90_pct, None);
    assert!(!res.is_critical);
}

#[test]
fn test_extreme_rapid_growth() {
    let snapshots = vec![
        make_snapshot((2026, 11, 1), 100 * 1024 * 1024 * 1024), // +100 GB/day
    ];
    let total = 100 * 1024 * 1024 * 1024;
    let free = 5 * 1024 * 1024 * 1024; // 5 GB free, 95 GB used (95%)

    let res = predict_fullness(&snapshots, free, total);
    assert_eq!(res.days_until_full, Some(1));
    assert_eq!(res.days_until_90_pct, Some(0)); // already >= 90%
    assert!(res.is_critical);
}

#[test]
fn test_boundary_90_pct_used() {
    let snapshots = vec![
        make_snapshot((2026, 11, 1), 1024 * 1024 * 1024), // +1 GB/day
    ];
    let total = 100 * 1024 * 1024 * 1024;
    let free = 10 * 1024 * 1024 * 1024; // exactly 90% used (10 GB free)

    let res = predict_fullness(&snapshots, free, total);
    assert_eq!(res.days_until_90_pct, Some(0));
    assert!(res.is_critical);
}

#[test]
fn test_full_drive_100_pct_with_positive_velocity() {
    let snapshots = vec![make_snapshot((2026, 11, 1), 1024 * 1024 * 1024)];
    let total = 100 * 1024 * 1024 * 1024;
    let free = 0; // 100% full

    let res = predict_fullness(&snapshots, free, total);
    assert_eq!(res.days_until_full, Some(0));
    assert_eq!(res.days_until_90_pct, Some(0));
    assert!(res.is_critical);
}

#[test]
fn test_full_drive_100_pct_with_zero_velocity_triggers_critical() {
    let snapshots = vec![make_snapshot((2026, 11, 1), 0)];
    let total = 100 * 1024 * 1024 * 1024;
    let free = 0; // 100% full

    let res = predict_fullness(&snapshots, free, total);
    // In current implementation, rate <= 0 causes days to be None, but usage_ratio = 1.0 ensures is_critical = true
    assert_eq!(res.usage_ratio, 1.0);
    assert!(res.is_critical);
}

#[test]
fn test_out_of_order_snapshots_handled_identically() {
    let s1 = make_snapshot((2026, 11, 5), 500);
    let s2 = make_snapshot((2026, 11, 1), 100);
    let s3 = make_snapshot((2026, 11, 3), 300);

    let res_unordered = predict_fullness(&[s1.clone(), s2.clone(), s3.clone()], 5000, 10000);
    let res_ordered = predict_fullness(&[s2, s3, s1], 5000, 10000);

    assert_eq!(res_unordered.burn_rate_7d, res_ordered.burn_rate_7d);
    assert_eq!(res_unordered.burn_rate_14d, res_ordered.burn_rate_14d);
    assert_eq!(res_unordered.burn_rate_30d, res_ordered.burn_rate_30d);
    assert_eq!(res_unordered.daily_net_bps, res_ordered.daily_net_bps);
}

#[test]
fn test_empty_snapshots_and_zero_total_bytes_safe() {
    let res = predict_fullness(&[], 0, 0);
    assert_eq!(res.daily_net_bps, 0);
    assert_eq!(res.days_until_full, None);
    assert_eq!(res.days_until_90_pct, None);
    assert_eq!(res.usage_ratio, 0.0);
    assert!(!res.is_critical);
    assert_eq!(res.sample_days, 0);
}

// ==============================================================================
// 2. SQUARIFIED TREEMAP ADVERSARIAL CHALLENGES
// ==============================================================================

#[test]
fn test_treemap_degenerate_single_tile() {
    let mut root = SpaceNode::empty(Path::new("C:\\"));
    root.physical_bytes = 1000;

    let mut solo = SpaceNode::empty(Path::new("C:\\solo"));
    solo.physical_bytes = 1000;
    root.children = vec![solo];

    let layout = SquarifiedTreemapGenerator::generate_layout(&root, 800.0, 600.0, 2);
    assert_eq!(layout.children.len(), 1);

    let child = &layout.children[0];
    assert_eq!(child.x, 0.0);
    assert_eq!(child.y, 0.0);
    assert!((child.width - 800.0).abs() < 1e-4);
    assert!((child.height - 600.0).abs() < 1e-4);
    assert!(((child.width * child.height) - 480_000.0).abs() < 1e-4);
}

#[test]
fn test_treemap_degenerate_1000_tiny_tiles() {
    let mut root = SpaceNode::empty(Path::new("C:\\"));
    root.physical_bytes = 1000;

    let mut children = Vec::with_capacity(1000);
    for i in 0..1000 {
        let mut child = SpaceNode::empty(Path::new(&format!("C:\\tile_{}", i)));
        child.physical_bytes = 1;
        children.push(child);
    }
    root.children = children;

    let w = 1920.0;
    let h = 1080.0;
    let total_area = w * h;

    let layout = SquarifiedTreemapGenerator::generate_layout(&root, w, h, 2);
    assert_eq!(layout.children.len(), 1000);

    let mut sum_area = 0.0;
    for child in &layout.children {
        assert!(!child.x.is_nan() && !child.y.is_nan());
        assert!(!child.width.is_nan() && !child.height.is_nan());
        assert!(!child.width.is_infinite() && !child.height.is_infinite());
        assert!(child.width > 0.0 && child.height > 0.0);
        assert!(child.x >= 0.0 && child.y >= 0.0);
        assert!(child.x + child.width <= w + 1e-4);
        assert!(child.y + child.height <= h + 1e-4);

        sum_area += child.width * child.height;
    }

    assert!((sum_area - total_area).abs() < 1.0);
}

#[test]
fn test_treemap_degenerate_zero_byte_tiles_filtered() {
    let base = Path::new("/root");
    let mut root = SpaceNode::empty(base);
    root.physical_bytes = 500;

    let mut z1 = SpaceNode::empty(&base.join("zero1"));
    z1.physical_bytes = 0;
    let mut r1 = SpaceNode::empty(&base.join("real"));
    r1.physical_bytes = 500;
    let mut z2 = SpaceNode::empty(&base.join("zero2"));
    z2.physical_bytes = 0;

    root.children = vec![z1, r1, z2];

    let layout = SquarifiedTreemapGenerator::generate_layout(&root, 800.0, 600.0, 2);
    // 0-byte children are filtered out so only the 500-byte child is laid out
    assert_eq!(layout.children.len(), 1);
    assert_eq!(layout.children[0].name, "real");
    assert!(((layout.children[0].width * layout.children[0].height) - 480_000.0).abs() < 1e-4);
}

#[test]
fn test_treemap_all_zero_bytes_returns_empty() {
    let mut root = SpaceNode::empty(Path::new("C:\\"));
    root.physical_bytes = 0;

    let mut z1 = SpaceNode::empty(Path::new("C:\\zero1"));
    z1.physical_bytes = 0;
    root.children = vec![z1];

    let layout = SquarifiedTreemapGenerator::generate_layout(&root, 800.0, 600.0, 2);
    assert_eq!(layout.children.len(), 0);
}

#[test]
fn test_treemap_zero_and_negative_bounds_returns_empty() {
    let mut root = SpaceNode::empty(Path::new("C:\\"));
    root.physical_bytes = 100;

    let mut child = SpaceNode::empty(Path::new("C:\\child"));
    child.physical_bytes = 100;
    root.children = vec![child];

    let l1 = SquarifiedTreemapGenerator::generate_layout(&root, 0.0, 600.0, 2);
    assert_eq!(l1.children.len(), 0);

    let l2 = SquarifiedTreemapGenerator::generate_layout(&root, 800.0, -10.0, 2);
    assert_eq!(l2.children.len(), 0);
}

// ==============================================================================
// 3. PATH REASON CLASSIFICATION ADVERSARIAL CHALLENGES
// ==============================================================================

#[test]
fn test_path_classification_extended_matrix() {
    // Media & Chat
    assert_eq!(
        classify_path_reason(Path::new("c:/users/app/dingtalk/cache")),
        Some("MEDIA_CHAT".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\WECHAT\\FILES")),
        Some("MEDIA_CHAT".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\telegram\\data")),
        Some("MEDIA_CHAT".into())
    );

    // Cache & Temp
    assert_eq!(
        classify_path_reason(Path::new("C:\\Users\\User\\.cache\\pip")),
        Some("CACHE_TEMP".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\Users\\User\\AppData\\Roaming\\npm-cache")),
        Some("CACHE_TEMP".into())
    );

    // Crash Dumps
    assert_eq!(
        classify_path_reason(Path::new("C:\\Users\\User\\Crashpad")),
        Some("CRASH_DUMP".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("C:\\Minidump\\dump.dmp")),
        Some("CRASH_DUMP".into())
    );

    // Container & VM
    assert_eq!(
        classify_path_reason(Path::new("C:\\Docker\\rootfs")),
        Some("CONTAINER_VM".into())
    );
    assert_eq!(
        classify_path_reason(Path::new("D:\\Disks\\harddisk.vhdx")),
        Some("CONTAINER_VM".into())
    );

    // Unmatched
    assert_eq!(
        classify_path_reason(Path::new("C:\\Photos\\image.png")),
        None
    );
    assert_eq!(classify_path_reason(Path::new("")), None);
}
