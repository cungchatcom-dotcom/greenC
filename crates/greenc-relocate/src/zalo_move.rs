//! Transactional Zalo PC chat-data relocation engine (`<ROOT>\Zalo Data` -> target NTFS drive).
//!
//! Guarantees (GC-ZALO + ):
//! - Principle: **chat data is NEVER lost; when in doubt, preserve all copies and report clearly**.
//! - Never modifies `%APPDATA%\ZaloData\config.json`.
//! - Estimates free space on BOTH source drive (e.g. `C:`) and target drive before moving
//!   (accounting for 24-hour temporary retention of the original on `C:`).
//! - Refuses if an active call (`ZaloCall.exe` visible window) is detected.
//! - Uses graceful Restart Manager shutdown (`RmShutdown(0)`, <= 30s), NEVER force-kills.
//! - Recursively verifies no Zalo process is running and no file (including `message/` DBs) is locked
//!   after shutdown, immediately before rename, immediately after rename, and before junction creation.
//! - Fail-closed streaming copy (`verified_copy_tree_strict` with SHA-256 + `sync_all`) and
//!   fail-closed `verify_zalo_copy_integrity` (propagates all I/O/permission errors; rejects nested
//!   symlinks/reparse points; verifies exact paths, sizes, and SHA-256).
//! - Keeps `moving_path` intact throughout reopen + 20-second live junction health watch; ONLY after
//!   the watch passes does it rename `moving_path` to `Zalo Data.greenc-original.<uuid>` and persist
//!   a DB retention record (auto-purge ONLY when health watch passed AND 24h elapsed AND junction
//!   remains healthy, or when the user explicitly requests purge).
//! - Safe rollback on broken junction: never ignores `graceful_shutdown` errors, never deletes
//!   recreated directories (renames to `.greenc-conflict.<uuid>`), preserves target messages as latest,
//!   re-verifies restored copy before renaming & cleaning target, and restores `moving_path` if target
//!   went offline.

use crate::candidate::{CandidateQualificationEngine, CandidateScanner, FREE_SPACE_RESERVE_BYTES};
use crate::recovery::RecoveryService;
use crate::verified_copy::{remove_dir_all_safe, to_extended_io_path, verified_copy_tree_strict};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use greenc_core::config::IS_STORE_BUILD;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{
    RelocationHealthStatus, RelocationRecord, RelocationStatus, RelocationStrategyKind,
    ZaloRetainedOriginal, ZaloRetainedStatus,
};
use greenc_core::task::TaskControl;
use greenc_platform::app_lifecycle::{
    AppLifecycleApi, DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT, ZALO_ACTIVE_CALL_MESSAGE,
    ZALO_MANUAL_QUIT_MESSAGE,
};
use greenc_platform::traits::{
    FileLockProbe, ProcessServiceProbe, ReparsePointApi, ShellFolderApi, Win32VolumeApi,
};
use greenc_platform::RelocationLock;
use greenc_scanner::zalo::{
    extract_drive_label, is_symlink_or_reparse, ZaloDiscovery, ZALO_PROCESS_NAMES,
};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Default post-reopen junction health watch duration (20 seconds).
pub const DEFAULT_POST_REOPEN_WATCH: Duration = Duration::from_secs(20);

/// Default poll interval during the 20-second post-reopen health watch (1 second).
pub const DEFAULT_POST_REOPEN_POLL: Duration = Duration::from_secs(1);

/// Default safety retention period before an auto-verified `.greenc-original.<uuid>` may be
/// eligible for automatic cleanup (24 hours).
pub const DEFAULT_ORIGINAL_RETENTION_HOURS: i64 = 24;

/// Maximum real healthy observation time credited per single background health tick (5 minutes).
/// Prevents a sudden wall-clock jump forward from expiring a retained original immediately.
pub const MAX_OBSERVATION_STEP_SECS: i64 = 300;

/// Automatic Zalo relocation stays off until undo, crash recovery and the 24 h purge are safe while
/// Zalo is being used: chat history cannot be re-created, so these paths must never be reachable early.
const ZALO_AUTO_MOVE_RELEASED: bool = false;

/// Returns `true` only on standard (non-Store) Windows builds where NTFS junctions in user
/// directories are natively supported and not virtualized by MSIX.
pub const fn is_zalo_auto_move_supported() -> bool {
    ZALO_AUTO_MOVE_RELEASED && cfg!(windows) && !IS_STORE_BUILD
}

/// Reason shown when automatic Zalo relocation is not available on this platform/edition.
pub fn zalo_auto_move_unsupported_reason() -> String {
    if IS_STORE_BUILD {
        "Bản Microsoft Store không hỗ trợ tự động chuyển dữ liệu Zalo do cơ chế ảo hoá AppData của MSIX. Vui lòng dùng hướng dẫn thủ công trong Zalo.".to_string()
    } else if !cfg!(windows) {
        "Tự động chuyển dữ liệu Zalo PC chỉ hỗ trợ trên Windows. Vui lòng dùng hướng dẫn thủ công trong Zalo.".to_string()
    } else if !ZALO_AUTO_MOVE_RELEASED {
        "Tự động chuyển dữ liệu Zalo đang được hoàn thiện. Vui lòng dùng hướng dẫn thủ công trong Zalo.".to_string()
    } else {
        String::new()
    }
}

/// Clear user-facing phases emitted during Zalo chat-data relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZaloMovePhase {
    Preparing,
    ClosingZalo,
    Moving,
    Verifying,
    Linking,
    Reopening,
    HealthCheck,
    RollingBack,
    Done,
}

impl ZaloMovePhase {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::ClosingZalo => "closing_zalo",
            Self::Moving => "moving",
            Self::Verifying => "verifying",
            Self::Linking => "linking",
            Self::Reopening => "reopening",
            Self::HealthCheck => "health_check",
            Self::RollingBack => "rolling_back",
            Self::Done => "done",
        }
    }

    pub const fn label_vi(&self) -> &'static str {
        match self {
            Self::Preparing => "Chuẩn bị",
            Self::ClosingZalo => "Đóng Zalo",
            Self::Moving => "Đang chuyển",
            Self::Verifying => "Kiểm tra",
            Self::Linking => "Đang chuyển",
            Self::Reopening => "Mở lại",
            Self::HealthCheck => "Kiểm tra",
            Self::RollingBack => "Hoàn tác",
            Self::Done => "Xong",
        }
    }
}

/// Real-time progress snapshot emitted by [`ZaloRelocator::move_zalo_data`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloMoveProgress {
    pub phase: ZaloMovePhase,
    pub percent: u32,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub total_files: u64,
    pub detail: String,
}

/// Verification counts returned by [`verify_zalo_copy_integrity`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloCopyVerification {
    pub total_files: u64,
    pub total_bytes: u64,
    pub message_files_hashed: u64,
    pub media_files_counted: u64,
}

/// Pre-move free-space estimate covering BOTH the source drive (e.g. `C:`, which holds the
/// temporary `.greenc-original.<uuid>` copy for 24h) and the destination drive (e.g. `D:`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloSpaceEstimate {
    pub source_drive: String,
    pub target_drive: String,
    pub chat_data_bytes: u64,
    pub source_free_bytes: u64,
    pub source_required_bytes: u64,
    pub source_retained_bytes: u64,
    pub source_ok: bool,
    pub source_notice: Option<String>,
    pub target_free_bytes: u64,
    pub target_required_bytes: u64,
    pub target_ok: bool,
    pub target_notice: Option<String>,
    pub can_proceed: bool,
    pub blocking_reason: Option<String>,
}

/// Discovered Zalo safety copy (`.greenc-original.*`, `.greenc-conflict.*`, `.greenc-moving.*`)
/// merged from SQLite persistence and live filesystem scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZaloSafetyCopyInfo {
    /// Opaque deterministic identifier generated by `discover_zalo_safety_copies` (used by IPC so
    /// the frontend NEVER passes arbitrary filesystem paths to restore/purge commands).
    pub copy_id: String,
    pub id: Option<i64>,
    pub relocation_id: Option<i64>,
    /// `"original"` | `"conflict"` | `"moving"` | `"restoring"` | `"purging"`
    pub kind: String,
    pub path: PathBuf,
    pub source_path: Option<PathBuf>,
    pub target_path: Option<PathBuf>,
    pub size_bytes: u64,
    pub file_count: u64,
    pub created_at: Option<DateTime<Utc>>,
    pub purge_after: Option<DateTime<Utc>>,
    pub health_verified: bool,
    pub status: String,
    pub exists_on_disk: bool,
    /// `true` ONLY when this copy is intact on disk, has authentic Zalo `message/` structure,
    /// and is an eligible kind (`original`, `conflict`, `moving` — NEVER `purging` or `restoring`).
    pub restorable: bool,
    pub detail: Option<String>,
}

/// Hook invoked with `(target_path)` right after verified copy completes.
pub type AfterCopyHook<'a> = &'a dyn Fn(&Path);
/// Hook invoked with `(source_path, target_path)` right before source rename.
pub type BeforeRenameHook<'a> = &'a dyn Fn(&Path, &Path);
/// Hook invoked with `(source_path, moving_path, target_path)` around rename/junction transitions.
pub type StageTransitionHook<'a> = &'a dyn Fn(&Path, &Path, &Path);

/// Deterministic lifecycle hooks for fault-injection testing at every critical step.
#[derive(Default)]
pub struct ZaloMoveHooks<'a> {
    pub after_copy: Option<AfterCopyHook<'a>>,
    pub before_rename: Option<BeforeRenameHook<'a>>,
    pub after_rename: Option<StageTransitionHook<'a>>,
    pub before_junction: Option<StageTransitionHook<'a>>,
    pub after_junction: Option<StageTransitionHook<'a>>,
}

/// Options controlling [`ZaloRelocator::move_zalo_data`].
#[derive(Debug, Clone)]
pub struct ZaloMoveOptions {
    pub target_drive: char,
    pub custom_target_path: Option<PathBuf>,
    pub dry_run: bool,
    pub reopen_zalo: bool,
    pub shutdown_timeout: Duration,
    pub post_reopen_watch_duration: Duration,
    pub post_reopen_poll_interval: Duration,
    /// Hours to retain `.greenc-original.<uuid>` after health watch passes (default 24h).
    pub original_retention_hours: i64,
    /// When `true` (default in production), enforces `is_zalo_auto_move_supported()`.
    /// Tests on Linux set this to `false` when exercising the transactional engine with mocks.
    pub enforce_platform_support: bool,
    /// When `true`, skips the filesystem write probe through the junction (only for pure in-memory
    /// mock reparse APIs that do not create real filesystem symlinks).
    pub skip_write_probe: bool,
}

impl Default for ZaloMoveOptions {
    fn default() -> Self {
        Self {
            target_drive: 'D',
            custom_target_path: None,
            dry_run: false,
            reopen_zalo: true,
            shutdown_timeout: DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
            post_reopen_watch_duration: DEFAULT_POST_REOPEN_WATCH,
            post_reopen_poll_interval: DEFAULT_POST_REOPEN_POLL,
            original_retention_hours: DEFAULT_ORIGINAL_RETENTION_HOURS,
            enforce_platform_support: true,
            skip_write_probe: false,
        }
    }
}

/// Structured result of moving (or dry-running) Zalo chat data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaloMoveReport {
    pub record_id: Option<i64>,
    pub dry_run: bool,
    pub already_on_other_drive: bool,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub target_drive: String,
    pub moved_bytes: u64,
    pub moved_files: u64,
    pub message_files_verified: u64,
    pub media_files_verified: u64,
    pub reopened_zalo: bool,
    pub health_verified: bool,
    pub auto_rolled_back: bool,
    pub retained_original_id: Option<i64>,
    pub retained_original_path: Option<PathBuf>,
    pub retained_original_bytes: u64,
    pub retained_purge_after: Option<String>,
    pub space_estimate: Option<ZaloSpaceEstimate>,
    pub status: String,
    pub message: String,
}

pub fn format_bytes_vi(bytes: u64) -> String {
    let gb = (bytes as f64) / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{gb:.1} GB").replace('.', ",")
    } else {
        let mb = (bytes as f64) / (1024.0 * 1024.0);
        if mb >= 1.0 {
            format!("{mb:.0} MB")
        } else {
            format!("{bytes} B")
        }
    }
}

/// Estimates and validates free space on BOTH source drive (e.g. `C:`) and target drive
/// (e.g. `D:`) before moving Zalo data, accounting for temporary 24-hour retention of the
/// original copy on the source drive.
pub fn estimate_zalo_move_space(
    source_drive: char,
    target_drive: char,
    chat_data_bytes: u64,
    volume_api: &dyn Win32VolumeApi,
) -> Result<ZaloSpaceEstimate> {
    let src_letter = source_drive.to_ascii_uppercase();
    let tgt_letter = target_drive.to_ascii_uppercase();
    let src_label = format!("{src_letter}:");
    let tgt_label = format!("{tgt_letter}:");

    let source_free = volume_api.get_free_space(src_letter)?;
    let target_free = volume_api.get_free_space(tgt_letter)?;
    let source_required = FREE_SPACE_RESERVE_BYTES;
    let target_required = chat_data_bytes.saturating_add(FREE_SPACE_RESERVE_BYTES);

    if src_letter == tgt_letter {
        let reason = format!(
            "Ổ đĩa đích ({tgt_label}) trùng với ổ nguồn ({src_label}). Vui lòng chọn ổ đĩa khác."
        );
        return Ok(ZaloSpaceEstimate {
            source_drive: src_label,
            target_drive: tgt_label,
            chat_data_bytes,
            source_free_bytes: source_free,
            source_required_bytes: source_required,
            source_retained_bytes: chat_data_bytes,
            source_ok: false,
            source_notice: Some(reason.clone()),
            target_free_bytes: target_free,
            target_required_bytes: target_required,
            target_ok: false,
            target_notice: Some(reason.clone()),
            can_proceed: false,
            blocking_reason: Some(reason),
        });
    }

    let source_ok = source_free >= source_required;
    let target_ok = target_free >= target_required;

    let source_notice = if !source_ok {
        Some(format!(
            "Thiếu dung lượng trên ổ {src_label}: hiện chỉ còn trống {}, cần tối thiểu {} để chuyển an toàn vì bản gốc ({}) sẽ được giữ tạm tối thiểu 24 giờ hoạt động ổn định trên ổ {src_label}.",
            format_bytes_vi(source_free),
            format_bytes_vi(source_required),
            format_bytes_vi(chat_data_bytes),
        ))
    } else if source_free < chat_data_bytes.saturating_add(source_required) {
        Some(format!(
            "Lưu ý: bản gốc ({}) sẽ được giữ tạm trên ổ {src_label} tối thiểu 24 giờ hoạt động ổn định để đảm bảo an toàn nên dung lượng ổ {src_label} chưa giảm ngay (hiện còn trống {}).",
            format_bytes_vi(chat_data_bytes),
            format_bytes_vi(source_free),
        ))
    } else {
        None
    };

    let target_notice = if !target_ok {
        Some(format!(
            "Thiếu dung lượng trên ổ đích {tgt_label}: hiện còn trống {}, cần tối thiểu {} (gồm {} dữ liệu Zalo + 512 MB dự trữ).",
            format_bytes_vi(target_free),
            format_bytes_vi(target_required),
            format_bytes_vi(chat_data_bytes),
        ))
    } else {
        None
    };

    let blocking_reason = if !source_ok && !target_ok {
        Some(format!(
            "{} {}",
            source_notice.as_deref().unwrap_or_default(),
            target_notice.as_deref().unwrap_or_default()
        ))
    } else if !source_ok {
        source_notice.clone()
    } else if !target_ok {
        target_notice.clone()
    } else {
        None
    };

    Ok(ZaloSpaceEstimate {
        source_drive: src_label,
        target_drive: tgt_label,
        chat_data_bytes,
        source_free_bytes: source_free,
        source_required_bytes: source_required,
        source_retained_bytes: chat_data_bytes,
        source_ok,
        source_notice,
        target_free_bytes: target_free,
        target_required_bytes: target_required,
        target_ok,
        target_notice,
        can_proceed: source_ok && target_ok,
        blocking_reason,
    })
}

fn classify_safety_copy_name(name: &str) -> Option<&'static str> {
    if name.contains(".greenc-purging.") {
        Some("purging")
    } else if name.contains(".greenc-original.") {
        Some("original")
    } else if name.contains(".greenc-conflict.") {
        Some("conflict")
    } else if name.contains(".greenc-moving.") {
        Some("moving")
    } else if name.starts_with("zalo-restoring.") || name.starts_with("restoring.") {
        Some("restoring")
    } else {
        None
    }
}

fn is_purging_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains(".greenc-purging."))
}

