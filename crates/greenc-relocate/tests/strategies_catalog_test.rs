//! Integration tests for Windows Known Folders redirection, OneDrive guard,
//! Developer tool configurations, and Guided app profiles (Tasks T040, T043, T044, T045).

use greenc_core::task::TaskControl;
use greenc_platform::traits::{
    MockFileLockProbe, MockProcessServiceProbe, MockShellFolderApi, MockVolumeApi, ShellFolderApi,
};
use greenc_platform::{FOLDERID_DOCUMENTS, FOLDERID_DOWNLOADS};
use greenc_relocate::candidate::{CandidateQualificationEngine, CandidateScanner};
use greenc_relocate::catalog::{
    get_app_catalog, get_tool_catalog, ADVANCED_SKIP_LIST, ADVANCED_THRESHOLD_BYTES,
};
use greenc_relocate::known_folders::KnownFolderService;
use greenc_storage::db::GreenCDb;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_known_folder_redirection_and_rollback() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test_kf.db");
    let db = GreenCDb::open(&db_path).unwrap();

    let drives = vec![greenc_core::models::DriveInfo {
        root: PathBuf::from("D:\\"),
        drive_letter: 'D',
        label: "Data".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 200 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    }];
    let vol_api = MockVolumeApi::new(drives);
    let shell_api = MockShellFolderApi::new();

    // Set up mock downloads folder
    let downloads_src = tmp.path().join("UserDownloads");
    fs::create_dir_all(&downloads_src).unwrap();
    fs::write(downloads_src.join("sample.pdf"), b"%PDF-1.4 sample file").unwrap();

    shell_api.register_folder(FOLDERID_DOWNLOADS, downloads_src.clone());

    let target_dir = tmp.path().join("D_Drive").join("Downloads");
    let ctrl = TaskControl::new();

    // 1. Success case
    let record = KnownFolderService::relocate_known_folder(
        "downloads",
        'D',
        Some(target_dir.clone()),
        &db,
        &shell_api,
        &vol_api,
        &ctrl,
        |_, _| {},
    )
    .expect("Known folder relocation must succeed");

    assert_eq!(record.status, greenc_core::models::RelocationStatus::Done);
    assert!(target_dir.join("sample.pdf").exists());

    // Verify ShellFolderApi registered new path
    assert_eq!(
        shell_api.get_known_folder(FOLDERID_DOWNLOADS).unwrap(),
        target_dir
    );

    // 2. Failure rollback case: simulate set_known_folder rejection
    let docs_src = tmp.path().join("UserDocuments");
    fs::create_dir_all(&docs_src).unwrap();
    fs::write(docs_src.join("important.docx"), b"docx content").unwrap();
    shell_api.register_folder(FOLDERID_DOCUMENTS, docs_src.clone());

    let docs_target = tmp.path().join("D_Drive").join("Documents");
    shell_api.set_fail_set(true);

    let fail_res = KnownFolderService::relocate_known_folder(
        "documents",
        'D',
        Some(docs_target.clone()),
        &db,
        &shell_api,
        &vol_api,
        &ctrl,
        |_, _| {},
    );

    assert!(fail_res.is_err());
    // Target directory must be cleaned on failure
    assert!(!docs_target.exists());
    // Source data preserved
    assert!(docs_src.join("important.docx").exists());
}

#[test]
fn test_onedrive_path_detection_and_blocking() {
    let tmp = TempDir::new().unwrap();
    let shell_api = MockShellFolderApi::new();

    let onedrive_root = tmp.path().join("OneDrive");
    let onedrive_docs = onedrive_root.join("Documents");
    fs::create_dir_all(&onedrive_docs).unwrap();

    shell_api.add_onedrive_path(onedrive_root.clone());

    assert!(shell_api.is_onedrive_path(&onedrive_docs));
    assert!(shell_api.is_onedrive_path(&onedrive_root));

    let local_folder = tmp.path().join("LocalFolder");
    assert!(!shell_api.is_onedrive_path(&local_folder));

    // Verify qualification engine rejects OneDrive path
    let res = CandidateQualificationEngine::check_onedrive(&onedrive_docs, &shell_api);
    assert!(res.is_err());
    match res.err().unwrap() {
        greenc_core::error::GreenCError::PreconditionBlocked { reason: msg, .. } => {
            assert!(msg.contains("OneDrive"));
        }
        other => panic!("Expected PreconditionBlocked for OneDrive, got {:?}", other),
    }
}

