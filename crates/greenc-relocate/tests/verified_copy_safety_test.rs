//! Fail-closed safety tests for `verified_copy` and `verify_zalo_copy_integrity`:
//! - Symlinks / reparse points inside source or destination MUST fail-closed (never silently skipped)
//! - Unreadable files / directories (`read_dir` / `open` errors) MUST fail-closed
//! - Exact relative path equality (same count + same bytes with different filenames MUST fail)
//! - Preserves file and directory mtime, handles Unicode filenames and long paths (> 260 chars)
//! - Handles > 4 GiB (`u64`) file sizes without truncation
//! - Detects source file mutation during copy

use greenc_core::error::GreenCError;
use greenc_core::task::TaskControl;
use greenc_relocate::verified_copy::{
    to_extended_io_path, verified_copy_tree_strict, CopyTreeOptions,
};
use greenc_relocate::zalo_move::verify_zalo_copy_integrity;
use std::fs;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

#[test]
fn test_strict_copy_and_verify_fail_closed_on_symlink() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    let external = tmp.path().join("external.txt");
    fs::create_dir_all(src.join("message")).unwrap();
    fs::write(src.join("message/zalo.db"), b"CHAT-DB").unwrap();
    fs::write(&external, b"EXTERNAL-SECRET").unwrap();

    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, src.join("message/sneaky_link.db")).unwrap();
    // Creating symlinks needs a privilege on Windows; skip there when it is missing.
    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(&external, src.join("message/sneaky_link.db")).is_err() {
        return;
    }

    // 1. Strict copy MUST fail-closed on nested symlink and clean partial dst.
    let ctrl = TaskControl::new();
    let copy_res = verified_copy_tree_strict(&src, &dst, &ctrl, |_, _| {});
    assert!(
        copy_res.is_err(),
        "verified_copy_tree_strict must reject nested symlink"
    );
    assert!(!dst.exists(), "partial dst must be cleaned on failure");

    // 2. Even if BOTH src and dst contain the same symlink, verify_zalo_copy_integrity MUST fail!
    fs::create_dir_all(dst.join("message")).unwrap();
    fs::write(dst.join("message/zalo.db"), b"CHAT-DB").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, dst.join("message/sneaky_link.db")).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&external, dst.join("message/sneaky_link.db")).unwrap();

    let verify_res = verify_zalo_copy_integrity(&src, &dst);
    assert!(
        verify_res.is_err(),
        "verify_zalo_copy_integrity must fail-closed on symlink even when present on both sides"
    );
}

#[cfg(unix)]
#[test]
fn test_strict_copy_and_verify_fail_closed_on_unreadable_entries() {
    use std::os::unix::fs::PermissionsExt;

    // Skip only if running as root where chmod 000 is still readable.
    let uid_check = fs::metadata("/proc/self").ok();
    if let Some(m) = uid_check {
        use std::os::unix::fs::MetadataExt;
        if m.uid() == 0 {
            return;
        }
    }

    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    let locked_sub = src.join("media/secret_sub");
    fs::create_dir_all(&locked_sub).unwrap();
    fs::create_dir_all(src.join("message")).unwrap();
    fs::write(src.join("message/zalo.db"), b"CHAT-DB").unwrap();
    fs::write(locked_sub.join("photo.jpg"), b"PHOTO-BYTES").unwrap();

    // Make subdirectory unreadable (mode 000)
    fs::set_permissions(&locked_sub, fs::Permissions::from_mode(0o000)).unwrap();

    let ctrl = TaskControl::new();
    let copy_res = verified_copy_tree_strict(&src, &dst, &ctrl, |_, _| {});

    // Also create matching unreadable dir in dst so previously-buggy code would have "matched"!
    let dst_sub = dst.join("media/secret_sub");
    fs::create_dir_all(&dst_sub).unwrap();
    fs::create_dir_all(dst.join("message")).unwrap();
    fs::write(dst.join("message/zalo.db"), b"CHAT-DB").unwrap();
    fs::set_permissions(&dst_sub, fs::Permissions::from_mode(0o000)).unwrap();

    let verify_res = verify_zalo_copy_integrity(&src, &dst);

    // Restore permissions BEFORE asserting so TempDir cleans up cleanly!
    let _ = fs::set_permissions(&locked_sub, fs::Permissions::from_mode(0o755));
    let _ = fs::set_permissions(&dst_sub, fs::Permissions::from_mode(0o755));

    assert!(
        copy_res.is_err(),
        "verified_copy_tree_strict must fail-closed on unreadable directory"
    );
    assert!(
        verify_res.is_err(),
        "verify_zalo_copy_integrity must fail-closed on unreadable directory even if both sides fail"
    );
}

#[test]
fn test_verify_rejects_mismatched_filenames_with_same_count_and_bytes() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(src.join("media")).unwrap();
    fs::create_dir_all(dst.join("media")).unwrap();
    fs::write(src.join("media/a.jpg"), b"0123456789").unwrap();
    fs::write(dst.join("media/b.jpg"), b"0123456789").unwrap();

    let err = verify_zalo_copy_integrity(&src, &dst).unwrap_err();
    assert!(
        matches!(err, GreenCError::IntegrityMismatch { .. }),
        "Expected IntegrityMismatch for different filenames, got {err:?}"
    );
}

