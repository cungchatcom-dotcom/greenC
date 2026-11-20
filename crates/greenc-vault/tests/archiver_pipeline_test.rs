//! Integration tests for ArchiverPipeline and SafeCleaner.

use greenc_core::error::GreenCError;
use greenc_core::models::{CleanCandidate, DriveInfo};
use greenc_core::task::TaskControl;
use greenc_platform::traits::{MockFileLockProbe, MockVolumeApi};
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_vault::cleanup::{CleanupOptions, DeletionMethod, SafeCleaner};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::tempdir;

#[test]
fn test_archiving_and_manifest_creation() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&vault_dir).unwrap();

    // Create 3 dummy candidate files
    let file1_path = src_dir.join("cache1.dat");
    let file2_path = src_dir.join("photo.jpg");
    let file3_path = src_dir.join("data.txt");

    {
        let mut f1 = File::create(&file1_path).unwrap();
        f1.write_all(b"Hello Cache 1 Data").unwrap();
        let mut f2 = File::create(&file2_path).unwrap();
        f2.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46])
            .unwrap();
        let mut f3 = File::create(&file3_path).unwrap();
        f3.write_all(b"Plain text compressible content line 1\nLine 2\nLine 3")
            .unwrap();
    }

    let candidates = vec![
        CleanCandidate {
            path: file1_path.clone(),
            size_bytes: std::fs::metadata(&file1_path).unwrap().len(),
            mtime: SystemTime::now(),
            is_locked: false,
            profile_id: "test".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: vec![file1_path.clone()],
        },
        CleanCandidate {
            path: file2_path.clone(),
            size_bytes: std::fs::metadata(&file2_path).unwrap().len(),
            mtime: SystemTime::now(),
            is_locked: false,
            profile_id: "test".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: vec![file2_path.clone()],
        },
        CleanCandidate {
            path: file3_path.clone(),
            size_bytes: std::fs::metadata(&file3_path).unwrap().len(),
            mtime: SystemTime::now(),
            is_locked: false,
            profile_id: "test".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: vec![file3_path.clone()],
        },
    ];

    let volume_api = MockVolumeApi::new(vec![DriveInfo {
        root: tmp.path().to_path_buf(),
        drive_letter: 'C',
        label: "OS".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 100 * 1024 * 1024 * 1024,
        free_bytes: 50 * 1024 * 1024 * 1024, // 50 GiB free
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    }]);

    let options = ArchiveOptions {
        vault_dir: vault_dir.clone(),
        prefetch_workers: 2,
        prefetch_max_file_size: 8 * 1024 * 1024,
        stream_chunk_size: 1024 * 1024,
        min_vault_reserve_bytes: 512 * 1024 * 1024,
        retention_days: 7,
    };

    let pipeline = ArchiverPipeline::new(options);
    let task_control = TaskControl::new();

    let manifest = pipeline
        .archive_candidates(
            1,
            "TestApp",
            &candidates,
            &volume_api,
            &task_control,
            |_progress| {},
        )
        .expect("Archiving should succeed");

    assert_eq!(manifest.files.len(), 3);
    assert!(manifest.total_size > 0);
    assert!(!manifest.sha256.is_empty());

    let zip_file = vault_dir.join(format!("{}.zip", manifest.archive_id));
    let manifest_file = vault_dir.join(format!("{}.manifest.json", manifest.archive_id));
    assert!(zip_file.exists());
    assert!(manifest_file.exists());
}

#[test]
fn test_reserve_space_violation() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&vault_dir).unwrap();

    let file_path = src_dir.join("sample.dat");
    {
        let mut f = File::create(&file_path).unwrap();
        f.write_all(&vec![0u8; 1024 * 1024]).unwrap(); // 1 MiB
    }

    let candidates = vec![CleanCandidate {
        path: file_path.clone(),
        size_bytes: 1024 * 1024,
        mtime: SystemTime::now(),
        is_locked: false,
        profile_id: "test".to_string(),
        is_canary: false,
        is_orphan: false,
        hardlink_siblings: vec![file_path],
    }];

    // Mock reporting only 100 MiB free space (< 512 MiB reserve + 1 MiB required)
    let volume_api = MockVolumeApi::new(vec![DriveInfo {
        root: tmp.path().to_path_buf(),
        drive_letter: 'C',
        label: "OS".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 10 * 1024 * 1024 * 1024,
        free_bytes: 100 * 1024 * 1024, // 100 MiB free
        is_system: true,
        is_eligible: true,
        ineligible_reason: None,
    }]);

    let options = ArchiveOptions {
        vault_dir: vault_dir.clone(),
        min_vault_reserve_bytes: 512 * 1024 * 1024,
        ..Default::default()
    };

    let pipeline = ArchiverPipeline::new(options);
    let task_control = TaskControl::new();

    let result = pipeline.archive_candidates(
        1,
        "TestApp",
        &candidates,
        &volume_api,
        &task_control,
        |_| {},
    );

    match result {
        Err(GreenCError::InsufficientVaultSpace(msg)) => {
            assert!(msg.contains("512 MiB reserve"));
        }
        other => panic!("Expected InsufficientVaultSpace, got {:?}", other),
    }

    // Verify zero leftover zip or manifest files
    let zip_entries: Vec<_> = std::fs::read_dir(&vault_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(zip_entries.len(), 0);
}

