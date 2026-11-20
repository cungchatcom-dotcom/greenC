use chrono::{NaiveDate, Utc};
use greenc_core::models::*;
use std::path::PathBuf;

#[test]
fn test_drive_info_and_usage() {
    let drive = DriveInfo {
        root: PathBuf::from("C:\\"),
        drive_letter: 'C',
        label: "Windows".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 100_000_000_000,
        free_bytes: 10_000_000_000,
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    };

    assert_eq!(drive.used_bytes(), 90_000_000_000);
    assert!((drive.usage_ratio() - 0.90).abs() < f64::EPSILON);
    assert!(drive.is_critical(0.85));
    assert!(!drive.is_critical(0.95));

    let json = serde_json::to_string(&drive).unwrap();
    let deserialized: DriveInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(drive, deserialized);
}

#[test]
fn test_reason_codes_serialization() {
    let code = WriteReasonCode::MediaChat;
    assert_eq!(code.as_str(), "MEDIA_CHAT");

    let json = serde_json::to_string(&code).unwrap();
    assert_eq!(json, "\"MEDIA_CHAT\"");

    let parsed: WriteReasonCode = serde_json::from_str("\"ORPHAN_LEFTOVER\"").unwrap();
    assert_eq!(parsed, WriteReasonCode::OrphanLeftover);
}

#[test]
fn test_daily_snapshot_and_events_roundtrip() {
    let snapshot = DailySnapshot {
        id: Some(1),
        drive_letter: "C:\\".to_string(),
        snapshot_date: NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(),
        total_bytes: 500_000_000_000,
        used_bytes: 400_000_000_000,
        free_bytes: 100_000_000_000,
        net_change_bytes: 1_000_000_000,
        reclaimed_bytes: 500_000_000,
        updated_at: Utc::now(),
    };

    let json = serde_json::to_string(&snapshot).unwrap();
    let deserialized: DailySnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(snapshot.drive_letter, deserialized.drive_letter);
    assert_eq!(snapshot.reclaimed_bytes, deserialized.reclaimed_bytes);
}

#[test]
fn test_vault_archive_and_relocation_status() {
    assert_eq!(VaultStatus::Quarantined.as_str(), "QUARANTINED");
    assert_eq!(
        VaultStatus::from_str_loose("ROLLED_BACK"),
        VaultStatus::RolledBack
    );

    assert_eq!(RelocationStrategyKind::Junction.as_str(), "junction");
    assert_eq!(RelocationStatus::Done.as_str(), "done");
    assert_eq!(RelocationHealthStatus::Healthy.as_str(), "healthy");
}

#[test]
fn test_contract_type_aliases() {
    let item = FileManifestItem {
        id: Some(42),
        archive_id: Some(1),
        original_path: PathBuf::from("C:\\test\\file.txt"),
        rel_arcname: "item_00001_file.txt".to_string(),
        size_bytes: 2048,
        sha256: "abc123hash".to_string(),
        mtime: 1000.0,
        atime: 1000.0,
        hardlink_paths: vec![],
        restored_individually: false,
    };

    // Verify VaultFileEntry alias works identically to FileManifestItem
    let entry: VaultFileEntry = item.clone();
    assert_eq!(entry.size_bytes, 2048);
    assert_eq!(entry.rel_arcname, "item_00001_file.txt");

    // Verify RelocationStrategy alias works identically to RelocationStrategyKind
    let strategy: RelocationStrategy = RelocationStrategyKind::Junction;
    assert_eq!(strategy, RelocationStrategy::Junction);
    assert_eq!(strategy.as_str(), "junction");

    let parsed_strategy = RelocationStrategy::from_str_loose("known_folder");
    assert_eq!(parsed_strategy, RelocationStrategy::KnownFolder);
}
