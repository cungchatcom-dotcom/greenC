#![deny(unsafe_code)]
//! Multi-strategy data relocation off C:, NTFS junctions, crash recovery, and in-place compaction.

pub mod candidate;
pub mod catalog;
pub mod compress;
pub mod known_folders;
pub mod mover;
pub mod recovery;
pub mod verified_copy;
pub mod vhdx;
pub mod zalo_move;

pub use candidate::{
    CandidateQualificationEngine, CandidateScanner, RelocationCandidate, FREE_SPACE_RESERVE_BYTES,
};
pub use catalog::{
    get_app_catalog, get_tool_catalog, AppCatalogProfile, ToolConfigProfile, ADVANCED_SKIP_LIST,
    ADVANCED_THRESHOLD_BYTES,
};
pub use compress::{
    CompactionCandidate, InPlaceCompactor, COLD_AGE_THRESHOLD_DAYS, MIN_COMPACTION_FILE_SIZE,
};
pub use known_folders::{KnownFolderInfo, KnownFolderService};
pub use mover::{MoveOptions, TransactionalMover};
pub use recovery::RecoveryService;
pub use verified_copy::{remove_dir_all_safe, verified_copy_tree, CopyStats};
pub use vhdx::{VhdxCompactor, VhdxImageCandidate};
pub use zalo_move::{
    format_zalo_move_confirmation, is_zalo_auto_move_supported, verify_junction_write_probe,
    verify_zalo_copy_integrity, zalo_auto_move_unsupported_reason, ZaloCopyVerification,
    ZaloMoveOptions, ZaloMovePhase, ZaloMoveProgress, ZaloMoveReport, ZaloRelocator,
    DEFAULT_POST_REOPEN_POLL, DEFAULT_POST_REOPEN_WATCH,
};
