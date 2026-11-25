use greenc_scanner::orphans::OrphanLeftoverRadar;
use greenc_scanner::profiles::{discover_dingtalk_profile, discover_zalo_profile};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Sets times on a file or directory; plain `File::open` cannot touch directories on Windows.
fn open_for_times(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES + FILE_FLAG_BACKUP_SEMANTICS
        std::fs::OpenOptions::new()
            .access_mode(0x0100)
            .share_mode(7)
            .custom_flags(0x0200_0000)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)
    }
}

#[test]
fn test_zalo_and_dingtalk_profile_discovery_protects_databases() {
    let tmp = TempDir::new().unwrap();
    let zalo_root = tmp.path().join("F_Drive").join("Zalo Data");
    fs::create_dir_all(zalo_root.join("media")).unwrap();
    fs::create_dir_all(zalo_root.join("message")).unwrap();
    fs::write(zalo_root.join("media").join("video.mp4"), vec![1u8; 4096]).unwrap();

    let cfg_dir = tmp.path().join("Roaming").join("ZaloData");
    fs::create_dir_all(cfg_dir.join("Cache")).unwrap();
    fs::write(cfg_dir.join("Cache").join("chunk.bin"), vec![5u8; 4096]).unwrap();
    let cfg_file = cfg_dir.join("config.json");
    let db_loc = zalo_root
        .join("message")
        .to_string_lossy()
        .replace('\\', "\\\\");
    fs::write(
        &cfg_file,
        format!(r#"{{"db_path": {{"location": "{db_loc}"}}}}"#),
    )
    .unwrap();

    let local = tmp.path().join("Local");
    let old_ver = local.join("Programs").join("Zalo").join("Zalo-24.1.0");
    let active_ver = local.join("Programs").join("Zalo").join("Zalo-24.9.0");
    fs::create_dir_all(&old_ver).unwrap();
    fs::create_dir_all(&active_ver).unwrap();
    let old_exe = old_ver.join("Zalo.exe");
    fs::write(&old_exe, vec![2u8; 2048]).unwrap();
    // Make old_ver older than 3 days so it qualifies as obsolete.
    let four_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(4 * 86_400);
    let _ = open_for_times(&old_exe).and_then(|f| f.set_modified(four_days_ago));
    let _ = open_for_times(&old_ver).and_then(|f| f.set_modified(four_days_ago));

    let mut running_dirs = HashSet::new();
    running_dirs.insert(active_ver.clone());

    let zalo = discover_zalo_profile(Some(&cfg_file), Some(&local), &running_dirs).unwrap();
    assert!(zalo.reclaimable_paths.contains(&old_ver));
    assert!(zalo.reclaimable_paths.contains(&cfg_dir.join("Cache")));
    assert!(!zalo.reclaimable_paths.contains(&active_ver));
    assert!(!zalo.reclaimable_paths.contains(&zalo_root.join("media")));
    assert!(!zalo.reclaimable_paths.contains(&zalo_root.join("message")));
    assert!(zalo.protected_paths.contains(&active_ver));
    assert!(zalo.protected_paths.contains(&zalo_root.join("message")));
    assert!(zalo.protected_paths.contains(&zalo_root.join("media")));
    assert!(zalo.physical_bytes >= 6144);

    // DingTalk discovery
    let dt_roaming = tmp.path().join("Roaming").join("DingTalk");
    let acct = dt_roaming.join("8899_v2");
    fs::create_dir_all(acct.join("EAppFiles").join("download")).unwrap();
    fs::create_dir_all(acct.join("DBFiles")).unwrap();
    fs::write(
        acct.join("EAppFiles").join("download").join("mini.amr"),
        vec![3u8; 4096],
    )
    .unwrap();
    let dt_local = local.join("DingTalk");
    fs::create_dir_all(dt_local.join("dumps")).unwrap();

    let dt = discover_dingtalk_profile(Some(&dt_roaming), Some(&dt_local)).unwrap();
    assert!(dt.protected_paths.contains(&acct.join("DBFiles")));
    assert!(dt.physical_bytes >= 4096);
}

#[test]
fn test_orphan_leftover_radar_flags_uninstalled_and_skips_active() {
    let tmp = TempDir::new().unwrap();
    let appdata = tmp.path().join("AppData");
    let orphan_dir = appdata.join("RemovedPhotoEditorPro");
    let active_dir = appdata.join("InstalledVideoSuite");
    let microsoft_dir = appdata.join("Microsoft");

    for dir in [&orphan_dir, &active_dir, &microsoft_dir] {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("cache.bin"), vec![9u8; 64 * 1024]).unwrap();
    }

    let radar = OrphanLeftoverRadar::new(10 * 1024, 0);
    let found = radar.scan(
        &[PathBuf::from(&appdata)],
        &["Installed Video Suite 2026".to_string()],
        &["explorer.exe".to_string()],
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].folder_name, "RemovedPhotoEditorPro");
}