#[test]
fn test_tool_catalog_and_app_catalog_definitions() {
    let tools = get_tool_catalog();
    assert_eq!(tools.len(), 10);

    let tool_ids: Vec<_> = tools.iter().map(|t| t.tool_id.as_str()).collect();
    assert!(tool_ids.contains(&"npm"));
    assert!(tool_ids.contains(&"pip"));
    assert!(tool_ids.contains(&"cargo"));
    assert!(tool_ids.contains(&"uv"));
    assert!(tool_ids.contains(&"pnpm"));
    assert!(tool_ids.contains(&"yarn"));
    assert!(tool_ids.contains(&"nuget"));
    assert!(tool_ids.contains(&"gradle"));
    assert!(tool_ids.contains(&"ollama"));
    assert!(tool_ids.contains(&"huggingface"));

    let apps = get_app_catalog();
    let app_ids: Vec<_> = apps.iter().map(|a| a.app_id.as_str()).collect();
    assert!(app_ids.contains(&"telegram"));
    assert!(app_ids.contains(&"discord"));
    assert!(app_ids.contains(&"slack"));
    assert!(app_ids.contains(&"jetbrains"));
    assert!(app_ids.contains(&"docker"));

    #[cfg(windows)]
    {
        assert!(app_ids.contains(&"capcut"));
        assert!(app_ids.contains(&"zalo"));
        assert!(app_ids.contains(&"wsl2"));

        // Verify WSL2 process list includes vmmem and vmmemwsl
        let wsl = apps.iter().find(|a| a.app_id == "wsl2").unwrap();
        assert!(wsl
            .process_names
            .iter()
            .any(|p| p.eq_ignore_ascii_case("vmmem.exe")));
        assert!(wsl
            .process_names
            .iter()
            .any(|p| p.eq_ignore_ascii_case("vmmemwsl.exe")));

        // Verify Docker process list includes Docker Desktop.exe
        let docker = apps.iter().find(|a| a.app_id == "docker").unwrap();
        assert!(docker
            .process_names
            .iter()
            .any(|p| p.eq_ignore_ascii_case("docker desktop.exe")));
    }

    #[cfg(target_os = "macos")]
    {
        assert!(app_ids.contains(&"xcode_derived"));
    }

    #[cfg(target_os = "linux")]
    {
        let docker = apps.iter().find(|a| a.app_id == "docker").unwrap();
        assert!(docker
            .process_names
            .iter()
            .any(|p| p.eq_ignore_ascii_case("dockerd")));
    }
}

#[test]
fn test_advanced_skip_list_coverage() {
    assert!(ADVANCED_SKIP_LIST.contains(&"microsoft"));
    assert!(ADVANCED_SKIP_LIST.contains(&"windows"));
    assert!(ADVANCED_SKIP_LIST.contains(&"packages"));
    assert!(ADVANCED_SKIP_LIST.contains(&"temp"));
    assert!(ADVANCED_SKIP_LIST.contains(&"system"));
    assert!(ADVANCED_SKIP_LIST.contains(&"system32"));
    assert!(ADVANCED_SKIP_LIST.contains(&"crashdumps"));
    assert!(ADVANCED_SKIP_LIST.contains(&"onedrive"));

    assert_eq!(ADVANCED_THRESHOLD_BYTES, 200 * 1024 * 1024);
}

#[test]
fn test_candidate_discovery_with_file_lock_probe() {
    let tmp = TempDir::new().unwrap();
    let vol_api = MockVolumeApi::new(vec![]);
    let shell_api = MockShellFolderApi::new();
    let lock_probe = MockFileLockProbe::new();
    let proc_probe = MockProcessServiceProbe::new();

    // Register a known folder with an exclusively locked file
    let docs = tmp.path().join("UserDocumentsLocked");
    fs::create_dir_all(&docs).unwrap();
    let active_db = docs.join("active.sqlite");
    fs::write(&active_db, b"SQLITE DATA").unwrap();
    lock_probe.lock(&active_db);

    shell_api.register_folder(FOLDERID_DOCUMENTS, docs.clone());

    // 1. Verify CandidateQualificationEngine detects file lock
    assert!(CandidateQualificationEngine::is_path_locked(
        &docs,
        &lock_probe
    ));
    let err = CandidateQualificationEngine::check_directory_locked(&docs, &lock_probe).unwrap_err();
    match err {
        greenc_core::error::GreenCError::FileLocked { reason: msg, .. } => {
            assert!(msg.contains("active.sqlite"));
        }
        other => panic!("Expected FileLocked, got {:?}", other),
    }

    // 2. Verify CandidateScanner marks locked candidate as is_locked = true
    let candidates =
        CandidateScanner::discover_all('D', &vol_api, &shell_api, &lock_probe, &proc_probe);
    let kf_doc = candidates
        .iter()
        .find(|c| c.id == "known_folder_documents")
        .unwrap();
    assert!(kf_doc.is_locked);
    assert!(!kf_doc.is_eligible);
}
