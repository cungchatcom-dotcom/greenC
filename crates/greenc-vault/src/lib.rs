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
pub mod canary;
pub mod cleanup;
pub mod manifest;
pub mod profiles;
pub mod rollback;
pub mod sniff;

pub use archiver::{ArchiveOptions, ArchivePhase, ArchiveProgress, ArchiverPipeline};
pub use canary::{CanaryProbeResult, CanarySentinel, CanaryVerifier};
pub use cleanup::{
    CleanupOptions, CleanupResult, DeletionMethod, SafeCleaner, ShellFileOperationApi,
    StandardFileDeleter,
};
pub use manifest::{compute_bytes_sha256, compute_file_sha256, VaultManifest};
pub use profiles::{CleanupProfile, PresetProfiles, ProfileCategory};
pub use rollback::{RestoreConflictStrategy, RestoreOptions, RestoreResult, RollbackEngine};
pub use sniff::{
    CompressionDecision, FormatSniffer, COMPRESSED_MAGIC_SIGNATURES, INCOMPRESSIBLE_EXTENSIONS,
};
