//! High-throughput parallel ZIP64 Canary Vault archiver, safety watchdog, and granular restore.
//!
//! Enforces zero-data-loss safety guarantees:
//! - 1-cycle automated sandbox canary probe before cleanup
//! - Hardware SHA-256 integrity verification
//! - Incompressible bypass for 47 file extensions and 15 magic headers
//! - Strict 512 MiB minimum drive reserve guard
//! - 8 Preset cleanup profiles
//! - Two-phase lock and snapshot verification
//! - Transactional auto-rollback on application missing-file errors or service stops

#![deny(unsafe_code)]

pub mod archiver;
pub mod cache_units;
pub mod canary;
pub mod cleanup;
pub mod guard;
pub mod manifest;
pub mod profiles;
pub mod rollback;
pub mod sniff;
pub mod suggest;
pub mod zalo_cleanup;

pub use archiver::{ArchiveOptions, ArchivePhase, ArchiveProgress, ArchiverPipeline};
pub use cache_units::{
    classify_dev_cache_root, compute_unit_latest_used, effective_last_used,
    is_forbidden_dev_cache_name, is_forbidden_dev_cache_path, is_linux_app_log_file,
    parse_gradle_version, scan_dev_cache_root, scan_dev_cache_root_with_atime_reliability,
    CleanUnit, CleanUnitKind, DEFAULT_DEV_CACHE_MIN_AGE_SECS, NOATIME_FALLBACK_MIN_AGE_SECS,
};
pub use canary::{CanaryProbeResult, CanarySentinel, CanaryVerifier};
pub use cleanup::{
    CleanupOptions, CleanupResult, DeletionMethod, SafeCleaner, ShellFileOperationApi,
    StandardFileDeleter,
};
pub use guard::{
    backup_vault_archive, cooldown_duration_for_failures, enforce_drive_quota_units,
    estimate_unit_net_bytes, execute_restore_staged_dir_job, is_valid_staging_unit_id,
    recover_staging_dir, recover_staging_dir_with_options, resolve_staging_dir_checked,
    run_guard_for_quota, scan_drive_units, GuardDriveOutcome, GuardRunOptions, StagingJournalEntry,
    StagingMode, StagingRecoveryOptions, StagingRecoveryReport, StagingState, BUSY_TOOL_CACHE_TTL,
    DEFAULT_GUARD_RETENTION_DAYS, FIXED_RETRY_COOLDOWN_MINUTES, GUARD_AUTO_CLEAN_PROFILE_IDS,
    MAX_STAGING_JOURNAL_BYTES, MIN_EFFECTIVE_NET_RATIO,
};
pub use manifest::{compute_bytes_sha256, compute_file_sha256, VaultManifest};
pub use profiles::{CleanupProfile, PresetProfiles, ProfileCategory};
pub use rollback::{
    execute_restore_files_job, map_path_into_restore_dir, safe_remove_vault_artifact,
    validate_restore_file_path, validate_vault_artifact_path, RestoreConflictStrategy,
    RestoreOptions, RestoreResult, RollbackEngine,
};
pub use sniff::{
    CompressionDecision, FormatSniffer, COMPRESSED_MAGIC_SIGNATURES, INCOMPRESSIBLE_EXTENSIONS,
};
pub use suggest::{
    measure_vault_summary, suggest_large_build_outputs, suggest_large_build_outputs_with_busy,
    BuildOutputSuggestion, SuggestOptions, SuggestReport, VaultSpaceSummary,
    DEFAULT_SUGGEST_MAX_DEPTH, DEFAULT_SUGGEST_MAX_DURATION_SECS, DEFAULT_SUGGEST_MIN_STALE_DAYS,
};
pub use zalo_cleanup::{
    execute_zalo_cleanup, format_zalo_locked_notice, prune_empty_dirs_no_follow,
    scan_zalo_cleanup_units, ZaloCleanupOptions, ZaloCleanupReport, ZaloCleanupScan,
};
