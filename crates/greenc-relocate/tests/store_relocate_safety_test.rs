//! Tests for Microsoft Store MSIX safety across relocation undo, junction repair, and KnownFolder undo.

use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
};
use greenc_core::task::TaskControl;
use greenc_platform::breakaway::{
    BreakawayExecOutput, BreakawayHelperClient, PrivateBreakawayJobDir, RestoreHelperRequest,
    RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE,
};
use greenc_platform::reparse::NativeReparsePointApi;
use greenc_platform::traits::{
    ProcessServiceProbe, ReparsePointApi, ShellFolderApi, Win32VolumeApi,
};
use greenc_relocate::mover::{execute_undo_relocation_dir_job, TransactionalMover};
use greenc_relocate::recovery::RecoveryService;
use greenc_storage::db::GreenCDb;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::tempdir;

struct DummyVolApi;
impl Win32VolumeApi for DummyVolApi {
    fn list_drives(&self) -> Result<Vec<greenc_core::models::DriveInfo>> {
        Ok(vec![])
    }
    fn get_disk_free_space(&self, _path: &Path) -> Result<(u64, u64)> {
        Ok((100 * 1024 * 1024 * 1024, 50 * 1024 * 1024 * 1024))
    }
    fn get_volume_info(&self, _drive_root: &Path) -> Result<(String, String, u32)> {
        Ok(("Data".to_string(), "NTFS".to_string(), 3))
    }
    fn is_elevated(&self) -> bool {
        false
    }
}

struct DummyShellApi;
impl ShellFolderApi for DummyShellApi {
    fn get_known_folder(&self, _folder_guid: &str) -> Result<PathBuf> {
        Ok(PathBuf::from("/tmp"))
    }
    fn set_known_folder(&self, _folder_guid: &str, _new_path: &Path) -> Result<()> {
        Ok(())
    }
    fn is_onedrive_path(&self, _path: &Path) -> bool {
        false
    }
}

struct DummyProcProbe;
impl ProcessServiceProbe for DummyProcProbe {
    fn is_process_running(&self, _exe_name: &str) -> bool {
        false
    }
    fn is_service_running(&self, _service_name: &str) -> Result<bool> {
        Ok(false)
    }
}

struct MockRelocBreakaway {
    packaged: bool,
    still_in_package: bool,
    calls: Arc<AtomicUsize>,
}

impl BreakawayHelperClient for MockRelocBreakaway {
    fn is_packaged(&self) -> bool {
        self.packaged
    }

    fn execute(
        &self,
        _job_dir: &PrivateBreakawayJobDir,
        stdin_json: &[u8],
    ) -> Result<BreakawayExecOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let req: RestoreHelperRequest = serde_json::from_slice(stdin_json)?;
        let reparse = NativeReparsePointApi;
        let (code, resp) = match req {
            RestoreHelperRequest::UndoRelocationDir {
                source_path,
                target_path,
            } => execute_undo_relocation_dir_job(
                &source_path,
                &target_path,
                &reparse,
                self.still_in_package,
            ),
            _ => (
                EXIT_HELPER_STILL_IN_PACKAGE,
                RestoreHelperResponse::failed("unsupported", "unexpected job"),
            ),
        };
        Ok(BreakawayExecOutput {
            exit_code: code,
            stdout: serde_json::to_string(&resp)?,
            stderr: String::new(),
        })
    }
}