fn norm_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn make_safety_copy_id(db_id: Option<i64>, path: &Path) -> String {
    if let Some(id) = db_id {
        return format!("db:{id}");
    }
    let mut hasher = Sha256::new();
    hasher.update(norm_path_key(path).as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("fs:{}", &digest[..16])
}

fn is_safety_copy_restorable(kind: &str, status: &str, path: &Path, exists: bool) -> bool {
    if !exists {
        return false;
    }
    if !matches!(kind, "original" | "conflict" | "moving") {
        return false;
    }
    if matches!(status, "purging" | "purged") || is_purging_path(path) {
        return false;
    }
    greenc_scanner::zalo::has_valid_zalo_data_structure(path)
}

/// Discovers all Zalo safety copies (`.greenc-original.*`, `.greenc-conflict.*`,
/// `.greenc-moving.*`) by combining persistent SQLite records and live filesystem scans.
pub fn discover_zalo_safety_copies(
    data_dir: Option<&Path>,
    db: &GreenCDb,
) -> Vec<ZaloSafetyCopyInfo> {
    let mut results: Vec<ZaloSafetyCopyInfo> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut scan_parents: Vec<PathBuf> = Vec::new();

    if let Some(dd) = data_dir {
        if let Some(p) = dd.parent() {
            scan_parents.push(p.to_path_buf());
        }
    }

    // 1. Load persistent DB records first
    if let Ok(db_rows) = db.list_zalo_retained_originals(None) {
        for row in db_rows {
            if let Some(p) = row.source_path.parent() {
                scan_parents.push(p.to_path_buf());
            }
            if let Some(p) = row.target_path.parent() {
                scan_parents.push(p.to_path_buf());
            }
            if let Some(p) = row.retained_path.parent() {
                scan_parents.push(p.to_path_buf());
            }

            let exists = fs::symlink_metadata(to_extended_io_path(&row.retained_path)).is_ok();
            // Skip already-resolved records whose directory no longer exists on disk
            if !exists
                && matches!(
                    row.status,
                    ZaloRetainedStatus::Purged | ZaloRetainedStatus::Restored
                )
            {
                continue;
            }
            let (live_size, live_files) = if exists {
                CandidateScanner::compute_dir_size(&row.retained_path)
            } else {
                (row.size_bytes, row.file_count)
            };
            let kind = if row.status == ZaloRetainedStatus::Purging
                || is_purging_path(&row.retained_path)
            {
                "purging".to_string()
            } else {
                row.retained_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(classify_safety_copy_name)
                    .unwrap_or("original")
                    .to_string()
            };

            seen_keys.insert(norm_path_key(&row.retained_path));
            let copy_id = make_safety_copy_id(row.id, &row.retained_path);
            let status_str = if !exists && row.status == ZaloRetainedStatus::Retained {
                ZaloRetainedStatus::Missing.as_str().to_string()
            } else {
                row.status.as_str().to_string()
            };
            let restorable =
                is_safety_copy_restorable(&kind, &status_str, &row.retained_path, exists);
            results.push(ZaloSafetyCopyInfo {
                copy_id,
                id: row.id,
                relocation_id: Some(row.relocation_id),
                kind,
                path: row.retained_path,
                source_path: Some(row.source_path),
                target_path: Some(row.target_path),
                size_bytes: if exists { live_size } else { row.size_bytes },
                file_count: if exists { live_files } else { row.file_count },
                created_at: Some(row.created_at),
                purge_after: Some(row.purge_after),
                health_verified: row.health_verified,
                status: status_str,
                exists_on_disk: exists,
                restorable,
                detail: row.detail,
            });
        }
    }

    // 2. Collect parent directories, known source paths, and any NeedsReview / offline target
    //    directories from Zalo relocation records so an offline or unrenamed target is ALWAYS
    //    listed to the user (Item 5).
    let mut fallback_source_path: Option<PathBuf> = data_dir.map(Path::to_path_buf);
    if let Ok(relocs) = db.list_relocations(None) {
        for rec in relocs {
            if rec.app_id.eq_ignore_ascii_case("zalo") {
                if fallback_source_path.is_none() {
                    fallback_source_path = Some(rec.source_path.clone());
                }
                if let Some(p) = rec.source_path.parent() {
                    scan_parents.push(p.to_path_buf());
                }
                if let Some(p) = rec.target_path.parent() {
                    scan_parents.push(p.to_path_buf());
                }
                let source_replaced_by_real_dir = rec.status == RelocationStatus::Done
                    && !is_symlink_or_reparse(&rec.source_path)
                    && fs::symlink_metadata(to_extended_io_path(&rec.source_path))
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
                if rec.status == RelocationStatus::NeedsReview
                    || rec.health_status == RelocationHealthStatus::TargetOffline
                    || source_replaced_by_real_dir
                {
                    let same_as_live_source = data_dir.is_some_and(|d| {
                        greenc_core::models::same_windows_path(d, &rec.target_path)
                    });
                    let tkey = norm_path_key(&rec.target_path);
                    if !same_as_live_source && seen_keys.insert(tkey) {
                        let exists =
                            fs::symlink_metadata(to_extended_io_path(&rec.target_path)).is_ok();
                        let (live_size, live_files) = if exists {
                            CandidateScanner::compute_dir_size(&rec.target_path)
                        } else {
                            (rec.size_bytes, 0)
                        };
                        let status_str = if exists {
                            "needs_review".to_string()
                        } else {
                            "target_offline".to_string()
                        };
                        let restorable = exists
                            && greenc_scanner::zalo::has_valid_zalo_data_structure(
                                &rec.target_path,
                            );
                        let copy_id = make_safety_copy_id(None, &rec.target_path);
                        results.push(ZaloSafetyCopyInfo {
                            copy_id,
                            id: None,
                            relocation_id: rec.id,
                            kind: "target".to_string(),
                            path: rec.target_path.clone(),
                            source_path: Some(rec.source_path.clone()),
                            target_path: Some(rec.target_path.clone()),
                            size_bytes: live_size,
                            file_count: live_files,
                            created_at: Some(rec.created_at),
                            purge_after: None,
                            health_verified: false,
                            status: status_str,
                            exists_on_disk: exists,
                            restorable,
                            detail: Some(if exists {
                                "Bản dữ liệu Zalo trên ổ đích cần kiểm tra (có thể chứa tin nhắn mới hơn)."
                                    .to_string()
                            } else {
                                format!(
                                    "Ổ đích ({}) đang ngoại tuyến và có thể chứa tin nhắn mới hơn — hãy kết nối lại ổ để kiểm tra.",
                                    rec.target_path.display()
                                )
                            }),
                        });
                    }
                }
            }
        }
    }

    // 3. Scan unique parent directories on disk for untracked `.greenc-original/.conflict/.moving`
    let mut visited_parents: HashSet<String> = HashSet::new();
    for parent in scan_parents {
        let pkey = norm_path_key(&parent);
        if !visited_parents.insert(pkey) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(kind) = classify_safety_copy_name(&name) else {
                continue;
            };
            let path = entry.path();
            let key = norm_path_key(&path);
            if !seen_keys.insert(key) {
                continue;
            }
            let (size_bytes, file_count) = CandidateScanner::compute_dir_size(&path);
            let created_at = fs::symlink_metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from);
            let status_str = if kind == "original" {
                "retained".to_string()
            } else if kind == "purging" {
                "purging".to_string()
            } else {
                "needs_review".to_string()
            };
            let copy_id = make_safety_copy_id(None, &path);
            let restorable = is_safety_copy_restorable(kind, &status_str, &path, true);
            results.push(ZaloSafetyCopyInfo {
                copy_id,
                id: None,
                relocation_id: None,
                kind: kind.to_string(),
                path,
                source_path: fallback_source_path.clone(),
                target_path: None,
                size_bytes,
                file_count,
                created_at,
                purge_after: None,
                health_verified: false,
                status: status_str,
                exists_on_disk: true,
                restorable,
                detail: Some(match kind {
                    "purging" => {
                        "Thư mục bản gốc đang bị xoá dở (.greenc-purging) — không thể khôi phục"
                            .to_string()
                    }
                    "conflict" => {
                        "Thư mục xung đột được giữ nguyên để tránh mất tin nhắn mới".to_string()
                    }
                    "moving" => "Thư mục đang chuyển dở từ lần trước — cần kiểm tra".to_string(),
                    "restoring" => {
                        "Thư mục đang khôi phục/hoàn tác dở từ lần trước — không tự khôi phục vì có thể chưa đủ tệp"
                            .to_string()
                    }
                    _ => "Bản gốc giữ tạm tìm thấy trên đĩa".to_string(),
                }),
            });
        }
    }

    results.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    results
}

fn sha256_file(path: &Path) -> Result<String> {
    let io_path = to_extended_io_path(path);
    let mut f = fs::File::open(&io_path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Snapshot of a directory tree's relative files, sizes, and modification times used to detect
/// any concurrent write or drift between copy/verification and the final rename/swap step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFingerprint {
    pub files: BTreeMap<PathBuf, (u64, Option<SystemTime>)>,
    pub total_files: u64,
    pub total_bytes: u64,
}

impl TreeFingerprint {
    /// Captures a fail-closed snapshot of every regular file's `(len, mtime)` under `root`.
    pub fn capture(root: &Path) -> Result<Self> {
        let mut files = BTreeMap::new();
        let mut total_bytes = 0u64;
        let root_io = to_extended_io_path(root);
        let root_meta = fs::symlink_metadata(&root_io)?;
        if is_symlink_or_reparse(&root_io) || root_meta.file_type().is_symlink() {
            return Err(GreenCError::PreconditionBlocked {
                operation: "tree_fingerprint".to_string(),
                reason: format!(
                    "Phát hiện liên kết/reparse point không an toàn tại: {}",
                    root.display()
                ),
            });
        }
        if !root_meta.is_dir() {
            return Err(GreenCError::InvalidPath(root.to_path_buf()));
        }

        let mut stack = vec![root.to_path_buf()];
        while let Some(curr) = stack.pop() {
            let curr_io = to_extended_io_path(&curr);
            for entry_res in fs::read_dir(&curr_io)? {
                let entry = entry_res?;
                let p = curr.join(entry.file_name());
                let p_io = to_extended_io_path(&p);
                let meta = fs::symlink_metadata(&p_io)?;
                if is_symlink_or_reparse(&p_io) || meta.file_type().is_symlink() {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "tree_fingerprint".to_string(),
                        reason: format!(
                            "Phát hiện liên kết/reparse point không an toàn tại: {}",
                            p.display()
                        ),
                    });
                }
                if meta.is_dir() {
                    stack.push(p);
                } else if meta.is_file() {
                    let rel = p
                        .strip_prefix(root)
                        .map_err(|_| GreenCError::InvalidPath(p.clone()))?
                        .to_path_buf();
                    let len = meta.len();
                    let mtime = meta.modified().ok();
                    total_bytes = total_bytes.saturating_add(len);
                    files.insert(rel, (len, mtime));
                } else {
                    return Err(GreenCError::PreconditionBlocked {
                        operation: "tree_fingerprint".to_string(),
                        reason: format!(
                            "Phát hiện tệp đặc biệt không an toàn tại: {}",
                            p.display()
                        ),
                    });
                }
            }
        }

        let total_files = files.len() as u64;
        Ok(Self {
            files,
            total_files,
            total_bytes,
        })
    }

    /// Re-captures `root` and fails with `IntegrityMismatch` if file count, any file size, or any
    /// file modification time changed since this snapshot was captured.
    pub fn verify_unchanged(&self, root: &Path) -> Result<()> {
        let now = Self::capture(root)?;
        if now.total_files != self.total_files {
            return Err(GreenCError::IntegrityMismatch {
                path: root.to_path_buf(),
                expected: format!("{} files", self.total_files),
                actual: format!("{} files (thay đổi trước khi đổi chỗ)", now.total_files),
            });
        }
        if now.total_bytes != self.total_bytes {
            return Err(GreenCError::IntegrityMismatch {
                path: root.to_path_buf(),
                expected: format!("{} bytes", self.total_bytes),
                actual: format!("{} bytes (thay đổi trước khi đổi chỗ)", now.total_bytes),
            });
        }
        for (rel, expected_meta) in &self.files {
            match now.files.get(rel) {
                Some(actual_meta) if actual_meta == expected_meta => {}
                Some((actual_len, _)) => {
                    return Err(GreenCError::IntegrityMismatch {
                        path: root.join(rel),
                        expected: format!("size={} mtime={:?}", expected_meta.0, expected_meta.1),
                        actual: format!("size={actual_len} (tệp bị sửa trước khi đổi chỗ)"),
                    });
                }
                None => {
                    return Err(GreenCError::IntegrityMismatch {
                        path: root.join(rel),
                        expected: "present".to_string(),
                        actual: "missing (bị xoá trước khi đổi chỗ)".to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Fail-closed recursive file collector:
/// - Propagates EVERY `read_dir`, entry, and metadata/permission error (NEVER uses `.flatten()`).
/// - Fails immediately on ANY symlink, NTFS junction, reparse point, or special file.
fn collect_relative_files(root: &Path, must_exist: bool) -> Result<BTreeMap<PathBuf, u64>> {
    let mut map = BTreeMap::new();
    let root_io = to_extended_io_path(root);
    let root_meta = match fs::symlink_metadata(&root_io) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !must_exist => return Ok(map),
        Err(e) => return Err(GreenCError::Io(e)),
    };
    if is_symlink_or_reparse(&root_io) || root_meta.file_type().is_symlink() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "zalo_verify".to_string(),
            reason: format!(
                "Phát hiện liên kết/reparse point không an toàn tại: {}",
                root.display()
            ),
        });
    }
    if !root_meta.is_dir() {
        return Err(GreenCError::PreconditionBlocked {
            operation: "zalo_verify".to_string(),
            reason: format!(
                "Đường dẫn kiểm tra không phải thư mục hợp lệ: {}",
                root.display()
            ),
        });
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(curr) = stack.pop() {
        let curr_io = to_extended_io_path(&curr);
        for entry_res in fs::read_dir(&curr_io)? {
            let entry = entry_res?;
            let p = curr.join(entry.file_name());
            let p_io = to_extended_io_path(&p);
            let meta = fs::symlink_metadata(&p_io)?;
            if is_symlink_or_reparse(&p_io) || meta.file_type().is_symlink() {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "zalo_verify".to_string(),
                    reason: format!(
                        "Phát hiện liên kết/reparse point không an toàn bên trong Zalo Data: {}",
                        p.display()
                    ),
                });
            }
            if meta.is_dir() {
                stack.push(p);
            } else if meta.is_file() {
                let rel = p
                    .strip_prefix(root)
                    .map_err(|_| GreenCError::InvalidPath(p.clone()))?
                    .to_path_buf();
                map.insert(rel, meta.len());
            } else {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "zalo_verify".to_string(),
                    reason: format!(
                        "Phát hiện tệp đặc biệt không an toàn bên trong Zalo Data: {}",
                        p.display()
                    ),
                });
            }
        }
    }
    Ok(map)
}

/// Strictly verifies a copied `Zalo Data` directory BEFORE any original may ever be deleted:
/// 1. Fail-closed traversal of both trees (any read/permission error or symlink/reparse fails).
/// 2. Exact key-by-key file path & byte length equality across the entire tree.
/// 3. Mandatory FULL SHA-256 hash comparison for EVERY copied file (both `message/` and all
///    other files regardless of size — never sampling only head/tail).
/// 4. Exact file count and byte count comparison for `media/`.
pub fn verify_zalo_copy_integrity(src: &Path, dst: &Path) -> Result<ZaloCopyVerification> {
    let src_all = collect_relative_files(src, true)?;
    let dst_all = collect_relative_files(dst, true)?;

    let src_files = src_all.len() as u64;
    let dst_files = dst_all.len() as u64;
    if src_files != dst_files {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: format!("{src_files} files"),
            actual: format!("{dst_files} files"),
        });
    }

    let src_bytes: u64 = src_all.values().copied().fold(0u64, u64::saturating_add);
    let dst_bytes: u64 = dst_all.values().copied().fold(0u64, u64::saturating_add);
    if src_bytes != dst_bytes {
        return Err(GreenCError::IntegrityMismatch {
            path: dst.to_path_buf(),
            expected: format!("{src_bytes} bytes"),
            actual: format!("{dst_bytes} bytes"),
        });
    }

    let mut message_files_hashed = 0u64;
    for (rel, &expected_len) in &src_all {
        let Some(&actual_len) = dst_all.get(rel) else {
            return Err(GreenCError::IntegrityMismatch {
                path: dst.join(rel),
                expected: format!("present ({expected_len} bytes)"),
                actual: "missing".to_string(),
            });
        };
        if actual_len != expected_len {
            return Err(GreenCError::IntegrityMismatch {
                path: dst.join(rel),
                expected: format!("{expected_len} bytes"),
                actual: format!("{actual_len} bytes"),
            });
        }

        let is_message = rel
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("message"));

        let src_file = src.join(rel);
        let dst_file = dst.join(rel);
        // Item 7: Full SHA-256 verification for EVERY copied file regardless of size.
        let h_src = sha256_file(&src_file)?;
        let h_dst = sha256_file(&dst_file)?;
        if h_src != h_dst {
            return Err(GreenCError::IntegrityMismatch {
                path: dst_file,
                expected: h_src,
                actual: h_dst,
            });
        }
        if is_message {
            message_files_hashed = message_files_hashed.saturating_add(1);
        }
    }

    let src_media = src.join("media");
    let dst_media = dst.join("media");
    let src_media_files = collect_relative_files(&src_media, false)?;
    let dst_media_files = collect_relative_files(&dst_media, false)?;
    if src_media_files.len() != dst_media_files.len() {
        return Err(GreenCError::IntegrityMismatch {
            path: dst_media,
            expected: format!("{} media files", src_media_files.len()),
            actual: format!("{} media files", dst_media_files.len()),
        });
    }
    let src_media_bytes: u64 = src_media_files
        .values()
        .copied()
        .fold(0u64, u64::saturating_add);
    let dst_media_bytes: u64 = dst_media_files
        .values()
        .copied()
        .fold(0u64, u64::saturating_add);
    if src_media_bytes != dst_media_bytes {
        return Err(GreenCError::IntegrityMismatch {
            path: dst_media,
            expected: format!("{src_media_bytes} media bytes"),
            actual: format!("{dst_media_bytes} media bytes"),
        });
    }

    Ok(ZaloCopyVerification {
        total_files: dst_files,
        total_bytes: dst_bytes,
        message_files_hashed,
        media_files_counted: dst_media_files.len() as u64,
    })
}

/// Recursively checks whether a directory or ANY file inside it (including `message/` DBs)
/// is locked by another process. Fails-closed on unreadable subdirs.
pub fn check_tree_locked(root: &Path, lock_probe: &dyn FileLockProbe) -> Result<()> {
    let root_io = to_extended_io_path(root);
    let meta = match fs::symlink_metadata(&root_io) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(GreenCError::Io(e)),
    };
    if lock_probe.is_file_locked(root) {
        return Err(GreenCError::FileLocked {
            path: root.to_path_buf(),
            reason: format!(
                "Thư mục đang bị khoá bởi tiến trình khác: {}",
                root.display()
            ),
        });
    }
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Ok(());
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(curr) = stack.pop() {
        let curr_io = to_extended_io_path(&curr);
        for entry_res in fs::read_dir(&curr_io)? {
            let entry = entry_res?;
            let p = curr.join(entry.file_name());
            if lock_probe.is_file_locked(&p) {
                return Err(GreenCError::FileLocked {
                    path: p.clone(),
                    reason: format!(
                        "Tệp dữ liệu Zalo đang được mở hoặc bị khoá: {}",
                        p.display()
                    ),
                });
            }
            let p_io = to_extended_io_path(&p);
            let m = fs::symlink_metadata(&p_io)?;
            if m.is_dir() && !m.file_type().is_symlink() {
                stack.push(p);
            }
        }
    }
    Ok(())
}

/// Verifies that no Zalo process is running and none of the given paths (recursively) are locked.
fn check_zalo_closed_and_unlocked_opt(
    paths: &[&Path],
    proc_probe: &dyn ProcessServiceProbe,
    lifecycle: Option<&dyn AppLifecycleApi>,
    lock_probe: &dyn FileLockProbe,
    reopened_reason: &str,
) -> Result<()> {
    let primary_path = paths.first().copied().unwrap_or_else(|| Path::new(""));
    for proc in ZALO_PROCESS_NAMES {
        let proc_running = proc_probe.is_process_running(proc)
            || proc_probe.is_process_running(&proc.to_lowercase());
        let exe_running = lifecycle
            .map(|lc| !lc.find_running_exe_paths(&[proc]).is_empty())
            .unwrap_or(false);
        if proc_running || exe_running {
            return Err(GreenCError::FileLocked {
                path: primary_path.to_path_buf(),
                reason: reopened_reason.to_string(),
            });
        }
    }
    for path in paths {
        if let Err(err) = check_tree_locked(path, lock_probe) {
            return match err {
                GreenCError::FileLocked { path: p, .. } => Err(GreenCError::FileLocked {
                    path: p,
                    reason: reopened_reason.to_string(),
                }),
                other => Err(other),
            };
        }
    }
    Ok(())
}

/// Verifies that no Zalo process is running (process-only check, no recursive tree lock walk).
/// Used at Checkpoints 2–3 between renaming `source_path` and creating the junction to keep the
/// critical window minimal (Item 11).
fn check_zalo_processes_closed(
    primary_path: &Path,
    proc_probe: &dyn ProcessServiceProbe,
    lifecycle: &dyn AppLifecycleApi,
    reopened_reason: &str,
) -> Result<()> {
    for proc in ZALO_PROCESS_NAMES {
        let proc_running = proc_probe.is_process_running(proc)
            || proc_probe.is_process_running(&proc.to_lowercase());
        let exe_running = !lifecycle.find_running_exe_paths(&[proc]).is_empty();
        if proc_running || exe_running {
            return Err(GreenCError::FileLocked {
                path: primary_path.to_path_buf(),
                reason: reopened_reason.to_string(),
            });
        }
    }
    Ok(())
}

/// Verifies that no Zalo process is running and none of the given paths (recursively) are locked.
fn check_zalo_closed_and_unlocked(
    paths: &[&Path],
    proc_probe: &dyn ProcessServiceProbe,
    lifecycle: &dyn AppLifecycleApi,
    lock_probe: &dyn FileLockProbe,
    reopened_reason: &str,
) -> Result<()> {
    check_zalo_closed_and_unlocked_opt(
        paths,
        proc_probe,
        Some(lifecycle),
        lock_probe,
        reopened_reason,
    )
}

/// Prefix for GreenC's own temporary write-probe files through a junction.
const JUNCTION_PROBE_PREFIX: &str = ".greenc-junction-probe-";

/// Verifies that the newly created junction at `link_path` is writable and resolves live to
/// `target_path` by creating, reading back, and deleting a temporary probe file.
pub fn verify_junction_write_probe(link_path: &Path, target_path: &Path) -> Result<()> {
    let probe_name = format!("{JUNCTION_PROBE_PREFIX}{}", uuid::Uuid::new_v4());
    let link_probe = link_path.join(&probe_name);
    let target_probe = target_path.join(&probe_name);
    let payload = format!("greenc-zalo-probe-{}", uuid::Uuid::new_v4());

    fs::write(&link_probe, payload.as_bytes()).map_err(|e| {
        GreenCError::Relocation(format!(
            "Không thể ghi tệp kiểm tra qua liên kết {}: {e}",
            link_path.display()
        ))
    })?;

    let read_res = fs::read_to_string(&target_probe);
    let _ = fs::remove_file(&link_probe);
    let _ = fs::remove_file(&target_probe);

    match read_res {
        Ok(got) if got == payload => Ok(()),
        Ok(_) => Err(GreenCError::Relocation(
            "Tệp kiểm tra qua liên kết không khớp nội dung tại thư mục đích".to_string(),
        )),
        Err(e) => Err(GreenCError::Relocation(format!(
            "Liên kết {} không trỏ tới thư mục đích {}: {e}",
            link_path.display(),
            target_path.display()
        ))),
    }
}

