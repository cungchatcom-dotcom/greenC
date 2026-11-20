//! Unit tests for fixed NTFS drive filtering, eligibility tagging, and recommendation fallback (Task T018).

use greenc_core::models::DriveInfo;
use greenc_platform::volume::{
    evaluate_drive_eligibility, get_system_drive_root, recommend_drive, recommend_drive_info,
    recommend_drive_letter, DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
    DRIVE_UNKNOWN,
};
use std::path::PathBuf;

#[test]
fn test_fixed_ntfs_eligible() {
    let (is_eligible, reason) = evaluate_drive_eligibility(DRIVE_FIXED, "NTFS", "Data");
    assert!(is_eligible);
    assert!(reason.is_none());

    let drive_d = DriveInfo {
        root: PathBuf::from("D:\\"),
        drive_letter: 'D',
        label: "Data".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 100 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible,
        ineligible_reason: reason,
    };

    assert!(drive_d.is_eligible);
    assert!(drive_d.ineligible_reason.is_none());
    assert_eq!(recommend_drive_letter(&[drive_d]), Some('D'));
}

#[test]
fn test_usb_removable_rejected() {
    // Dynamically test removable, remote, cdrom, and ramdisk types
    for &dtype in &[
        DRIVE_REMOVABLE,
        DRIVE_REMOTE,
        DRIVE_CDROM,
        DRIVE_RAMDISK,
        DRIVE_UNKNOWN,
    ] {
        let (is_eligible, reason) = evaluate_drive_eligibility(dtype, "NTFS", "FlashDrive");
        assert!(!is_eligible, "Drive type {} should not be eligible", dtype);
        assert_eq!(
            reason.as_deref(),
            Some("Ổ di động hoặc ổ mạng không được hỗ trợ")
        );
    }

    let (usb_ok, usb_reason) = evaluate_drive_eligibility(DRIVE_REMOVABLE, "NTFS", "USB Flash");
    let usb = DriveInfo {
        root: PathBuf::from("E:\\"),
        drive_letter: 'E',
        label: "USB Flash".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 64 * 1024 * 1024 * 1024,
        free_bytes: 32 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: usb_ok,
        ineligible_reason: usb_reason,
    };

    assert!(!usb.is_eligible);
    assert_eq!(
        usb.ineligible_reason.as_deref(),
        Some("Ổ di động hoặc ổ mạng không được hỗ trợ")
    );
    assert_eq!(recommend_drive(&[usb]), None);
}

#[test]
fn test_exfat_fat32_rejected() {
    let (exfat_ok, exfat_reason) = evaluate_drive_eligibility(DRIVE_FIXED, "exFAT", "External");
    assert!(!exfat_ok);
    assert_eq!(
        exfat_reason.as_deref(),
        Some("Định dạng exFAT không hỗ trợ liên kết thư mục và tệp lớn")
    );

    let (fat32_ok, fat32_reason) = evaluate_drive_eligibility(DRIVE_FIXED, "FAT32", "OldUSB");
    assert!(!fat32_ok);
    assert_eq!(
        fat32_reason.as_deref(),
        Some("Định dạng FAT32 không hỗ trợ liên kết thư mục và tệp lớn")
    );

    let (empty_ok, empty_reason) = evaluate_drive_eligibility(DRIVE_FIXED, "", "RawDisk");
    assert!(!empty_ok);
    assert_eq!(
        empty_reason.as_deref(),
        Some("Định dạng không rõ không hỗ trợ liên kết thư mục và tệp lớn")
    );

    let (refs_ok, refs_reason) = evaluate_drive_eligibility(DRIVE_FIXED, "ReFS", "StoragePool");
    assert!(!refs_ok);
    assert_eq!(
        refs_reason.as_deref(),
        Some("Định dạng ReFS không hỗ trợ liên kết thư mục và tệp lớn")
    );

    let exfat = DriveInfo {
        root: PathBuf::from("E:\\"),
        drive_letter: 'E',
        label: "External".to_string(),
        filesystem: "exFAT".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 200 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: exfat_ok,
        ineligible_reason: exfat_reason,
    };

    let fat32 = DriveInfo {
        root: PathBuf::from("F:\\"),
        drive_letter: 'F',
        label: "OldUSB".to_string(),
        filesystem: "FAT32".to_string(),
        total_bytes: 32 * 1024 * 1024 * 1024,
        free_bytes: 16 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: fat32_ok,
        ineligible_reason: fat32_reason,
    };

    assert!(!exfat.is_eligible);
    assert!(!fat32.is_eligible);
    assert_eq!(recommend_drive(&[exfat, fat32]), None);
}

