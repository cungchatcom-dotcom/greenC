use greenc_platform::unix_parsers::{
    assign_unix_drive_letter, detect_atime_reliability_from_proc_mounts,
    evaluate_unix_drive_eligibility, is_ignored_unix_mount, is_protected_unix_path,
    is_supported_unix_fs, parse_pmset_battery, parse_proc_mounts, parse_xdg_user_dirs,
    unescape_proc_mount_field, AtimeReliability,
};
use std::path::{Path, PathBuf};

#[test]
fn test_parse_proc_mounts_and_unescape() {
    let sample = r#"
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
/dev/sda1 /media/user/My\040Drive exfat rw,nosuid,nodev 0 0
/dev/nvme1n1p1 /mnt/data btrfs rw,compress=zstd 0 0
tmpfs /run tmpfs rw,nosuid,nodev 0 0
"#;
    let mounts = parse_proc_mounts(sample);
    assert_eq!(mounts.len(), 5);
    assert_eq!(mounts[0].mount_point, "/");
    assert_eq!(mounts[0].filesystem, "ext4");
    assert_eq!(mounts[2].mount_point, "/media/user/My Drive");
    assert_eq!(mounts[2].filesystem, "exfat");
    assert_eq!(
        unescape_proc_mount_field(r"/mnt/Big\040Disk"),
        "/mnt/Big Disk"
    );
}

#[test]
fn test_unix_fs_and_mount_filters() {
    assert!(is_supported_unix_fs("apfs"));
    assert!(is_supported_unix_fs("APFS"));
    assert!(is_supported_unix_fs("ext4"));
    assert!(is_supported_unix_fs("btrfs"));
    assert!(is_supported_unix_fs("zfs"));
    assert!(!is_supported_unix_fs("tmpfs"));
    assert!(!is_supported_unix_fs("devfs"));
    assert!(!is_supported_unix_fs("squashfs"));

    assert!(!is_ignored_unix_mount("/"));
    assert!(!is_ignored_unix_mount("/Volumes/ExternalSSD"));
    assert!(!is_ignored_unix_mount("/mnt/data"));
    assert!(is_ignored_unix_mount("/proc"));
    assert!(is_ignored_unix_mount("/sys/fs/cgroup"));
    assert!(is_ignored_unix_mount("/snap/core22/100"));
    assert!(is_ignored_unix_mount("/System/Volumes/VM"));
    assert!(is_ignored_unix_mount("/System/Volumes/Preboot"));
    assert!(is_ignored_unix_mount("/System/Volumes/Data"));
}

#[test]
fn test_unix_drive_eligibility_and_letter_assignment() {
    assert_eq!(
        evaluate_unix_drive_eligibility("apfs", "/", "Macintosh HD"),
        (true, None)
    );
    assert_eq!(
        evaluate_unix_drive_eligibility("ext4", "/mnt/data", "data"),
        (true, None)
    );
    assert!(!evaluate_unix_drive_eligibility("exfat", "/Volumes/USB", "USB").0);
    assert!(!evaluate_unix_drive_eligibility("apfs", "/Volumes/Recovery", "Recovery").0);

    assert_eq!(assign_unix_drive_letter("/"), '/');
    assert_eq!(assign_unix_drive_letter("/Volumes/ExternalSSD"), 'E');
    assert_eq!(assign_unix_drive_letter("/mnt/data/"), 'D');
    assert_eq!(assign_unix_drive_letter("/Volumes/1234"), 'V');
}

#[test]
fn test_protected_unix_paths() {
    assert!(is_protected_unix_path(Path::new("/")));
    assert!(is_protected_unix_path(Path::new("/System")));
    assert!(is_protected_unix_path(Path::new(
        "/System/Library/CoreServices"
    )));
    assert!(is_protected_unix_path(Path::new("/usr/bin")));
    assert!(is_protected_unix_path(Path::new("/etc/passwd")));
    assert!(is_protected_unix_path(Path::new(
        "/Applications/Safari.app"
    )));

    assert!(!is_protected_unix_path(Path::new(
        "/Users/alice/Library/Caches/Chrome"
    )));
    assert!(!is_protected_unix_path(Path::new("/home/alice/.cache/pip")));
    assert!(!is_protected_unix_path(Path::new("/Volumes/Data/GreenC")));
}

#[test]
fn test_parse_pmset_battery() {
    let ac = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=12345)\t95%; charged; 0:00 remaining present: true\n";
    assert_eq!(parse_pmset_battery(ac), (false, false, 95));

    let bat_ok = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=12345)\t62%; discharging; 4:10 remaining present: true\n";
    assert_eq!(parse_pmset_battery(bat_ok), (true, false, 62));

    let bat_low = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=12345)\t15%; discharging; 0:45 remaining present: true\n";
    assert_eq!(parse_pmset_battery(bat_low), (true, true, 15));
}

#[test]
fn test_parse_xdg_user_dirs() {
    let content = r#"
# Comment line
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_DOWNLOAD_DIR="$HOME/Downloads"
XDG_DOCUMENTS_DIR="/mnt/docs"
"#;
    let home = Path::new("/home/alice");
    let map = parse_xdg_user_dirs(content, home);
    assert_eq!(
        map.get("XDG_DESKTOP_DIR"),
        Some(&PathBuf::from("/home/alice/Desktop"))
    );
    assert_eq!(
        map.get("XDG_DOWNLOAD_DIR"),
        Some(&PathBuf::from("/home/alice/Downloads"))
    );
    assert_eq!(
        map.get("XDG_DOCUMENTS_DIR"),
        Some(&PathBuf::from("/mnt/docs"))
    );
}

#[test]
fn test_detect_atime_reliability_from_proc_mounts() {
    let mounts = r#"
/dev/nvme0n1p1 / ext4 rw,relatime 0 0
/dev/nvme0n1p2 /home ext4 rw,noatime,errors=remount-ro 0 0
/dev/nvme0n1p3 /home/alice/fast ext4 rw,strictatime 0 0
/dev/sda1 /mnt/data btrfs rw,relatime,space_cache=v2 0 0
/dev/sda2 /mnt/data btrfs rw,noatime 0 0
"#;
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/var/log/app.log"), mounts),
        AtimeReliability::Reliable
    );
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/home/alice/.gradle/caches"), mounts),
        AtimeReliability::UnreliableNoAtime
    );
    // Boundary check: `/home2` must match `/` (relatime), NOT `/home` (noatime)!
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/home2/alice/.gradle"), mounts),
        AtimeReliability::Reliable
    );
    // More specific child mount overrides parent mount.
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/home/alice/fast/cache"), mounts),
        AtimeReliability::Reliable
    );
    // Stacked mount at the exact same mount point: later entry wins.
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/mnt/data/work"), mounts),
        AtimeReliability::UnreliableNoAtime
    );
    // Empty /proc/mounts snapshot fails safe to Unknown.
    assert_eq!(
        detect_atime_reliability_from_proc_mounts(Path::new("/home/alice"), ""),
        AtimeReliability::Unknown
    );
}
