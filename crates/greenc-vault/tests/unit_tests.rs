//! Unit tests for greenc-vault format sniffer, canary sentinel, and manifest roundtrip.

use chrono::Utc;
use greenc_core::models::FileManifestItem;
use greenc_vault::canary::{CanarySentinel, CanaryVerifier};
use greenc_vault::manifest::VaultManifest;
use greenc_vault::profiles::PresetProfiles;
use greenc_vault::sniff::{
    CompressionDecision, FormatSniffer, COMPRESSED_MAGIC_SIGNATURES, INCOMPRESSIBLE_EXTENSIONS,
};
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_incompressible_extension_bypass() {
    // 1. Verify all 47 literal extensions in the catalog are detected as Stored
    assert_eq!(INCOMPRESSIBLE_EXTENSIONS.len(), 47);

    for &ext in INCOMPRESSIBLE_EXTENSIONS {
        assert!(
            FormatSniffer::is_incompressible_extension(ext),
            "Extension '{}' must be recognized as incompressible",
            ext
        );
        let path = PathBuf::from(format!("sample.{}", ext));
        let decision = FormatSniffer::determine_compression(&path, &[]);
        assert_eq!(
            decision,
            CompressionDecision::Stored,
            "File with extension '{}' must decide Stored",
            ext
        );
    }

    // 2. Verify compressible extensions default to Deflated
    let compressible = [
        "txt", "log", "json", "xml", "csv", "sql", "html", "rs", "cpp",
    ];
    for &ext in &compressible {
        assert!(
            !FormatSniffer::is_incompressible_extension(ext),
            "Extension '{}' must not be marked incompressible",
            ext
        );
        let path = PathBuf::from(format!("document.{}", ext));
        let decision = FormatSniffer::determine_compression(&path, &[]);
        assert_eq!(
            decision,
            CompressionDecision::Deflated,
            "Compressible file '{}' must decide Deflated",
            ext
        );
    }
}

#[test]
fn test_magic_header_sniff() {
    // Verify all 15 magic header signatures are recognized as Stored without extension
    for &(magic, format_name) in COMPRESSED_MAGIC_SIGNATURES {
        let mut header = magic.to_vec();
        // Pad to at least 16 bytes
        header.resize(16, 0xAA);

        assert!(
            FormatSniffer::matches_magic_header(&header),
            "Magic bytes for '{}' must match header",
            format_name
        );

        let path = PathBuf::from("extensionless_blob");
        let decision = FormatSniffer::determine_compression(&path, &header);
        assert_eq!(
            decision,
            CompressionDecision::Stored,
            "Magic header '{}' must decide Stored",
            format_name
        );
    }

    // Test ISO-BMFF (ftyp at offset 4..8)
    let mut ftyp_header = vec![0x00, 0x00, 0x00, 0x20];
    ftyp_header.extend_from_slice(b"ftypisom");
    assert!(FormatSniffer::matches_magic_header(&ftyp_header));

    // Test RIFF WEBP
    let webp_header = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
    assert!(FormatSniffer::matches_magic_header(&webp_header));
}

#[test]
fn test_64k_sample_probe() {
    // Repetitive text (highly compressible): should decide Deflated
    let compressible_sample = vec![b'A'; 64 * 1024];
    let dec_comp = FormatSniffer::probe_compression(&compressible_sample);
    assert_eq!(dec_comp, CompressionDecision::Deflated);

    // Pseudorandom bytes (high entropy, incompressible): should decide Stored
    let mut random_sample = vec![0u8; 64 * 1024];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for byte in &mut random_sample {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *byte = (state >> 56) as u8;
    }
    let dec_rand = FormatSniffer::probe_compression(&random_sample);
    assert_eq!(dec_rand, CompressionDecision::Stored);
}

