//! Integration tests for transparent WOF CompactOS, ratio verification,
//! decompression undo, and VHDX virtual disk trim (Tasks T049, T050, T051).

use greenc_core::task::TaskControl;
use greenc_relocate::compress::InPlaceCompactor;
use greenc_relocate::vhdx::VhdxCompactor;
use greenc_storage::db::GreenCDb;
use greenc_win32::traits::{
    MockCompactionApi, MockFileLockProbe, MockProcessServiceProbe, WofAlgorithm,
};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_wof_compact_directory_and_ratio_verification() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test_compact.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let comp_dir = tmp.path().join("RustToolchain");
    fs::create_dir_all(&comp_dir).unwrap();

    // Create test files >= 4 KiB with compressible extensions
    let big_data = vec![b'A'; 65536]; // 64 KiB
    fs::write(comp_dir.join("rustc.exe"), &big_data).unwrap();
    fs::write(comp_dir.join("std.dll"), &big_data).unwrap();

    let compaction_api = MockCompactionApi::new();
    // Default mock ratio is 0.50 (50% physical bytes -> 50% savings, well over 10% threshold)
    compaction_api.set_compression_ratio(0.50);

    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();

    let record = InPlaceCompactor::compact_directory(
        &comp_dir,
        WofAlgorithm::Xpress8k,
        &db,
        &compaction_api,
        &lock_probe,
        &ctrl,
        |_, _| {},
    )
    .expect("Compaction must succeed");

    assert_eq!(record.status, "compacted");
    assert_eq!(record.file_count, 2);
    assert!(record.physical_after_bytes < record.physical_before_bytes);

    // Verify record in SQLite database
    let db_rec = db
        .get_compaction_record(&comp_dir)
        .unwrap()
        .expect("Record must exist");
    assert_eq!(db_rec.status, "compacted");
    assert_eq!(db_rec.algorithm, "XPRESS8K");
}

#[test]
fn test_wof_compact_rolls_back_when_gain_is_poor() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test_poor_gain.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let comp_dir = tmp.path().join("Incompressible");
    fs::create_dir_all(&comp_dir).unwrap();

    let data = vec![b'Z'; 16384]; // 16 KiB
    fs::write(comp_dir.join("random.dat"), &data).unwrap();

    let compaction_api = MockCompactionApi::new();
    // Set ratio to 0.98 (only 2% savings -> fails 10% threshold requirement)
    compaction_api.set_compression_ratio(0.98);

    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();

    let record = InPlaceCompactor::compact_directory(
        &comp_dir,
        WofAlgorithm::Xpress8k,
        &db,
        &compaction_api,
        &lock_probe,
        &ctrl,
        |_, _| {},
    )
    .unwrap();

    // 0 files accepted because ratio was poor and rolled back
    assert_eq!(record.file_count, 0);
    assert_eq!(record.physical_after_bytes, record.physical_before_bytes);
}

#[test]
fn test_decompression_undo_reverts_files_and_db_status() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test_decompress.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let comp_dir = tmp.path().join("NodeModules");
    fs::create_dir_all(&comp_dir).unwrap();

    let data = vec![b'X'; 32768];
    fs::write(comp_dir.join("library.node"), &data).unwrap();

    let compaction_api = MockCompactionApi::new();
    let lock_probe = MockFileLockProbe::new();
    let ctrl = TaskControl::new();

    // 1. Compact
    let rec = InPlaceCompactor::compact_directory(
        &comp_dir,
        WofAlgorithm::Xpress8k,
        &db,
        &compaction_api,
        &lock_probe,
        &ctrl,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(rec.status, "compacted");

    // 2. Undo decompression
    InPlaceCompactor::decompress_directory(&comp_dir, &db, &compaction_api, &ctrl)
        .expect("Decompression must succeed");

    // Verify DB updated to "decompressed"
    let db_rec = db
        .get_compaction_record(&comp_dir)
        .unwrap()
        .expect("Record must exist");
    assert_eq!(db_rec.status, "decompressed");
}