#[test]
fn store_undo_appdata_junction_uses_breakaway_and_preserves_target_on_refusal() {
    let tmp = tempdir().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let vol = DummyVolApi;
    let shell = DummyShellApi;
    let proc_probe = DummyProcProbe;
    let ctrl = TaskControl::new();

    let source_appdata = tmp
        .path()
        .join("Users")
        .join("Alice")
        .join("AppData")
        .join("Roaming")
        .join("Telegram Desktop");
    let target_dir = tmp.path().join("D_Drive").join("Telegram");
    std::fs::create_dir_all(source_appdata.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(target_dir.join("chat.bin"), b"real-chat-data-123").unwrap();
    reparse
        .create_junction(&source_appdata, &target_dir)
        .unwrap();

    let rec_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "telegram".to_string(),
            app_name: "Telegram".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: source_appdata.clone(),
            target_path: target_dir.clone(),
            size_bytes: 18,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
        })
        .unwrap();

    // 1. In Store mode when breakaway is refused (still inside MSIX package):
    //    MUST fail with PreconditionBlocked, keep junction & target intact, and keep DB `Done`!
    #[cfg(feature = "store")]
    {
        let calls_refused = Arc::new(AtomicUsize::new(0));
        let refused_client = MockRelocBreakaway {
            packaged: true,
            still_in_package: true,
            calls: Arc::clone(&calls_refused),
        };
        let err = TransactionalMover::undo_relocation_with_breakaway(
            rec_id,
            &db,
            &vol,
            &reparse,
            &shell,
            &proc_probe,
            &ctrl,
            &refused_client,
        )
        .expect_err("must refuse undo when breakaway fails");
        assert!(matches!(err, GreenCError::PreconditionBlocked { .. }));
        assert_eq!(calls_refused.load(Ordering::SeqCst), 1);
        assert!(
            reparse.is_reparse_point(&source_appdata),
            "junction must remain intact when breakaway fails"
        );
        assert!(
            target_dir.join("chat.bin").is_file(),
            "target data must remain intact when breakaway fails"
        );
        let rec = db
            .list_relocations(None)
            .unwrap()
            .into_iter()
            .find(|r| r.id == Some(rec_id))
            .unwrap();
        assert_eq!(rec.status, RelocationStatus::Done);
    }

    // 2. When breakaway succeeds (or in standard non-Store build): restores real dir & removes target!
    let calls_ok = Arc::new(AtomicUsize::new(0));
    let ok_client = MockRelocBreakaway {
        packaged: true,
        still_in_package: false,
        calls: Arc::clone(&calls_ok),
    };
    let undone = TransactionalMover::undo_relocation_with_breakaway(
        rec_id,
        &db,
        &vol,
        &reparse,
        &shell,
        &proc_probe,
        &ctrl,
        &ok_client,
    )
    .expect("undo should succeed");

    assert_eq!(undone.status, RelocationStatus::Undone);
    assert!(!reparse.is_reparse_point(&source_appdata));
    assert_eq!(
        std::fs::read(source_appdata.join("chat.bin")).unwrap(),
        b"real-chat-data-123"
    );
    assert!(!target_dir.exists());
    if cfg!(feature = "store") {
        assert_eq!(calls_ok.load(Ordering::SeqCst), 1);
    } else {
        assert_eq!(calls_ok.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn store_blocks_known_folder_undo_and_appdata_junction_repair_without_deleting_anything() {
    let tmp = tempdir().unwrap();
    let db = GreenCDb::open(tmp.path().join("greenc.db")).unwrap();
    let reparse = NativeReparsePointApi;
    let vol = DummyVolApi;
    let shell = DummyShellApi;
    let proc_probe = DummyProcProbe;
    let ctrl = TaskControl::new();

    // KnownFolder undo must NOT delete target_path in Store mode.
    let kf_src = tmp.path().join("Documents");
    let kf_tgt = tmp.path().join("D_Documents");
    std::fs::create_dir_all(&kf_tgt).unwrap();
    std::fs::write(kf_tgt.join("doc.txt"), b"important-doc").unwrap();
    let kf_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "known_folder_documents".to_string(),
            app_name: "Documents".to_string(),
            strategy: RelocationStrategyKind::KnownFolder,
            source_path: kf_src.clone(),
            target_path: kf_tgt.clone(),
            size_bytes: 13,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::Healthy,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
        })
        .unwrap();

    let kf_res =
        TransactionalMover::undo_relocation(kf_id, &db, &vol, &reparse, &shell, &proc_probe, &ctrl);
    if cfg!(feature = "store") {
        assert!(matches!(
            kf_res,
            Err(GreenCError::PreconditionBlocked { .. })
        ));
        assert!(
            kf_tgt.join("doc.txt").is_file(),
            "Store KnownFolder undo refusal must NOT delete target files"
        );
    } else {
        assert!(kf_res.is_ok());
    }

    // repair_junction_target in AppData must NOT delete existing junction in Store mode!
    let appdata_link = tmp
        .path()
        .join("Users")
        .join("Alice")
        .join("AppData")
        .join("Local")
        .join("Discord");
    let old_target = tmp.path().join("OldTarget");
    let new_target = tmp.path().join("NewTarget");
    std::fs::create_dir_all(appdata_link.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&old_target).unwrap();
    std::fs::create_dir_all(&new_target).unwrap();
    reparse.create_junction(&appdata_link, &old_target).unwrap();

    let repair_id = db
        .save_relocation(&RelocationRecord {
            id: None,
            app_id: "discord".to_string(),
            app_name: "Discord".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: appdata_link.clone(),
            target_path: old_target.clone(),
            size_bytes: 100,
            status: RelocationStatus::Done,
            health_status: RelocationHealthStatus::LinkBroken,
            process_names: vec![],
            created_at: Utc::now(),
            detail: None,
        })
        .unwrap();

    let repair_res = RecoveryService::repair_junction_target(repair_id, &new_target, &db, &reparse);
    if cfg!(feature = "store") {
        assert!(matches!(
            repair_res,
            Err(GreenCError::PreconditionBlocked { .. })
        ));
        assert!(
            reparse.is_reparse_point(&appdata_link),
            "Store refusal in repair_junction_target must NOT destroy the existing junction"
        );
    } else {
        assert!(repair_res.is_ok());
    }
}