#[test]
fn test_manifest_roundtrip() {
    let tmp = tempdir().unwrap();
    let manifest_path = tmp.path().join("test_archive.manifest.json");

    let now = Utc::now();
    let files = vec![FileManifestItem {
        id: None,
        archive_id: None,
        original_path: PathBuf::from(r"C:\AppData\file1.dat"),
        rel_arcname: "item_00001_file1.dat".to_string(),
        size_bytes: 1024,
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        mtime: 1731760000.0,
        atime: 1731760000.0,
        hardlink_paths: vec![
            PathBuf::from(r"C:\AppData\file1.dat"),
            PathBuf::from(r"C:\AppData\file1_link.dat"),
        ],
        restored_individually: false,
    }];

    let manifest = VaultManifest::new(
        "vault_test_001".to_string(),
        1,
        "Zalo Desktop".to_string(),
        now,
        7,
        PathBuf::from(r"C:\AppData"),
        files.clone(),
        1024,
        512,
        "archive_hash_123".to_string(),
    );

    // Write to file
    manifest.write_to_file(&manifest_path).unwrap();
    assert!(manifest_path.exists());

    // Read back
    let loaded = VaultManifest::read_from_file(&manifest_path).unwrap();
    assert_eq!(loaded.archive_id, "vault_test_001");
    assert_eq!(loaded.app_name, "Zalo Desktop");
    assert_eq!(loaded.total_size, 1024);
    assert_eq!(loaded.compressed_size, 512);
    assert_eq!(loaded.files.len(), 1);
    assert_eq!(loaded.files[0].hardlink_paths.len(), 2);
    assert_eq!(loaded.files[0].sha256, files[0].sha256);
}

#[test]
fn test_canary_sentinel() {
    let (sentinel, bytes) = CanaryVerifier::generate_sentinel("vault_canary_01", 42, "hash_999");
    assert_eq!(sentinel.archive_id, "vault_canary_01");
    assert_eq!(sentinel.file_count, 42);
    assert!(!sentinel.nonce.is_empty());
    assert!(!bytes.is_empty());

    let parsed: CanarySentinel = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed.archive_id, sentinel.archive_id);
    assert_eq!(parsed.nonce, sentinel.nonce);
}

#[test]
fn test_preset_profiles_catalog() {
    let profiles = PresetProfiles::all();
    assert_eq!(profiles.len(), 8);

    let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&"windows_temp"));
    assert!(ids.contains(&"browser_cache"));
    assert!(ids.contains(&"chat_media"));
    assert!(ids.contains(&"crash_dumps"));
    assert!(ids.contains(&"package_managers"));
    assert!(ids.contains(&"app_logs"));
    assert!(ids.contains(&"old_versions"));
    assert!(ids.contains(&"orphan_radar"));

    for p in &profiles {
        assert!(!p.name.is_empty());
        assert!(!p.description.is_empty());
    }
}

#[test]
#[cfg(windows)]
fn old_versions_profile_follows_windir_instead_of_hard_coded_c_drive() {
    let tmp = tempfile::tempdir().unwrap();
    let fake_windir = tmp.path().join("Windows");
    std::fs::create_dir_all(fake_windir.join(r"SoftwareDistribution\Download")).unwrap();
    std::fs::create_dir_all(tmp.path().join("Windows.old")).unwrap();

    let saved = std::env::var_os("windir");
    std::env::set_var("windir", &fake_windir);
    let paths = greenc_vault::profiles::PresetProfiles::resolve_profile_paths("old_versions");
    match saved {
        Some(v) => std::env::set_var("windir", v),
        None => std::env::remove_var("windir"),
    }

    assert!(paths.contains(&tmp.path().join("Windows.old")));
    assert!(paths.iter().all(|p| p.starts_with(tmp.path())), "{paths:?}");
}

#[test]
fn test_store_staging_skips_appdata_vault_and_uses_non_appdata_home() {
    let tmp = tempdir().unwrap();
    let fake_appdata_vault = tmp
        .path()
        .join("AppData")
        .join("Local")
        .join("GreenC-Store")
        .join("Vault");
    let fake_home = tmp.path().join("UserHome");
    let unit_root = tmp.path().join("cache_root").join("unit_a");
    let allowed_parent = tmp.path().join("cache_root");
    std::fs::create_dir_all(&fake_appdata_vault).unwrap();
    std::fs::create_dir_all(&fake_home).unwrap();
    std::fs::create_dir_all(&unit_root).unwrap();

    let chosen = greenc_vault::guard::resolve_staging_dir_checked(
        &unit_root,
        &allowed_parent,
        &fake_appdata_vault,
        None,
        Some(&fake_home),
        None,
    )
    .expect("staging dir should resolve");

    if cfg!(feature = "store") {
        assert!(
            !chosen.starts_with(&fake_appdata_vault),
            "Store builds must skip AppData-hosted Vault staging, got {}",
            chosen.display()
        );
        assert!(
            chosen.starts_with(&fake_home),
            "Store builds must fall back to user-owned non-AppData staging, got {}",
            chosen.display()
        );
    } else {
        assert!(
            chosen.starts_with(&fake_appdata_vault),
            "Standard builds keep using same-drive Vault staging, got {}",
            chosen.display()
        );
    }
}