#[test]
fn test_vhdx_compaction_and_running_vm_block() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test_vhdx.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let vhdx_file = tmp.path().join("ext4.vhdx");
    fs::write(&vhdx_file, vec![0u8; 1024 * 1024]).unwrap(); // 1 MiB dummy image

    let compaction_api = MockCompactionApi::new();
    let proc_probe = MockProcessServiceProbe::new();

    // Verify each VM host process individually triggers PreconditionBlocked:
    // 1. wsl.exe (classic WSL CLI)
    // 2. vmmem.exe (WSL2 Hyper-V worker process)
    // 3. vmmemwsl.exe (Windows 11 WSL2 specialized worker process)
    // 4. Docker Desktop.exe (Docker Desktop GUI / engine supervisor)
    let vm_processes = [
        ("wsl.exe", "wsl.exe"),
        ("vmmem.exe", "vmmem.exe"),
        ("vmmemwsl.exe", "vmmemwsl.exe"),
        ("Docker Desktop.exe", "Docker Desktop.exe"),
    ];

    for (proc_name, label) in vm_processes {
        proc_probe.set_process_running(proc_name, true);

        let blocked_res =
            VhdxCompactor::compact_vhdx_image(&vhdx_file, &db, &compaction_api, &proc_probe);

        assert!(
            blocked_res.is_err(),
            "Compaction must be blocked when {} is running",
            label
        );
        match blocked_res.err().unwrap() {
            greenc_core::error::GreenCError::PreconditionBlocked { reason: msg, .. } => {
                assert!(
                    msg.contains("wsl --shutdown"),
                    "Error message for {} must contain shutdown advice",
                    label
                );
            }
            other => panic!(
                "Expected PreconditionBlocked for running {}, got {:?}",
                label, other
            ),
        }

        // Reset process state before testing next process
        proc_probe.set_process_running(proc_name, false);
    }

    // Succeeded when all VM processes are stopped
    let record = VhdxCompactor::compact_vhdx_image(&vhdx_file, &db, &compaction_api, &proc_probe)
        .expect("VHDX compaction must succeed when all VM processes are stopped");

    assert_eq!(record.kind, greenc_core::models::CompactionKind::VhdxTrim);
    assert_eq!(record.algorithm, "VIRTDISK");
    assert_eq!(record.status, "compacted");
}

/// `compact undo` on a folder GreenC never compacted must be refused: it would otherwise
/// decompress CompactOS / user-compressed files and silently grow the drive.
#[test]
fn test_undo_refuses_folders_greenc_never_compacted() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("db.db")).unwrap();
    let dir = tmp.path().join("NotOurs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.bin"), vec![1u8; 8192]).unwrap();

    let api = MockCompactionApi::new();
    let err = InPlaceCompactor::decompress_directory(&dir, &db, &api, &TaskControl::new())
        .expect_err("undo without a compaction record must be refused");
    assert!(matches!(
        err,
        greenc_core::error::GreenCError::PreconditionBlocked { .. }
    ));
}

/// Failed decompressions must be reported, and the record must not claim "decompressed".
#[test]
fn test_undo_reports_failed_files_instead_of_claiming_success() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("db.db")).unwrap();
    let dir = tmp.path().join("Sdk");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("lib.dll"), vec![b'A'; 65536]).unwrap();

    let api = MockCompactionApi::new();
    let ctrl = TaskControl::new();
    InPlaceCompactor::compact_directory(
        &dir,
        WofAlgorithm::Xpress8k,
        &db,
        &api,
        &MockFileLockProbe::new(),
        &ctrl,
        |_, _| {},
    )
    .unwrap();

    api.set_fail_decompress(true);
    let report = InPlaceCompactor::decompress_directory(&dir, &db, &api, &ctrl).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(!report.is_complete());
    let rec = db.get_compaction_record(&dir).unwrap().unwrap();
    assert_ne!(rec.status, "decompressed");

    // Retrying once the problem is gone completes the undo (idempotent).
    api.set_fail_decompress(false);
    let report = InPlaceCompactor::decompress_directory(&dir, &db, &api, &ctrl).unwrap();
    assert!(report.is_complete());
    let report = InPlaceCompactor::decompress_directory(&dir, &db, &api, &ctrl).unwrap();
    assert!(report.is_complete(), "second undo is a harmless no-op");
    assert_eq!(
        db.get_compaction_record(&dir).unwrap().unwrap().status,
        "decompressed"
    );
}

/// Protected system folders must be refused up front with PreconditionBlocked, not
/// "succeed" with 0 bytes saved because every per-file refusal was swallowed.
#[test]
fn test_compact_refuses_protected_system_folder_up_front() {
    let tmp = TempDir::new().unwrap();
    let db = GreenCDb::open(tmp.path().join("db.db")).unwrap();
    let windir = std::env::var("windir").unwrap_or_else(|_| r"C:\Windows".to_string());
    let protected = std::path::Path::new(&windir).join(r"System32\drivers\etc");
    if !protected.exists() {
        return;
    }
    let err = InPlaceCompactor::compact_directory(
        &protected,
        WofAlgorithm::Xpress8k,
        &db,
        &MockCompactionApi::new(),
        &MockFileLockProbe::new(),
        &TaskControl::new(),
        |_, _| {},
    )
    .expect_err("protected folder must be refused");
    assert!(matches!(
        err,
        greenc_core::error::GreenCError::PreconditionBlocked { .. }
    ));
    assert!(db.get_compaction_record(&protected).unwrap().is_none());
}
