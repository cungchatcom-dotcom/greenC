//! Transactional rollback engine and safety watchdog auto-rollback (Tasks T034, T035).
//!
//! Features:
//! - Full archive restoration and single-file selective restore
//! - Non-destructive conflict handling (`.restored_<timestamp>`)
//! - Hardware SHA-256 integrity verification during extraction
//! - HardLink sibling reconstruction and timestamp restoration
//! - Automatic watchdog missing-file log scanning and service crash auto-rollback
//! - Automatic quarantine expiration purging

use crate::manifest::{compute_file_sha256, VaultManifest};
use chrono::Utc;
use greenc_core::config::{default_vault_dir, is_windows_appdata_app_dir};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{VaultFileItem, VaultStatus};
use greenc_platform::breakaway::{
    current_package_full_name, format_breakaway_unavailable_guidance, BreakawayHelperClient,
    HelperConflictStrategy, NativeBreakawayHelperClient, PrivateBreakawayJobDir, RestoreFileJob,
    RestoreFileOutcome, RestoreHelperRequest, RestoreHelperResponse, EXIT_HELPER_STILL_IN_PACKAGE,
};
use greenc_platform::handle::{ensure_not_protected_system_path, is_owned_by_current_user};
use greenc_platform::traits::ProcessServiceProbe;
use greenc_storage::db::GreenCDb;
use greenc_storage::settings::AppSettings;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{File, FileTimes};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use zip::ZipArchive;

fn path_has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Validates that a Vault artifact path (`zip_path` or `manifest_path`) read from DB/manifest is
/// strictly contained inside `vault_dir`:
/// - absolute, no `..`, filename ends with `expected_suffix` (`.zip` or `.manifest.json`),
/// - `vault_dir` is a real directory (not a symlink),
/// - `path` is a real regular file (not a symlink), owned by the current user,
/// - canonicalized `path` starts with canonicalized `vault_dir`.
pub fn validate_vault_artifact_path(
    path: &Path,
    vault_dir: &Path,
    expected_suffix: &str,
) -> Result<PathBuf> {
    if !path.is_absolute() || path_has_parent_dir(path) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_validation".to_string(),
            reason: format!(
                "Đường dẫn gói Vault '{}' không hợp lệ (không tuyệt đối hoặc chứa '..')",
                path.display()
            ),
        });
    }
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !fname.ends_with(expected_suffix) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_validation".to_string(),
            reason: format!(
                "Đường dẫn gói Vault '{}' không khớp hậu tố '{}'",
                path.display(),
                expected_suffix
            ),
        });
    }

    let vmeta =
        std::fs::symlink_metadata(vault_dir).map_err(|e| GreenCError::PreconditionBlocked {
            operation: "vault_artifact_validation".to_string(),
            reason: format!(
                "Không kiểm tra được thư mục Vault '{}': {e}",
                vault_dir.display()
            ),
        })?;
    if vmeta.file_type().is_symlink() || !vmeta.is_dir() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_validation".to_string(),
            reason: format!(
                "Thư mục Vault '{}' là symlink hoặc không phải thư mục thật",
                vault_dir.display()
            ),
        });
    }
    let canon_vault = vault_dir.canonicalize()?;

    let pmeta = std::fs::symlink_metadata(path)?;
    if pmeta.file_type().is_symlink() || !pmeta.is_file() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_validation".to_string(),
            reason: format!(
                "Tệp Vault '{}' là symlink hoặc không phải tệp thường",
                path.display()
            ),
        });
    }
    match is_owned_by_current_user(path) {
        Ok(true) => {}
        Ok(false) => {
            return Err(GreenCError::PreconditionBlocked {
                operation: "vault_artifact_validation".to_string(),
                reason: format!(
                    "Tệp Vault '{}' không thuộc người dùng hiện tại",
                    path.display()
                ),
            });
        }
        Err(e) => {
            return Err(GreenCError::PreconditionBlocked {
                operation: "vault_artifact_validation".to_string(),
                reason: format!(
                    "Không xác minh được chủ sở hữu tệp Vault '{}': {e}",
                    path.display()
                ),
            });
        }
    }

    let canon_path = path.canonicalize()?;
    if !canon_path.starts_with(&canon_vault) || canon_path == canon_vault {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_validation".to_string(),
            reason: format!(
                "Tệp Vault '{}' nằm ngoài thư mục Vault '{}'",
                path.display(),
                canon_vault.display()
            ),
        });
    }

    Ok(canon_path)
}

/// Safely deletes a Vault `.zip` or `.manifest.json` file ONLY after verifying it lies strictly
/// inside `vault_dir` (no `..`, not a symlink, regular file owned by current user).
/// Returns `Ok(true)` if removed, `Ok(false)` if already absent inside `vault_dir`, or `Err` if
/// the path fails containment checks.
pub fn safe_remove_vault_artifact(
    path: &Path,
    vault_dir: &Path,
    expected_suffix: &str,
) -> Result<bool> {
    if !path.is_absolute() || path_has_parent_dir(path) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_remove".to_string(),
            reason: format!(
                "Từ chối xoá đường dẫn gói Vault '{}' không hợp lệ",
                path.display()
            ),
        });
    }
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !fname.ends_with(expected_suffix) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_remove".to_string(),
            reason: format!(
                "Từ chối xoá tệp '{}' không khớp hậu tố '{}'",
                path.display(),
                expected_suffix
            ),
        });
    }

    let vmeta =
        std::fs::symlink_metadata(vault_dir).map_err(|e| GreenCError::PreconditionBlocked {
            operation: "vault_artifact_remove".to_string(),
            reason: format!(
                "Không kiểm tra được thư mục Vault '{}': {e}",
                vault_dir.display()
            ),
        })?;
    if vmeta.file_type().is_symlink() || !vmeta.is_dir() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_artifact_remove".to_string(),
            reason: format!(
                "Thư mục Vault '{}' là symlink hoặc không phải thư mục thật",
                vault_dir.display()
            ),
        });
    }
    let canon_vault = vault_dir.canonicalize()?;

    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Even when absent, confirm parent resides within `vault_dir` so tampered paths are rejected.
            let parent_inside = path
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .is_some_and(|cp| cp.starts_with(&canon_vault));
            if !parent_inside {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "vault_artifact_remove".to_string(),
                    reason: format!(
                        "Đường dẫn gói Vault '{}' nằm ngoài thư mục Vault '{}'",
                        path.display(),
                        canon_vault.display()
                    ),
                });
            }
            Ok(false)
        }
        Err(e) => Err(GreenCError::Io(e)),
        Ok(_) => {
            let verified = validate_vault_artifact_path(path, vault_dir, expected_suffix)?;
            std::fs::remove_file(&verified)?;
            Ok(true)
        }
    }
}

