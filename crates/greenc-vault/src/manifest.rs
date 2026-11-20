//! Manifest serialization, sidecar file handling, and database indexing (Tasks T027, T033).
//!
//! Handles `.manifest.json` generation and parsing with lossless backward
//! compatibility for both GreenC v2 and legacy Python GreenC archives.

use chrono::{DateTime, Utc};
use greenc_core::error::Result;
use greenc_core::models::{FileManifestItem, VaultArchive, VaultFileItem, VaultStatus};
use greenc_storage::db::GreenCDb;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// Manifest file metadata representing a Canary Vault archive container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultManifest {
    /// Unique identifier for this archive (UUID or timestamped name).
    pub archive_id: String,
    /// Associated quota rule ID (0 for ad-hoc / 1-click cleanups).
    #[serde(default)]
    pub rule_id: i64,
    /// Display name of the application or profile.
    #[serde(default)]
    pub app_name: String,
    /// Process name associated with the app (e.g. "Zalo.exe").
    #[serde(default)]
    pub process_name: String,
    /// Service name associated with the app, if any.
    #[serde(default)]
    pub service_name: String,
    /// Timestamp when the archive was created.
    pub created_at: DateTime<Utc>,
    /// Expiration timestamp when retention window elapses.
    #[serde(default = "default_expiration")]
    pub expires_at: DateTime<Utc>,
    /// Retention window in days (default 7).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Original root path of the candidate files.
    #[serde(default)]
    pub original_root: PathBuf,
    /// Total uncompressed physical bytes across all archived files.
    #[serde(alias = "original_bytes")]
    pub total_size: u64,
    /// Total compressed container bytes on disk.
    #[serde(alias = "compressed_bytes")]
    pub compressed_size: u64,
    /// SHA-256 hash of the complete `.zip` file container.
    #[serde(alias = "archive_sha256")]
    pub sha256: String,
    /// List of archived files and their integrity metadata.
    pub files: Vec<FileManifestItem>,
}

fn default_expiration() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::days(7)
}

fn default_retention_days() -> u32 {
    7
}

impl VaultManifest {
    /// Creates a new `VaultManifest`.
    pub fn new(
        archive_id: String,
        rule_id: i64,
        app_name: String,
        created_at: DateTime<Utc>,
        retention_days: u32,
        original_root: PathBuf,
        files: Vec<FileManifestItem>,
        total_size: u64,
        compressed_size: u64,
        sha256: String,
    ) -> Self {
        let expires_at = created_at + chrono::Duration::days(retention_days as i64);
        Self {
            archive_id,
            rule_id,
            app_name,
            process_name: String::new(),
            service_name: String::new(),
            created_at,
            expires_at,
            retention_days,
            original_root,
            total_size,
            compressed_size,
            sha256,
            files,
        }
    }

    /// Reads and deserializes a sidecar `.manifest.json` file.
    pub fn read_from_file(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let manifest: Self = serde_json::from_reader(reader)?;
        Ok(manifest)
    }

    /// Serializes and writes this manifest synchronously to the specified path.
    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json_bytes = serde_json::to_vec_pretty(self)?;
        let mut file = File::create(path)?;
        file.write_all(&json_bytes)?;
        file.sync_all()?;
        Ok(())
    }

    /// Persists this manifest and its files into the SQLite database.
    pub fn persist_to_db(
        &self,
        db: &GreenCDb,
        zip_path: &Path,
        manifest_path: &Path,
    ) -> Result<i64> {
        let archive_model = VaultArchive {
            id: None,
            rule_id: self.rule_id,
            app_name: self.app_name.clone(),
            zip_path: zip_path.to_path_buf(),
            manifest_path: manifest_path.to_path_buf(),
            file_count: self.files.len() as u64,
            original_bytes: self.total_size,
            compressed_bytes: self.compressed_size,
            created_at: self.created_at,
            expires_at: self.expires_at,
            status: VaultStatus::Quarantined,
            status_reason: "Quarantined via Canary Vault".to_string(),
            resolved_at: None,
        };

        let file_models: Vec<VaultFileItem> = self.files.clone();
        db.create_vault_archive(&archive_model, &file_models)
    }
}

/// Computes hardware-accelerated SHA-256 for a given file on disk.
pub fn compute_file_sha256(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(128 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let digest = hasher.finalize();
    Ok(format!("{:02x}", digest))
}

/// Computes SHA-256 for an in-memory byte slice.
pub fn compute_bytes_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:02x}", hasher.finalize())
}