#[test]
fn test_recovery_partition_rejected() {
    for label in &[
        "Windows Recovery",
        "Recovery",
        "RECOVERY_PARTITION",
        "My recovery drive",
    ] {
        let (rec_ok, rec_reason) = evaluate_drive_eligibility(DRIVE_FIXED, "NTFS", label);
        assert!(!rec_ok, "Label '{}' should be rejected as recovery", label);
        assert_eq!(
            rec_reason.as_deref(),
            Some("Phân vùng khôi phục của hệ thống")
        );
    }

    let (recovery_ok, recovery_reason) =
        evaluate_drive_eligibility(DRIVE_FIXED, "NTFS", "Windows Recovery");
    let recovery = DriveInfo {
        root: PathBuf::from("R:\\"),
        drive_letter: 'R',
        label: "Windows Recovery".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 20 * 1024 * 1024 * 1024,
        free_bytes: 5 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: recovery_ok,
        ineligible_reason: recovery_reason,
    };

    assert!(!recovery.is_eligible);
    assert_eq!(recommend_drive(&[recovery]), None);
}

#[test]
fn test_system_drive_root_formatting() {
    let sys_root = get_system_drive_root();
    #[cfg(windows)]
    {
        assert!(
            sys_root.ends_with(":\\"),
            "System drive root '{}' must end with ':\\'",
            sys_root
        );
        assert_eq!(
            sys_root.matches(':').count(),
            1,
            "System drive root '{}' must have exactly 1 colon (no double colons)",
            sys_root
        );
        assert!(
            !sys_root.contains("::"),
            "System drive root '{}' must not contain '::'",
            sys_root
        );
    }
    #[cfg(unix)]
    {
        assert_eq!(sys_root, "/");
    }
}

#[test]
fn test_system_drive_never_recommended_over_roomy_secondary() {
    let drive_c = DriveInfo {
        root: PathBuf::from("C:\\"),
        drive_letter: 'C',
        label: "Windows".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 100 * 1024 * 1024 * 1024, // 100 GiB on C:
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    };

    let drive_d = DriveInfo {
        root: PathBuf::from("D:\\"),
        drive_letter: 'D',
        label: "Data".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 256 * 1024 * 1024 * 1024,
        free_bytes: 20 * 1024 * 1024 * 1024, // 20 GiB on D: (>= 10 GiB)
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    // Even though C: has 100 GiB and D: has only 20 GiB, D: MUST be recommended
    // because C: is the system drive and D: has >= 10 GiB free space.
    let drives = [drive_c, drive_d];
    assert_eq!(recommend_drive_letter(&drives), Some('D'));
}

#[test]
fn test_system_drive_fallback_when_alone() {
    let drive_c = DriveInfo {
        root: PathBuf::from("C:\\"),
        drive_letter: 'C',
        label: "Windows".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 256 * 1024 * 1024 * 1024,
        free_bytes: 50 * 1024 * 1024 * 1024,
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    };

    // When only system drive C: is present and eligible, recommendation falls back to C:
    assert_eq!(recommend_drive_letter(&[drive_c]), Some('C'));
}

#[test]
fn test_secondary_under_10gb_fallback() {
    let drive_c = DriveInfo {
        root: PathBuf::from("C:\\"),
        drive_letter: 'C',
        label: "Windows".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 256 * 1024 * 1024 * 1024,
        free_bytes: 15 * 1024 * 1024 * 1024, // 15 GiB
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    };

    let drive_d = DriveInfo {
        root: PathBuf::from("D:\\"),
        drive_letter: 'D',
        label: "SecondarySmall".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        free_bytes: 8 * 1024 * 1024 * 1024, // 8 GiB (< 10 GiB)
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    // D is eligible but < 10 GiB. Roomy secondary pool is empty.
    // Pool falls back to [C, D], picking C (15 GiB > 8 GiB).
    let drives = [drive_c.clone(), drive_d.clone()];
    assert_eq!(recommend_drive_letter(&drives), Some('C'));

    // If C has only 4 GiB and D has 8 GiB, fallback picks D (8 GiB > 4 GiB)
    let drive_c_small = DriveInfo {
        free_bytes: 4 * 1024 * 1024 * 1024,
        ..drive_c
    };
    let drives_c_small = [drive_c_small, drive_d];
    assert_eq!(recommend_drive_letter(&drives_c_small), Some('D'));
}

#[test]
fn test_zero_free_space_handling() {
    let full_drive = DriveInfo {
        root: PathBuf::from("D:\\"),
        drive_letter: 'D',
        label: "FullData".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 0,
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    assert_eq!(full_drive.free_bytes, 0);
    assert_eq!(full_drive.usage_ratio(), 1.0);
    // Safe recommendation without division by zero
    assert_eq!(recommend_drive_letter(&[full_drive]), Some('D'));
}

#[test]
fn test_empty_drives_list_returns_none() {
    let empty: [DriveInfo; 0] = [];
    assert_eq!(recommend_drive(&empty), None);
    assert_eq!(recommend_drive_letter(&empty), None);
    assert_eq!(recommend_drive_info(&empty), None);
}