#[test]
fn test_two_phase_safe_cleanup() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    std::fs::create_dir_all(&src_dir).unwrap();

    let file_unlocked = src_dir.join("unlocked.dat");
    let file_locked = src_dir.join("locked.dat");
    let file_modified = src_dir.join("modified.dat");

    {
        File::create(&file_unlocked)
            .unwrap()
            .write_all(b"unlocked content")
            .unwrap();
        File::create(&file_locked)
            .unwrap()
            .write_all(b"locked content")
            .unwrap();
        File::create(&file_modified)
            .unwrap()
            .write_all(b"original modified content")
            .unwrap();
    }

    let meta_unlocked = std::fs::metadata(&file_unlocked).unwrap();
    let meta_locked = std::fs::metadata(&file_locked).unwrap();
    let meta_modified = std::fs::metadata(&file_modified).unwrap();

    let mut snapshots = HashMap::new();
    snapshots.insert(
        file_unlocked.clone(),
        (meta_unlocked.len(), meta_unlocked.modified().unwrap()),
    );
    snapshots.insert(
        file_locked.clone(),
        (meta_locked.len(), meta_locked.modified().unwrap()),
    );
    snapshots.insert(
        file_modified.clone(),
        (meta_modified.len(), meta_modified.modified().unwrap()),
    );

    // Simulate in-flight modification
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut f = File::create(&file_modified).unwrap();
        f.write_all(b"in-flight modified content with different length")
            .unwrap();
    }

    let lock_probe = MockFileLockProbe::new();
    lock_probe.lock(&file_locked);

    let volume_api = MockVolumeApi::default();
    let cleaner = SafeCleaner::new(
        &lock_probe,
        &volume_api,
        None,
        CleanupOptions {
            deletion_method: DeletionMethod::Permanent,
            min_reserve_bytes: 512 * 1024 * 1024,
            prune_empty_directories: false,
        },
    );

    let manifest_items = vec![
        greenc_core::models::FileManifestItem {
            id: None,
            archive_id: None,
            original_path: file_unlocked.clone(),
            rel_arcname: "item_00001_unlocked.dat".to_string(),
            size_bytes: meta_unlocked.len(),
            sha256: "hash1".to_string(),
            mtime: 0.0,
            atime: 0.0,
            hardlink_paths: vec![file_unlocked.clone()],
            restored_individually: false,
        },
        greenc_core::models::FileManifestItem {
            id: None,
            archive_id: None,
            original_path: file_locked.clone(),
            rel_arcname: "item_00002_locked.dat".to_string(),
            size_bytes: meta_locked.len(),
            sha256: "hash2".to_string(),
            mtime: 0.0,
            atime: 0.0,
            hardlink_paths: vec![file_locked.clone()],
            restored_individually: false,
        },
        greenc_core::models::FileManifestItem {
            id: None,
            archive_id: None,
            original_path: file_modified.clone(),
            rel_arcname: "item_00003_modified.dat".to_string(),
            size_bytes: meta_modified.len(),
            sha256: "hash3".to_string(),
            mtime: 0.0,
            atime: 0.0,
            hardlink_paths: vec![file_modified.clone()],
            restored_individually: false,
        },
    ];

    let result = cleaner
        .unlink_archived_items(&manifest_items, &snapshots)
        .unwrap();

    // Exactly 1 file deleted (unlocked)
    assert_eq!(result.files_deleted, 1);
    assert!(!file_unlocked.exists(), "Unlocked file must be deleted");

    // Locked file must be preserved
    assert!(
        file_locked.exists(),
        "Locked file must be preserved on disk"
    );

    // Modified file must be preserved
    assert!(
        file_modified.exists(),
        "Modified file must be preserved on disk"
    );

    assert_eq!(result.skipped_files_count, 2);
}

#[test]
fn test_relative_vault_dir_resolution() {
    let root = ArchiverPipeline::resolve_volume_root(Path::new("vault"));
    assert!(!root.as_os_str().is_empty());
    assert!(root.to_string_lossy().contains(':') || root.to_string_lossy().starts_with('/'));

    let root_dot = ArchiverPipeline::resolve_volume_root(Path::new(r".\vault"));
    assert!(!root_dot.as_os_str().is_empty());

    #[cfg(windows)]
    {
        let root_abs = ArchiverPipeline::resolve_volume_root(Path::new(r"D:\Data\Vault"));
        assert_eq!(root_abs, PathBuf::from(r"D:\"));
    }
}

#[test]
fn test_hardlink_candidate_deduplication() {
    let tmp = tempdir().unwrap();
    let src_dir = tmp.path().join("source");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&vault_dir).unwrap();

    let primary = src_dir.join("primary.dat");
    let sibling = src_dir.join("sibling.dat");
    std::fs::write(&primary, b"Shared hardlink content").unwrap();

    let ok = std::fs::hard_link(&primary, &sibling).is_ok();

    if ok {
        let candidates = vec![
            CleanCandidate {
                path: primary.clone(),
                size_bytes: 23,
                mtime: SystemTime::now(),
                is_locked: false,
                profile_id: "test".to_string(),
                is_canary: false,
                is_orphan: false,
                hardlink_siblings: vec![primary.clone(), sibling.clone()],
            },
            CleanCandidate {
                path: sibling.clone(),
                size_bytes: 23,
                mtime: SystemTime::now(),
                is_locked: false,
                profile_id: "test".to_string(),
                is_canary: false,
                is_orphan: false,
                hardlink_siblings: vec![sibling.clone(), primary.clone()],
            },
        ];

        let pipeline = ArchiverPipeline::new(ArchiveOptions {
            vault_dir: vault_dir.clone(),
            ..Default::default()
        });

        let deduped = pipeline.deduplicate_candidates(&candidates);
        assert_eq!(
            deduped.len(),
            1,
            "Hardlinks must be deduplicated to 1 candidate"
        );
        assert_eq!(deduped[0].hardlink_siblings.len(), 2);
    }
}
