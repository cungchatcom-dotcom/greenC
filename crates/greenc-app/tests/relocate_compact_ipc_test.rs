//! Tests for Relocation and Compaction IPC commands and state management (Task T047, T052).

use greenc_app::commands::compact::{CompactionCandidateDto, CompactionResultDto};
use greenc_app::commands::relocate::{KnownFolderDto, RelocationCandidateDto, RelocationResultDto};
use greenc_app::state::AppState;
use greenc_core::models::{
    CompactionKind, CompactionRecord, RelocationHealthStatus, RelocationRecord, RelocationStatus,
    RelocationStrategyKind,
};
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_relocation_dtos_serialization() {
    let cand_dto = RelocationCandidateDto {
        id: "telegram".to_string(),
        name: "Telegram Desktop".to_string(),
        strategy: "junction".to_string(),
        source_path: "C:\\Users\\User\\AppData\\Roaming\\Telegram Desktop".to_string(),
        target_path: "D:\\GreenC\\Relocated\\telegram".to_string(),
        size_bytes: 1024 * 1024 * 500,
        file_count: 1500,
        is_locked: false,
        locked_processes: vec!["telegram.exe".to_string()],
        how_description: "NTFS Junction link".to_string(),
        is_advanced: false,
        is_eligible: true,
        ineligible_reason: None,
    };

    let json = serde_json::to_string(&cand_dto).expect("Serialization must succeed");
    assert!(json.contains("Telegram Desktop"));
    assert!(json.contains("524288000"));

    let deserialized: RelocationCandidateDto =
        serde_json::from_str(&json).expect("Deserialization must succeed");
    assert_eq!(deserialized.id, "telegram");
    assert_eq!(deserialized.strategy, "junction");

    let kf_dto = KnownFolderDto {
        folder_id: "downloads".to_string(),
        guid: "374DE290-123F-4565-9164-39C4925E467B".to_string(),
        name: "Downloads".to_string(),
        current_path: "C:\\Users\\User\\Downloads".to_string(),
        target_path: "D:\\Downloads".to_string(),
        size_bytes: 1024 * 1024 * 1000,
        file_count: 50,
        is_redirected: false,
        is_onedrive: false,
        target_drive: "D".to_string(),
    };

    let kf_json = serde_json::to_string(&kf_dto).unwrap();
    assert!(kf_json.contains("374DE290-123F-4565-9164-39C4925E467B"));

    let res_dto = RelocationResultDto {
        id: Some(42),
        candidate_id: "telegram".to_string(),
        app_name: "Telegram Desktop".to_string(),
        strategy: "junction".to_string(),
        source_path: "C:\\Users\\User\\AppData\\Roaming\\Telegram Desktop".to_string(),
        target_path: "D:\\GreenC\\Relocated\\telegram".to_string(),
        size_bytes: 524288000,
        status: "done".to_string(),
        health_status: "healthy".to_string(),
        detail: Some("Hoạt động bình thường".to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
        success: true,
        error_message: None,
    };

    let res_json = serde_json::to_string(&res_dto).unwrap();
    assert!(res_json.contains("\"id\":42"));
    assert!(res_json.contains("\"success\":true"));
}

#[test]
fn test_compaction_dtos_serialization() {
    let comp_cand = CompactionCandidateDto {
        id: "dir_cargo".to_string(),
        name: "SDK .cargo".to_string(),
        path: "C:\\Users\\User\\.cargo".to_string(),
        kind: "wof_directory".to_string(),
        logical_bytes: 1024 * 1024 * 500,
        physical_bytes: 1024 * 1024 * 500,
        estimated_reclaimable_bytes: 1024 * 1024 * 200,
        file_count: 8500,
        recommended_algorithm: "XPRESS8K".to_string(),
        is_locked: false,
        locked_processes: vec![],
        is_compacted: false,
        selected: true,
    };

    let json = serde_json::to_string(&comp_cand).unwrap();
    assert!(json.contains("SDK .cargo"));
    assert!(json.contains("XPRESS8K"));

    let comp_res = CompactionResultDto {
        task_id: "compact-123".to_string(),
        success: true,
        kind: "wof_directory".to_string(),
        target_path: "C:\\Users\\User\\.cargo".to_string(),
        algorithm: "XPRESS8K".to_string(),
        files_processed: 8500,
        files_compacted: 8400,
        files_skipped: 100,
        logical_bytes: 1024 * 1024 * 500,
        physical_before_bytes: 1024 * 1024 * 500,
        physical_after_bytes: 1024 * 1024 * 300,
        reclaimed_bytes: 1024 * 1024 * 200,
        compression_ratio: 0.60,
        error_message: None,
    };

    let res_json = serde_json::to_string(&comp_res).unwrap();
    assert!(res_json.contains("0.6"));
    assert!(res_json.contains("209715200"));
}

#[test]
fn test_relocation_and_compaction_history_persistence() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("greenc.db");
    let state = AppState::new(db_path).unwrap();

    // Insert relocation record
    let rel_rec = RelocationRecord {
        id: None,
        app_id: "test_app".to_string(),
        app_name: "Test Application".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: PathBuf::from("C:\\TestApp"),
        target_path: PathBuf::from("D:\\TestApp"),
        size_bytes: 1024 * 1024,
        status: RelocationStatus::Done,
        health_status: RelocationHealthStatus::Healthy,
        process_names: vec!["test.exe".to_string()],
        created_at: chrono::Utc::now(),
        detail: Some("Test detail".to_string()),
        moving_path: None,
        target_user_repaired: false,
    };
    let rec_id = state.db.save_relocation(&rel_rec).unwrap();
    assert!(rec_id > 0);

    let list = state.db.list_relocations(None).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].app_id, "test_app");

    // Insert compaction record
    let comp_rec = CompactionRecord {
        id: None,
        target_path: PathBuf::from("C:\\SDK"),
        kind: CompactionKind::WofDirectory,
        algorithm: "XPRESS8K".to_string(),
        logical_bytes: 1000,
        physical_before_bytes: 1000,
        physical_after_bytes: 500,
        file_count: 5,
        status: "compacted".to_string(),
        updated_at: chrono::Utc::now(),
    };
    let comp_id = state.db.save_compaction_record(&comp_rec).unwrap();
    assert!(comp_id > 0);

    let comp_list = state.db.list_compaction_records(None).unwrap();
    assert_eq!(comp_list.len(), 1);
    assert_eq!(comp_list[0].algorithm, "XPRESS8K");
}