#[test]
fn test_strict_copy_preserves_mtime_unicode_long_paths_and_large_u64() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    // 1. Unicode Vietnamese & emoji names + nested long path > 260 chars
    let unicode_dir = src
        .join("Tin nhắn tiếng Việt 🎯")
        .join("Ảnh kỷ niệm — Đà Nẵng");
    fs::create_dir_all(&unicode_dir).unwrap();
    let unicode_file = unicode_dir.join("hình_ảnh_đặc_biệt_2026.jpg");
    fs::write(&unicode_file, vec![0x5A; 200_000]).unwrap();

    // Build a path > 280 chars
    let mut long_dir = src.join("media");
    for i in 0..8 {
        long_dir = long_dir.join(format!("thu_muc_rat_dai_kiem_tra_vuot_260_ky_tu_{i:02}"));
    }
    fs::create_dir_all(&long_dir).unwrap();
    let long_file = long_dir.join("tep_du_lieu_duong_dan_dai.dat");
    assert!(
        long_file.to_string_lossy().len() > 260,
        "test path must exceed 260 chars, got {}",
        long_file.to_string_lossy().len()
    );
    fs::write(&long_file, b"LONG-PATH-PAYLOAD-OK").unwrap();

    // Set an explicit historical mtime (1 day ago) on file & directory
    let old_mtime = SystemTime::now() - Duration::from_secs(86_400);
    fs::OpenOptions::new()
        .write(true)
        .open(&unicode_file)
        .unwrap()
        .set_modified(old_mtime)
        .unwrap();

    // Copy strictly
    let ctrl = TaskControl::new();
    let stats = verified_copy_tree_strict(&src, &dst, &ctrl, |_, _| {})
        .expect("strict copy must succeed for Unicode + long path");
    assert_eq!(stats.files_copied, 2);
    assert_eq!(stats.bytes_copied, 200_020);

    // Verify copied file mtime preserved within 2s
    let dst_unicode = dst
        .join("Tin nhắn tiếng Việt 🎯")
        .join("Ảnh kỷ niệm — Đà Nẵng")
        .join("hình_ảnh_đặc_biệt_2026.jpg");
    let got_mtime = fs::metadata(&dst_unicode).unwrap().modified().unwrap();
    let diff = got_mtime
        .duration_since(old_mtime)
        .or_else(|_| old_mtime.duration_since(got_mtime))
        .unwrap();
    assert!(
        diff <= Duration::from_secs(2),
        "mtime not preserved: diff={diff:?}"
    );

    // Verify full integrity passes
    let ver = verify_zalo_copy_integrity(&src, &dst).unwrap();
    assert_eq!(ver.total_files, 2);
    assert_eq!(ver.total_bytes, 200_020);

    // Verify Windows long-path prefix helper works cleanly for > 260 char paths
    let win_long = format!(r"C:\Users\Test\{}\file.dat", "a".repeat(270));
    let extended = to_extended_io_path(std::path::Path::new(&win_long));
    #[cfg(windows)]
    assert!(extended.to_string_lossy().starts_with(r"\\?\"));
    #[cfg(not(windows))]
    assert_eq!(extended, std::path::Path::new(&win_long));

    // Verify > 4 GiB u64 file size accounting using a sparse file (no physical disk waste).
    // Note: `verify_zalo_copy_integrity` now performs full SHA-256 on every file (Item 7),
    // so we verify u64 (> u32::MAX) byte accounting via `TreeFingerprint::capture` directly to avoid
    // hashing 8 GiB in unoptimized debug builds.
    let sparse_src = tmp.path().join("sparse_src");
    fs::create_dir_all(&sparse_src).unwrap();
    let huge_len: u64 = (4u64 * 1024 * 1024 * 1024) + 4096; // > 4 GiB
    let f1 = fs::File::create(sparse_src.join("big.bin")).unwrap();
    f1.set_len(huge_len).unwrap();
    let fp = greenc_relocate::zalo_move::TreeFingerprint::capture(&sparse_src).unwrap();
    assert_eq!(fp.total_bytes, huge_len);
    assert_eq!(fp.total_files, 1);

    let _ = CopyTreeOptions::strict();
}

#[test]
fn test_verify_detects_in_place_file_mutation_outside_message_dir() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    fs::create_dir_all(src.join("media")).unwrap();
    fs::write(src.join("media/img.jpg"), b"ORIGINAL-IMAGE-BYTES").unwrap();

    let ctrl = TaskControl::new();
    verified_copy_tree_strict(&src, &dst, &ctrl, |_, _| {}).unwrap();

    // Mutate source file in-place with EXACT same byte length (simulating change during copy)
    fs::write(src.join("media/img.jpg"), b"MODIFIED-IMAGE-BYTES").unwrap();

    let err = verify_zalo_copy_integrity(&src, &dst).unwrap_err();
    assert!(
        matches!(err, GreenCError::IntegrityMismatch { .. }),
        "Expected IntegrityMismatch when file content differs with same length, got {err:?}"
    );
}