/// Snapshots regular files under `root` (ignoring GreenC's own `.greenc-junction-probe-*` files)
/// so the post-reopen health watch can verify that Zalo itself actually wrote to `target_path`.
fn snapshot_target_non_probe_files(root: &Path) -> BTreeMap<PathBuf, (u64, Option<SystemTime>)> {
    let mut map = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(curr) = stack.pop() {
        let curr_io = to_extended_io_path(&curr);
        let Ok(rd) = fs::read_dir(&curr_io) else {
            continue;
        };
        for entry in rd.flatten() {
            let fname = entry.file_name();
            if fname
                .to_str()
                .is_some_and(|n| n.starts_with(JUNCTION_PROBE_PREFIX))
            {
                continue;
            }
            let p = curr.join(&fname);
            let p_io = to_extended_io_path(&p);
            let Ok(meta) = fs::symlink_metadata(&p_io) else {
                continue;
            };
            if is_symlink_or_reparse(&p_io) || meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else if meta.is_file() {
                if let Ok(rel) = p.strip_prefix(root) {
                    map.insert(rel.to_path_buf(), (meta.len(), meta.modified().ok()));
                }
            }
        }
    }
    map
}

fn is_sqlite_sidecar_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with("-wal") || lower.ends_with("-shm") || lower.ends_with("-journal")
}

fn is_sqlite_db_candidate(rel: &Path, reference_file: &Path) -> bool {
    let ext_match = rel.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        e.eq_ignore_ascii_case("db")
            || e.eq_ignore_ascii_case("sqlite")
            || e.eq_ignore_ascii_case("sqlite3")
    });
    if ext_match {
        return true;
    }
    let io_path = to_extended_io_path(reference_file);
    let Ok(mut f) = fs::File::open(&io_path) else {
        return false;
    };
    let mut header = [0u8; 16];
    f.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0"
}

fn verify_sqlite_db_openable(path: &Path, operation: &str) -> Result<()> {
    let io_path = to_extended_io_path(path);
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(&io_path, flags).map_err(|e| {
        GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Không thể mở cơ sở dữ liệu SQLite '{}' để xác minh: {e}",
                path.display()
            ),
        }
    })?;
    let check: String = conn
        .query_row("PRAGMA quick_check(1);", [], |r| r.get(0))
        .map_err(|e| GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Cơ sở dữ liệu SQLite '{}' không đọc/xác minh được (PRAGMA quick_check): {e}",
                path.display()
            ),
        })?;
    if !check.eq_ignore_ascii_case("ok") {
        return Err(GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Cơ sở dữ liệu SQLite '{}' lỗi toàn vẹn ({check})",
                path.display()
            ),
        });
    }
    Ok(())
}

/// Fail-closed gate verifying `live_dir` covers every message database present in `copy_dir`
/// on disk (ignoring `-wal`/`-shm`/`-journal` sidecars and NOT counting `media/` files):
/// - Both `copy_dir/message` and `live_dir/message` must be real readable directories.
/// - Every non-sidecar file in `copy_dir/message` must exist in `live_dir/message`, must NOT be
///   significantly smaller (>= 80% of on-disk copy size, non-zero if copy is non-zero), and every
///   SQLite DB file must open cleanly and pass `PRAGMA quick_check(1)`.
pub(crate) fn verify_live_zalo_covers_copy(
    live_dir: &Path,
    copy_dir: &Path,
    operation: &str,
) -> Result<()> {
    let copy_msg_dir = copy_dir.join("message");
    let copy_msg_files = collect_relative_files(&copy_msg_dir, true).map_err(|e| {
        GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Không thể đọc thư mục message/ của bản gốc {} để đối chiếu: {e}",
                copy_dir.display()
            ),
        }
    })?;

    let live_msg_dir = live_dir.join("message");
    let live_msg_files = collect_relative_files(&live_msg_dir, true).map_err(|e| {
        GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Không thể xác minh thư mục message/ tại vị trí đang dùng {}: {e}",
                live_dir.display()
            ),
        }
    })?;

    let mut checked_dbs = 0u64;
    for (rel, &orig_size) in &copy_msg_files {
        let fname = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_sqlite_sidecar_name(fname) {
            continue;
        }
        checked_dbs = checked_dbs.saturating_add(1);

        let Some(&live_size) = live_msg_files.get(rel) else {
            return Err(GreenCError::PreconditionBlocked {
                operation: operation.to_string(),
                reason: format!(
                    "Không thể xoá bản sao vì tệp cơ sở dữ liệu message/{} có trong bản gốc nhưng bị thiếu tại {}.",
                    rel.display(),
                    live_dir.display()
                ),
            });
        };

        if orig_size > 0
            && (live_size == 0 || live_size.saturating_mul(100) < orig_size.saturating_mul(80))
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: operation.to_string(),
                reason: format!(
                    "Không thể xoá bản sao vì tệp message/{} tại {} nhỏ hơn đáng kể so với bản gốc ({live_size} bytes so với {orig_size} bytes).",
                    rel.display(),
                    live_dir.display()
                ),
            });
        }

        let copy_file = copy_msg_dir.join(rel);
        let live_file = live_msg_dir.join(rel);
        if is_sqlite_db_candidate(rel, &copy_file) {
            verify_sqlite_db_openable(&live_file, operation)?;
        }
    }

    if checked_dbs == 0 && live_msg_files.is_empty() {
        return Err(GreenCError::PreconditionBlocked {
            operation: operation.to_string(),
            reason: format!(
                "Không tìm thấy cơ sở dữ liệu tin nhắn hợp lệ trong message/ tại {}.",
                live_dir.display()
            ),
        });
    }

    Ok(())
}

/// Formats the plain-language one-time confirmation message for moving Zalo chat data.
pub fn format_zalo_move_confirmation(size_bytes: u64, target_drive: char) -> String {
    let drive = target_drive.to_ascii_uppercase();
    let size_text = format_bytes_vi(size_bytes);
    format!(
        "GreenC sẽ đóng Zalo, chuyển {size_text} dữ liệu chat sang {drive}:, rồi mở lại Zalo. Bản gốc trên ổ C: được giữ tạm tối thiểu 24 giờ hoạt động ổn định thực tế (chỉ tự dọn khi Zalo đã mở lại, xác minh khoẻ và đủ 24 giờ quan sát; nếu chưa xác minh sẽ giữ đến khi bạn tự xoá). Tin nhắn và ảnh giữ nguyên, có thể hoàn tác."
    )
}

/// Cleans abandoned `<target_name>.greenc-copying.*` staging directories left behind by a crash
/// during the copy phase.
///
/// Safety gate: ONLY deletes `.greenc-copying.*` staging dirs when `source_path` is a real,
/// non-reparse directory (so original data on C: is 100% intact) and caller holds `RelocationLock`.
pub fn cleanup_abandoned_copy_staging_dirs(
    target_path: &Path,
    source_path: &Path,
    reparse_api: &dyn ReparsePointApi,
) -> usize {
    if reparse_api.is_reparse_point(source_path) {
        return 0;
    }
    let source_ok = fs::symlink_metadata(to_extended_io_path(source_path))
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink());
    if !source_ok {
        return 0;
    }
    let Some(parent) = target_path.parent() else {
        return 0;
    };
    let Some(base_name) = target_path.file_name().and_then(|n| n.to_str()) else {
        return 0;
    };
    let prefix = format!("{base_name}.greenc-copying.");
    let Ok(entries) = fs::read_dir(parent) else {
        return 0;
    };
    let mut cleaned = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let p = entry.path();
        if fs::symlink_metadata(to_extended_io_path(&p))
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            remove_dir_all_safe(&p);
            if !path_exists_lstat(&p) {
                cleaned += 1;
            }
        }
    }
    cleaned
}

fn source_drive_char(discovery: &ZaloDiscovery, source_path: &Path) -> char {
    discovery
        .data_drive
        .as_deref()
        .and_then(|s| s.chars().next())
        .filter(|c| c.is_ascii_alphabetic())
        .or_else(|| {
            extract_drive_label(source_path)
                .and_then(|s| s.chars().next())
                .filter(|c| c.is_ascii_alphabetic())
        })
        .unwrap_or('C')
        .to_ascii_uppercase()
}