/// Validates that a restore target path is absolute, contains no `..` traversal components,
/// is not a symlink, and does not target a protected Windows/Unix OS or MSIX package path.
pub fn validate_restore_file_path(path: &Path, operation: &str) -> Result<()> {
    if !path.is_absolute() || path_has_parent_dir(path) {
        return Err(GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Đường dẫn khôi phục '{}' không tuyệt đối hoặc chứa '..'",
                path.display()
            ),
        });
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(GreenCError::PreconditionBlocked {
                operation: operation.to_string(),
                reason: format!("Từ chối ghi/xoá qua symlink '{}'", path.display()),
            });
        }
    }
    ensure_not_protected_system_path(path, operation)?;
    Ok(())
}

/// Maps `original_path` safely underneath an alternate `restore_dir` (e.g.,
/// `Documents\GreenC Restored`) without allowing `..` traversal or drive/root clobbering.
pub fn map_path_into_restore_dir(original_path: &Path, restore_dir: &Path) -> Result<PathBuf> {
    validate_restore_file_path(restore_dir, "vault_restore_dir")?;
    if let Ok(meta) = std::fs::symlink_metadata(restore_dir) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(GreenCError::PreconditionBlocked {
                operation: "vault_restore_dir".to_string(),
                reason: format!(
                    "Thư mục khôi phục '{}' là symlink hoặc không phải thư mục thật",
                    restore_dir.display()
                ),
            });
        }
    }

    let raw = original_path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let mut mapped = restore_dir.to_path_buf();
    let mut pushed_any = false;

    // Handle Windows drive prefix (`C:\...`) consistently even when running tests on Unix.
    let remainder = {
        let bytes = stripped.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            mapped.push((bytes[0] as char).to_ascii_uppercase().to_string());
            pushed_any = true;
            &stripped[2..]
        } else {
            stripped
        }
    };

    for seg in remainder.split(['\\', '/']) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(GreenCError::PreconditionBlocked {
                operation: "vault_restore_dir".to_string(),
                reason: format!(
                    "Đường dẫn gốc '{}' chứa '..' không thể ánh xạ an toàn",
                    original_path.display()
                ),
            });
        }
        mapped.push(seg);
        pushed_any = true;
    }

    if !pushed_any {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_restore_dir".to_string(),
            reason: format!(
                "Không trích xuất được đường dẫn tương đối từ '{}'",
                original_path.display()
            ),
        });
    }

    validate_restore_file_path(&mapped, "vault_restore_dir")?;
    Ok(mapped)
}

/// Strategy to resolve collisions when the restore target file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreConflictStrategy {
    /// Non-destructive: writes to `<original_name>.restored_<timestamp>`.
    RenameWithTimestamp,
    /// Overwrites only if the existing file is 0 bytes; otherwise renames.
    #[default]
    OverwriteIfZeroLength,
    /// Overwrites existing file unconditionally.
    Overwrite,
}

impl From<RestoreConflictStrategy> for HelperConflictStrategy {
    fn from(value: RestoreConflictStrategy) -> Self {
        match value {
            RestoreConflictStrategy::RenameWithTimestamp => Self::RenameWithTimestamp,
            RestoreConflictStrategy::OverwriteIfZeroLength => Self::OverwriteIfZeroLength,
            RestoreConflictStrategy::Overwrite => Self::Overwrite,
        }
    }
}

impl From<HelperConflictStrategy> for RestoreConflictStrategy {
    fn from(value: HelperConflictStrategy) -> Self {
        match value {
            HelperConflictStrategy::RenameWithTimestamp => Self::RenameWithTimestamp,
            HelperConflictStrategy::OverwriteIfZeroLength => Self::OverwriteIfZeroLength,
            HelperConflictStrategy::Overwrite => Self::Overwrite,
        }
    }
}

/// Options configuring archive restoration.
#[derive(Clone)]
pub struct RestoreOptions {
    /// Conflict strategy for existing files on disk.
    pub conflict_strategy: RestoreConflictStrategy,
    /// Whether to verify SHA-256 integrity during extraction.
    pub verify_sha256: bool,
    /// Whether to recreate NTFS hardlink siblings.
    pub restore_hardlinks: bool,
    /// Whether to restore original access and modification timestamps.
    pub restore_timestamps: bool,
    /// Whether to add restored paths to permanent protected files list.
    pub add_to_protected_list: bool,
    /// Optional explicit Vault directory used to enforce strict containment on `zip_path` and
    /// `manifest_path`. When `None`, resolves via `AppSettings::vault_dir(&default_vault_dir())`.
    pub vault_dir: Option<PathBuf>,
    /// Optional alternate destination directory (e.g., `Documents\GreenC Restored`) used when the
    /// user chooses to restore outside AppData (including fallback if MSIX breakaway is blocked).
    pub restore_dir: Option<PathBuf>,
    /// Optional injectable MSIX breakaway helper client (defaults to [`NativeBreakawayHelperClient`]).
    pub breakaway_client: Option<Arc<dyn BreakawayHelperClient>>,
}

impl std::fmt::Debug for RestoreOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreOptions")
            .field("conflict_strategy", &self.conflict_strategy)
            .field("verify_sha256", &self.verify_sha256)
            .field("restore_hardlinks", &self.restore_hardlinks)
            .field("restore_timestamps", &self.restore_timestamps)
            .field("add_to_protected_list", &self.add_to_protected_list)
            .field("vault_dir", &self.vault_dir)
            .field("restore_dir", &self.restore_dir)
            .field("has_breakaway_client", &self.breakaway_client.is_some())
            .finish()
    }
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            conflict_strategy: RestoreConflictStrategy::OverwriteIfZeroLength,
            verify_sha256: true,
            restore_hardlinks: true,
            restore_timestamps: true,
            add_to_protected_list: true,
            vault_dir: None,
            restore_dir: None,
            breakaway_client: None,
        }
    }
}

