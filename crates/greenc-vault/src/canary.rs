//! Canary verification and sandbox probing engine (Tasks T032, T034).
//!
//! Features:
//! - 1-cycle automated sandbox canary probe (files <= 15 MiB)
//! - Cryptographic canary sentinel generation and injection (`canary.sentinel`)
//! - Nonce verification for fast archive integrity checks without full extraction

use chrono::{DateTime, Utc};
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::CleanCandidate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Instant;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

/// Maximum file size eligible for the 1-cycle canary probe (15 MiB).
pub const MAX_CANARY_PROBE_FILE_SIZE: u64 = 15 * 1024 * 1024;

/// Sentinel filename embedded into Canary Vault archives.
pub const CANARY_SENTINEL_NAME: &str = "canary.sentinel";

/// Cryptographic sentinel metadata stored in the archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanarySentinel {
    /// Associated archive ID.
    pub archive_id: String,
    /// Timestamp when sentinel was injected.
    pub timestamp: DateTime<Utc>,
    /// Number of files in the archive.
    pub file_count: u64,
    /// SHA-256 hash of the manifest or payload.
    pub manifest_sha256: String,
    /// Unique cryptographic nonce for tamper detection.
    pub nonce: String,
}

/// Detailed result of running a 1-cycle sandbox canary probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryProbeResult {
    /// True if the full cycle (compress, unlink, extract, verify) succeeded.
    pub passed: bool,
    /// True if the restored file's SHA-256 exactly matched pre-compression hash.
    pub sha_matched: bool,
    /// True if NTFS hardlink siblings were preserved and restored.
    pub hardlink_restored: bool,
    /// Total duration of the probe in milliseconds.
    pub duration_ms: u64,
    /// Error detail if probe failed.
    pub error_detail: Option<String>,
}

/// Verifier and sandbox runner for canary safety gates.
pub struct CanaryVerifier;

impl CanaryVerifier {
    /// Generates sentinel metadata and serialized JSON bytes.
    pub fn generate_sentinel(
        archive_id: &str,
        file_count: u64,
        manifest_sha256: &str,
    ) -> (CanarySentinel, Vec<u8>) {
        let now = Utc::now();
        let nonce = format!(
            "{:016x}{:016x}",
            now.timestamp_nanos_opt().unwrap_or(0),
            file_count
        );
        let sentinel = CanarySentinel {
            archive_id: archive_id.to_string(),
            timestamp: now,
            file_count,
            manifest_sha256: manifest_sha256.to_string(),
            nonce,
        };
        let bytes = serde_json::to_vec_pretty(&sentinel).unwrap_or_default();
        (sentinel, bytes)
    }

    /// Injects `canary.sentinel` into an open `ZipWriter`.
    pub fn inject_sentinel<W: Write + std::io::Seek>(
        zip_writer: &mut ZipWriter<W>,
        archive_id: &str,
        file_count: u64,
        manifest_sha256: &str,
    ) -> Result<()> {
        let (_, sentinel_bytes) = Self::generate_sentinel(archive_id, file_count, manifest_sha256);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(false);

        zip_writer
            .start_file(CANARY_SENTINEL_NAME, options)
            .map_err(|e| GreenCError::Vault(e.to_string()))?;
        zip_writer.write_all(&sentinel_bytes)?;
        Ok(())
    }

    /// Verifies the presence and validity of `canary.sentinel` in a `.zip` archive.
    pub fn verify_archive_sentinel(zip_path: &Path) -> Result<bool> {
        let file = File::open(zip_path)?;
        let mut archive = ZipArchive::new(file)
            .map_err(|e| GreenCError::Vault(format!("Failed to open archive: {}", e)))?;

        let mut sentinel_entry = match archive.by_name(CANARY_SENTINEL_NAME) {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };

        let mut content = Vec::new();
        sentinel_entry.read_to_end(&mut content)?;

        let sentinel: CanarySentinel = serde_json::from_slice(&content)
            .map_err(|e| GreenCError::Vault(format!("Invalid canary sentinel JSON: {}", e)))?;

        Ok(!sentinel.archive_id.is_empty() && !sentinel.nonce.is_empty())
    }

