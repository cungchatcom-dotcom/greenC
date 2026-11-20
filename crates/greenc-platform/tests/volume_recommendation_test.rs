use greenc_core::models::DriveInfo;
use greenc_platform::volume::recommend_drive;
use std::path::PathBuf;

#[test]
fn test_volume_recommendation_hierarchy() {
    let drive_c = DriveInfo {
        root: PathBuf::from("C:\\"),
        drive_letter: 'C',
        label: "Windows".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        free_bytes: 15 * 1024 * 1024 * 1024,
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    };

    let drive_d = DriveInfo {
        root: PathBuf::from("D:\\"),
        drive_letter: 'D',
        label: "Data".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 1000 * 1024 * 1024 * 1024,
        free_bytes: 600 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let drive_e = DriveInfo {
        root: PathBuf::from("E:\\"),
        drive_letter: 'E',
        label: "USB".to_string(),
        filesystem: "exFAT".to_string(),
        total_bytes: 64 * 1024 * 1024 * 1024,
        free_bytes: 30 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: false,
        ineligible_reason: Some("exFAT not supported".to_string()),
    };

    let drive_f = DriveInfo {
        root: PathBuf::from("F:\\"),
        drive_letter: 'F',
        label: "Small".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 5 * 1024 * 1024 * 1024, // < 10 GiB
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let drives = vec![drive_c.clone(), drive_d.clone(), drive_e, drive_f];

    // Drive D is non-system, eligible, with >= 10 GiB free space and highest free bytes (600 GB)
    let recommended = recommend_drive(&drives).expect("Should recommend a drive");
    assert_eq!(recommended.drive_letter, 'D');

    // Remove Drive D; Drive C is eligible system drive, but only eligible one with >= 10 GiB free space
    let drives_without_d = vec![drive_c, drives[3].clone()];
    let recommended_fallback =
        recommend_drive(&drives_without_d).expect("Should recommend fallback");
    // Between C (15 GB free, system) and F (5 GB free, non-system):
    // In algorithm: roomy pool checks non-system with >= 10 GiB. Neither is in roomy pool (C is system, F has 5 GB).
    // So pool is all eligible (C: 15 GB, F: 5 GB). Max free space is C (15 GB).
    assert_eq!(recommended_fallback.drive_letter, 'C');
}