fn make_sibling_path(path: &Path, tag: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Zalo Data");
    let parent = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(r"C:\"));
    parent.join(format!("{name}.{tag}.{}", uuid::Uuid::new_v4()))
}

fn path_exists_lstat(path: &Path) -> bool {
    fs::symlink_metadata(to_extended_io_path(path)).is_ok()
}

/// Safely recovers when junction creation fails or Zalo reopens after `source_path` was renamed
/// to `moving_path`:
/// 1. Never deletes a real `source_path` created by Zalo — renames it to `.greenc-conflict.<uuid>`.
/// 2. Restores `moving_path` -> `source_path`.
/// 3. Keeps `target_path` intact UNTIL `moving_path` is verified back at `source_path`; if restore
///    fails, keeps ALL copies and marks `NeedsReview`.
fn recover_after_rename_failure(
    source_path: &Path,
    moving_path: &Path,
    target_path: &Path,
    reparse_api: &dyn ReparsePointApi,
    record: &mut RelocationRecord,
    db: &GreenCDb,
    cause: &str,
) -> (Option<PathBuf>, bool, String) {
    let mut conflict_path: Option<PathBuf> = None;

    if reparse_api.is_reparse_point(source_path) {
        if let Err(err) = reparse_api.delete_junction(source_path) {
            let msg = format!(
                "{cause}. Không xoá được liên kết lỗi tại {} ({err}); GreenC giữ nguyên mọi bản dữ liệu: bản gốc tạm ({}), bản đích ({}).",
                source_path.display(),
                moving_path.display(),
                target_path.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            return (None, false, msg);
        }
    } else if path_exists_lstat(source_path) {
        let candidate_conflict = make_sibling_path(source_path, "greenc-conflict");
        match fs::rename(source_path, &candidate_conflict) {
            Ok(()) => {
                conflict_path = Some(candidate_conflict);
            }
            Err(err) => {
                let msg = format!(
                    "{cause}. Zalo đã tạo thư mục mới tại {} nhưng không đổi tên được sang bản xung đột ({err}); GreenC giữ nguyên tất cả bản: thư mục mới ({}), bản gốc tạm ({}), bản đích ({}).",
                    source_path.display(),
                    source_path.display(),
                    moving_path.display(),
                    target_path.display()
                );
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.detail = Some(msg.clone());
                let _ = db.save_relocation_durable(record);
                return (None, false, msg);
            }
        }
    }

    match fs::rename(moving_path, source_path) {
        Ok(()) if path_exists_lstat(source_path) && !path_exists_lstat(moving_path) => {
            // Original is verified back at `source_path`.
            // Item 7: NEVER delete `target_path` directly if renaming it fails!
            let target_cleanup = make_sibling_path(target_path, "greenc-rollback-done");
            let mut target_rename_failed: Option<String> = None;
            if path_exists_lstat(target_path) {
                match fs::rename(target_path, &target_cleanup) {
                    Ok(()) => {
                        remove_dir_all_safe(&target_cleanup);
                    }
                    Err(err) => {
                        tracing::warn!(
                            "Failed to rename rollback target {} -> {}: {err}; preserving target untouched",
                            target_path.display(),
                            target_cleanup.display()
                        );
                        target_rename_failed = Some(err.to_string());
                    }
                }
            }

            let target_note = target_rename_failed
                .as_ref()
                .map(|e| {
                    format!(
                        " Không đổi tên được bản đích {} để dọn ({e}); GreenC giữ nguyên bản đích để tránh mất dữ liệu.",
                        target_path.display()
                    )
                })
                .unwrap_or_default();

            let msg = if let Some(ref cp) = conflict_path {
                format!(
                    "{cause}. GreenC đã trả bản gốc về {} và giữ thư mục mới do Zalo tạo tại {} (bản xung đột .greenc-conflict) để không mất dữ liệu.{target_note}",
                    source_path.display(),
                    cp.display()
                )
            } else {
                format!(
                    "{cause}. GreenC đã trả dữ liệu gốc về {} an toàn.{target_note}",
                    source_path.display()
                )
            };
            record.status = if target_rename_failed.is_some() {
                RelocationStatus::NeedsReview
            } else {
                RelocationStatus::Failed
            };
            record.health_status = RelocationHealthStatus::Healthy;
            record.moving_path = None;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            (conflict_path, true, msg)
        }
        Ok(()) => {
            let msg = format!(
                "{cause}. Không xác nhận được bản gốc đã trở về {}; GreenC giữ nguyên tất cả bản: bản gốc tạm ({}), bản đích ({}).",
                source_path.display(),
                moving_path.display(),
                target_path.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            (conflict_path, false, msg)
        }
        Err(rename_err) => {
            let mut kept = vec![
                format!("bản gốc tạm ({})", moving_path.display()),
                format!("bản đích ({})", target_path.display()),
            ];
            if let Some(ref cp) = conflict_path {
                kept.push(format!("bản xung đột ({})", cp.display()));
            }
            let msg = format!(
                "{cause}. Không thể trả bản gốc về {} ({rename_err}); GreenC giữ nguyên tất cả bản: {}.",
                source_path.display(),
                kept.join(", ")
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            (conflict_path, false, msg)
        }
    }
}

pub struct ZaloRelocator;

impl ZaloRelocator {
    /// Executes the full transactional Zalo chat-data move with optional progress reporting.
    #[allow(clippy::too_many_arguments)]
    pub fn move_zalo_data<F>(
        discovery: &ZaloDiscovery,
        opts: &ZaloMoveOptions,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        ctrl: &TaskControl,
        on_progress: F,
    ) -> Result<ZaloMoveReport>
    where
        F: FnMut(ZaloMoveProgress),
    {
        Self::move_zalo_data_with_hooks(
            discovery,
            opts,
            db,
            volume_api,
            reparse_api,
            shell_api,
            lock_probe,
            proc_probe,
            lifecycle,
            ctrl,
            on_progress,
            &ZaloMoveHooks::default(),
        )
    }

    /// Backwards-compatible wrapper with an optional `after_copy_hook`.
    #[allow(clippy::too_many_arguments)]
    pub fn move_zalo_data_with_hook<F>(
        discovery: &ZaloDiscovery,
        opts: &ZaloMoveOptions,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        ctrl: &TaskControl,
        on_progress: F,
        after_copy_hook: Option<&dyn Fn(&Path)>,
    ) -> Result<ZaloMoveReport>
    where
        F: FnMut(ZaloMoveProgress),
    {
        let hooks = ZaloMoveHooks {
            after_copy: after_copy_hook,
            ..Default::default()
        };
        Self::move_zalo_data_with_hooks(
            discovery,
            opts,
            db,
            volume_api,
            reparse_api,
            shell_api,
            lock_probe,
            proc_probe,
            lifecycle,
            ctrl,
            on_progress,
            &hooks,
        )
    }

    /// Full transactional Zalo chat-data relocation with fault-injection hooks at each stage.
    #[allow(clippy::too_many_arguments)]
    pub fn move_zalo_data_with_hooks<F>(
        discovery: &ZaloDiscovery,
        opts: &ZaloMoveOptions,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        shell_api: &dyn ShellFolderApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        ctrl: &TaskControl,
        mut on_progress: F,
        hooks: &ZaloMoveHooks<'_>,
    ) -> Result<ZaloMoveReport>
    where
        F: FnMut(ZaloMoveProgress),
    {
        if ctrl.is_cancelled() {
            return Err(GreenCError::Cancelled);
        }

        // Acquire machine-wide relocation lock before touching Zalo or filesystem (GUI + CLI).
        let _reloc_lock = if !opts.dry_run {
            Some(RelocationLock::acquire_for_db(db.path())?)
        } else {
            None
        };

        let target_letter = opts.target_drive.to_ascii_uppercase();
        let target_drive_str = format!("{target_letter}:");

        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Preparing,
            percent: 2,
            done_bytes: 0,
            total_bytes: discovery.chat_data_bytes,
            done_files: 0,
            total_files: discovery.chat_data_file_count,
            detail: "Kiểm tra điều kiện và dung lượng trống trước khi chuyển...".to_string(),
        });

        // 1. Platform & Store edition guard
        if opts.enforce_platform_support && !is_zalo_auto_move_supported() {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_move".to_string(),
                reason: zalo_auto_move_unsupported_reason(),
            });
        }

        // 2. Discovery validation
        let source_path =
            discovery
                .data_dir
                .clone()
                .ok_or_else(|| GreenCError::PreconditionBlocked {
                    operation: "zalo_move".to_string(),
                    reason: "Không tìm thấy thư mục dữ liệu Zalo hợp lệ trong config.json"
                        .to_string(),
                })?;

        if discovery.data_is_known_relocation {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_move".to_string(),
                reason: "Dữ liệu Zalo đã được GreenC chuyển sang ổ khác rồi".to_string(),
            });
        }

        // 3. Already on another drive? Report cleanly, do NOT move!
        if !discovery.data_on_system_drive {
            let drv = discovery
                .data_drive
                .clone()
                .unwrap_or_else(|| "khác".to_string());
            let msg = format!("Dữ liệu Zalo đã ở ổ {drv}, không cần chuyển");
            return Ok(ZaloMoveReport {
                record_id: None,
                dry_run: opts.dry_run,
                already_on_other_drive: true,
                source_path: source_path.clone(),
                target_path: source_path,
                target_drive: drv,
                moved_bytes: 0,
                moved_files: 0,
                message_files_verified: 0,
                media_files_verified: 0,
                reopened_zalo: false,
                health_verified: true,
                auto_rolled_back: false,
                retained_original_id: None,
                retained_original_path: None,
                retained_original_bytes: 0,
                retained_purge_after: None,
                space_estimate: None,
                status: "already_on_other_drive".to_string(),
                message: msg,
            });
        }

        if !source_path.exists() || !source_path.is_dir() {
            return Err(GreenCError::NotFound {
                entity: "ZaloDataDir".to_string(),
                id: source_path.display().to_string(),
            });
        }

        // Item 10: Accept source folder ONLY when it has authentic Zalo structure (`message/`).
        if !greenc_scanner::zalo::has_valid_zalo_data_structure(&source_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_move".to_string(),
                reason: format!(
                    "Thư mục '{}' không có cấu trúc dữ liệu Zalo hợp lệ (thiếu thư mục message/). GreenC từ chối chuyển để bảo vệ dữ liệu.",
                    source_path.display()
                ),
            });
        }

        let target_path = opts
            .custom_target_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(format!("{target_letter}:\\GreenC\\Relocated\\zalo")));

        // 4. Target drive eligibility + dual-drive (source C: & target) space estimation
        let src_drive = source_drive_char(discovery, &source_path);
        CandidateQualificationEngine::check_target_volume_eligible(target_letter, volume_api)?;
        let space_estimate = estimate_zalo_move_space(
            src_drive,
            target_letter,
            discovery.chat_data_bytes,
            volume_api,
        )?;
        if !space_estimate.can_proceed {
            return Err(GreenCError::InsufficientVaultSpace(
                space_estimate.blocking_reason.clone().unwrap_or_else(|| {
                    "Không đủ dung lượng trống trên ổ nguồn hoặc ổ đích để chuyển Zalo an toàn."
                        .to_string()
                }),
            ));
        }

        CandidateQualificationEngine::check_onedrive(&source_path, shell_api)?;
        CandidateQualificationEngine::check_target_directory_empty_or_absent(&target_path)?;

        // 5. Active call check: REFUSE if `ZaloCall.exe` has a visible window!
        if lifecycle.has_visible_window(&["ZaloCall.exe"]) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_move".to_string(),
                reason: ZALO_ACTIVE_CALL_MESSAGE.to_string(),
            });
        }

        // 6. Dry-run returns verified plan + space estimate without shutting down Zalo or writing disk.
        if opts.dry_run {
            return Ok(ZaloMoveReport {
                record_id: None,
                dry_run: true,
                already_on_other_drive: false,
                source_path,
                target_path,
                target_drive: target_drive_str,
                moved_bytes: discovery.chat_data_bytes,
                moved_files: discovery.chat_data_file_count,
                message_files_verified: discovery.message_file_count,
                media_files_verified: discovery.media_file_count,
                reopened_zalo: false,
                health_verified: false,
                auto_rolled_back: false,
                retained_original_id: None,
                retained_original_path: None,
                retained_original_bytes: discovery.chat_data_bytes,
                retained_purge_after: None,
                space_estimate: Some(space_estimate),
                status: "dry_run".to_string(),
                message: format_zalo_move_confirmation(discovery.chat_data_bytes, target_letter),
            });
        }

        // 7. Gracefully close Zalo via Restart Manager (NEVER force kill)
        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::ClosingZalo,
            percent: 8,
            done_bytes: 0,
            total_bytes: discovery.chat_data_bytes,
            done_files: 0,
            total_files: discovery.chat_data_file_count,
            detail: "Đang đóng Zalo nhẹ nhàng...".to_string(),
        });

        let mut shutdown_session = lifecycle.graceful_shutdown(
            &[source_path.clone(), discovery.roaming_dir.clone()],
            ZALO_PROCESS_NAMES,
            opts.shutdown_timeout,
        )?;

        // Item 11: Helper to ALWAYS reopen Zalo on any abort/cancel/error after shutdown
        // (unless Zalo is already running, e.g. user/auto-restart opened it mid-move).
        let reopen_on_abort = |session: &mut greenc_platform::app_lifecycle::AppShutdownSession| {
            if opts.reopen_zalo && !session.captured_exes.is_empty() {
                let already_running =
                    check_zalo_processes_closed(&source_path, proc_probe, lifecycle, "").is_err();
                if !already_running {
                    let _ = lifecycle.restart_app(session);
                }
            }
        };

        // Ensure no lingering Zalo process or recursive file lock (including `message/` DBs) remains
        if let Err(err) = check_zalo_closed_and_unlocked(
            &[&source_path],
            proc_probe,
            lifecycle,
            lock_probe,
            ZALO_MANUAL_QUIT_MESSAGE,
        ) {
            reopen_on_abort(&mut shutdown_session);
            return Err(err);
        }

        if let Some(parent) = target_path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                reopen_on_abort(&mut shutdown_session);
                return Err(GreenCError::Io(err));
            }
        }

        // Clean any abandoned `.greenc-copying.*` staging dirs from a previous crash during copy
        // (safe because we hold `RelocationLock` and `source_path` is a real intact directory).
        let _ = cleanup_abandoned_copy_staging_dirs(&target_path, &source_path, reparse_api);

        // 8. Phase 1: Capture source fingerprint (size/mtime/file-count), persist durable Moving
        //    record, and copy into `<target>.greenc-copying.<uuid>` staging dir so a mid-copy crash
        //    never leaves an uncleaned partial directory at the final `target_path`.
        let source_snapshot = match TreeFingerprint::capture(&source_path) {
            Ok(fp) => fp,
            Err(err) => {
                reopen_on_abort(&mut shutdown_session);
                return Err(err);
            }
        };

        let target_staging = make_sibling_path(&target_path, "greenc-copying");
        let mut record = RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: source_path.clone(),
            target_path: target_path.clone(),
            size_bytes: discovery.chat_data_bytes,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: ZALO_PROCESS_NAMES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            created_at: Utc::now(),
            detail: Some("Đang sao chép dữ liệu chat Zalo sang ổ đích...".to_string()),
            moving_path: None,
            target_user_repaired: false,
        };
        let rec_id = match db.save_relocation_durable(&record) {
            Ok(id) => id,
            Err(err) => {
                reopen_on_abort(&mut shutdown_session);
                return Err(err);
            }
        };
        record.id = Some(rec_id);

        // Item 10: Pre-create staging destination on target drive (e.g. D:) and restrict its ACL
        // to the current user ONLY BEFORE copying a single byte of private Zalo chat data.
        let staging_prep_res: Result<()> = (|| {
            fs::create_dir(&target_staging)?;
            greenc_platform::restrict_dir_to_current_user(&target_staging)?;
            Ok(())
        })();
        if let Err(err) = staging_prep_res {
            remove_dir_all_safe(&target_staging);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!(
                "Không thiết lập được quyền chỉ người dùng hiện tại cho thư mục đích: {err}"
            ));
            let _ = db.save_relocation_durable(&record);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(err);
        }

        let total_bytes = discovery.chat_data_bytes;
        let total_files = discovery.chat_data_file_count;
        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Moving,
            percent: 12,
            done_bytes: 0,
            total_bytes,
            done_files: 0,
            total_files,
            detail: "Đang sao chép và kiểm băm SHA-256 dữ liệu chat sang ổ mới...".to_string(),
        });

        let copy_res =
            verified_copy_tree_strict(&source_path, &target_staging, ctrl, |done_b, done_f| {
                let frac = if total_bytes > 0 {
                    (done_b as f64 / total_bytes as f64).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let pct = 12 + (frac * 58.0) as u32;
                on_progress(ZaloMoveProgress {
                    phase: ZaloMovePhase::Moving,
                    percent: pct,
                    done_bytes: done_b,
                    total_bytes,
                    done_files: done_f,
                    total_files,
                    detail: format!("Đã sao chép {done_f} tệp..."),
                });
            });

        if let Err(err) = copy_res {
            remove_dir_all_safe(&target_staging);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!("Sao chép dữ liệu thất bại: {err}"));
            let _ = db.save_relocation_durable(&record);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(err);
        }

        if let Some(hook) = hooks.after_copy {
            hook(&target_staging);
        }

        // 9. Mandatory fail-closed pre-rename verification (exact paths/sizes, full SHA-256, media count)
        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Verifying,
            percent: 72,
            done_bytes: total_bytes,
            total_bytes,
            done_files: total_files,
            total_files,
            detail: "Đang kiểm tra toàn vẹn cơ sở dữ liệu tin nhắn và ảnh...".to_string(),
        });

        let verification = match verify_zalo_copy_integrity(&source_path, &target_staging) {
            Ok(v) => v,
            Err(err) => {
                remove_dir_all_safe(&target_staging);
                record.status = RelocationStatus::Failed;
                record.detail = Some(format!("Xác minh toàn vẹn dữ liệu thất bại: {err}"));
                let _ = db.save_relocation_durable(&record);
                if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                    let _ = lifecycle.restart_app(&mut shutdown_session);
                }
                return Err(err);
            }
        };

        if ctrl.is_cancelled() {
            remove_dir_all_safe(&target_staging);
            record.status = RelocationStatus::Failed;
            record.detail = Some("Người dùng đã huỷ quá trình chuyển dữ liệu".to_string());
            let _ = db.save_relocation_durable(&record);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(GreenCError::Cancelled);
        }

        // Atomically finalize target staging dir into `target_path` on the target drive
        if let Err(err) = fs::rename(&target_staging, &target_path) {
            remove_dir_all_safe(&target_staging);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!("Không đổi tên được thư mục đích hoàn chỉnh: {err}"));
            let _ = db.save_relocation_durable(&record);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(GreenCError::Io(err));
        }

        // Item 10: Re-assert current-user-only ACL on the finalized target folder on D:.
        if let Err(err) = greenc_platform::restrict_dir_to_current_user(&target_path) {
            remove_dir_all_safe(&target_path);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!(
                "Không xác minh/đặt được ACL chỉ người dùng hiện tại trên thư mục đích: {err}"
            ));
            let _ = db.save_relocation_durable(&record);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(err);
        }

        // 10. Phase 2: Persist durable `Moving` record with exact `moving_path` & rename source
        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Linking,
            percent: 80,
            done_bytes: verification.total_bytes,
            total_bytes: verification.total_bytes,
            done_files: verification.total_files,
            total_files: verification.total_files,
            detail: "Đang tạo liên kết thư mục sang ổ mới...".to_string(),
        });

        let moving_path = make_sibling_path(&source_path, "greenc-moving");
        record.size_bytes = verification.total_bytes;
        record.moving_path = Some(moving_path.clone());
        record.detail = Some("Đang đổi chỗ thư mục và tạo liên kết sang ổ mới...".to_string());
        if let Err(err) = db.save_relocation_durable(&record) {
            remove_dir_all_safe(&target_path);
            reopen_on_abort(&mut shutdown_session);
            return Err(err);
        }

        // Checkpoint 1 (Item 5): Check if Zalo auto-reopened right BEFORE rename. Never force-kill!
        if let Some(hook) = hooks.before_rename {
            hook(&source_path, &target_path);
        }
        let mid_reopen_msg = "Phát hiện Zalo vừa tự mở lại giữa chừng; GreenC đã dừng an toàn và không ép tắt Zalo. Hãy thoát Zalo rồi thử lại.";
        if let Err(err) = check_zalo_closed_and_unlocked(
            &[&source_path],
            proc_probe,
            lifecycle,
            lock_probe,
            mid_reopen_msg,
        ) {
            remove_dir_all_safe(&target_path);
            record.status = RelocationStatus::Failed;
            record.detail = Some(err.to_string());
            let _ = db.save_relocation_durable(&record);
            reopen_on_abort(&mut shutdown_session);
            return Err(err);
        }

        // Item 7: Immediately BEFORE rename, re-verify source size/mtime/file-count matches the
        // snapshot captured at copy time. If anything changed, abort cleanly and keep original!
        if let Err(err) = source_snapshot.verify_unchanged(&source_path) {
            remove_dir_all_safe(&target_path);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!(
                "Dữ liệu nguồn thay đổi ngay trước khi đổi tên ({err}); GreenC đã huỷ và giữ nguyên bản gốc."
            ));
            let _ = db.save_relocation_durable(&record);
            reopen_on_abort(&mut shutdown_session);
            return Err(err);
        }

        if let Err(rename_err) = fs::rename(&source_path, &moving_path) {
            remove_dir_all_safe(&target_path);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!("Lỗi đổi tên thư mục nguồn: {rename_err}"));
            let _ = db.save_relocation_durable(&record);
            reopen_on_abort(&mut shutdown_session);
            return Err(GreenCError::Io(rename_err));
        }

        // Checkpoint 2 (Item 11): Process-only check immediately AFTER rename
        // (no recursive tree lock walk on `moving_path` while `source_path` is absent).
        if let Some(hook) = hooks.after_rename {
            hook(&source_path, &moving_path, &target_path);
        }
        if check_zalo_processes_closed(&source_path, proc_probe, lifecycle, mid_reopen_msg).is_err()
        {
            let (_, _, detail) = recover_after_rename_failure(
                &source_path,
                &moving_path,
                &target_path,
                reparse_api,
                &mut record,
                db,
                mid_reopen_msg,
            );
            return Err(GreenCError::FileLocked {
                path: source_path,
                reason: detail,
            });
        }

        // Checkpoint 3 (Item 11): Process-only check immediately BEFORE create_junction.
        if let Some(hook) = hooks.before_junction {
            hook(&source_path, &moving_path, &target_path);
        }
        if check_zalo_processes_closed(&source_path, proc_probe, lifecycle, mid_reopen_msg).is_err()
        {
            let (_, _, detail) = recover_after_rename_failure(
                &source_path,
                &moving_path,
                &target_path,
                reparse_api,
                &mut record,
                db,
                mid_reopen_msg,
            );
            return Err(GreenCError::FileLocked {
                path: source_path,
                reason: detail,
            });
        }

        // 11. Phase 3: Create junction, verify target, AND run write-permission probe through junction.
        // If `source_path` already reappeared (e.g. Zalo created a dir between checks), fail cleanly
        // so Item 3 safe conflict-recovery runs!
        let link_res = (|| -> Result<()> {
            if path_exists_lstat(&source_path) {
                return Err(GreenCError::Relocation(format!(
                    "Thư mục {} bất ngờ xuất hiện lại trước khi tạo liên kết",
                    source_path.display()
                )));
            }
            reparse_api.create_junction(&source_path, &target_path)?;
            let verified_target = reparse_api.get_junction_target(&source_path)?;
            if !greenc_core::models::same_windows_path(&verified_target, &target_path) {
                return Err(GreenCError::Relocation(format!(
                    "Liên kết trỏ sai đích: kỳ vọng {}, nhận {}",
                    target_path.display(),
                    verified_target.display()
                )));
            }
            if !opts.skip_write_probe {
                verify_junction_write_probe(&source_path, &target_path)?;
            }
            Ok(())
        })();

        if let Err(j_err) = link_res {
            let cause = format!("Lỗi tạo liên kết thư mục: {j_err}");
            let (conflict_opt, _restored, detail) = recover_after_rename_failure(
                &source_path,
                &moving_path,
                &target_path,
                reparse_api,
                &mut record,
                db,
                &cause,
            );
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            // Preserve the original Win32 error type when there was no conflict, or return a
            // detailed Relocation error naming the conflict path when a conflict occurred.
            return if conflict_opt.is_some() {
                Err(GreenCError::Relocation(detail))
            } else {
                Err(j_err)
            };
        }

        if let Some(hook) = hooks.after_junction {
            hook(&source_path, &moving_path, &target_path);
        }

        // 12. IMPORTANT (Item 1): DO NOT delete `moving_path` here!
        // Keep `moving_path` intact on disk AND keep `record.status = Moving` in DB while
        // reopening Zalo and running the 20-second post-reopen junction health watch!
        let mut reopened_zalo = false;
        let mut post_reopen_health_verified = false;

        if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
            on_progress(ZaloMoveProgress {
                phase: ZaloMovePhase::Reopening,
                percent: 88,
                done_bytes: verification.total_bytes,
                total_bytes: verification.total_bytes,
                done_files: verification.total_files,
                total_files: verification.total_files,
                detail: "Đang mở lại Zalo (vẫn giữ nguyên bản gốc trên ổ C:)...".to_string(),
            });

            // Capture baseline of non-probe files on target BEFORE reopening Zalo so we can
            // detect genuine writes made by Zalo itself (not GreenC's own write probe).
            let pre_reopen_target_snapshot = snapshot_target_non_probe_files(&target_path);

            reopened_zalo = lifecycle
                .restart_app(&mut shutdown_session)
                .unwrap_or(false);

            // Watch junction health for `post_reopen_watch_duration` (default 20s)
            on_progress(ZaloMoveProgress {
                phase: ZaloMovePhase::HealthCheck,
                percent: 93,
                done_bytes: verification.total_bytes,
                total_bytes: verification.total_bytes,
                done_files: verification.total_files,
                total_files: verification.total_files,
                detail: "Đang kiểm tra ổn định sau khi mở lại Zalo...".to_string(),
            });

            let watch_start = Instant::now();
            let mut zalo_alive_all_ticks = true;
            let mut observed_ticks: u64 = 0;
            let mut verified_write_through_target = false;
            let mut observed_zalo_write = false;
            loop {
                let health = RecoveryService::check_single_junction_live(
                    &source_path,
                    &target_path,
                    reparse_api,
                );

                let probe_ok = if opts.skip_write_probe {
                    true
                } else {
                    match verify_junction_write_probe(&source_path, &target_path) {
                        Ok(()) => {
                            verified_write_through_target = true;
                            true
                        }
                        Err(_) => false,
                    }
                };

                if health != RelocationHealthStatus::Healthy || !probe_ok {
                    on_progress(ZaloMoveProgress {
                        phase: ZaloMovePhase::RollingBack,
                        percent: 95,
                        done_bytes: 0,
                        total_bytes: verification.total_bytes,
                        done_files: 0,
                        total_files: verification.total_files,
                        detail:
                            "Phát hiện liên kết hỏng sau khi mở Zalo, đang kiểm tra an toàn để hoàn tác..."
                                .to_string(),
                    });

                    let rollback_msg = Self::rollback_broken_zalo_relocation(
                        &mut record,
                        &moving_path,
                        src_drive,
                        opts.reopen_zalo,
                        &shutdown_session.captured_exes,
                        db,
                        volume_api,
                        reparse_api,
                        lock_probe,
                        proc_probe,
                        lifecycle,
                        opts.shutdown_timeout,
                        ctrl,
                    )?;
                    return Err(GreenCError::Relocation(rollback_msg));
                }

                // Item 5: verify a real Zalo process stays alive across EVERY tick of the watch!
                let zalo_alive_now = ZALO_PROCESS_NAMES.iter().any(|p| {
                    proc_probe.is_process_running(p)
                        || proc_probe.is_process_running(&p.to_lowercase())
                        || !lifecycle.find_running_exe_paths(&[p]).is_empty()
                });
                if !zalo_alive_now {
                    zalo_alive_all_ticks = false;
                } else if !observed_zalo_write {
                    // Item 2: verify Zalo itself actually wrote to target (excluding
                    // GreenC's own `.greenc-junction-probe-*` files).
                    let current_snapshot = snapshot_target_non_probe_files(&target_path);
                    if current_snapshot != pre_reopen_target_snapshot {
                        observed_zalo_write = true;
                    }
                }
                observed_ticks = observed_ticks.saturating_add(1);

                if watch_start.elapsed() >= opts.post_reopen_watch_duration {
                    break;
                }
                let remaining = opts
                    .post_reopen_watch_duration
                    .saturating_sub(watch_start.elapsed());
                std::thread::sleep(opts.post_reopen_poll_interval.min(remaining));
            }
            // `health_verified` is ONLY true when Zalo was actually reopened, stayed alive for
            // the entire watch duration, GreenC verified the junction works, AND Zalo itself
            // actually wrote to `target_path`!
            post_reopen_health_verified = reopened_zalo
                && observed_ticks > 0
                && zalo_alive_all_ticks
                && verified_write_through_target
                && observed_zalo_write;
        }

        // 13. Phase 4 (Item 1 & Item 11): Persist the retained-original row durably
        // BEFORE renaming `moving_path` -> `Zalo Data.greenc-original.<uuid>`, then update the
        // row with the renamed path so a crash at any instruction stays tracked!
        let now = Utc::now();
        let retention_hours = opts.original_retention_hours.max(1);
        let purge_after = now + ChronoDuration::hours(retention_hours);
        let original_path = make_sibling_path(&source_path, "greenc-original");

        let mut retained_rec = ZaloRetainedOriginal {
            id: None,
            relocation_id: rec_id,
            source_path: source_path.clone(),
            target_path: target_path.clone(),
            retained_path: moving_path.clone(),
            size_bytes: verification.total_bytes,
            file_count: verification.total_files,
            message_file_count: verification.message_files_hashed,
            health_verified: post_reopen_health_verified,
            created_at: now,
            purge_after,
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now),
            status: ZaloRetainedStatus::Retained,
            resolved_at: None,
            detail: Some("Đang hoàn tất đổi tên bản gốc giữ tạm...".to_string()),
        };
        let retained_id = db.save_zalo_retained_original_durable(&retained_rec)?;
        retained_rec.id = Some(retained_id);

        let (final_retained_path, retained_status, retained_detail) =
            match fs::rename(&moving_path, &original_path) {
                Ok(()) => (
                    original_path,
                    ZaloRetainedStatus::Retained,
                    if post_reopen_health_verified {
                        format!(
                            "Đã kiểm tra ổn định sau khi mở lại Zalo; bản gốc được giữ tạm {retention_hours} giờ trước khi cho phép dọn."
                        )
                    } else {
                        "Bản gốc được giữ tạm an toàn; chỉ xoá khi người dùng xác nhận.".to_string()
                    },
                ),
                Err(err) => (
                    moving_path.clone(),
                    ZaloRetainedStatus::NeedsReview,
                    format!(
                        "Không đổi tên được bản gốc sang .greenc-original ({err}); GreenC vẫn giữ nguyên tại {}.",
                        moving_path.display()
                    ),
                ),
            };

        retained_rec.retained_path = final_retained_path.clone();
        retained_rec.status = retained_status;
        retained_rec.detail = Some(retained_detail.clone());
        db.save_zalo_retained_original_durable(&retained_rec)?;

        record.status = RelocationStatus::Done;
        record.moving_path = None;
        record.health_status = RelocationHealthStatus::Healthy;
        record.detail = Some(format!(
            "Đã chuyển dữ liệu Zalo sang {target_drive_str}; bản gốc ({}) đang được giữ tạm tại {}",
            format_bytes_vi(verification.total_bytes),
            final_retained_path.display()
        ));
        db.save_relocation_durable(&record)?;

        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Done,
            percent: 100,
            done_bytes: verification.total_bytes,
            total_bytes: verification.total_bytes,
            done_files: verification.total_files,
            total_files: verification.total_files,
            detail: format!(
                "Đã chuyển dữ liệu Zalo sang {target_drive_str} (giữ tạm bản gốc trong {retention_hours} giờ)"
            ),
        });

        Ok(ZaloMoveReport {
            record_id: record.id,
            dry_run: false,
            already_on_other_drive: false,
            source_path,
            target_path,
            target_drive: target_drive_str,
            moved_bytes: verification.total_bytes,
            moved_files: verification.total_files,
            message_files_verified: verification.message_files_hashed,
            media_files_verified: verification.media_files_counted,
            reopened_zalo,
            health_verified: post_reopen_health_verified,
            auto_rolled_back: false,
            retained_original_id: Some(retained_id),
            retained_original_path: Some(final_retained_path),
            retained_original_bytes: verification.total_bytes,
            retained_purge_after: Some(purge_after.to_rfc3339()),
            space_estimate: Some(space_estimate),
            status: "done".to_string(),
            message: format!(
                "Đã chuyển xong dữ liệu Zalo sang ổ {target_letter}: (bản gốc {} đang được giữ tạm {retention_hours} giờ để đảm bảo an toàn).",
                format_bytes_vi(verification.total_bytes)
            ),
        })
    }

    /// Safe rollback when the junction breaks during the post-reopen health watch (Item 2):
    /// (a) Never ignores `graceful_shutdown` errors; verifies Zalo is fully closed & no file is
    ///     locked before touching anything — otherwise refuses rollback and preserves ALL copies.
    /// (b) Never deletes a real `source_path` directory; renames it to `Zalo Data.greenc-conflict.<uuid>`.
    /// (c) Uses the 24-hour retention undo path for `target_path` (renames to `.greenc-original.<uuid>`
    ///     and persists durably in DB instead of deleting immediately; NEVER deletes if rename fails).
    /// (d) Uses `target_path` as the latest copy so messages written during watch are preserved.
    ///     If `target_path` went offline, restores `moving_path` back to `source_path`.
    /// (e) Persists all state transitions durably and reopens Zalo after rollback.
    #[allow(clippy::too_many_arguments)]
    fn rollback_broken_zalo_relocation(
        record: &mut RelocationRecord,
        moving_path: &Path,
        source_drive: char,
        reopen_zalo: bool,
        fallback_exes: &[PathBuf],
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
        ctrl: &TaskControl,
    ) -> Result<String> {
        let source_path = record.source_path.clone();
        let target_path = record.target_path.clone();
        let rec_id = record.id.unwrap_or(0);

        // Helper to reopen Zalo after rollback completes or aborts post-shutdown.
        let reopen_after_rollback =
            |session_opt: &mut Option<greenc_platform::app_lifecycle::AppShutdownSession>| {
                if !reopen_zalo {
                    return;
                }
                if let Some(ref mut session) = session_opt {
                    if !session.captured_exes.is_empty() {
                        let _ = lifecycle.restart_app(session);
                        return;
                    }
                }
                if !fallback_exes.is_empty() {
                    let mut fallback =
                        greenc_platform::app_lifecycle::AppShutdownSession::with_captured_exes(
                            fallback_exes.to_vec(),
                            false,
                        );
                    let _ = lifecycle.restart_app(&mut fallback);
                }
            };

        // Helper to preserve `moving_path` as `.greenc-original.<uuid>` durably in DB.
        let preserve_moving_in_db = |note: &str| -> PathBuf {
            let now = Utc::now();
            let orig_candidate = make_sibling_path(&source_path, "greenc-original");
            let kept = if path_exists_lstat(moving_path) {
                match fs::rename(moving_path, &orig_candidate) {
                    Ok(()) => orig_candidate,
                    Err(_) => moving_path.to_path_buf(),
                }
            } else {
                moving_path.to_path_buf()
            };
            let (sz, fc) = CandidateScanner::compute_dir_size(&kept);
            let _ = db.save_zalo_retained_original_durable(&ZaloRetainedOriginal {
                id: None,
                relocation_id: rec_id,
                source_path: source_path.clone(),
                target_path: target_path.clone(),
                retained_path: kept.clone(),
                size_bytes: sz,
                file_count: fc,
                message_file_count: 0,
                health_verified: false,
                created_at: now,
                purge_after: now + ChronoDuration::hours(DEFAULT_ORIGINAL_RETENTION_HOURS),
                purge_after_valid: true,
                observed_healthy_secs: 0,
                last_observed_at: Some(now),
                status: ZaloRetainedStatus::NeedsReview,
                resolved_at: None,
                detail: Some(note.to_string()),
            });
            kept
        };

        // (a) Check active call, gracefully shut down Zalo, and verify NO process or file lock remains!
        let mut rollback_session: Option<greenc_platform::app_lifecycle::AppShutdownSession> = None;
        let refuse_reason = if lifecycle.has_visible_window(&["ZaloCall.exe"]) {
            Some(ZALO_ACTIVE_CALL_MESSAGE.to_string())
        } else {
            let shutdown_res = lifecycle.graceful_shutdown(
                &[
                    source_path.clone(),
                    target_path.clone(),
                    moving_path.to_path_buf(),
                ],
                ZALO_PROCESS_NAMES,
                shutdown_timeout,
            );
            match shutdown_res {
                Err(e) => Some(e.to_string()),
                Ok(session) => {
                    rollback_session = Some(session);
                    check_zalo_closed_and_unlocked(
                        &[&source_path, &target_path, moving_path],
                        proc_probe,
                        lifecycle,
                        lock_probe,
                        "Zalo vẫn đang chạy hoặc còn tệp dữ liệu bị khoá",
                    )
                    .err()
                    .map(|e| e.to_string())
                }
            }
        };

        if let Some(why) = refuse_reason {
            let kept_orig = preserve_moving_in_db(
                "Liên kết hỏng nhưng Zalo chưa đóng hoặc còn tệp khoá nên không tự hoàn tác",
            );
            let msg = format!(
                "Liên kết hỏng sau khi mở lại Zalo nhưng không thể đóng Zalo an toàn ({why}); GreenC KHÔNG tự hoàn tác và đã giữ nguyên tất cả bản dữ liệu: vị trí cũ ({}), bản gốc ({}), bản đích ({}). Hãy thoát hẳn Zalo rồi kiểm tra.",
                source_path.display(),
                kept_orig.display(),
                target_path.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            if rollback_session.is_some() {
                reopen_after_rollback(&mut rollback_session);
            }
            return Ok(msg);
        }

        // Case 1: Target drive went offline during health watch -> restore from kept `moving_path`!
        if !target_path.exists() {
            let mut conflict_path: Option<PathBuf> = None;
            if reparse_api.is_reparse_point(&source_path) {
                let _ = reparse_api.delete_junction(&source_path);
            } else if path_exists_lstat(&source_path) {
                let cp = make_sibling_path(&source_path, "greenc-conflict");
                if fs::rename(&source_path, &cp).is_ok() {
                    conflict_path = Some(cp);
                } else {
                    let kept_orig = preserve_moving_in_db(
                        "Ổ đích bị ngắt kết nối và không đổi tên được thư mục mới tại vị trí cũ",
                    );
                    let msg = format!(
                        "Ổ đích bị ngắt kết nối nhưng không thể di dời thư mục mới tại {}; GreenC giữ nguyên tất cả bản tại {} và {}.",
                        source_path.display(),
                        source_path.display(),
                        kept_orig.display()
                    );
                    record.status = RelocationStatus::NeedsReview;
                    record.health_status = RelocationHealthStatus::TargetOffline;
                    record.detail = Some(msg.clone());
                    let _ = db.save_relocation_durable(record);
                    reopen_after_rollback(&mut rollback_session);
                    return Ok(msg);
                }
            }

            if path_exists_lstat(moving_path) && fs::rename(moving_path, &source_path).is_ok() {
                let msg = if let Some(ref cp) = conflict_path {
                    format!(
                        "Ổ đích ({}) bị ngắt kết nối sau khi mở lại Zalo; GreenC đã tự hoàn tác bản gốc về {} và giữ thư mục mới tại {}. Giữ trạng thái Cần kiểm tra vì ổ đích ({}) có thể còn tin nhắn mới hơn khi kết nối lại.",
                        target_path.display(),
                        source_path.display(),
                        cp.display(),
                        target_path.display()
                    )
                } else {
                    format!(
                        "Ổ đích ({}) bị ngắt kết nối sau khi mở lại Zalo; GreenC đã tự hoàn tác bản gốc về {} và giữ trạng thái Cần kiểm tra vì ổ đích ({}) có thể còn tin nhắn mới hơn khi kết nối lại.",
                        target_path.display(),
                        source_path.display(),
                        target_path.display()
                    )
                };
                // Item 5: Keep `NeedsReview` + `TargetOffline` with target path (NEVER `Undone`)
                // so the offline target remains visible and discoverable when the drive reconnects.
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::TargetOffline;
                record.moving_path = None;
                record.detail = Some(msg.clone());
                db.save_relocation_durable(record)?;
                reopen_after_rollback(&mut rollback_session);
                return Ok(msg);
            }

            let kept_orig =
                preserve_moving_in_db("Ổ đích bị ngắt kết nối và không thể khôi phục tự động");
            let msg = format!(
                "Ổ đích bị ngắt kết nối và không thể hoàn tác tự động; GreenC đã giữ nguyên bản gốc tại {}.",
                kept_orig.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::TargetOffline;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            reopen_after_rollback(&mut rollback_session);
            return Ok(msg);
        }

        // Case 2: Target is online -> use `target_path` as the LATEST copy and follow the 24h
        // retention undo path (NEVER deletes target immediately or when rename fails!).
        let target_snapshot = match TreeFingerprint::capture(&target_path) {
            Ok(fp) => fp,
            Err(err) => {
                let kept_orig =
                    preserve_moving_in_db("Không chụp được trạng thái thư mục đích khi hoàn tác");
                let msg = format!(
                    "Liên kết hỏng và không đọc được thư mục đích ({err}); GreenC giữ nguyên bản gốc ({}) và bản đích ({}).",
                    kept_orig.display(),
                    target_path.display()
                );
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.detail = Some(msg.clone());
                let _ = db.save_relocation_durable(record);
                reopen_after_rollback(&mut rollback_session);
                return Ok(msg);
            }
        };
        let target_bytes = target_snapshot.total_bytes;
        if let Ok(free_c) = volume_api.get_free_space(source_drive) {
            if free_c < target_bytes.saturating_add(FREE_SPACE_RESERVE_BYTES) {
                let kept_orig = preserve_moving_in_db(
                    "Không đủ chỗ trống trên ổ nguồn để chép lại bản mới nhất từ ổ đích khi hoàn tác",
                );
                let msg = format!(
                    "Liên kết hỏng nhưng ổ {source_drive}: không đủ chỗ trống ({}) để chép lại bản mới nhất ({}) mà vẫn giữ bản gốc an toàn; GreenC KHÔNG xoá gì và giữ nguyên tất cả bản: bản gốc ({}), bản đích ({}).",
                    format_bytes_vi(free_c),
                    format_bytes_vi(target_bytes),
                    kept_orig.display(),
                    target_path.display()
                );
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.detail = Some(msg.clone());
                let _ = db.save_relocation_durable(record);
                reopen_after_rollback(&mut rollback_session);
                return Ok(msg);
            }
        }

        let source_parent = source_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(r"C:\"));
        let restore_tmp = source_parent.join(format!("zalo-restoring.{}", uuid::Uuid::new_v4()));

        // Copy target (latest) -> restore_tmp strictly, and verify BEFORE touching `source_path` or `target_path`!
        let copy_and_verify = (|| -> Result<()> {
            verified_copy_tree_strict(&target_path, &restore_tmp, ctrl, |_, _| {})?;
            verify_zalo_copy_integrity(&target_path, &restore_tmp)?;
            Ok(())
        })();

        if let Err(err) = copy_and_verify {
            remove_dir_all_safe(&restore_tmp);
            let kept_orig = preserve_moving_in_db("Sao chép hoàn tác từ đích bị lỗi");
            let msg = format!(
                "Liên kết hỏng và quá trình chép hoàn tác từ đích gặp lỗi ({err}); GreenC giữ nguyên tất cả bản: vị trí cũ ({}), bản gốc ({}), bản đích ({}).",
                source_path.display(),
                kept_orig.display(),
                target_path.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            reopen_after_rollback(&mut rollback_session);
            return Ok(msg);
        }

        // Immediately before swap: re-verify Zalo is still closed/unlocked and target unchanged.
        let pre_swap_ok = (|| -> Result<()> {
            check_zalo_closed_and_unlocked(
                &[&source_path, &target_path, moving_path, &restore_tmp],
                proc_probe,
                lifecycle,
                lock_probe,
                "Phát hiện Zalo vừa mở lại hoặc có tệp bị khoá ngay trước khi hoàn tác",
            )?;
            target_snapshot.verify_unchanged(&target_path)?;
            Ok(())
        })();
        if let Err(err) = pre_swap_ok {
            remove_dir_all_safe(&restore_tmp);
            let kept_orig =
                preserve_moving_in_db("Thư mục đích thay đổi hoặc bị khoá ngay trước khi hoàn tác");
            let msg = format!(
                "Không thể hoàn tác vì phát hiện thay đổi trước khi đổi chỗ ({err}); GreenC giữ nguyên tất cả bản: vị trí cũ ({}), bản gốc ({}), bản đích ({}).",
                source_path.display(),
                kept_orig.display(),
                target_path.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            reopen_after_rollback(&mut rollback_session);
            return Ok(msg);
        }

        // Persist durable `Moving` with `restore_tmp` BEFORE mutating `source_path`!
        record.status = RelocationStatus::Moving;
        record.moving_path = Some(restore_tmp.clone());
        record.detail = Some("Đang tự hoàn tác dữ liệu Zalo về vị trí cũ...".to_string());
        db.save_relocation_durable(record)?;

        // (b) Vacate `source_path` WITHOUT EVER calling `remove_dir_all(&source_path)`!
        // Any real directory/file created by Zalo is renamed to `Zalo Data.greenc-conflict.<uuid>`.
        let mut conflict_path: Option<PathBuf> = None;
        if reparse_api.is_reparse_point(&source_path) {
            if let Err(err) = reparse_api.delete_junction(&source_path) {
                remove_dir_all_safe(&restore_tmp);
                let kept_orig = preserve_moving_in_db("Không xoá được liên kết hỏng khi hoàn tác");
                let msg = format!(
                    "Không xoá được liên kết hỏng tại {} ({err}); GreenC giữ nguyên bản gốc ({}) và bản đích ({}).",
                    source_path.display(),
                    kept_orig.display(),
                    target_path.display()
                );
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.moving_path = None;
                record.detail = Some(msg.clone());
                let _ = db.save_relocation_durable(record);
                reopen_after_rollback(&mut rollback_session);
                return Ok(msg);
            }
        } else if path_exists_lstat(&source_path) {
            let cp = make_sibling_path(&source_path, "greenc-conflict");
            if let Err(err) = fs::rename(&source_path, &cp) {
                remove_dir_all_safe(&restore_tmp);
                let kept_orig =
                    preserve_moving_in_db("Không đổi tên được thư mục mới sang bản xung đột");
                let msg = format!(
                    "Không thể di dời thư mục mới tại {} sang bản xung đột ({err}); GreenC giữ nguyên tất cả bản: {}, {}, {}.",
                    source_path.display(),
                    source_path.display(),
                    kept_orig.display(),
                    target_path.display()
                );
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.moving_path = None;
                record.detail = Some(msg.clone());
                let _ = db.save_relocation_durable(record);
                reopen_after_rollback(&mut rollback_session);
                return Ok(msg);
            }
            conflict_path = Some(cp);
        }

        if let Err(err) = fs::rename(&restore_tmp, &source_path) {
            if let Some(ref cp) = conflict_path {
                let _ = fs::rename(cp, &source_path);
            }
            let kept_orig = preserve_moving_in_db("Không thể đổi tên bản khôi phục vào vị trí cũ");
            let msg = format!(
                "Không thể đặt bản khôi phục vào {} ({err}); GreenC giữ nguyên tất cả bản: {}, {}, {}.",
                source_path.display(),
                restore_tmp.display(),
                kept_orig.display(),
                target_path.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.moving_path = Some(restore_tmp.clone());
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            reopen_after_rollback(&mut rollback_session);
            return Ok(msg);
        }

        // (c) Re-verify restored `source_path` against `target_path` BEFORE touching `target_path`!
        if let Err(err) = verify_zalo_copy_integrity(&target_path, &source_path) {
            let kept_orig =
                preserve_moving_in_db("Kiểm tra toàn vẹn bản khôi phục sau hoàn tác không khớp");
            let msg = format!(
                "Kiểm tra toàn vẹn sau hoàn tác không khớp ({err}); GreenC giữ nguyên bản đích ({}) và bản gốc ({}).",
                target_path.display(),
                kept_orig.display()
            );
            record.status = RelocationStatus::NeedsReview;
            record.health_status = RelocationHealthStatus::LinkBroken;
            record.moving_path = None;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation_durable(record);
            reopen_after_rollback(&mut rollback_session);
            return Ok(msg);
        }

        // Item 7: Follow the 24-hour retention undo path for `target_path`!
        // Rename `target_path` -> `*.greenc-original.<uuid>` and persist durably in DB for 24h.
        // NEVER delete `target_path` immediately, and NEVER delete if rename fails!
        let now = Utc::now();
        let purge_after = now + ChronoDuration::hours(DEFAULT_ORIGINAL_RETENTION_HOURS);
        let target_retained = make_sibling_path(&target_path, "greenc-original");
        let (final_retained_target, target_retained_status, target_retain_note, target_rename_failed) =
            match fs::rename(&target_path, &target_retained) {
                Ok(()) => (
                    target_retained.clone(),
                    ZaloRetainedStatus::Retained,
                    format!(
                        " Bản sao trên ổ đích được đổi tên và giữ tạm 24 giờ tại {} để đảm bảo an toàn.",
                        target_retained.display()
                    ),
                    false,
                ),
                Err(rename_err) => (
                    target_path.clone(),
                    ZaloRetainedStatus::NeedsReview,
                    format!(
                        " Không đổi tên được thư mục đích {} ({rename_err}); GreenC vẫn giữ nguyên để tránh mất dữ liệu.",
                        target_path.display()
                    ),
                    true,
                ),
            };
        let _ = db.save_zalo_retained_original_durable(&ZaloRetainedOriginal {
            id: None,
            relocation_id: rec_id,
            source_path: source_path.clone(),
            target_path: target_path.clone(),
            retained_path: final_retained_target,
            size_bytes: target_snapshot.total_bytes,
            file_count: target_snapshot.total_files,
            message_file_count: 0,
            health_verified: false,
            created_at: now,
            purge_after,
            purge_after_valid: true,
            observed_healthy_secs: 0,
            last_observed_at: Some(now),
            status: target_retained_status,
            resolved_at: None,
            detail: Some(target_retain_note.trim().to_string()),
        });

        // Handle `moving_path`: if identical to restored `source_path`, clean ONLY if rename
        // succeeds; if rename fails or contents differ, preserve durably in DB!
        let mut preserved_original_note = String::new();
        if path_exists_lstat(moving_path) {
            if verify_zalo_copy_integrity(moving_path, &source_path).is_ok() {
                let moving_done = make_sibling_path(moving_path, "greenc-rollback-done");
                if fs::rename(moving_path, &moving_done).is_ok() {
                    remove_dir_all_safe(&moving_done);
                } else {
                    let kept_orig = preserve_moving_in_db(
                        "Không đổi tên được bản gốc tạm để dọn sau hoàn tác; giữ nguyên an toàn",
                    );
                    preserved_original_note =
                        format!(" Bản gốc tạm vẫn được giữ tại {}.", kept_orig.display());
                }
            } else {
                let kept_orig = preserve_moving_in_db(
                    "Bản gốc trước khi chuyển được giữ lại vì bản đích có tin nhắn mới hơn khi hoàn tác",
                );
                preserved_original_note = format!(
                    " Bản gốc trước khi chuyển vẫn được giữ tại {}.",
                    kept_orig.display()
                );
            }
        }

        let conflict_note = conflict_path
            .as_ref()
            .map(|cp| format!(" Thư mục mới do Zalo tạo được giữ tại {}.", cp.display()))
            .unwrap_or_default();

        let final_msg = format!(
            "Liên kết thư mục bị hỏng sau khi mở lại Zalo; GreenC đã tự hoàn tác bản dữ liệu mới nhất về {} an toàn.{conflict_note}{target_retain_note}{preserved_original_note}",
            source_path.display()
        );

        record.status = if target_rename_failed {
            RelocationStatus::NeedsReview
        } else {
            RelocationStatus::Undone
        };
        record.moving_path = None;
        record.health_status = RelocationHealthStatus::LinkBroken;
        record.detail = Some(final_msg.clone());
        db.save_relocation_durable(record)?;

        reopen_after_rollback(&mut rollback_session);
        Ok(final_msg)
    }

    /// Fail-closed pre-purge check (Item 2): compares `target_path` directly against
    /// the on-disk retained original (`rec.retained_path`):
    /// - Every DB in `message/` (excluding `-wal`/`-shm`/`-journal` sidecars) must exist on
    ///   target, must not be significantly smaller than the on-disk original, and must open
    ///   cleanly as a valid SQLite database (`PRAGMA quick_check(1)`).
    /// - Does NOT count `media/` files so deleting photos/videos inside Zalo never blocks purge.
    fn verify_target_intact_before_purge(rec: &ZaloRetainedOriginal) -> Result<()> {
        verify_live_zalo_covers_copy(&rec.target_path, &rec.retained_path, "zalo_purge_original")
    }

    /// Renames `rec.retained_path` to `<base>.greenc-purging.<uuid>` and durably persists
    /// `status = Purging` BEFORE calling `remove_dir_all`, so any crash or error mid-delete leaves
    /// an unambiguous half-purged marker that can NEVER be mistaken for a restorable backup.
    fn execute_safe_purge_locked(
        rec: &mut ZaloRetainedOriginal,
        db: &GreenCDb,
        done_msg: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let id = rec.id.ok_or_else(|| GreenCError::NotFound {
            entity: "ZaloRetainedOriginal".to_string(),
            id: "missing_id".to_string(),
        })?;
        if !is_purging_path(&rec.retained_path) {
            let base_name = rec
                .source_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Zalo Data");
            let parent = rec
                .retained_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(r"C:\"));
            let purging_path = parent.join(format!(
                "{base_name}.greenc-purging.{}",
                uuid::Uuid::new_v4()
            ));

            // Rename BEFORE persisting `Purging` so a crash prior to rename can never leave
            // an unrenamed `.greenc-original.*` stuck in `Purging` status.
            fs::rename(&rec.retained_path, &purging_path)?;

            rec.retained_path = purging_path;
            rec.status = ZaloRetainedStatus::Purging;
            rec.detail = Some("Đang xoá bản gốc giữ tạm (.greenc-purging)...".to_string());
            db.save_zalo_retained_original_durable(rec)?;
        } else if rec.status != ZaloRetainedStatus::Purging {
            rec.status = ZaloRetainedStatus::Purging;
            db.update_zalo_retained_original_status_durable(
                id,
                ZaloRetainedStatus::Purging.as_str(),
                "Đang hoàn tất xoá bản gốc dở (.greenc-purging)...",
            )?;
        }

        // Delete AFTER `.greenc-purging.*` rename + durable Purging status are committed.
        // If `remove_dir_all` fails mid-way, status STAYS `Purging` (never restorable!).
        if path_exists_lstat(&rec.retained_path) {
            fs::remove_dir_all(to_extended_io_path(&rec.retained_path))?;
        }

        db.update_zalo_retained_original_status_durable(
            id,
            ZaloRetainedStatus::Purged.as_str(),
            done_msg,
        )?;
        rec.status = ZaloRetainedStatus::Purged;
        rec.resolved_at = Some(now);
        rec.detail = Some(done_msg.to_string());
        Ok(())
    }

    /// Automatically purges retained `.greenc-original.<uuid>` directories ONLY when ALL safety
    /// conditions hold:
    /// (a) `health_verified == true` (the post-reopen health watch passed with live Zalo & write probe),
    /// (b) `purge_after_valid == true` AND `now >= purge_after` (unreadable `purge_after` NEVER expires),
    /// (c) accumulated real bounded healthy observation time >= required retention duration (clock
    ///     jumps forward cannot expire immediately!), AND
    /// (d) the relocation is still `Done`, live junction is `Healthy`, `target_path` & `message/`
    ///     exist, and live file counts are >= recorded counts.
    pub fn purge_expired_originals(
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        now: DateTime<Utc>,
    ) -> Result<Vec<ZaloRetainedOriginal>> {
        let Some(_reloc_lock) = RelocationLock::try_acquire_for_db(db.path())? else {
            return Ok(Vec::new());
        };
        let mut candidates = db.list_zalo_retained_originals(Some("retained"))?;
        // Also finish cleaning any interrupted `.greenc-purging.*` records!
        if let Ok(purging_rows) = db.list_zalo_retained_originals(Some("purging")) {
            candidates.extend(purging_rows);
        }
        let relocs = db.list_relocations(None)?;
        let mut purged = Vec::new();

        for mut rec in candidates {
            let Some(id) = rec.id else {
                continue;
            };
            if !path_exists_lstat(&rec.retained_path) {
                let final_st = if rec.status == ZaloRetainedStatus::Purging {
                    ZaloRetainedStatus::Purged
                } else {
                    ZaloRetainedStatus::Missing
                };
                db.update_zalo_retained_original_status_durable(
                    id,
                    final_st.as_str(),
                    if final_st == ZaloRetainedStatus::Purged {
                        "Đã hoàn tất dọn bản gốc giữ tạm"
                    } else {
                        "Thư mục bản gốc không còn tồn tại trên đĩa"
                    },
                )?;
                continue;
            }
            if rec.status == ZaloRetainedStatus::Purging {
                if is_purging_path(&rec.retained_path) {
                    let msg = "Đã hoàn tất dọn bản gốc xoá dở (.greenc-purging)";
                    if Self::execute_safe_purge_locked(&mut rec, db, msg, now).is_ok() {
                        purged.push(rec);
                    }
                    continue;
                }
                // Status was `Purging` but path was NEVER renamed to `.greenc-purging.*`:
                // revert durably to `Retained` and fall through to enforce ALL safety gates!
                let revert_msg =
                    "Trạng thái xoá chưa hoàn tất đổi tên sang .greenc-purging; đã trả về trạng thái giữ tạm để kiểm tra đủ điều kiện an toàn.";
                db.update_zalo_retained_original_status_durable(
                    id,
                    ZaloRetainedStatus::Retained.as_str(),
                    revert_msg,
                )?;
                rec.status = ZaloRetainedStatus::Retained;
                rec.detail = Some(revert_msg.to_string());
            }
            if !rec.health_verified || !rec.purge_after_valid {
                continue;
            }
            let reloc_ok = relocs.iter().any(|r| {
                r.id == Some(rec.relocation_id)
                    && r.status == RelocationStatus::Done
                    && RecoveryService::check_single_junction_live(
                        &r.source_path,
                        &r.target_path,
                        reparse_api,
                    ) == RelocationHealthStatus::Healthy
            });
            // Item 11: Cheap per-tick check ONLY (junction live + `target/message` dir
            // stat). Defer the heavy SQLite + full message DB verification until the record is
            // actually due so the 45-second Guardian tick stays lightweight!
            if !reloc_ok || !rec.target_path.exists() || !rec.target_path.join("message").is_dir() {
                continue;
            }

            // Accumulate bounded real healthy observation time so a sudden clock jump forward
            // cannot expire the retained original immediately.
            let prev = rec.last_observed_at.unwrap_or(rec.created_at);
            let delta_secs = (now - prev).num_seconds();
            if delta_secs > 0 {
                let credit = delta_secs.min(MAX_OBSERVATION_STEP_SECS) as u64;
                rec.observed_healthy_secs = rec.observed_healthy_secs.saturating_add(credit);
            }
            rec.last_observed_at = Some(now);
            let _ = db.update_zalo_retained_observation(id, rec.observed_healthy_secs, now);

            let required_observed_secs =
                (rec.purge_after - rec.created_at).num_seconds().max(3600) as u64;
            if now < rec.purge_after || rec.observed_healthy_secs < required_observed_secs {
                continue;
            }

            // Record IS due -> now run the full fail-closed SQLite + message DB check!
            if Self::verify_target_intact_before_purge(&rec).is_err() {
                continue;
            }

            let msg = "Đã tự động dọn bản gốc giữ tạm sau 24 giờ kiểm tra ổn định";
            if let Err(err) = Self::execute_safe_purge_locked(&mut rec, db, msg, now) {
                tracing::warn!(
                    "Could not auto-purge expired Zalo original {}: {err}",
                    rec.retained_path.display()
                );
                continue;
            }
            purged.push(rec);
        }
        Ok(purged)
    }

    /// Explicitly purges a retained `.greenc-original.<uuid>` copy at the user's request
    /// ("Xoá bản gốc"). Still refuses if `target_path` is missing, `message/` or file count
    /// dropped below recorded values, or the junction is unhealthy so the user cannot accidentally
    /// delete their last intact copy.
    pub fn purge_retained_original(
        retained_id: i64,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
    ) -> Result<ZaloRetainedOriginal> {
        let _reloc_lock = RelocationLock::acquire_for_db(db.path())?;
        let mut rec =
            db.get_zalo_retained_original(retained_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "ZaloRetainedOriginal".to_string(),
                    id: retained_id.to_string(),
                })?;

        if rec.status == ZaloRetainedStatus::Purged {
            return Ok(rec);
        }

        if !path_exists_lstat(&rec.retained_path) {
            let final_st = if rec.status == ZaloRetainedStatus::Purging {
                ZaloRetainedStatus::Purged
            } else {
                ZaloRetainedStatus::Missing
            };
            db.update_zalo_retained_original_status_durable(
                retained_id,
                final_st.as_str(),
                if final_st == ZaloRetainedStatus::Purged {
                    "Đã hoàn tất xoá bản gốc giữ tạm"
                } else {
                    "Thư mục bản gốc không còn tồn tại trên đĩa"
                },
            )?;
            rec.status = final_st;
            return Ok(rec);
        }

        // If DB says `Purging` but the path was NEVER renamed to `.greenc-purging.*`,
        // revert durably to `Retained` and run ALL safety gates before purging!
        if rec.status == ZaloRetainedStatus::Purging && !is_purging_path(&rec.retained_path) {
            let revert_msg =
                "Trạng thái xoá chưa đổi tên sang .greenc-purging; đã trả về trạng thái giữ tạm để kiểm tra đủ điều kiện an toàn.";
            db.update_zalo_retained_original_status_durable(
                retained_id,
                ZaloRetainedStatus::Retained.as_str(),
                revert_msg,
            )?;
            rec.status = ZaloRetainedStatus::Retained;
            rec.detail = Some(revert_msg.to_string());
        }

        // Only skip safety gates when the directory was ALREADY renamed to `.greenc-purging.*`
        // (i.e. a genuine half-purged copy).
        if !is_purging_path(&rec.retained_path) {
            let live_health = RecoveryService::check_single_junction_live(
                &rec.source_path,
                &rec.target_path,
                reparse_api,
            );
            if live_health != RelocationHealthStatus::Healthy || !rec.target_path.exists() {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "zalo_purge_original".to_string(),
                    reason: format!(
                        "Không thể xoá bản gốc vì dữ liệu Zalo ở ổ đích ({}) hoặc liên kết hiện không ổn định ({}). Hãy khôi phục bản gốc thay vì xoá.",
                        rec.target_path.display(),
                        live_health.as_str()
                    ),
                });
            }

            // Item 5: compare target `message/` against retained original!
            Self::verify_target_intact_before_purge(&rec)?;
        }

        let msg = "Người dùng đã xác nhận xoá bản gốc giữ tạm";
        Self::execute_safe_purge_locked(&mut rec, db, msg, Utc::now())?;
        Ok(rec)
    }

    /// Explicitly restores a retained `.greenc-original.<uuid>` copy back to `source_path`
    /// ("Khôi phục"), preserving any post-move `target_path` data as `.greenc-conflict.<uuid>`
    /// so zero messages can ever be lost. Refuses when `target_path` is offline unless the caller
    /// explicitly confirms via [`restore_retained_original_with_options`].
    #[allow(clippy::too_many_arguments)]
    pub fn restore_retained_original(
        retained_id: i64,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
    ) -> Result<ZaloRetainedOriginal> {
        Self::restore_retained_original_with_options(
            retained_id,
            false,
            db,
            reparse_api,
            lock_probe,
            proc_probe,
            lifecycle,
            shutdown_timeout,
        )
    }

    /// Explicitly restores a retained `.greenc-original.<uuid>` copy back to `source_path`.
    /// When `rec.target_path` is offline:
    /// - If `allow_offline_target == false`: REFUSES with `PreconditionBlocked` (newer messages
    ///   may exist on the offline drive).
    /// - If `allow_offline_target == true`: restores the retained copy AND keeps the relocation
    ///   in `NeedsReview` + `TargetOffline` naming `target_path` so the offline copy stays listed.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_retained_original_with_options(
        retained_id: i64,
        allow_offline_target: bool,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
    ) -> Result<ZaloRetainedOriginal> {
        let _reloc_lock = RelocationLock::acquire_for_db(db.path())?;
        let mut rec =
            db.get_zalo_retained_original(retained_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "ZaloRetainedOriginal".to_string(),
                    id: retained_id.to_string(),
                })?;

        // Item 6: A half-purged (`.greenc-purging.*` / `Purging`) or already `Purged` copy is
        // incomplete and MUST NEVER be restored over live chat data!
        if matches!(
            rec.status,
            ZaloRetainedStatus::Purging | ZaloRetainedStatus::Purged
        ) || is_purging_path(&rec.retained_path)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_original".to_string(),
                reason: "Bản gốc này đang bị xoá dở (.greenc-purging) nên không còn nguyên vẹn và không thể khôi phục.".to_string(),
            });
        }

        if !path_exists_lstat(&rec.retained_path) {
            return Err(GreenCError::NotFound {
                entity: "ZaloRetainedOriginalDir".to_string(),
                id: rec.retained_path.display().to_string(),
            });
        }

        if cfg!(feature = "store")
            && greenc_core::config::is_windows_appdata_app_dir(&rec.source_path)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_original".to_string(),
                reason: format!(
                    "Bản Microsoft Store không hỗ trợ tự động khôi phục thư mục Zalo trong AppData ('{}') do cơ chế ảo hoá MSIX; bản gốc ({}) và bản đích ({}) được giữ nguyên. Hãy dùng bản cài EXE hoặc khôi phục thủ công.",
                    rec.source_path.display(),
                    rec.retained_path.display(),
                    rec.target_path.display()
                ),
            });
        }

        if !greenc_scanner::zalo::has_valid_zalo_data_structure(&rec.retained_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_original".to_string(),
                reason: format!(
                    "Bản gốc '{}' thiếu thư mục message/ hợp lệ nên không thể khôi phục.",
                    rec.retained_path.display()
                ),
            });
        }

        // Item 5: Refuse restore when target is offline unless explicitly confirmed!
        let target_is_offline = !path_exists_lstat(&rec.target_path);
        if target_is_offline && !allow_offline_target {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_offline_target".to_string(),
                reason: format!(
                    "Ổ đích ({}) hiện đang ngoại tuyến và có thể còn tin nhắn mới hơn bản gốc. Hãy kết nối lại ổ đích trước, hoặc xác nhận rõ nếu bạn vẫn muốn khôi phục bản cũ.",
                    rec.target_path.display()
                ),
            });
        }

        if lifecycle.has_visible_window(&["ZaloCall.exe"]) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_original".to_string(),
                reason: ZALO_ACTIVE_CALL_MESSAGE.to_string(),
            });
        }

        let mut session = lifecycle.graceful_shutdown(
            &[
                rec.source_path.clone(),
                rec.target_path.clone(),
                rec.retained_path.clone(),
            ],
            ZALO_PROCESS_NAMES,
            shutdown_timeout,
        )?;

        let reopen_session = |sess: &mut greenc_platform::app_lifecycle::AppShutdownSession| {
            if !sess.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(sess);
            }
        };

        if let Err(err) = check_zalo_closed_and_unlocked(
            &[&rec.source_path, &rec.target_path, &rec.retained_path],
            proc_probe,
            lifecycle,
            lock_probe,
            ZALO_MANUAL_QUIT_MESSAGE,
        ) {
            reopen_session(&mut session);
            return Err(err);
        }

        // Persist durable intermediate state on BOTH `zalo_retained_originals` AND `relocations`
        // BEFORE touching the junction or renaming so a crash right after `delete_junction` is
        // always visible to `RecoveryService::recover_interrupted`!
        let prev_status = rec.status;
        db.update_zalo_retained_original_status_durable(
            retained_id,
            ZaloRetainedStatus::Restoring.as_str(),
            "Đang khôi phục bản gốc Zalo về vị trí cũ...",
        )?;

        type SavedRelocState = (
            RelocationRecord,
            RelocationStatus,
            RelocationHealthStatus,
            Option<String>,
            Option<PathBuf>,
        );
        let mut active_relocs_before: Vec<SavedRelocState> = Vec::new();
        if let Ok(relocs) = db.list_relocations(None) {
            for mut reloc in relocs {
                let matches_rec = reloc.id == Some(rec.relocation_id)
                    || (reloc.app_id.eq_ignore_ascii_case("zalo")
                        && greenc_core::models::same_windows_path(
                            &reloc.source_path,
                            &rec.source_path,
                        ));
                if matches_rec
                    && matches!(
                        reloc.status,
                        RelocationStatus::Done
                            | RelocationStatus::NeedsReview
                            | RelocationStatus::Moving
                    )
                {
                    let prev_st = reloc.status;
                    let prev_h = reloc.health_status;
                    let prev_d = reloc.detail.clone();
                    let prev_mp = reloc.moving_path.clone();
                    reloc.status = RelocationStatus::Moving;
                    reloc.moving_path = None;
                    reloc.detail = Some("Đang khôi phục bản gốc Zalo về vị trí cũ...".to_string());
                    db.save_relocation_durable(&reloc)?;
                    active_relocs_before.push((reloc, prev_st, prev_h, prev_d, prev_mp));
                }
            }
        }
        let rollback_relocs =
            |items: &[SavedRelocState], force_needs_review: bool, note: Option<&str>| {
                for (reloc, prev_st, prev_h, prev_d, prev_mp) in items {
                    let mut r = reloc.clone();
                    if force_needs_review {
                        r.status = RelocationStatus::NeedsReview;
                        r.health_status = RelocationHealthStatus::LinkBroken;
                        r.detail = note.map(str::to_string).or_else(|| prev_d.clone());
                    } else {
                        r.status = *prev_st;
                        r.health_status = *prev_h;
                        r.detail = prev_d.clone();
                    }
                    r.moving_path = prev_mp.clone();
                    let _ = db.save_relocation_durable(&r);
                }
            };

        let was_junction = reparse_api.is_reparse_point(&rec.source_path);
        let old_junction_target = if was_junction {
            reparse_api
                .get_junction_target(&rec.source_path)
                .unwrap_or_else(|_| rec.target_path.clone())
        } else {
            rec.target_path.clone()
        };

        let mut notes: Vec<String> = Vec::new();
        let mut moved_src_conflict: Option<PathBuf> = None;
        if was_junction {
            if let Err(err) = reparse_api.delete_junction(&rec.source_path) {
                let _ = db.update_zalo_retained_original_status_durable(
                    retained_id,
                    prev_status.as_str(),
                    &format!("Không xoá được liên kết trước khi khôi phục: {err}"),
                );
                rollback_relocs(&active_relocs_before, false, None);
                reopen_session(&mut session);
                return Err(err);
            }
        } else if path_exists_lstat(&rec.source_path) {
            let src_conflict = make_sibling_path(&rec.source_path, "greenc-conflict");
            if let Err(err) = fs::rename(&rec.source_path, &src_conflict) {
                let _ = db.update_zalo_retained_original_status_durable(
                    retained_id,
                    prev_status.as_str(),
                    &format!("Không đổi tên được thư mục hiện tại sang bản xung đột: {err}"),
                );
                rollback_relocs(&active_relocs_before, false, None);
                reopen_session(&mut session);
                return Err(GreenCError::Io(err));
            }
            notes.push(format!(
                "thư mục hiện tại tại vị trí cũ được giữ ở {}",
                src_conflict.display()
            ));
            moved_src_conflict = Some(src_conflict);
        }

        if let Err(rename_err) = fs::rename(&rec.retained_path, &rec.source_path) {
            // Rollback safely: rebuild the junction (or move back conflict dir) AND reopen Zalo!
            // If rebuilding the junction or returning the conflict fails, NEVER save `Done`!
            let mut rollback_ok = true;
            if was_junction {
                if let Err(j_err) =
                    reparse_api.create_junction(&rec.source_path, &old_junction_target)
                {
                    tracing::error!(
                        "Failed to rebuild junction {} -> {} after restore rename failure: {j_err}",
                        rec.source_path.display(),
                        old_junction_target.display()
                    );
                    rollback_ok = false;
                }
            } else if let Some(ref cp) = moved_src_conflict {
                if fs::rename(cp, &rec.source_path).is_err() {
                    rollback_ok = false;
                }
            }
            let rollback_status = if rollback_ok {
                prev_status
            } else {
                ZaloRetainedStatus::NeedsReview
            };
            let rb_msg = format!(
                "Không đổi tên được bản gốc về {} ({rename_err}); {}.",
                rec.source_path.display(),
                if rollback_ok {
                    "đã dựng lại liên kết cũ và giữ nguyên dữ liệu"
                } else {
                    "không dựng lại được liên kết cũ — cần người dùng kiểm tra"
                }
            );
            let _ = db.update_zalo_retained_original_status_durable(
                retained_id,
                rollback_status.as_str(),
                &rb_msg,
            );
            rollback_relocs(&active_relocs_before, !rollback_ok, Some(&rb_msg));
            reopen_session(&mut session);
            return Err(GreenCError::Io(rename_err));
        }

        // Never discard target_path (it may have messages written after move!): rename to conflict copy.
        let mut target_rename_failed = false;
        if target_is_offline {
            notes.push(format!(
                "ổ đích ({}) đang ngoại tuyến — giữ trạng thái Cần kiểm tra vì ổ đích có thể còn tin nhắn mới hơn khi kết nối lại",
                rec.target_path.display()
            ));
        } else if path_exists_lstat(&rec.target_path) {
            let tgt_conflict = make_sibling_path(&rec.target_path, "greenc-conflict");
            if fs::rename(&rec.target_path, &tgt_conflict).is_ok() {
                notes.push(format!(
                    "bản dữ liệu trên ổ đích được giữ lại tại {}",
                    tgt_conflict.display()
                ));
            } else {
                target_rename_failed = true;
                notes.push(format!(
                    "không đổi tên được bản dữ liệu trên ổ đích ({}); GreenC vẫn giữ nguyên để tránh mất dữ liệu",
                    rec.target_path.display()
                ));
            }
        }

        let detail = if notes.is_empty() {
            format!(
                "Đã khôi phục bản gốc về {} an toàn",
                rec.source_path.display()
            )
        } else {
            format!(
                "Đã khôi phục bản gốc về {}; {}",
                rec.source_path.display(),
                notes.join("; ")
            )
        };

        db.update_zalo_retained_original_status_durable(
            retained_id,
            ZaloRetainedStatus::Restored.as_str(),
            &detail,
        )?;

        if let Ok(relocs) = db.list_relocations(None) {
            if let Some(mut reloc) = relocs.into_iter().find(|r| r.id == Some(rec.relocation_id)) {
                if target_is_offline {
                    // Item 5: Keep NeedsReview + TargetOffline with target path so the
                    // offline target remains tracked and listed.
                    reloc.status = RelocationStatus::NeedsReview;
                    reloc.health_status = RelocationHealthStatus::TargetOffline;
                } else if target_rename_failed {
                    reloc.status = RelocationStatus::NeedsReview;
                    reloc.health_status = RelocationHealthStatus::Healthy;
                } else {
                    reloc.status = RelocationStatus::Undone;
                    reloc.health_status = RelocationHealthStatus::Healthy;
                }
                reloc.moving_path = None;
                reloc.detail = Some(detail.clone());
                let _ = db.save_relocation_durable(&reloc);
            }
        }

        reopen_session(&mut session);

        rec.status = ZaloRetainedStatus::Restored;
        rec.resolved_at = Some(Utc::now());
        rec.detail = Some(detail);
        Ok(rec)
    }

    /// Restores a Zalo safety copy strictly by its opaque `copy_id` returned from
    /// [`discover_zalo_safety_copies`]. Both copy path and target source path come ONLY from
    /// DB records / discovery — NEVER from untrusted IPC path arguments.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_safety_copy_by_id(
        copy_id: &str,
        discovered_data_dir: Option<&Path>,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
    ) -> Result<String> {
        Self::restore_safety_copy_by_id_with_options(
            copy_id,
            false,
            discovered_data_dir,
            db,
            reparse_api,
            lock_probe,
            proc_probe,
            lifecycle,
            shutdown_timeout,
        )
    }

    /// Variant of [`restore_safety_copy_by_id`] accepting `allow_offline_target` confirmation.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_safety_copy_by_id_with_options(
        copy_id: &str,
        allow_offline_target: bool,
        discovered_data_dir: Option<&Path>,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
    ) -> Result<String> {
        let copies = discover_zalo_safety_copies(discovered_data_dir, db);
        let found = copies
            .into_iter()
            .find(|c| c.copy_id == copy_id)
            .ok_or_else(|| GreenCError::NotFound {
                entity: "ZaloSafetyCopy".to_string(),
                id: copy_id.to_string(),
            })?;

        if found.kind == "purging" || found.status == "purging" || found.status == "purged" {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: "Bản sao này đang bị xoá dở (.greenc-purging) nên không còn nguyên vẹn và không thể khôi phục.".to_string(),
            });
        }
        if found.kind == "restoring" {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: "Thư mục khôi phục dở (zalo-restoring) có thể chưa đầy đủ dữ liệu nên không được khôi phục tự động.".to_string(),
            });
        }
        if !found.exists_on_disk {
            return Err(GreenCError::NotFound {
                entity: "ZaloSafetyCopyDir".to_string(),
                id: found.path.display().to_string(),
            });
        }
        if !greenc_scanner::zalo::has_valid_zalo_data_structure(&found.path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: format!(
                    "Bản sao '{}' thiếu thư mục message/ hợp lệ nên không thể khôi phục.",
                    found.path.display()
                ),
            });
        }

        if let Some(rid) = found.id {
            let restored = Self::restore_retained_original_with_options(
                rid,
                allow_offline_target,
                db,
                reparse_api,
                lock_probe,
                proc_probe,
                lifecycle,
                shutdown_timeout,
            )?;
            return Ok(restored
                .detail
                .unwrap_or_else(|| "Đã khôi phục bản sao Zalo về vị trí cũ an toàn.".to_string()));
        }

        let source_path = found
            .source_path
            .or_else(|| discovered_data_dir.map(Path::to_path_buf))
            .ok_or_else(|| GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason:
                    "Không xác định được thư mục dữ liệu Zalo gốc từ bản ghi hoặc cấu hình Zalo."
                        .to_string(),
            })?;

        Self::restore_safety_copy_internal(
            &found.path,
            &source_path,
            found.kind == "target",
            allow_offline_target,
            db,
            reparse_api,
            lock_probe,
            proc_probe,
            lifecycle,
            shutdown_timeout,
            None,
        )
    }

    /// Confirmed deletion for ANY listed Zalo safety copy (`original`, `conflict`, `moving`,
    /// `restoring`, `target`, or leftover `purging` — tracked or untracked) strictly by its opaque
    /// `copy_id` returned from [`discover_zalo_safety_copies`] (Item 9).
    ///
    /// Safety invariant:
    /// - NEVER accepts a raw filesystem path from IPC; resolves `copy_id` via DB/discovery.
    /// - For non-purging copies, resolves the currently LIVE Zalo data directory (either the
    ///   healthy junction target or the live real source directory) and refuses unless the live
    ///   directory is distinct from `copy` AND passes [`verify_live_zalo_covers_copy`] (every
    ///   SQLite DB in `message/` exists on the live copy, is non-truncated, and opens cleanly).
    /// - Always renames to `.greenc-purging.<uuid>` BEFORE removing files so partial deletion
    ///   can never be mistaken for an intact backup.
    pub fn delete_safety_copy_by_id(
        copy_id: &str,
        discovered_data_dir: Option<&Path>,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
    ) -> Result<String> {
        let _reloc_lock = RelocationLock::try_acquire_for_db(db.path())?.ok_or_else(|| {
            GreenCError::PreconditionBlocked {
                operation: "zalo_delete_safety_copy".to_string(),
                reason: "Một tác vụ di chuyển/khôi phục GreenC khác đang chạy; không thể xoá bản sao lúc này.".to_string(),
            }
        })?;

        let copies = discover_zalo_safety_copies(discovered_data_dir, db);
        let found = copies
            .into_iter()
            .find(|c| c.copy_id == copy_id)
            .ok_or_else(|| GreenCError::NotFound {
                entity: "ZaloSafetyCopy".to_string(),
                id: copy_id.to_string(),
            })?;

        if !found.exists_on_disk {
            if let Some(rid) = found.id {
                if let Some(mut rec) = db.get_zalo_retained_original(rid)? {
                    if rec.status == ZaloRetainedStatus::Purging {
                        rec.status = ZaloRetainedStatus::Purged;
                        rec.resolved_at = Some(Utc::now());
                        rec.detail = Some(
                            "Thư mục .greenc-purging đã không còn trên đĩa; đánh dấu đã dọn xong."
                                .to_string(),
                        );
                        db.save_zalo_retained_original_durable(&rec)?;
                        return Ok(rec.detail.unwrap_or_default());
                    }
                }
            }
            return Err(GreenCError::NotFound {
                entity: "ZaloSafetyCopyDir".to_string(),
                id: found.path.display().to_string(),
            });
        }

        // Case 1: Already renamed to `.greenc-purging.*` -> finish removing the half-deleted dir.
        if is_purging_path(&found.path) {
            if let Some(rid) = found.id {
                if let Some(mut rec) = db.get_zalo_retained_original(rid)? {
                    Self::execute_safe_purge_locked(
                        &mut rec,
                        db,
                        "Đã dọn nốt thư mục xoá dở (.greenc-purging) theo xác nhận của người dùng.",
                        Utc::now(),
                    )?;
                    return Ok(rec
                        .detail
                        .unwrap_or_else(|| "Đã xoá nốt bản sao Zalo.".to_string()));
                }
            }
            fs::remove_dir_all(to_extended_io_path(&found.path))?;
            return Ok(format!(
                "Đã xoá nốt thư mục xoá dở tại {}.",
                found.path.display()
            ));
        }

        // Case 2 (Item 1): If DB row was left in `Purging` without `.greenc-purging.*`,
        // revert to `Retained` durably before running full live-source verification gates.
        let mut tracked_row = match found.id {
            Some(rid) => db.get_zalo_retained_original(rid)?,
            None => None,
        };
        if let Some(ref mut rec) = tracked_row {
            if rec.status == ZaloRetainedStatus::Purging && !is_purging_path(&rec.retained_path) {
                rec.status = ZaloRetainedStatus::Retained;
                rec.detail = Some(
                    "Phát hiện trạng thái Purging chưa đổi tên .greenc-purging; khôi phục về Retained và chạy đủ cổng kiểm tra."
                        .to_string(),
                );
                db.save_zalo_retained_original_durable(rec)?;
            }
        }

        // Determine the currently LIVE Zalo data directory to verify against.
        let source_path = found
            .source_path
            .as_deref()
            .or(discovered_data_dir)
            .ok_or_else(|| GreenCError::PreconditionBlocked {
                operation: "zalo_delete_safety_copy".to_string(),
                reason: "Không xác định được thư mục dữ liệu Zalo đang hoạt động để đối chiếu; từ chối xoá bản sao.".to_string(),
            })?;

        let live_dir = if reparse_api.is_reparse_point(source_path) {
            let target = reparse_api.get_junction_target(source_path).map_err(|_| {
                GreenCError::PreconditionBlocked {
                    operation: "zalo_delete_safety_copy".to_string(),
                    reason: format!(
                        "Không đọc được đích của liên kết Zalo tại {}; từ chối xoá bản sao.",
                        source_path.display()
                    ),
                }
            })?;
            if !target.exists() || !target.is_dir() {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "zalo_delete_safety_copy".to_string(),
                    reason: format!(
                        "Liên kết Zalo trỏ tới {} nhưng thư mục đích không tồn tại hoặc ngoại tuyến; từ chối xoá bản sao.",
                        target.display()
                    ),
                });
            }
            target
        } else if source_path.exists() && source_path.is_dir() {
            source_path.to_path_buf()
        } else {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_delete_safety_copy".to_string(),
                reason: format!(
                    "Không tìm thấy thư mục Zalo đang hoạt động tại {}; từ chối xoá bản sao để bảo vệ dữ liệu duy nhất.",
                    source_path.display()
                ),
            });
        };

        if greenc_core::models::same_windows_path(&live_dir, &found.path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_delete_safety_copy".to_string(),
                reason: "Không thể xoá thư mục dữ liệu Zalo đang hoạt động.".to_string(),
            });
        }

        // Fail-closed live-source coverage verification:
        // - If the safety copy has `message/`, EVERY message SQLite DB in it must exist in
        //   `live_dir/message/`, be non-truncated, and open cleanly as SQLite.
        // - If the safety copy is partial/lacks `message/` (e.g. aborted `zalo-restoring.*`),
        //   still verify `live_dir/message/` itself exists with valid openable SQLite DBs!
        if found.path.join("message").is_dir() {
            verify_live_zalo_covers_copy(&live_dir, &found.path, "zalo_delete_safety_copy")?;
        } else {
            verify_live_zalo_covers_copy(&live_dir, &live_dir, "zalo_delete_safety_copy")?;
        }

        // Safe two-phase delete (rename to `.greenc-purging.<uuid>` FIRST, then remove).
        let success_note = format!(
            "Đã xoá bản sao {} sau khi xác nhận người dùng và đối chiếu đầy đủ với nguồn sống ({}).",
            found.path.display(),
            live_dir.display()
        );
        if let Some(mut rec) = tracked_row {
            Self::execute_safe_purge_locked(&mut rec, db, &success_note, Utc::now())?;
            return Ok(success_note);
        }

        let purging_path = make_sibling_path(&found.path, "greenc-purging");
        fs::rename(&found.path, &purging_path)?;
        fs::remove_dir_all(to_extended_io_path(&purging_path))?;
        Ok(success_note)
    }

    /// Restores ANY valid Zalo safety copy (`original`, `conflict`, or `moving` —
    /// tracked or untracked on disk) back to `target_source_path`, while preserving any existing
    /// directory at `target_source_path` or linked target as `.greenc-conflict.<uuid>`.
    ///
    /// Explicitly REFUSES `.greenc-purging.*`, partial `zalo-restoring.*`/`restoring.*`,
    /// directories without `message/`, or DB rows in `Purging`/`Purged` status!
    #[allow(clippy::too_many_arguments)]
    pub fn restore_safety_copy_by_path(
        copy_path: &Path,
        target_source_path: &Path,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
    ) -> Result<String> {
        Self::restore_safety_copy_internal(
            copy_path,
            target_source_path,
            false,
            false,
            db,
            reparse_api,
            lock_probe,
            proc_probe,
            lifecycle,
            shutdown_timeout,
            None,
        )
    }

    /// Internal / testable variant of [`restore_safety_copy_by_path`] with an optional
    /// `before_swap_hook` invoked immediately before the pre-swap Zalo-closed & fingerprint check.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_safety_copy_by_path_with_hook(
        copy_path: &Path,
        target_source_path: &Path,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
        before_swap_hook: Option<&dyn Fn()>,
    ) -> Result<String> {
        Self::restore_safety_copy_internal(
            copy_path,
            target_source_path,
            false,
            false,
            db,
            reparse_api,
            lock_probe,
            proc_probe,
            lifecycle,
            shutdown_timeout,
            before_swap_hook,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn restore_safety_copy_internal(
        copy_path: &Path,
        target_source_path: &Path,
        allow_registered_target_copy: bool,
        allow_offline_target: bool,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        lock_probe: &dyn FileLockProbe,
        proc_probe: &dyn ProcessServiceProbe,
        lifecycle: &dyn AppLifecycleApi,
        shutdown_timeout: Duration,
        before_swap_hook: Option<&dyn Fn()>,
    ) -> Result<String> {
        // Validate copy filename is an allowed Zalo safety copy (NEVER `.greenc-purging.*` or partial `restoring.*`).
        let fname = copy_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let kind = classify_safety_copy_name(fname);
        if is_purging_path(copy_path) || matches!(kind, Some("purging")) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: "Bản sao này đang bị xoá dở (.greenc-purging) nên không còn nguyên vẹn và không thể khôi phục.".to_string(),
            });
        }
        if matches!(kind, Some("restoring")) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: "Thư mục khôi phục dở (zalo-restoring) có thể chưa đầy đủ dữ liệu nên không được khôi phục tự động.".to_string(),
            });
        }
        if !allow_registered_target_copy
            && !matches!(kind, Some("original") | Some("conflict") | Some("moving"))
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: format!(
                    "Thư mục '{}' không phải bản sao an toàn hợp lệ của Zalo",
                    copy_path.display()
                ),
            });
        }

        // If this path matches a tracked DB row, check its status and delegate to the tracked flow.
        if let Ok(rows) = db.list_zalo_retained_originals(None) {
            if let Some(row) = rows
                .into_iter()
                .find(|r| greenc_core::models::same_windows_path(&r.retained_path, copy_path))
            {
                if let Some(rid) = row.id {
                    let restored = Self::restore_retained_original_with_options(
                        rid,
                        allow_offline_target,
                        db,
                        reparse_api,
                        lock_probe,
                        proc_probe,
                        lifecycle,
                        shutdown_timeout,
                    )?;
                    return Ok(restored.detail.unwrap_or_else(|| {
                        "Đã khôi phục bản sao Zalo về vị trí cũ an toàn.".to_string()
                    }));
                }
            }
        }

        let _reloc_lock = RelocationLock::acquire_for_db(db.path())?;

        let copy_io = to_extended_io_path(copy_path);
        let copy_meta = fs::symlink_metadata(&copy_io).map_err(|_| GreenCError::NotFound {
            entity: "ZaloSafetyCopyDir".to_string(),
            id: copy_path.display().to_string(),
        })?;
        if !copy_meta.is_dir()
            || copy_meta.file_type().is_symlink()
            || reparse_api.is_reparse_point(copy_path)
        {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: format!(
                    "Bản sao '{}' phải là thư mục thật (không phải liên kết)",
                    copy_path.display()
                ),
            });
        }

        // Mandatory authentic Zalo structure (`message/` directory must exist).
        if !greenc_scanner::zalo::has_valid_zalo_data_structure(copy_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: format!(
                    "Bản sao '{}' thiếu thư mục message/ hợp lệ của Zalo nên không thể khôi phục.",
                    copy_path.display()
                ),
            });
        }

        if lifecycle.has_visible_window(&["ZaloCall.exe"]) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_safety_copy".to_string(),
                reason: ZALO_ACTIVE_CALL_MESSAGE.to_string(),
            });
        }

        // Resolve linked target BEFORE shutdown/lock check so we check locks on the ENTIRE
        // target tree (not stopping at the symlink/junction at `target_source_path`!).
        let was_junction = reparse_api.is_reparse_point(target_source_path);
        let old_junction_target = if was_junction {
            reparse_api.get_junction_target(target_source_path).ok()
        } else {
            None
        };

        // Item 5: If a linked target is offline, refuse unless explicitly confirmed!
        let linked_target_offline = old_junction_target.as_ref().is_some_and(|jt| {
            !greenc_core::models::same_windows_path(jt, copy_path) && !path_exists_lstat(jt)
        });
        if linked_target_offline && !allow_offline_target {
            let offline_disp = old_junction_target
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_restore_offline_target".to_string(),
                reason: format!(
                    "Ổ đích ({offline_disp}) hiện đang ngoại tuyến và có thể còn tin nhắn mới hơn. Hãy kết nối lại ổ đích trước, hoặc xác nhận rõ nếu bạn vẫn muốn khôi phục bản sao này."
                ),
            });
        }

        let mut shutdown_resources =
            vec![target_source_path.to_path_buf(), copy_path.to_path_buf()];
        if let Some(ref jt) = old_junction_target {
            shutdown_resources.push(jt.clone());
        }
        let mut session = lifecycle.graceful_shutdown(
            &shutdown_resources,
            ZALO_PROCESS_NAMES,
            shutdown_timeout,
        )?;

        let reopen_session = |sess: &mut greenc_platform::app_lifecycle::AppShutdownSession| {
            if !sess.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(sess);
            }
        };

        let mut lock_paths: Vec<&Path> = vec![target_source_path, copy_path];
        if let Some(ref jt) = old_junction_target {
            lock_paths.push(jt.as_path());
        }
        if let Err(err) = check_zalo_closed_and_unlocked(
            &lock_paths,
            proc_probe,
            lifecycle,
            lock_probe,
            ZALO_MANUAL_QUIT_MESSAGE,
        ) {
            reopen_session(&mut session);
            return Err(err);
        }

        // Capture fail-closed fingerprints BEFORE any staging copy so we can verify zero drift
        // right before swapping!
        let copy_snapshot = match TreeFingerprint::capture(copy_path) {
            Ok(fp) => fp,
            Err(err) => {
                reopen_session(&mut session);
                return Err(err);
            }
        };
        let target_snapshot = if let Some(ref jt) = old_junction_target {
            if !greenc_core::models::same_windows_path(jt, copy_path) && path_exists_lstat(jt) {
                match TreeFingerprint::capture(jt) {
                    Ok(fp) => Some((jt.clone(), fp)),
                    Err(err) => {
                        reopen_session(&mut session);
                        return Err(err);
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // If `copy_path` is on a different volume from `target_source_path`, stage a verified
        // strict copy in `target_source_path`'s parent first so the final swap is an atomic rename.
        let source_parent = target_source_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(r"C:\"));
        let copy_parent = copy_path.parent().map(Path::to_path_buf);
        let same_parent = copy_parent
            .as_deref()
            .is_some_and(|p| greenc_core::models::same_windows_path(p, &source_parent));

        let mut staged_copy: Option<PathBuf> = None;
        let restore_src = if same_parent {
            copy_path.to_path_buf()
        } else {
            let tmp_stage = source_parent.join(format!("zalo-restoring.{}", uuid::Uuid::new_v4()));
            let ctrl = TaskControl::new();
            let copy_ok = (|| -> Result<()> {
                verified_copy_tree_strict(copy_path, &tmp_stage, &ctrl, |_, _| {})?;
                verify_zalo_copy_integrity(copy_path, &tmp_stage)?;
                Ok(())
            })();
            if let Err(err) = copy_ok {
                remove_dir_all_safe(&tmp_stage);
                reopen_session(&mut session);
                return Err(err);
            }
            staged_copy = Some(tmp_stage.clone());
            tmp_stage
        };

        if let Some(hook) = before_swap_hook {
            hook();
        }

        // Immediately BEFORE swap: re-verify Zalo is STILL closed/unlocked across ALL trees
        // AND neither `copy_path` nor linked `target` drifted since snapshot (just like undo)!
        let pre_swap_check = (|| -> Result<()> {
            let mut pre_swap_paths: Vec<&Path> = vec![target_source_path, copy_path];
            if let Some(ref jt) = old_junction_target {
                pre_swap_paths.push(jt.as_path());
            }
            if let Some(ref st) = staged_copy {
                pre_swap_paths.push(st.as_path());
            }
            check_zalo_closed_and_unlocked(
                &pre_swap_paths,
                proc_probe,
                lifecycle,
                lock_probe,
                "Phát hiện Zalo vừa mở lại hoặc có tệp bị khoá ngay trước khi khôi phục; GreenC đã dừng an toàn.",
            )?;
            copy_snapshot.verify_unchanged(copy_path)?;
            if let Some((ref jt, ref fp)) = target_snapshot {
                fp.verify_unchanged(jt)?;
            }
            Ok(())
        })();
        if let Err(err) = pre_swap_check {
            if let Some(ref st) = staged_copy {
                remove_dir_all_safe(st);
            }
            reopen_session(&mut session);
            return Err(err);
        }

        // Persist durable intermediate state BEFORE touching junction or renaming!
        let mut active_relocs_before: Vec<(RelocationRecord, RelocationStatus, Option<String>)> =
            Vec::new();
        if let Ok(relocs) = db.list_relocations(None) {
            for mut reloc in relocs {
                if reloc.app_id.eq_ignore_ascii_case("zalo")
                    && greenc_core::models::same_windows_path(
                        &reloc.source_path,
                        target_source_path,
                    )
                    && matches!(
                        reloc.status,
                        RelocationStatus::Done
                            | RelocationStatus::NeedsReview
                            | RelocationStatus::Moving
                    )
                {
                    let prev_st = reloc.status;
                    let prev_detail = reloc.detail.clone();
                    reloc.status = RelocationStatus::Moving;
                    reloc.detail = Some("Đang khôi phục bản sao an toàn Zalo...".to_string());
                    db.save_relocation_durable(&reloc)?;
                    active_relocs_before.push((reloc, prev_st, prev_detail));
                }
            }
        }
        let rollback_active_relocs =
            |items: &[(RelocationRecord, RelocationStatus, Option<String>)],
             force_needs_review: bool| {
                for (reloc, prev_st, prev_detail) in items {
                    let mut r = reloc.clone();
                    r.status = if force_needs_review {
                        RelocationStatus::NeedsReview
                    } else {
                        *prev_st
                    };
                    r.detail = prev_detail.clone();
                    let _ = db.save_relocation_durable(&r);
                }
            };

        let mut notes: Vec<String> = Vec::new();
        let mut moved_src_conflict: Option<PathBuf> = None;

        if was_junction {
            if let Err(err) = reparse_api.delete_junction(target_source_path) {
                if let Some(ref st) = staged_copy {
                    remove_dir_all_safe(st);
                }
                rollback_active_relocs(&active_relocs_before, false);
                reopen_session(&mut session);
                return Err(err);
            }
        } else if path_exists_lstat(target_source_path) {
            let src_conflict = make_sibling_path(target_source_path, "greenc-conflict");
            if let Err(err) = fs::rename(target_source_path, &src_conflict) {
                if let Some(ref st) = staged_copy {
                    remove_dir_all_safe(st);
                }
                rollback_active_relocs(&active_relocs_before, false);
                reopen_session(&mut session);
                return Err(GreenCError::Io(err));
            }
            notes.push(format!(
                "thư mục hiện tại tại vị trí cũ được giữ ở {}",
                src_conflict.display()
            ));
            moved_src_conflict = Some(src_conflict);
        }

        if let Err(rename_err) = fs::rename(&restore_src, target_source_path) {
            let mut rollback_ok = true;
            if was_junction {
                if let Some(ref jt) = old_junction_target {
                    if reparse_api.create_junction(target_source_path, jt).is_err() {
                        rollback_ok = false;
                    }
                }
            } else if let Some(ref cp) = moved_src_conflict {
                if fs::rename(cp, target_source_path).is_err() {
                    rollback_ok = false;
                }
            }
            if let Some(ref st) = staged_copy {
                remove_dir_all_safe(st);
            }
            rollback_active_relocs(&active_relocs_before, !rollback_ok);
            reopen_session(&mut session);
            return Err(GreenCError::Io(rename_err));
        }

        // Preserve any linked target data as `.greenc-conflict.<uuid>` if it wasn't `copy_path` itself.
        // NEVER swallow rename failure: mark active relocation(s) as `NeedsReview` and return Err!
        let mut target_rename_error: Option<String> = None;
        if linked_target_offline {
            if let Some(ref jt) = old_junction_target {
                notes.push(format!(
                    "ổ đích ({}) đang ngoại tuyến — giữ trạng thái Cần kiểm tra vì ổ đích có thể còn tin nhắn mới hơn khi kết nối lại",
                    jt.display()
                ));
            }
        } else if let Some(ref jt) = old_junction_target {
            if !greenc_core::models::same_windows_path(jt, copy_path) && path_exists_lstat(jt) {
                let tgt_conflict = make_sibling_path(jt, "greenc-conflict");
                match fs::rename(jt, &tgt_conflict) {
                    Ok(()) => {
                        notes.push(format!(
                            "bản dữ liệu trên ổ đích được giữ lại tại {}",
                            tgt_conflict.display()
                        ));
                    }
                    Err(err) => {
                        let msg = format!(
                            "không đổi tên được thư mục đích {} sang bản xung đột ({err}); GreenC giữ nguyên thư mục đích để tránh mất dữ liệu",
                            jt.display()
                        );
                        notes.push(msg.clone());
                        target_rename_error = Some(msg);
                    }
                }
            }
        }

        if staged_copy.is_some() {
            notes.push(format!(
                "bản sao nguồn tại {} vẫn được giữ nguyên",
                copy_path.display()
            ));
        }

        let detail = if notes.is_empty() {
            format!(
                "Đã khôi phục bản sao Zalo về {} an toàn",
                target_source_path.display()
            )
        } else {
            format!(
                "Đã khôi phục bản sao Zalo về {}; {}",
                target_source_path.display(),
                notes.join("; ")
            )
        };

        let final_reloc_status = if target_rename_error.is_some() || linked_target_offline {
            RelocationStatus::NeedsReview
        } else {
            RelocationStatus::Undone
        };
        let final_health = if linked_target_offline {
            RelocationHealthStatus::TargetOffline
        } else {
            RelocationHealthStatus::Healthy
        };

        // Mark any active Zalo relocation record for `target_source_path` durably
        if let Ok(relocs) = db.list_relocations(None) {
            for mut reloc in relocs {
                if reloc.app_id.eq_ignore_ascii_case("zalo")
                    && greenc_core::models::same_windows_path(
                        &reloc.source_path,
                        target_source_path,
                    )
                    && matches!(
                        reloc.status,
                        RelocationStatus::Done
                            | RelocationStatus::NeedsReview
                            | RelocationStatus::Moving
                    )
                {
                    reloc.status = final_reloc_status;
                    reloc.health_status = final_health;
                    reloc.moving_path = None;
                    reloc.detail = Some(detail.clone());
                    let _ = db.save_relocation_durable(&reloc);
                }
            }
        }

        reopen_session(&mut session);
        if target_rename_error.is_some() {
            return Err(GreenCError::Relocation(detail));
        }
        Ok(detail)
    }

    /// Dedicated safe Zalo Undo path (Item 2, ):
    /// Assumes the caller ALREADY holds `RelocationLock` for `db.path()`.
    /// 1. Refuses if a Zalo call is active; gracefully shuts down Zalo (if `lifecycle` provided)
    ///    and verifies all Zalo processes are closed and files unlocked.
    /// 2. Checks free space on source drive using live target size + 512 MiB reserve.
    /// 3. Captures `TreeFingerprint` (file count, sizes, mtimes) of `target_path`.
    /// 4. Copies `target_path` -> `zalo-restoring.<uuid>` via `verified_copy_tree_strict` and runs
    ///    full `verify_zalo_copy_integrity`.
    /// 5. Immediately before swap: re-checks Zalo is still closed/unlocked AND `target_path`
    ///    size/mtime/file-count is completely unchanged; aborts safely on any mismatch.
    /// 6. Swaps junction -> restored directory (re-creating junction on rename failure; moving any
    ///    unexpected real dir at `source_path` to `.greenc-conflict.<uuid>`).
    /// 7. Renames `target_path` -> `*.greenc-original.<uuid>` and retains it for 24h in DB instead
    ///    of deleting immediately; never swallows deletion/rename errors.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn undo_zalo_relocation_locked(
        record: &mut RelocationRecord,
        source_drive: char,
        db: &GreenCDb,
        volume_api: &dyn Win32VolumeApi,
        reparse_api: &dyn ReparsePointApi,
        proc_probe: &dyn ProcessServiceProbe,
        lock_probe: &dyn FileLockProbe,
        lifecycle: Option<&dyn AppLifecycleApi>,
        ctrl: &TaskControl,
        before_swap_hook: Option<&dyn Fn()>,
    ) -> Result<RelocationRecord> {
        let source_path = record.source_path.clone();
        let target_path = record.target_path.clone();
        let rec_id = record.id.unwrap_or(0);

        if !target_path.exists() || !target_path.is_dir() {
            return Err(GreenCError::NotFound {
                entity: "RelocatedTarget".to_string(),
                id: target_path.display().to_string(),
            });
        }

        // Item 6: If `source_path` is no longer a junction/reparse point and a REAL
        // directory now occupies `source_path` (e.g. Zalo was reinstalled), REFUSE automatic undo
        // so we never silently displace active chat data created after reinstall!
        if !reparse_api.is_reparse_point(&source_path) && path_exists_lstat(&source_path) {
            return Err(GreenCError::PreconditionBlocked {
                operation: "zalo_undo_real_source_exists".to_string(),
                reason: format!(
                    "Vị trí cũ ({}) hiện là thư mục Zalo thật (có thể do Zalo đã cài lại hoặc tạo mới), không còn là liên kết tới ổ đích ({}). GreenC từ chối hoàn tác tự động để tránh ghi đè tin nhắn hiện tại — hãy kiểm tra bản sao ở thẻ Zalo PC.",
                    source_path.display(),
                    target_path.display()
                ),
            });
        }

        // Item 9: Check free space on source drive (C:) BEFORE shutting down Zalo so
        // insufficient space fails immediately without disrupting the user's running Zalo app.
        let (upfront_target_bytes, _) = CandidateScanner::compute_dir_size(&target_path);
        let upfront_required_bytes = record.size_bytes.max(upfront_target_bytes);
        CandidateQualificationEngine::check_free_space_reserve(
            source_drive,
            upfront_required_bytes,
            volume_api,
        )?;

        // 1. Refuse during active call; gracefully close Zalo if lifecycle provided.
        let mut shutdown_session = None;
        if let Some(lc) = lifecycle {
            if lc.has_visible_window(&["ZaloCall.exe"]) {
                return Err(GreenCError::PreconditionBlocked {
                    operation: "zalo_undo".to_string(),
                    reason: ZALO_ACTIVE_CALL_MESSAGE.to_string(),
                });
            }
            let session = lc.graceful_shutdown(
                &[source_path.clone(), target_path.clone()],
                ZALO_PROCESS_NAMES,
                DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
            )?;
            shutdown_session = Some(session);
        }

        let reopen_if_needed =
            |session_opt: &mut Option<greenc_platform::app_lifecycle::AppShutdownSession>| {
                if let (Some(lc), Some(ref mut session)) = (lifecycle, session_opt.as_mut()) {
                    if !session.captured_exes.is_empty() {
                        let _ = lc.restart_app(session);
                    }
                }
            };

        // Ensure Zalo is closed and no files in source or target are locked.
        if let Err(err) = check_zalo_closed_and_unlocked_opt(
            &[&source_path, &target_path],
            proc_probe,
            lifecycle,
            lock_probe,
            ZALO_MANUAL_QUIT_MESSAGE,
        ) {
            reopen_if_needed(&mut shutdown_session);
            return Err(err);
        }

        // 2. Snapshot target tree (fail-closed) and validate live free space on source drive.
        let target_snapshot = match TreeFingerprint::capture(&target_path) {
            Ok(fp) => fp,
            Err(err) => {
                reopen_if_needed(&mut shutdown_session);
                return Err(err);
            }
        };
        let required_bytes = record.size_bytes.max(target_snapshot.total_bytes);
        if let Err(err) = CandidateQualificationEngine::check_free_space_reserve(
            source_drive,
            required_bytes,
            volume_api,
        ) {
            reopen_if_needed(&mut shutdown_session);
            return Err(err);
        }

        let source_parent = source_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(r"C:\"));
        let restore_tmp = source_parent.join(format!("zalo-restoring.{}", uuid::Uuid::new_v4()));

        // 3. Strict fail-closed copy + full integrity verification.
        let copy_and_verify = (|| -> Result<ZaloCopyVerification> {
            verified_copy_tree_strict(&target_path, &restore_tmp, ctrl, |_, _| {})?;
            verify_zalo_copy_integrity(&target_path, &restore_tmp)
        })();
        if let Err(err) = copy_and_verify {
            remove_dir_all_safe(&restore_tmp);
            reopen_if_needed(&mut shutdown_session);
            return Err(err);
        }

        if let Some(hook) = before_swap_hook {
            hook();
        }

        // 4. Immediately BEFORE swap: re-verify Zalo is STILL closed/unlocked AND target
        //    size/mtime/file-count did NOT change since copy!
        let pre_swap_check = (|| -> Result<()> {
            check_zalo_closed_and_unlocked_opt(
                &[&source_path, &target_path, &restore_tmp],
                proc_probe,
                lifecycle,
                lock_probe,
                "Phát hiện Zalo vừa mở lại hoặc có tệp bị khoá ngay trước khi hoàn tác; GreenC đã dừng an toàn.",
            )?;
            target_snapshot.verify_unchanged(&target_path)?;
            Ok(())
        })();
        if let Err(err) = pre_swap_check {
            if let Err(cleanup_err) = fs::remove_dir_all(to_extended_io_path(&restore_tmp)) {
                tracing::warn!(
                    "Failed to remove temporary restore dir {} after pre-swap abort: {cleanup_err}",
                    restore_tmp.display()
                );
            }
            reopen_if_needed(&mut shutdown_session);
            return Err(err);
        }

        // 5. Persist durable intermediate state BEFORE deleting junction / renaming!
        let prev_status = record.status;
        let prev_health = record.health_status;
        let prev_detail = record.detail.clone();
        record.status = RelocationStatus::Moving;
        record.moving_path = Some(restore_tmp.clone());
        record.detail = Some("Đang hoàn tác dữ liệu Zalo về vị trí cũ...".to_string());
        db.save_relocation_durable(record)?;

        // Vacate `source_path` safely (never deleting a real directory).
        let was_junction = reparse_api.is_reparse_point(&source_path);
        let mut conflict_note = String::new();
        let mut moved_src_conflict: Option<PathBuf> = None;
        if was_junction {
            if let Err(err) = reparse_api.delete_junction(&source_path) {
                let _ = fs::remove_dir_all(to_extended_io_path(&restore_tmp));
                record.status = prev_status;
                record.health_status = prev_health;
                record.moving_path = None;
                record.detail = prev_detail;
                let _ = db.save_relocation_durable(record);
                reopen_if_needed(&mut shutdown_session);
                return Err(err);
            }
        } else if path_exists_lstat(&source_path) {
            let cp = make_sibling_path(&source_path, "greenc-conflict");
            if let Err(err) = fs::rename(&source_path, &cp) {
                let _ = fs::remove_dir_all(to_extended_io_path(&restore_tmp));
                record.status = prev_status;
                record.health_status = prev_health;
                record.moving_path = None;
                record.detail = prev_detail;
                let _ = db.save_relocation_durable(record);
                reopen_if_needed(&mut shutdown_session);
                return Err(GreenCError::Io(err));
            }
            conflict_note = format!(
                " Thư mục hiện tại tại vị trí cũ được giữ ở {}.",
                cp.display()
            );
            moved_src_conflict = Some(cp);
        }

        // 6. Atomically move verified `restore_tmp` into `source_path`.
        // On failure: rebuild the junction (or move back any conflict directory). If rebuilding
        // fails, NEVER save `Done` — persist `NeedsReview` + `LinkBroken` durably!
        if let Err(rename_err) = fs::rename(&restore_tmp, &source_path) {
            let mut rollback_ok = true;
            if was_junction {
                if let Err(j_err) = reparse_api.create_junction(&source_path, &target_path) {
                    tracing::error!(
                        "Failed to rebuild junction {} -> {} after Zalo undo rename failure: {j_err}",
                        source_path.display(),
                        target_path.display()
                    );
                    rollback_ok = false;
                }
            }
            if let Some(ref cp) = moved_src_conflict {
                if fs::rename(cp, &source_path).is_err() {
                    rollback_ok = false;
                }
            }

            if rollback_ok {
                let _ = fs::remove_dir_all(to_extended_io_path(&restore_tmp));
                record.status = prev_status;
                record.health_status = prev_health;
                record.moving_path = None;
                record.detail = prev_detail;
            } else {
                record.status = RelocationStatus::NeedsReview;
                record.health_status = RelocationHealthStatus::LinkBroken;
                record.moving_path = Some(restore_tmp.clone());
                record.detail = Some(format!(
                    "Hoàn tác thất bại ({rename_err}) và không thể dựng lại liên kết/trả thư mục gốc về vị trí cũ — cần người dùng kiểm tra (bản tạm: {}, bản đích: {}).",
                    restore_tmp.display(),
                    target_path.display()
                ));
            }
            let _ = db.save_relocation_durable(record);
            reopen_if_needed(&mut shutdown_session);
            return Err(GreenCError::Io(rename_err));
        }

        // 7. If target was manually specified by user (`repair_junction_target`), NEVER rename or
        //    delete it! Otherwise rename `target_path` to a retained backup and keep 24h in DB.
        let retain_note = if record.target_user_repaired {
            format!(
                "GreenC giữ nguyên thư mục đích {} do người dùng chỉ định (không đổi tên và không tự xoá).",
                target_path.display()
            )
        } else {
            let now = Utc::now();
            let purge_after = now + ChronoDuration::hours(DEFAULT_ORIGINAL_RETENTION_HOURS);
            let retained_target = make_sibling_path(&target_path, "greenc-original");
            let (final_retained, retained_status, note) =
                match fs::rename(&target_path, &retained_target) {
                    Ok(()) => (
                        retained_target.clone(),
                        ZaloRetainedStatus::Retained,
                        format!(
                            "Bản sao trên ổ đích được đổi tên và giữ tạm 24 giờ tại {} để đảm bảo an toàn.",
                            retained_target.display()
                        ),
                    ),
                    Err(rename_err) => (
                        target_path.clone(),
                        ZaloRetainedStatus::NeedsReview,
                        format!(
                            "Không đổi tên được thư mục đích {} ({rename_err}); GreenC vẫn giữ nguyên để tránh mất dữ liệu.",
                            target_path.display()
                        ),
                    ),
                };

            let _ = db.save_zalo_retained_original_durable(&ZaloRetainedOriginal {
                id: None,
                relocation_id: rec_id,
                source_path: source_path.clone(),
                target_path: target_path.clone(),
                retained_path: final_retained,
                size_bytes: target_snapshot.total_bytes,
                file_count: target_snapshot.total_files,
                message_file_count: 0,
                health_verified: false,
                created_at: now,
                purge_after,
                purge_after_valid: true,
                observed_healthy_secs: 0,
                last_observed_at: Some(now),
                status: retained_status,
                resolved_at: None,
                detail: Some(note.clone()),
            });
            note
        };

        record.status = RelocationStatus::Undone;
        record.moving_path = None;
        record.health_status = RelocationHealthStatus::Healthy;
        record.detail = Some(format!(
            "Hoàn tác dữ liệu Zalo về {} thành công.{conflict_note} {retain_note}",
            source_path.display()
        ));
        db.save_relocation_durable(record)?;

        reopen_if_needed(&mut shutdown_session);
        Ok(record.clone())
    }
}
