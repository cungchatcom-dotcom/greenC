//! Transactional Zalo PC chat-data relocation engine (`<ROOT>\Zalo Data` -> target NTFS drive).
//!
//! Guarantees (GC-ZALO + GC-ZALO2):
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
use greenc_scanner::zalo::{
    extract_drive_label, is_symlink_or_reparse, ZaloDiscovery, ZALO_PROCESS_NAMES,
};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default post-reopen junction health watch duration (20 seconds).
pub const DEFAULT_POST_REOPEN_WATCH: Duration = Duration::from_secs(20);

/// Default poll interval during the 20-second post-reopen health watch (1 second).
pub const DEFAULT_POST_REOPEN_POLL: Duration = Duration::from_secs(1);

/// Default safety retention period before an auto-verified `.greenc-original.<uuid>` may be
/// eligible for automatic cleanup (24 hours).
pub const DEFAULT_ORIGINAL_RETENTION_HOURS: i64 = 24;

/// Maximum file size outside `message/` for full SHA-256 re-verification in
/// [`verify_zalo_copy_integrity`] (`64 MiB`). Files inside `message/` are ALWAYS
/// fully SHA-256 hashed regardless of size; files outside `message/` larger than this
/// threshold have their header/tail chunks hashed (in addition to full streaming SHA-256
/// already performed during `verified_copy_tree_strict`).
const VERIFY_NON_MESSAGE_FULL_HASH_LIMIT: u64 = 64 * 1024 * 1024;

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
    pub id: Option<i64>,
    pub relocation_id: Option<i64>,
    /// `"original"` | `"conflict"` | `"moving"`
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
            "Thiếu dung lượng trên ổ {src_label}: hiện chỉ còn trống {}, cần tối thiểu {} để chuyển an toàn vì bản gốc ({}) sẽ được giữ tạm trong 24 giờ trên ổ {src_label}.",
            format_bytes_vi(source_free),
            format_bytes_vi(source_required),
            format_bytes_vi(chat_data_bytes),
        ))
    } else if source_free < chat_data_bytes.saturating_add(source_required) {
        Some(format!(
            "Lưu ý: bản gốc ({}) sẽ được giữ tạm trên ổ {src_label} trong 24 giờ để đảm bảo an toàn nên dung lượng ổ {src_label} chưa giảm ngay (hiện còn trống {}).",
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
    if name.contains(".greenc-original.") {
        Some("original")
    } else if name.contains(".greenc-conflict.") {
        Some("conflict")
    } else if name.contains(".greenc-moving.") {
        Some("moving")
    } else {
        None
    }
}

fn norm_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
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
            let kind = row
                .retained_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(classify_safety_copy_name)
                .unwrap_or("original")
                .to_string();

            seen_keys.insert(norm_path_key(&row.retained_path));
            results.push(ZaloSafetyCopyInfo {
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
                status: if !exists && row.status == ZaloRetainedStatus::Retained {
                    ZaloRetainedStatus::Missing.as_str().to_string()
                } else {
                    row.status.as_str().to_string()
                },
                exists_on_disk: exists,
                detail: row.detail,
            });
        }
    }

    // 2. Collect parent directories from any Zalo relocation records
    if let Ok(relocs) = db.list_relocations(None) {
        for rec in relocs {
            if rec.app_id.eq_ignore_ascii_case("zalo") {
                if let Some(p) = rec.source_path.parent() {
                    scan_parents.push(p.to_path_buf());
                }
                if let Some(p) = rec.target_path.parent() {
                    scan_parents.push(p.to_path_buf());
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
            results.push(ZaloSafetyCopyInfo {
                id: None,
                relocation_id: None,
                kind: kind.to_string(),
                path,
                source_path: data_dir.map(Path::to_path_buf),
                target_path: None,
                size_bytes,
                file_count,
                created_at,
                purge_after: None,
                health_verified: false,
                status: if kind == "original" {
                    "retained".to_string()
                } else {
                    "needs_review".to_string()
                },
                exists_on_disk: true,
                detail: Some(match kind {
                    "conflict" => {
                        "Thư mục xung đột được giữ nguyên để tránh mất tin nhắn mới".to_string()
                    }
                    "moving" => "Thư mục đang chuyển dở từ lần trước — cần kiểm tra".to_string(),
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

fn sha256_head_tail(path: &Path, len: u64) -> Result<String> {
    let io_path = to_extended_io_path(path);
    let mut f = fs::File::open(&io_path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let head_n = f.read(&mut buf)?;
    hasher.update(&buf[..head_n]);
    if len > 65536 {
        f.seek(SeekFrom::Start(len - 65536))?;
        let tail_n = f.read(&mut buf)?;
        hasher.update(&buf[..tail_n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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
/// 3. Mandatory SHA-256 hash comparison for EVERY file inside `message/` (the chat database),
///    plus full SHA-256 for all other files `<= 64 MiB` (and head/tail hash for larger files).
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
        if is_message || expected_len <= VERIFY_NON_MESSAGE_FULL_HASH_LIMIT {
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
        } else {
            let h_src = sha256_head_tail(&src_file, expected_len)?;
            let h_dst = sha256_head_tail(&dst_file, actual_len)?;
            if h_src != h_dst {
                return Err(GreenCError::IntegrityMismatch {
                    path: dst_file,
                    expected: h_src,
                    actual: h_dst,
                });
            }
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
fn check_zalo_closed_and_unlocked(
    paths: &[&Path],
    proc_probe: &dyn ProcessServiceProbe,
    lifecycle: &dyn AppLifecycleApi,
    lock_probe: &dyn FileLockProbe,
    reopened_reason: &str,
) -> Result<()> {
    let primary_path = paths.first().copied().unwrap_or_else(|| Path::new(""));
    for proc in ZALO_PROCESS_NAMES {
        if proc_probe.is_process_running(proc)
            || !lifecycle.find_running_exe_paths(&[proc]).is_empty()
        {
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

/// Verifies that the newly created junction at `link_path` is writable and resolves live to
/// `target_path` by creating, reading back, and deleting a temporary probe file.
pub fn verify_junction_write_probe(link_path: &Path, target_path: &Path) -> Result<()> {
    let probe_name = format!(".greenc-junction-probe-{}", uuid::Uuid::new_v4());
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

/// Formats the plain-language one-time confirmation message for moving Zalo chat data.
pub fn format_zalo_move_confirmation(size_bytes: u64, target_drive: char) -> String {
    let drive = target_drive.to_ascii_uppercase();
    let size_text = format_bytes_vi(size_bytes);
    format!(
        "GreenC sẽ đóng Zalo, chuyển {size_text} dữ liệu chat sang {drive}:, rồi mở lại Zalo. Bản gốc sẽ được giữ tạm 24 giờ trên ổ C: để đảm bảo an toàn, tin nhắn và ảnh giữ nguyên, có thể hoàn tác."
    )
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
            let _ = db.save_relocation(record);
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
                let _ = db.save_relocation(record);
                return (None, false, msg);
            }
        }
    }

    match fs::rename(moving_path, source_path) {
        Ok(()) if path_exists_lstat(source_path) && !path_exists_lstat(moving_path) => {
            // Original is verified back at `source_path` -> safe to clean target copy now.
            // Rename target first, then remove.
            let target_cleanup = make_sibling_path(target_path, "greenc-rollback-done");
            if fs::rename(target_path, &target_cleanup).is_ok() {
                remove_dir_all_safe(&target_cleanup);
            } else {
                remove_dir_all_safe(target_path);
            }

            let msg = if let Some(ref cp) = conflict_path {
                format!(
                    "{cause}. GreenC đã trả bản gốc về {} và giữ thư mục mới do Zalo tạo tại {} (bản xung đột .greenc-conflict) để không mất dữ liệu.",
                    source_path.display(),
                    cp.display()
                )
            } else {
                format!(
                    "{cause}. GreenC đã trả dữ liệu gốc về {} an toàn.",
                    source_path.display()
                )
            };
            record.status = RelocationStatus::Failed;
            record.health_status = RelocationHealthStatus::Healthy;
            record.detail = Some(msg.clone());
            let _ = db.save_relocation(record);
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
            let _ = db.save_relocation(record);
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
            let _ = db.save_relocation(record);
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

        // Ensure no lingering Zalo process or recursive file lock (including `message/` DBs) remains
        check_zalo_closed_and_unlocked(
            &[&source_path],
            proc_probe,
            lifecycle,
            lock_probe,
            ZALO_MANUAL_QUIT_MESSAGE,
        )?;

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // 8. Phase 1: Strict fail-closed verified streaming copy (SHA-256 + sync_all)
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
            verified_copy_tree_strict(&source_path, &target_path, ctrl, |done_b, done_f| {
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
            remove_dir_all_safe(&target_path);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(err);
        }

        if let Some(hook) = hooks.after_copy {
            hook(&target_path);
        }

        // 9. Mandatory fail-closed pre-rename verification (exact paths/sizes, SHA-256, media count)
        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Verifying,
            percent: 72,
            done_bytes: total_bytes,
            total_bytes,
            done_files: total_files,
            total_files,
            detail: "Đang kiểm tra toàn vẹn cơ sở dữ liệu tin nhắn và ảnh...".to_string(),
        });

        let verification = match verify_zalo_copy_integrity(&source_path, &target_path) {
            Ok(v) => v,
            Err(err) => {
                remove_dir_all_safe(&target_path);
                if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                    let _ = lifecycle.restart_app(&mut shutdown_session);
                }
                return Err(err);
            }
        };

        if ctrl.is_cancelled() {
            remove_dir_all_safe(&target_path);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(GreenCError::Cancelled);
        }

        // 10. Phase 2: Persist `Moving` record & atomically rename source to `.greenc-moving.<uuid>`
        on_progress(ZaloMoveProgress {
            phase: ZaloMovePhase::Linking,
            percent: 80,
            done_bytes: verification.total_bytes,
            total_bytes: verification.total_bytes,
            done_files: verification.total_files,
            total_files: verification.total_files,
            detail: "Đang tạo liên kết thư mục sang ổ mới...".to_string(),
        });

        let mut record = RelocationRecord {
            id: None,
            app_id: "zalo".to_string(),
            app_name: "Zalo PC".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: source_path.clone(),
            target_path: target_path.clone(),
            size_bytes: verification.total_bytes,
            status: RelocationStatus::Moving,
            health_status: RelocationHealthStatus::Healthy,
            process_names: ZALO_PROCESS_NAMES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            created_at: Utc::now(),
            detail: None,
        };
        let rec_id = db.save_relocation(&record)?;
        record.id = Some(rec_id);

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
            let _ = db.save_relocation(&record);
            return Err(err);
        }

        let moving_path = make_sibling_path(&source_path, "greenc-moving");

        if let Err(rename_err) = fs::rename(&source_path, &moving_path) {
            remove_dir_all_safe(&target_path);
            record.status = RelocationStatus::Failed;
            record.detail = Some(format!("Lỗi đổi tên thư mục nguồn: {rename_err}"));
            let _ = db.save_relocation(&record);
            if opts.reopen_zalo && !shutdown_session.captured_exes.is_empty() {
                let _ = lifecycle.restart_app(&mut shutdown_session);
            }
            return Err(GreenCError::Io(rename_err));
        }

        // Checkpoint 2 (Item 5): Check immediately AFTER rename (before creating junction).
        if let Some(hook) = hooks.after_rename {
            hook(&source_path, &moving_path, &target_path);
        }
        if check_zalo_closed_and_unlocked(
            &[&source_path, &moving_path],
            proc_probe,
            lifecycle,
            lock_probe,
            mid_reopen_msg,
        )
        .is_err()
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

        // Checkpoint 3 (Item 5): Check immediately BEFORE create_junction.
        if let Some(hook) = hooks.before_junction {
            hook(&source_path, &moving_path, &target_path);
        }
        if check_zalo_closed_and_unlocked(
            &[&source_path, &moving_path],
            proc_probe,
            lifecycle,
            lock_probe,
            mid_reopen_msg,
        )
        .is_err()
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
            loop {
                let health = RecoveryService::check_single_junction_live(
                    &source_path,
                    &target_path,
                    reparse_api,
                );

                let probe_ok = if opts.skip_write_probe {
                    true
                } else {
                    verify_junction_write_probe(&source_path, &target_path).is_ok()
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

                if watch_start.elapsed() >= opts.post_reopen_watch_duration {
                    break;
                }
                let remaining = opts
                    .post_reopen_watch_duration
                    .saturating_sub(watch_start.elapsed());
                std::thread::sleep(opts.post_reopen_poll_interval.min(remaining));
            }
            post_reopen_health_verified = reopened_zalo;
        }

        // 13. Phase 4 (Item 1): Rename `moving_path` -> `Zalo Data.greenc-original.<uuid>` and
        // persist in DB for 24-hour safety retention (NEVER delete immediately!).
        let now = Utc::now();
        let retention_hours = opts.original_retention_hours.max(1);
        let purge_after = now + ChronoDuration::hours(retention_hours);
        let original_path = make_sibling_path(&source_path, "greenc-original");

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

        let retained_rec = ZaloRetainedOriginal {
            id: None,
            relocation_id: rec_id,
            source_path: source_path.clone(),
            target_path: target_path.clone(),
            retained_path: final_retained_path.clone(),
            size_bytes: verification.total_bytes,
            file_count: verification.total_files,
            health_verified: post_reopen_health_verified,
            created_at: now,
            purge_after,
            status: retained_status,
            resolved_at: None,
            detail: Some(retained_detail.clone()),
        };
        let retained_id = db.save_zalo_retained_original(&retained_rec)?;

        record.status = RelocationStatus::Done;
        record.health_status = RelocationHealthStatus::Healthy;
        record.detail = Some(format!(
            "Đã chuyển dữ liệu Zalo sang {target_drive_str}; bản gốc ({}) đang được giữ tạm tại {}",
            format_bytes_vi(verification.total_bytes),
            final_retained_path.display()
        ));
        db.save_relocation(&record)?;

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
    /// (c) Never removes `target_path` before restored copy is fully verified AND target is renamed first.
    /// (d) Uses `target_path` as the latest copy so messages written during watch are preserved.
    ///     If `target_path` went offline, restores `moving_path` back to `source_path`.
    #[allow(clippy::too_many_arguments)]
    fn rollback_broken_zalo_relocation(
        record: &mut RelocationRecord,
        moving_path: &Path,
        source_drive: char,
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

        // Helper to preserve `moving_path` as `.greenc-original.<uuid>` in DB when rollback aborts.
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
            let _ = db.save_zalo_retained_original(&ZaloRetainedOriginal {
                id: None,
                relocation_id: rec_id,
                source_path: source_path.clone(),
                target_path: target_path.clone(),
                retained_path: kept.clone(),
                size_bytes: sz,
                file_count: fc,
                health_verified: false,
                created_at: now,
                purge_after: now + ChronoDuration::hours(DEFAULT_ORIGINAL_RETENTION_HOURS),
                status: ZaloRetainedStatus::NeedsReview,
                resolved_at: None,
                detail: Some(note.to_string()),
            });
            kept
        };

        // (a) Check active call, gracefully shut down Zalo, and verify NO process or file lock remains!
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
                Ok(_) => check_zalo_closed_and_unlocked(
                    &[&source_path, &target_path, moving_path],
                    proc_probe,
                    lifecycle,
                    lock_probe,
                    "Zalo vẫn đang chạy hoặc còn tệp dữ liệu bị khoá",
                )
                .err()
                .map(|e| e.to_string()),
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
            let _ = db.save_relocation(record);
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
                    let _ = db.save_relocation(record);
                    return Ok(msg);
                }
            }

            if path_exists_lstat(moving_path) && fs::rename(moving_path, &source_path).is_ok() {
                let msg = if let Some(ref cp) = conflict_path {
                    format!(
                        "Ổ đích bị ngắt kết nối sau khi mở lại Zalo; GreenC đã tự hoàn tác dữ liệu gốc về {} và giữ thư mục mới tại {}.",
                        source_path.display(),
                        cp.display()
                    )
                } else {
                    format!(
                        "Ổ đích bị ngắt kết nối sau khi mở lại Zalo; GreenC đã tự hoàn tác dữ liệu gốc về {} an toàn.",
                        source_path.display()
                    )
                };
                record.status = RelocationStatus::Undone;
                record.health_status = RelocationHealthStatus::TargetOffline;
                record.detail = Some(msg.clone());
                db.save_relocation(record)?;
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
            let _ = db.save_relocation(record);
            return Ok(msg);
        }

        // Case 2: Target is online -> use `target_path` as the LATEST copy (Item 2d: preserves
        // any new messages written to target during the 20s health watch!).
        let (target_bytes, _) = CandidateScanner::compute_dir_size(&target_path);
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
                let _ = db.save_relocation(record);
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
            let _ = db.save_relocation(record);
            return Ok(msg);
        }

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
                record.detail = Some(msg.clone());
                let _ = db.save_relocation(record);
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
                record.detail = Some(msg.clone());
                let _ = db.save_relocation(record);
                return Ok(msg);
            }
            conflict_path = Some(cp);
        }

        if let Err(err) = fs::rename(&restore_tmp, &source_path) {
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
            record.detail = Some(msg.clone());
            let _ = db.save_relocation(record);
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
            record.detail = Some(msg.clone());
            let _ = db.save_relocation(record);
            return Ok(msg);
        }

        // Rename `target_path` to a temporary retained name first, then clean it only after verification!
        let target_retained = make_sibling_path(&target_path, "greenc-rollback-done");
        if fs::rename(&target_path, &target_retained).is_ok() {
            remove_dir_all_safe(&target_retained);
        }

        // Handle `moving_path`: if identical to restored `source_path`, clean after rename; if it
        // differs (e.g. target had newer files), preserve `moving_path` as `.greenc-original.<uuid>`!
        let mut preserved_original_note = String::new();
        if path_exists_lstat(moving_path) {
            if verify_zalo_copy_integrity(moving_path, &source_path).is_ok() {
                let moving_done = make_sibling_path(moving_path, "greenc-rollback-done");
                if fs::rename(moving_path, &moving_done).is_ok() {
                    remove_dir_all_safe(&moving_done);
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
            "Liên kết thư mục bị hỏng sau khi mở lại Zalo; GreenC đã tự hoàn tác bản dữ liệu mới nhất về {} an toàn.{conflict_note}{preserved_original_note}",
            source_path.display()
        );

        record.status = RelocationStatus::Undone;
        record.health_status = RelocationHealthStatus::LinkBroken;
        record.detail = Some(final_msg.clone());
        db.save_relocation(record)?;
        Ok(final_msg)
    }

    /// Automatically purges retained `.greenc-original.<uuid>` directories ONLY when ALL safety
    /// conditions hold:
    /// (a) `health_verified == true` (the post-reopen health watch passed),
    /// (b) `now >= purge_after` (default 24 hours elapsed), AND
    /// (c) the relocation is still `Done`, `target_path` exists, and live junction health is `Healthy`.
    pub fn purge_expired_originals(
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
        now: DateTime<Utc>,
    ) -> Result<Vec<ZaloRetainedOriginal>> {
        let retained = db.list_zalo_retained_originals(Some("retained"))?;
        let relocs = db.list_relocations(None)?;
        let mut purged = Vec::new();

        for mut rec in retained {
            let Some(id) = rec.id else {
                continue;
            };
            if !path_exists_lstat(&rec.retained_path) {
                db.update_zalo_retained_original_status(
                    id,
                    ZaloRetainedStatus::Missing.as_str(),
                    "Thư mục bản gốc không còn tồn tại trên đĩa",
                )?;
                continue;
            }
            if !rec.health_verified || now < rec.purge_after {
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
            if !reloc_ok || !rec.target_path.exists() {
                continue;
            }

            if let Err(err) = fs::remove_dir_all(to_extended_io_path(&rec.retained_path)) {
                tracing::warn!(
                    "Could not auto-purge expired Zalo original {}: {err}",
                    rec.retained_path.display()
                );
                continue;
            }
            let msg = "Đã tự động dọn bản gốc giữ tạm sau 24 giờ kiểm tra ổn định";
            db.update_zalo_retained_original_status(id, ZaloRetainedStatus::Purged.as_str(), msg)?;
            rec.status = ZaloRetainedStatus::Purged;
            rec.resolved_at = Some(now);
            rec.detail = Some(msg.to_string());
            purged.push(rec);
        }
        Ok(purged)
    }

    /// Explicitly purges a retained `.greenc-original.<uuid>` copy at the user's request
    /// ("Xoá bản gốc"). Still refuses if `target_path` is missing or the junction is unhealthy
    /// so the user cannot accidentally delete their last intact copy.
    pub fn purge_retained_original(
        retained_id: i64,
        db: &GreenCDb,
        reparse_api: &dyn ReparsePointApi,
    ) -> Result<ZaloRetainedOriginal> {
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
            db.update_zalo_retained_original_status(
                retained_id,
                ZaloRetainedStatus::Missing.as_str(),
                "Thư mục bản gốc không còn tồn tại trên đĩa",
            )?;
            rec.status = ZaloRetainedStatus::Missing;
            return Ok(rec);
        }

        // Safety gate: never purge original if target is gone or junction is broken!
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

        fs::remove_dir_all(to_extended_io_path(&rec.retained_path))?;
        let msg = "Người dùng đã xác nhận xoá bản gốc giữ tạm";
        db.update_zalo_retained_original_status(
            retained_id,
            ZaloRetainedStatus::Purged.as_str(),
            msg,
        )?;
        rec.status = ZaloRetainedStatus::Purged;
        rec.resolved_at = Some(Utc::now());
        rec.detail = Some(msg.to_string());
        Ok(rec)
    }

    /// Explicitly restores a retained `.greenc-original.<uuid>` copy back to `source_path`
    /// ("Khôi phục"), preserving any post-move `target_path` data as `.greenc-conflict.<uuid>`
    /// so zero messages can ever be lost.
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
        let mut rec =
            db.get_zalo_retained_original(retained_id)?
                .ok_or_else(|| GreenCError::NotFound {
                    entity: "ZaloRetainedOriginal".to_string(),
                    id: retained_id.to_string(),
                })?;

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

        check_zalo_closed_and_unlocked(
            &[&rec.source_path, &rec.target_path, &rec.retained_path],
            proc_probe,
            lifecycle,
            lock_probe,
            ZALO_MANUAL_QUIT_MESSAGE,
        )?;

        let mut notes: Vec<String> = Vec::new();
        if reparse_api.is_reparse_point(&rec.source_path) {
            reparse_api.delete_junction(&rec.source_path)?;
        } else if path_exists_lstat(&rec.source_path) {
            let src_conflict = make_sibling_path(&rec.source_path, "greenc-conflict");
            fs::rename(&rec.source_path, &src_conflict)?;
            notes.push(format!(
                "thư mục hiện tại tại vị trí cũ được giữ ở {}",
                src_conflict.display()
            ));
        }

        fs::rename(&rec.retained_path, &rec.source_path)?;

        // Never discard target_path (it may have messages written after move!): rename to conflict copy.
        if path_exists_lstat(&rec.target_path) {
            let tgt_conflict = make_sibling_path(&rec.target_path, "greenc-conflict");
            if fs::rename(&rec.target_path, &tgt_conflict).is_ok() {
                notes.push(format!(
                    "bản dữ liệu trên ổ đích được giữ lại tại {}",
                    tgt_conflict.display()
                ));
            } else {
                notes.push(format!(
                    "bản dữ liệu trên ổ đích vẫn giữ nguyên tại {}",
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

        db.update_zalo_retained_original_status(
            retained_id,
            ZaloRetainedStatus::Restored.as_str(),
            &detail,
        )?;

        if let Ok(relocs) = db.list_relocations(None) {
            if let Some(mut reloc) = relocs.into_iter().find(|r| r.id == Some(rec.relocation_id)) {
                reloc.status = RelocationStatus::Undone;
                reloc.health_status = RelocationHealthStatus::Healthy;
                reloc.detail = Some(detail.clone());
                let _ = db.save_relocation(&reloc);
            }
        }

        if !session.captured_exes.is_empty() {
            let _ = lifecycle.restart_app(&mut session);
        }

        rec.status = ZaloRetainedStatus::Restored;
        rec.resolved_at = Some(Utc::now());
        rec.detail = Some(detail);
        Ok(rec)
    }
}