/// Result summary of an archive or selective rollback operation.
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// ID of the affected archive.
    pub archive_id: i64,
    /// Number of files restored.
    pub restored_count: usize,
    /// Total bytes restored.
    pub restored_bytes: u64,
    /// Restored file paths on disk.
    pub restored_paths: Vec<PathBuf>,
    /// Whether this rollback was triggered automatically by the watchdog.
    pub auto_triggered: bool,
    /// Reason string for the rollback.
    pub reason: String,
}

/// Transactional Rollback Engine for restoring Canary Vault archives.
pub struct RollbackEngine<'a> {
    db: &'a GreenCDb,
    options: RestoreOptions,
}

impl<'a> RollbackEngine<'a> {
    /// Creates a new rollback engine bound to a SQLite storage instance.
    pub fn new(db: &'a GreenCDb, options: RestoreOptions) -> Self {
        Self { db, options }
    }

    fn effective_vault_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.options.vault_dir {
            return Ok(dir.clone());
        }
        AppSettings::new(self.db).vault_dir(&default_vault_dir())
    }

    fn breakaway_client(&self) -> Arc<dyn BreakawayHelperClient> {
        self.options
            .breakaway_client
            .clone()
            .unwrap_or_else(|| Arc::new(NativeBreakawayHelperClient))
    }

    fn map_target_path(&self, original_path: &Path) -> Result<PathBuf> {
        if let Some(ref restore_dir) = self.options.restore_dir {
            map_path_into_restore_dir(original_path, restore_dir)
        } else {
            validate_restore_file_path(original_path, "vault_restore")?;
            Ok(original_path.to_path_buf())
        }
    }

    fn should_use_store_breakaway(
        &self,
        items: &[&VaultFileItem],
        include_hardlinks: bool,
        client: &dyn BreakawayHelperClient,
    ) -> Result<bool> {
        if !cfg!(feature = "store") || !client.is_packaged() {
            return Ok(false);
        }
        // When restoring to an explicit alternate directory (e.g. `Documents\GreenC Restored`),
        // only the alternate root itself determines whether writes land inside MSIX-virtualized
        // AppData — recreated relative subdirectories like `...\AppData\Local\...` underneath
        // `Documents\GreenC Restored` are NOT virtualized.
        if let Some(ref restore_dir) = self.options.restore_dir {
            return Ok(is_windows_appdata_app_dir(restore_dir));
        }
        for item in items {
            let dest = self.map_target_path(&item.original_path)?;
            if is_windows_appdata_app_dir(&dest) {
                return Ok(true);
            }
            if include_hardlinks && self.options.restore_hardlinks && item.hardlink_paths.len() > 1
            {
                for sibling in &item.hardlink_paths {
                    if sibling != &item.original_path {
                        if let Ok(mapped) = self.map_target_path(sibling) {
                            if is_windows_appdata_app_dir(&mapped) {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    /// Extracts archive entries into a private non-AppData job directory, launches
    /// `greenc-cli __restore-helper` outside the MSIX package, and strictly verifies that every
    /// restored file exists on the real filesystem with a matching SHA-256 digest before returning.
    fn restore_items_via_breakaway(
        &self,
        zip_reader: &mut ZipArchive<File>,
        items: &[&VaultFileItem],
        client: &dyn BreakawayHelperClient,
        include_hardlinks: bool,
    ) -> Result<(Vec<PathBuf>, u64)> {
        if items.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let job_dir = PrivateBreakawayJobDir::create()?;
        let mut jobs: Vec<RestoreFileJob> = Vec::with_capacity(items.len());
        let mut total_bytes = 0u64;

        for (idx, item) in items.iter().enumerate() {
            let dest_path = self.map_target_path(&item.original_path)?;
            validate_restore_file_path(&dest_path, "vault_restore")?;

            let staged_source = job_dir.payload_dir().join(format!("item_{idx}.bin"));
            let mut entry = zip_reader.by_name(&item.rel_arcname).map_err(|e| {
                GreenCError::Vault(format!("ZIP entry {} not found: {}", item.rel_arcname, e))
            })?;

            let mut out = File::create(&staged_source)?;
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 64 * 1024];
            let mut written = 0u64;
            loop {
                let n = entry.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
                written = written.saturating_add(n as u64);
            }
            out.flush()?;
            out.sync_all()?;
            drop(out);

            let extracted_sha256 = format!("{:02x}", hasher.finalize());
            if self.options.verify_sha256 && extracted_sha256 != item.sha256 {
                return Err(GreenCError::IntegrityMismatch {
                    path: dest_path,
                    expected: item.sha256.clone(),
                    actual: extracted_sha256,
                });
            }

            let mut siblings = Vec::new();
            if include_hardlinks && self.options.restore_hardlinks && item.hardlink_paths.len() > 1
            {
                for sibling in &item.hardlink_paths {
                    if sibling != &item.original_path {
                        match self.map_target_path(sibling) {
                            Ok(mapped_sib) if mapped_sib != dest_path => siblings.push(mapped_sib),
                            Ok(_) => {}
                            Err(e) => warn!(
                                "Skipping unsafe hardlink sibling {}: {e}",
                                sibling.display()
                            ),
                        }
                    }
                }
            }

            total_bytes = total_bytes.saturating_add(item.size_bytes.max(written));
            jobs.push(RestoreFileJob {
                original_path: item.original_path.clone(),
                staged_source,
                dest_path,
                expected_sha256: extracted_sha256,
                expected_size: written,
                atime: item.atime,
                mtime: item.mtime,
                hardlink_siblings: siblings,
            });
        }

        let req = RestoreHelperRequest::RestoreFiles {
            payload_root: job_dir.payload_dir().to_path_buf(),
            conflict_strategy: self.options.conflict_strategy.into(),
            verify_sha256: self.options.verify_sha256,
            restore_hardlinks: include_hardlinks && self.options.restore_hardlinks,
            restore_timestamps: self.options.restore_timestamps,
            items: jobs.clone(),
        };
        let req_bytes = serde_json::to_vec(&req)?;

        let exec_out = match client.execute(&job_dir, &req_bytes) {
            Ok(out) => out,
            Err(err) => {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "vault_restore_store_breakaway".to_string(),
                    reason: format_breakaway_unavailable_guidance(
                        "khôi phục Vault",
                        &err.to_string(),
                    ),
                });
            }
        };

        if exec_out.exit_code == EXIT_HELPER_STILL_IN_PACKAGE {
            let detail = serde_json::from_str::<RestoreHelperResponse>(&exec_out.stdout)
                .ok()
                .and_then(|r| r.error)
                .unwrap_or_else(|| {
                    "Tiến trình trợ giúp vẫn còn danh tính gói MSIX sau khi yêu cầu thoát gói"
                        .to_string()
                });
            return Err(GreenCError::PreconditionBlocked {
                operation: "vault_restore_store_breakaway".to_string(),
                reason: format_breakaway_unavailable_guidance("khôi phục Vault", &detail),
            });
        }

        let resp: RestoreHelperResponse = serde_json::from_str(&exec_out.stdout).map_err(|e| {
            GreenCError::Vault(format!(
                "Tiến trình trợ giúp khôi phục ngoài gói trả kết quả không hợp lệ (exit={}): {e}; stderr={}",
                exec_out.exit_code,
                exec_out.stderr.trim()
            ))
        })?;

        if !resp.running_outside_package || resp.error_code.as_deref() == Some("still_in_package") {
            let detail = resp.error.unwrap_or_else(|| {
                "Tiến trình trợ giúp không xác nhận được trạng thái ngoài gói MSIX".to_string()
            });
            return Err(GreenCError::PreconditionBlocked {
                operation: "vault_restore_store_breakaway".to_string(),
                reason: format_breakaway_unavailable_guidance("khôi phục Vault", &detail),
            });
        }

        if exec_out.exit_code != 0 || !resp.ok {
            let detail = resp.error.unwrap_or_else(|| {
                if exec_out.stderr.trim().is_empty() {
                    format!("exit code {}", exec_out.exit_code)
                } else {
                    exec_out.stderr.trim().to_string()
                }
            });
            return Err(GreenCError::Vault(format!(
                "Khôi phục Vault ngoài gói MSIX thất bại (gói Vault được giữ nguyên): {detail}"
            )));
        }

        if resp.files.len() != jobs.len() {
            return Err(GreenCError::Vault(format!(
                "Xác minh khôi phục ngoài gói không đủ số tệp: kỳ vọng {}, nhận {} (gói Vault được giữ nguyên)",
                jobs.len(),
                resp.files.len()
            )));
        }

        let mut restored_paths = Vec::with_capacity(resp.files.len());
        for (job, outcome) in jobs.iter().zip(resp.files.iter()) {
            if !outcome.verified_exists {
                return Err(GreenCError::Vault(format!(
                    "Xác minh ngoài gói thất bại: tệp thật '{}' không tồn tại sau khôi phục (gói Vault được giữ nguyên)",
                    outcome.restored_path.display()
                )));
            }
            if self.options.verify_sha256
                && !outcome
                    .verified_sha256
                    .eq_ignore_ascii_case(&job.expected_sha256)
            {
                return Err(GreenCError::IntegrityMismatch {
                    path: outcome.restored_path.clone(),
                    expected: job.expected_sha256.clone(),
                    actual: outcome.verified_sha256.clone(),
                });
            }
            restored_paths.push(outcome.restored_path.clone());
            for sib in &outcome.restored_siblings {
                restored_paths.push(sib.clone());
            }
        }

        Ok((restored_paths, total_bytes))
    }

    /// Converts float Unix seconds to SystemTime.
    fn sec_to_system_time(sec: f64) -> SystemTime {
        if sec <= 0.0 {
            UNIX_EPOCH
        } else {
            UNIX_EPOCH + Duration::from_secs_f64(sec)
        }
    }

    /// Full archive rollback (manual or automatic).
    pub fn rollback_archive(
        &self,
        archive_id: i64,
        reason: &str,
        auto_triggered: bool,
    ) -> Result<RestoreResult> {
        let archive =
            self.db
                .get_vault_archive(archive_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "VaultArchive".to_string(),
                    id: archive_id.to_string(),
                })?;

        let vault_dir = self.effective_vault_dir()?;
        let verified_zip = validate_vault_artifact_path(&archive.zip_path, &vault_dir, ".zip")?;

        let files = self.db.get_vault_files(archive_id)?;
        let mut seen_originals = HashSet::new();
        let mut eligible_items: Vec<&VaultFileItem> = Vec::new();
        for item in &files {
            // Guard against legacy archives containing duplicate hardlink entries
            if !seen_originals.insert(item.original_path.clone()) {
                continue;
            }
            // Already brought back by a selective restore: restoring again would only duplicate it.
            if item.restored_individually {
                continue;
            }
            eligible_items.push(item);
        }

        let zip_file = File::open(&verified_zip)?;
        let mut zip_reader = ZipArchive::new(zip_file)
            .map_err(|e| GreenCError::Vault(format!("Failed to open ZIP archive: {}", e)))?;

        let client = self.breakaway_client();
        let (restored_paths, restored_bytes) =
            if self.should_use_store_breakaway(&eligible_items, true, client.as_ref())? {
                self.restore_items_via_breakaway(
                    &mut zip_reader,
                    &eligible_items,
                    client.as_ref(),
                    true,
                )?
            } else {
                let mut restored_paths = Vec::new();
                let mut restored_bytes = 0u64;
                let mut restored_paths_set = HashSet::new();

                for item in &eligible_items {
                    let target_base = self.map_target_path(&item.original_path)?;
                    let primary_dest = self.resolve_restore_target(&target_base)?;
                    if let Some(parent) = primary_dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    // Extract entry from zip
                    let mut entry = zip_reader.by_name(&item.rel_arcname).map_err(|e| {
                        GreenCError::Vault(format!(
                            "ZIP entry {} not found: {}",
                            item.rel_arcname, e
                        ))
                    })?;

                    let mut out = File::create(&primary_dest)?;
                    let mut hasher = Sha256::new();
                    let mut buf = [0u8; 64 * 1024];

                    loop {
                        let n = entry.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                        out.write_all(&buf[..n])?;
                    }
                    out.flush()?;
                    out.sync_all()?;

                    // Verify SHA-256
                    let extracted_sha256 = format!("{:02x}", hasher.finalize());
                    if self.options.verify_sha256 && extracted_sha256 != item.sha256 {
                        drop(out);
                        let _ = std::fs::remove_file(&primary_dest);
                        return Err(GreenCError::IntegrityMismatch {
                            path: primary_dest,
                            expected: item.sha256.clone(),
                            actual: extracted_sha256,
                        });
                    }

                    // Restore original timestamps
                    if self.options.restore_timestamps {
                        let times = FileTimes::new()
                            .set_accessed(Self::sec_to_system_time(item.atime))
                            .set_modified(Self::sec_to_system_time(item.mtime));
                        let _ = out.set_times(times);
                    }
                    drop(out);

                    if !primary_dest.is_file() {
                        return Err(GreenCError::Vault(format!(
                            "Tệp khôi phục '{}' không tồn tại sau khi ghi",
                            primary_dest.display()
                        )));
                    }

                    restored_paths_set.insert(primary_dest.clone());
                    restored_paths.push(primary_dest.clone());

                    // Reconstruct HardLink siblings
                    if self.options.restore_hardlinks && item.hardlink_paths.len() > 1 {
                        for sibling in &item.hardlink_paths {
                            let mapped_sibling = match self.map_target_path(sibling) {
                                Ok(p) => p,
                                Err(e) => {
                                    warn!(
                                        "Skipping unsafe hardlink sibling {}: {e}",
                                        sibling.display()
                                    );
                                    continue;
                                }
                            };
                            if sibling != &item.original_path && mapped_sibling != primary_dest {
                                if let Err(e) = validate_restore_file_path(
                                    &mapped_sibling,
                                    "vault_restore_sibling",
                                ) {
                                    warn!(
                                        "Skipping unsafe hardlink sibling {}: {e}",
                                        mapped_sibling.display()
                                    );
                                    continue;
                                }
                                let target_sibling =
                                    if let Ok(meta) = std::fs::symlink_metadata(&mapped_sibling) {
                                        if meta.file_type().is_symlink() || !meta.is_file() {
                                            continue;
                                        }
                                        let is_empty = meta.len() == 0;

                                        if is_empty
                                            || self.options.conflict_strategy
                                                == RestoreConflictStrategy::Overwrite
                                        {
                                            let _ = std::fs::remove_file(&mapped_sibling);
                                            mapped_sibling.clone()
                                        } else if self.options.conflict_strategy
                                            == RestoreConflictStrategy::RenameWithTimestamp
                                            || self.options.conflict_strategy
                                                == RestoreConflictStrategy::OverwriteIfZeroLength
                                        {
                                            self.resolve_restore_target(&mapped_sibling)?
                                        } else {
                                            mapped_sibling.clone()
                                        }
                                    } else {
                                        mapped_sibling.clone()
                                    };

                                if let Some(parent) = target_sibling.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }

                                let link_ok =
                                    std::fs::hard_link(&primary_dest, &target_sibling).is_ok();
                                if !link_ok && !target_sibling.exists() {
                                    // Cross-volume fallback
                                    let _ = std::fs::copy(&primary_dest, &target_sibling);
                                }

                                restored_paths_set.insert(target_sibling.clone());
                                restored_paths_set.insert(mapped_sibling);
                                restored_paths.push(target_sibling);
                            }
                        }
                    }

                    restored_bytes += item.size_bytes;
                }

                (restored_paths, restored_bytes)
            };

        // Add permanently to protected files list
        if self.options.add_to_protected_list && archive.rule_id > 0 {
            let str_paths: Vec<String> = restored_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let _ = self.db.add_protected_files(archive.rule_id, &str_paths);
        }

        // Clean up ZIP and sidecar manifest strictly inside vault_dir
        let _ = safe_remove_vault_artifact(&archive.zip_path, &vault_dir, ".zip");
        let _ = safe_remove_vault_artifact(&archive.manifest_path, &vault_dir, ".manifest.json");

        // Update database state
        let status_reason = if auto_triggered {
            format!("[AUTO-ROLLBACK] {}", reason)
        } else {
            format!("[MANUAL ROLLBACK] {}", reason)
        };
        self.db.update_vault_status(
            archive_id,
            VaultStatus::RolledBack.as_str(),
            &status_reason,
        )?;

        Ok(RestoreResult {
            archive_id,
            restored_count: restored_paths.len(),
            restored_bytes,
            restored_paths,
            auto_triggered,
            reason: reason.to_string(),
        })
    }

    /// Granular single-file selective restore.
    pub fn restore_files_selective(
        &self,
        archive_id: i64,
        file_ids: &[i64],
    ) -> Result<RestoreResult> {
        let archive =
            self.db
                .get_vault_archive(archive_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "VaultArchive".to_string(),
                    id: archive_id.to_string(),
                })?;

        let vault_dir = self.effective_vault_dir()?;
        let verified_zip = validate_vault_artifact_path(&archive.zip_path, &vault_dir, ".zip")?;

        let all_files = self.db.get_vault_files(archive_id)?;
        let target_files: Vec<&VaultFileItem> = all_files
            .iter()
            .filter(|f| f.id.map(|id| file_ids.contains(&id)).unwrap_or(false))
            .collect();

        if target_files.is_empty() {
            return Err(GreenCError::Vault(
                "No matching file IDs found in archive".to_string(),
            ));
        }

        let zip_file = File::open(&verified_zip)?;
        let mut zip_reader = ZipArchive::new(zip_file)
            .map_err(|e| GreenCError::Vault(format!("Failed to open ZIP archive: {}", e)))?;

        let client = self.breakaway_client();
        let (restored_paths, restored_bytes) =
            if self.should_use_store_breakaway(&target_files, false, client.as_ref())? {
                let (paths, bytes) = self.restore_items_via_breakaway(
                    &mut zip_reader,
                    &target_files,
                    client.as_ref(),
                    false,
                )?;
                for item in &target_files {
                    if let Some(fid) = item.id {
                        let _ = self.db.mark_file_restored(fid);
                    }
                }
                (paths, bytes)
            } else {
                let mut restored_paths = Vec::new();
                let mut restored_bytes = 0u64;

                for item in target_files {
                    let target_base = self.map_target_path(&item.original_path)?;
                    let primary_dest = self.resolve_restore_target(&target_base)?;
                    if let Some(parent) = primary_dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    let mut entry = zip_reader.by_name(&item.rel_arcname).map_err(|e| {
                        GreenCError::Vault(format!(
                            "ZIP entry {} not found: {}",
                            item.rel_arcname, e
                        ))
                    })?;

                    let mut out = File::create(&primary_dest)?;
                    let mut hasher = Sha256::new();
                    let mut buf = [0u8; 64 * 1024];

                    loop {
                        let n = entry.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                        out.write_all(&buf[..n])?;
                    }
                    out.flush()?;
                    out.sync_all()?;

                    let extracted_sha256 = format!("{:02x}", hasher.finalize());
                    if self.options.verify_sha256 && extracted_sha256 != item.sha256 {
                        drop(out);
                        let _ = std::fs::remove_file(&primary_dest);
                        return Err(GreenCError::IntegrityMismatch {
                            path: primary_dest,
                            expected: item.sha256.clone(),
                            actual: extracted_sha256,
                        });
                    }

                    if self.options.restore_timestamps {
                        let times = FileTimes::new()
                            .set_accessed(Self::sec_to_system_time(item.atime))
                            .set_modified(Self::sec_to_system_time(item.mtime));
                        let _ = out.set_times(times);
                    }
                    drop(out);

                    if !primary_dest.is_file() {
                        return Err(GreenCError::Vault(format!(
                            "Tệp khôi phục '{}' không tồn tại sau khi ghi",
                            primary_dest.display()
                        )));
                    }

                    if let Some(fid) = item.id {
                        let _ = self.db.mark_file_restored(fid);
                    }

                    restored_paths.push(primary_dest);
                    restored_bytes += item.size_bytes;
                }

                (restored_paths, restored_bytes)
            };

        Ok(RestoreResult {
            archive_id,
            restored_count: restored_paths.len(),
            restored_bytes,
            restored_paths,
            auto_triggered: false,
            reason: "Selective manual file restore".to_string(),
        })
    }

    /// Resolves destination path according to conflict strategy.
    fn resolve_restore_target(&self, original_path: &Path) -> Result<PathBuf> {
        resolve_restore_target_with_strategy(original_path, self.options.conflict_strategy)
    }

    /// Periodic Safety Watchdog evaluation:
    /// 1. Checks service status of quarantined archives.
    /// 2. Scans application logs for missing-file error keywords.
    /// 3. Purges expired archives past their retention period.
    pub fn run_watchdog_cycle(
        &self,
        service_probe: &dyn ProcessServiceProbe,
    ) -> Result<Vec<RestoreResult>> {
        let vault_dir = self.effective_vault_dir()?;
        let active_archives = self
            .db
            .list_vault_archives(Some(VaultStatus::Quarantined.as_str()))?;
        let now = Utc::now();
        let mut results = Vec::new();

        for arch in active_archives {
            let arch_id = match arch.id {
                Some(id) => id,
                None => continue,
            };

            // 1. Check expiration for auto-purge (strictly bound to `vault_dir`)
            if arch.expires_at <= now {
                info!(
                    "Quarantine retention period expired for archive ID {}. Purging.",
                    arch_id
                );
                let zip_res = safe_remove_vault_artifact(&arch.zip_path, &vault_dir, ".zip");
                let man_res =
                    safe_remove_vault_artifact(&arch.manifest_path, &vault_dir, ".manifest.json");
                if let Err(ref e) = zip_res {
                    warn!(
                        "Refused to purge expired archive {} zip_path '{}': {e}",
                        arch_id,
                        arch.zip_path.display()
                    );
                }
                if let Err(ref e) = man_res {
                    warn!(
                        "Refused to purge expired archive {} manifest_path '{}': {e}",
                        arch_id,
                        arch.manifest_path.display()
                    );
                }
                if zip_res.is_ok() && man_res.is_ok() {
                    let _ = self.db.update_vault_status(
                        arch_id,
                        VaultStatus::Purged.as_str(),
                        "Auto-purged after quarantine retention window expired",
                    );
                }
                continue;
            }

            // 2. Service crash check
            // If the archive's quota rule was associated with a Windows service and service stopped
            // We can inspect manifest or storage (only if manifest lies strictly inside `vault_dir`)
            let manifest =
                validate_vault_artifact_path(&arch.manifest_path, &vault_dir, ".manifest.json")
                    .ok()
                    .and_then(|p| VaultManifest::read_from_file(&p).ok());
            if let Some(ref m) = manifest {
                if !m.service_name.is_empty() {
                    if let Ok(running) = service_probe.is_service_running(&m.service_name) {
                        if !running {
                            warn!("Service {} linked to archive {} stopped. Triggering auto-rollback.", m.service_name, arch_id);
                            if let Ok(res) = self.rollback_archive(
                                arch_id,
                                &format!("Windows service {} stopped unexpectedly", m.service_name),
                                true,
                            ) {
                                results.push(res);
                                continue;
                            }
                        }
                    }
                }
            }

            // 3. Application error log check
            if let Some(ref m) = manifest {
                if let Some(log_error) = self.scan_target_logs_for_errors(m) {
                    warn!("Missing file error detected in application log: {}. Triggering auto-rollback.", log_error);
                    if let Ok(res) = self.rollback_archive(
                        arch_id,
                        &format!("Missing file error in log: {}", log_error),
                        true,
                    ) {
                        results.push(res);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Scans recent log files in the target root for missing file error keywords.
    fn scan_target_logs_for_errors(&self, manifest: &VaultManifest) -> Option<String> {
        let root = &manifest.original_root;
        if !root.exists() || !root.is_dir() {
            return None;
        }

        let error_keywords = [
            "filenotfounderror",
            "enoent",
            "cannot find the file",
            "cannot open",
            "missing file",
            "failed to load",
            "no such file",
        ];

        let read_dir = std::fs::read_dir(root).ok()?;
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            let is_log = ext == "log" || ext == "txt" || ext == "err" || ext == "calf";
            if !is_log {
                continue;
            }

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Size cap <= 10 MiB to prevent memory exhaustion
            if meta.len() > 10 * 1024 * 1024 {
                continue;
            }

            // Filter recent logs modified after archive creation
            let mtime_sec = match meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            {
                Some(d) => d.as_secs_f64(),
                None => continue,
            };

            let arch_sec = manifest.created_at.timestamp() as f64 - 5.0;
            if mtime_sec < arch_sec {
                continue;
            }

            // Read log file content
            if let Ok(mut f) = File::open(&path) {
                let mut content = String::new();
                if f.read_to_string(&mut content).is_ok() {
                    let lower = content.to_lowercase();
                    for &kw in &error_keywords {
                        if lower.contains(kw) {
                            // Verify if any archived file name is referenced in the log
                            for archived_file in &manifest.files {
                                if let Some(fname) = archived_file
                                    .original_path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                {
                                    if lower.contains(&fname.to_lowercase()) {
                                        return Some(format!(
                                            "Found '{}' referencing '{}' in {}",
                                            kw,
                                            fname,
                                            path.display()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }
}

/// Resolves destination path according to `conflict_strategy`, validating the target path first
/// and refusing to overwrite symlinks.
fn resolve_restore_target_with_strategy(
    original_path: &Path,
    conflict_strategy: RestoreConflictStrategy,
) -> Result<PathBuf> {
    validate_restore_file_path(original_path, "vault_restore")?;

    let Ok(meta) = std::fs::symlink_metadata(original_path) else {
        return Ok(original_path.to_path_buf());
    };
    if meta.file_type().is_symlink() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "vault_restore".to_string(),
            reason: format!(
                "Từ chối khôi phục ghi đè symlink '{}'",
                original_path.display()
            ),
        });
    }
    if meta.len() == 0 && conflict_strategy == RestoreConflictStrategy::OverwriteIfZeroLength {
        return Ok(original_path.to_path_buf());
    }

    if conflict_strategy == RestoreConflictStrategy::Overwrite {
        return Ok(original_path.to_path_buf());
    }

    // Non-destructive rename: file.dat.restored_YYYYMMDD_HHMMSS
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let file_name = original_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    // Seconds resolution collides when restores run back to back: never reuse an existing name,
    // otherwise File::create would truncate an earlier restored copy.
    let mut candidate = original_path.with_file_name(format!("{file_name}.restored_{timestamp}"));
    let mut n = 2u32;
    while std::fs::symlink_metadata(&candidate).is_ok() {
        candidate = original_path.with_file_name(format!("{file_name}.restored_{timestamp}_{n}"));
        n += 1;
    }
    Ok(candidate)
}

fn validate_staged_payload_file(
    staged_source: &Path,
    canon_payload_root: &Path,
) -> Result<PathBuf> {
    if !staged_source.is_absolute() || path_has_parent_dir(staged_source) {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restore_helper_payload".to_string(),
            reason: format!(
                "Tệp nguồn trung gian '{}' không tuyệt đối hoặc chứa '..'",
                staged_source.display()
            ),
        });
    }
    let meta = std::fs::symlink_metadata(staged_source)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restore_helper_payload".to_string(),
            reason: format!(
                "Tệp nguồn trung gian '{}' là symlink hoặc không phải tệp thường",
                staged_source.display()
            ),
        });
    }
    if !is_owned_by_current_user(staged_source)? {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restore_helper_payload".to_string(),
            reason: format!(
                "Tệp nguồn trung gian '{}' không thuộc người dùng hiện tại",
                staged_source.display()
            ),
        });
    }
    let canon = staged_source.canonicalize()?;
    if !canon.starts_with(canon_payload_root) || canon == canon_payload_root {
        return Err(GreenCError::PreconditionBlocked {
            operation: "restore_helper_payload".to_string(),
            reason: format!(
                "Tệp nguồn trung gian '{}' nằm ngoài thư mục payload '{}'",
                staged_source.display(),
                canon_payload_root.display()
            ),
        });
    }
    Ok(canon)
}

/// Executes a `RestoreFiles` job inside the unpackaged `greenc-cli __restore-helper` child process.
///
/// Security & correctness invariants:
/// 1. Refuses immediately with [`EXIT_HELPER_STILL_IN_PACKAGE`] if `running_in_package` is `true`.
/// 2. Re-validates `payload_root` (real user-owned directory outside AppData) and every
///    `staged_source` (regular user-owned file strictly inside `payload_root`).
/// 3. Re-validates every destination and hardlink sibling with [`validate_restore_file_path`].
/// 4. Writes, flushes, calls `sync_all`, closes, and then **re-opens and re-hashes** every written
///    destination file from the real disk to confirm existence and exact SHA-256 match.
pub fn execute_restore_files_job(
    payload_root: &Path,
    conflict_strategy: HelperConflictStrategy,
    verify_sha256: bool,
    restore_hardlinks: bool,
    restore_timestamps: bool,
    items: &[RestoreFileJob],
    running_in_package: bool,
) -> (i32, RestoreHelperResponse) {
    if running_in_package {
        return (
            EXIT_HELPER_STILL_IN_PACKAGE,
            RestoreHelperResponse::still_in_package(current_package_full_name().as_deref()),
        );
    }

    if !payload_root.is_absolute()
        || path_has_parent_dir(payload_root)
        || is_windows_appdata_app_dir(payload_root)
    {
        return (
            2,
            RestoreHelperResponse::failed(
                "invalid_payload_root",
                format!(
                    "Thư mục payload '{}' không hợp lệ hoặc nằm trong AppData",
                    payload_root.display()
                ),
            ),
        );
    }

    let pmeta = match std::fs::symlink_metadata(payload_root) {
        Ok(m) => m,
        Err(e) => {
            return (
                1,
                RestoreHelperResponse::failed(
                    "payload_root_missing",
                    format!(
                        "Không đọc được thư mục payload '{}': {e}",
                        payload_root.display()
                    ),
                ),
            );
        }
    };
    if pmeta.file_type().is_symlink() || !pmeta.is_dir() {
        return (
            2,
            RestoreHelperResponse::failed(
                "invalid_payload_root",
                format!(
                    "Thư mục payload '{}' là symlink hoặc không phải thư mục thật",
                    payload_root.display()
                ),
            ),
        );
    }
    match is_owned_by_current_user(payload_root) {
        Ok(true) => {}
        Ok(false) => {
            return (
                2,
                RestoreHelperResponse::failed(
                    "untrusted_payload_root",
                    format!(
                        "Thư mục payload '{}' không thuộc người dùng hiện tại",
                        payload_root.display()
                    ),
                ),
            );
        }
        Err(e) => {
            return (
                2,
                RestoreHelperResponse::failed(
                    "untrusted_payload_root",
                    format!(
                        "Không xác minh được chủ sở hữu thư mục payload '{}': {e}",
                        payload_root.display()
                    ),
                ),
            );
        }
    }
    let canon_payload_root = match payload_root.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return (
                1,
                RestoreHelperResponse::failed(
                    "payload_root_canonicalize",
                    format!("Không chuẩn hoá được thư mục payload: {e}"),
                ),
            );
        }
    };

    let strategy: RestoreConflictStrategy = conflict_strategy.into();
    let mut outcomes: Vec<RestoreFileOutcome> = Vec::with_capacity(items.len());
    let mut total_bytes = 0u64;

    for job in items {
        let verified_src =
            match validate_staged_payload_file(&job.staged_source, &canon_payload_root) {
                Ok(p) => p,
                Err(e) => {
                    return (
                        2,
                        RestoreHelperResponse::failed("invalid_staged_source", e.to_string()),
                    );
                }
            };

        if let Err(e) = validate_restore_file_path(&job.dest_path, "restore_helper") {
            return (
                2,
                RestoreHelperResponse::failed("invalid_dest_path", e.to_string()),
            );
        }

        if verify_sha256
            && (job.expected_sha256.len() != 64
                || !job.expected_sha256.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return (
                2,
                RestoreHelperResponse::failed(
                    "invalid_sha256",
                    format!(
                        "Mã băm SHA-256 kỳ vọng không hợp lệ cho '{}'",
                        job.dest_path.display()
                    ),
                ),
            );
        }

        let resolved_dest = match resolve_restore_target_with_strategy(&job.dest_path, strategy) {
            Ok(p) => p,
            Err(e) => {
                return (
                    2,
                    RestoreHelperResponse::failed("invalid_dest_path", e.to_string()),
                );
            }
        };

        if let Some(parent) = resolved_dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return (
                    1,
                    RestoreHelperResponse::failed(
                        "create_parent_failed",
                        format!("Không tạo được thư mục đích '{}': {e}", parent.display()),
                    ),
                );
            }
        }

        if let Ok(meta) = std::fs::symlink_metadata(&resolved_dest) {
            if meta.file_type().is_symlink() {
                return (
                    2,
                    RestoreHelperResponse::failed(
                        "symlink_dest",
                        format!("Từ chối ghi đè symlink '{}'", resolved_dest.display()),
                    ),
                );
            }
        }

        let bytes_written = match (|| -> std::io::Result<u64> {
            let mut src = File::open(&verified_src)?;
            let mut dst = File::create(&resolved_dest)?;
            let mut buf = [0u8; 64 * 1024];
            let mut written = 0u64;
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                dst.write_all(&buf[..n])?;
                written = written.saturating_add(n as u64);
            }
            dst.flush()?;
            dst.sync_all()?;
            if restore_timestamps {
                let times = FileTimes::new()
                    .set_accessed(RollbackEngine::sec_to_system_time(job.atime))
                    .set_modified(RollbackEngine::sec_to_system_time(job.mtime));
                let _ = dst.set_times(times);
            }
            drop(dst);
            Ok(written)
        })() {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&resolved_dest);
                return (
                    1,
                    RestoreHelperResponse::failed(
                        "write_failed",
                        format!("Ghi tệp '{}' thất bại: {e}", resolved_dest.display()),
                    ),
                );
            }
        };

        // Post-write verification from outside the MSIX package: confirm the real file exists,
        // is a regular file (not symlink), and its on-disk SHA-256 matches `expected_sha256`.
        let disk_meta = match std::fs::symlink_metadata(&resolved_dest) {
            Ok(m) if !m.file_type().is_symlink() && m.is_file() => m,
            Ok(_) => {
                let _ = std::fs::remove_file(&resolved_dest);
                return (
                    1,
                    RestoreHelperResponse::failed(
                        "verify_not_regular_file",
                        format!(
                            "Tệp sau khi ghi '{}' không phải tệp thường",
                            resolved_dest.display()
                        ),
                    ),
                );
            }
            Err(e) => {
                return (
                    1,
                    RestoreHelperResponse::failed(
                        "verify_missing",
                        format!(
                            "Không thấy tệp thật '{}' sau khi ghi: {e}",
                            resolved_dest.display()
                        ),
                    ),
                );
            }
        };

        let disk_sha256 = match compute_file_sha256(&resolved_dest) {
            Ok(h) => h,
            Err(e) => {
                return (
                    1,
                    RestoreHelperResponse::failed(
                        "verify_hash_read_failed",
                        format!(
                            "Không đọc lại được băm SHA-256 của '{}': {e}",
                            resolved_dest.display()
                        ),
                    ),
                );
            }
        };

        if verify_sha256 && !disk_sha256.eq_ignore_ascii_case(&job.expected_sha256) {
            let _ = std::fs::remove_file(&resolved_dest);
            return (
                1,
                RestoreHelperResponse::failed(
                    "integrity_mismatch",
                    format!(
                        "Sai băm SHA-256 sau ghi tại '{}': kỳ vọng {}, thực tế {}",
                        resolved_dest.display(),
                        job.expected_sha256,
                        disk_sha256
                    ),
                ),
            );
        }

        if disk_meta.len() != bytes_written {
            let _ = std::fs::remove_file(&resolved_dest);
            return (
                1,
                RestoreHelperResponse::failed(
                    "size_mismatch",
                    format!(
                        "Kích thước tệp thật '{}' không khớp sau khi ghi ({} != {})",
                        resolved_dest.display(),
                        disk_meta.len(),
                        bytes_written
                    ),
                ),
            );
        }

        let mut restored_siblings = Vec::new();
        if restore_hardlinks {
            for sibling in &job.hardlink_siblings {
                if sibling == &resolved_dest {
                    continue;
                }
                if validate_restore_file_path(sibling, "restore_helper_sibling").is_err() {
                    continue;
                }
                let target_sibling = if let Ok(meta) = std::fs::symlink_metadata(sibling) {
                    if meta.file_type().is_symlink() || !meta.is_file() {
                        continue;
                    }
                    if meta.len() == 0 || strategy == RestoreConflictStrategy::Overwrite {
                        let _ = std::fs::remove_file(sibling);
                        sibling.clone()
                    } else {
                        match resolve_restore_target_with_strategy(sibling, strategy) {
                            Ok(p) => p,
                            Err(_) => continue,
                        }
                    }
                } else {
                    sibling.clone()
                };

                if let Some(parent) = target_sibling.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let link_ok = std::fs::hard_link(&resolved_dest, &target_sibling).is_ok();
                if !link_ok && !target_sibling.exists() {
                    let _ = std::fs::copy(&resolved_dest, &target_sibling);
                }
                if target_sibling.is_file() {
                    restored_siblings.push(target_sibling);
                }
            }
        }

        total_bytes = total_bytes.saturating_add(bytes_written);
        outcomes.push(RestoreFileOutcome {
            original_path: job.original_path.clone(),
            restored_path: resolved_dest,
            bytes_written,
            verified_exists: true,
            verified_sha256: disk_sha256,
            restored_siblings,
        });
    }

    (
        0,
        RestoreHelperResponse {
            ok: true,
            running_outside_package: true,
            error_code: None,
            error: None,
            files: outcomes,
            verified_dir: None,
            verified_file_count: items.len() as u64,
            verified_bytes: total_bytes,
        },
    )
}