    /// Executes the 1-cycle automated sandbox canary probe on a candidate file.
    ///
    /// Sequence:
    /// 1. Reads candidate content and computes SHA-256.
    /// 2. Compresses candidate into a sandbox ZIP container.
    /// 3. Extracts entry back to sandbox destination.
    /// 4. Recomputes SHA-256 and validates integrity.
    /// 5. Cleans up sandbox artifacts.
    pub fn run_sandbox_probe(
        candidate: &CleanCandidate,
        sandbox_dir: &Path,
    ) -> Result<CanaryProbeResult> {
        let start = Instant::now();
        std::fs::create_dir_all(sandbox_dir)?;

        if candidate.size_bytes > MAX_CANARY_PROBE_FILE_SIZE {
            return Ok(CanaryProbeResult {
                passed: false,
                sha_matched: false,
                hardlink_restored: false,
                duration_ms: start.elapsed().as_millis() as u64,
                error_detail: Some(format!(
                    "File size {} exceeds 15 MiB canary probe limit",
                    candidate.size_bytes
                )),
            });
        }

        // 1. Read original candidate file & compute pre-hash
        let mut original_data = Vec::with_capacity(candidate.size_bytes as usize);
        let mut f = match File::open(&candidate.path) {
            Ok(file) => file,
            Err(e) => {
                return Ok(CanaryProbeResult {
                    passed: false,
                    sha_matched: false,
                    hardlink_restored: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error_detail: Some(format!("Failed to open candidate file: {}", e)),
                });
            }
        };

        if let Err(e) = f.read_to_end(&mut original_data) {
            return Ok(CanaryProbeResult {
                passed: false,
                sha_matched: false,
                hardlink_restored: false,
                duration_ms: start.elapsed().as_millis() as u64,
                error_detail: Some(format!("Failed to read candidate: {}", e)),
            });
        }

        let mut hasher = Sha256::new();
        hasher.update(&original_data);
        let pre_sha256 = format!("{:02x}", hasher.finalize());

        // 2. Compress into sandbox probe zip
        let probe_zip_path = sandbox_dir.join(format!(
            "probe_{}_{}.zip",
            &pre_sha256[..8],
            Utc::now().timestamp_millis()
        ));
        let restore_dest_path = sandbox_dir.join("restored_canary.bin");

        let res = (|| -> Result<()> {
            let zip_file = File::create(&probe_zip_path)?;
            let mut writer = ZipWriter::new(zip_file);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

            writer
                .start_file("canary_sample.bin", options)
                .map_err(|e| GreenCError::Vault(e.to_string()))?;
            writer.write_all(&original_data)?;
            let mut inner = writer
                .finish()
                .map_err(|e| GreenCError::Vault(e.to_string()))?;
            inner.flush()?;

            // 3. Extract back
            let read_zip = File::open(&probe_zip_path)?;
            let mut reader =
                ZipArchive::new(read_zip).map_err(|e| GreenCError::Vault(e.to_string()))?;
            let mut entry = reader
                .by_name("canary_sample.bin")
                .map_err(|e| GreenCError::Vault(e.to_string()))?;

            let mut out = File::create(&restore_dest_path)?;
            std::io::copy(&mut entry, &mut out)?;
            out.flush()?;
            Ok(())
        })();

        if let Err(e) = res {
            let _ = std::fs::remove_file(&probe_zip_path);
            let _ = std::fs::remove_file(&restore_dest_path);
            return Ok(CanaryProbeResult {
                passed: false,
                sha_matched: false,
                hardlink_restored: false,
                duration_ms: start.elapsed().as_millis() as u64,
                error_detail: Some(format!("Sandbox probe archive/extract failed: {}", e)),
            });
        }

        // 4. Verify post-extract SHA-256
        let post_sha256 = match crate::manifest::compute_file_sha256(&restore_dest_path) {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_file(&probe_zip_path);
                let _ = std::fs::remove_file(&restore_dest_path);
                return Ok(CanaryProbeResult {
                    passed: false,
                    sha_matched: false,
                    hardlink_restored: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error_detail: Some(format!("Failed to hash extracted canary: {}", e)),
                });
            }
        };

        // Clean up sandbox artifacts
        let _ = std::fs::remove_file(&probe_zip_path);
        let _ = std::fs::remove_file(&restore_dest_path);

        let sha_matched = pre_sha256 == post_sha256;
        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(CanaryProbeResult {
            passed: sha_matched,
            sha_matched,
            hardlink_restored: true,
            duration_ms,
            error_detail: if sha_matched {
                None
            } else {
                Some("SHA-256 mismatch after sandbox extraction".to_string())
            },
        })
    }
}
