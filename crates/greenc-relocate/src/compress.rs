//! In-place transparent filesystem compaction (WOF CompactOS & LZNT1) (Tasks T049, T051).

use chrono::Utc;
use greenc_core::error::{GreenCError, Result};
use greenc_core::models::{CompactionKind, CompactionRecord};
use greenc_core::task::TaskControl;
use greenc_storage::db::GreenCDb;
use greenc_win32::traits::{CompactionApi, FileLockProbe, WofAlgorithm};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Outcome of undoing an in-place compaction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecompressReport {
    pub files_total: u64,
    /// Files that could not be decompressed, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

impl DecompressReport {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

pub const MIN_COMPACTION_FILE_SIZE: u64 = 4096; // 4 KiB minimum
pub const COLD_AGE_THRESHOLD_DAYS: u64 = 30; // 30 days cold

/// File extensions well-suited for WOF CompactOS transparent compression.
const COMPRESSIBLE_EXTENSIONS: &[&str] = &[
    "exe", "dll", "bin", "pak", "dat", "log", "txt", "sys", "lib", "pdb", "obj", "wasm", "node",
    "json", "xml", "csv", "tsv", "iso", "vmdk", "qcow2",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCandidate {
    pub path: PathBuf,
    pub file_count: u64,
    pub logical_bytes: u64,
    pub estimated_reclaim_bytes: u64,
    pub category: String,
}

pub struct InPlaceCompactor;

impl InPlaceCompactor {
    /// Discovers candidate cold files within a directory matching size, age, and extension criteria.
    pub fn scan_candidate_files(dir: &Path, age_days: u64) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if !dir.exists() || !dir.is_dir() {
            return candidates;
        }

        let now = SystemTime::now();
        let max_age = Duration::from_secs(age_days * 86400);

        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            if let Ok(entries) = fs::read_dir(&current) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Ok(ft) = entry.file_type() {
                        if ft.is_symlink() {
                            continue;
                        }
                        if ft.is_dir() {
                            stack.push(path);
                        } else if ft.is_file() {
                            if let Ok(meta) = entry.metadata() {
                                let len = meta.len();
                                if len >= MIN_COMPACTION_FILE_SIZE {
                                    // Check extension
                                    let ext = path
                                        .extension()
                                        .and_then(|e| e.to_str())
                                        .map(|e| e.to_lowercase())
                                        .unwrap_or_default();

                                    let is_ext_match =
                                        COMPRESSIBLE_EXTENSIONS.contains(&ext.as_str());

                                    // Check cold age
                                    let is_cold = meta
                                        .modified()
                                        .ok()
                                        .and_then(|mtime| now.duration_since(mtime).ok())
                                        .map(|d| d >= max_age)
                                        .unwrap_or(false);

                                    if is_ext_match || is_cold {
                                        candidates.push(path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        candidates
    }

    /// Discovers top-level directories in common developer/app locations suitable for compaction.
    pub fn discover_directories_for_compaction() -> Vec<CompactionCandidate> {
        let mut results = Vec::new();
        let mut probe_roots = Vec::new();

        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            let p = PathBuf::from(userprofile);
            probe_roots.push(p.join(".cargo"));
            probe_roots.push(p.join(".rustup"));
            probe_roots.push(p.join(".gradle"));
            probe_roots.push(p.join(".m2"));
            probe_roots.push(p.join(".nuget"));
        }

        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(local_appdata);
            probe_roots.push(p.join("Programs"));
            probe_roots.push(p.join("pip").join("cache"));
            probe_roots.push(p.join("uv"));
            probe_roots.push(p.join("Yarn"));
        }

        for root in probe_roots {
            if root.exists() && root.is_dir() {
                let candidates = Self::scan_candidate_files(&root, COLD_AGE_THRESHOLD_DAYS);
                if !candidates.is_empty() {
                    let mut total_bytes = 0u64;
                    for f in &candidates {
                        if let Ok(meta) = fs::metadata(f) {
                            total_bytes += meta.len();
                        }
                    }

                    if total_bytes >= 10 * 1024 * 1024 {
                        // At least 10 MiB
                        let est_reclaim = (total_bytes as f64 * 0.40) as u64; // Est. 40% reclaim
                        results.push(CompactionCandidate {
                            path: root,
                            file_count: candidates.len() as u64,
                            logical_bytes: total_bytes,
                            estimated_reclaim_bytes: est_reclaim,
                            category: "cold_directory".to_string(),
                        });
                    }
                }
            }
        }

        results
    }

    /// Compresses all eligible files inside a target directory using WOF CompactOS,
    /// reverting any file that fails to achieve >= 10% space reduction.
    pub fn compact_directory<F>(
        dir: &Path,
        algorithm: WofAlgorithm,
        db: &GreenCDb,
        compaction_api: &dyn CompactionApi,
        lock_probe: &dyn FileLockProbe,
        ctrl: &TaskControl,
        mut on_progress: F,
    ) -> Result<CompactionRecord>
    where
        F: FnMut(u64, u64), // (done_bytes, done_files)
    {
        if !dir.exists() {
            return Err(GreenCError::NotFound {
                entity: "DirectoryToCompact".to_string(),
                id: dir.display().to_string(),
            });
        }
        // Refuse protected system folders as a whole instead of "succeeding" with 0 bytes saved
        // after every per-file refusal was swallowed.
        greenc_win32::handle::ensure_not_protected_system_path(dir, "compact")?;

        let files = if dir.is_file() {
            vec![dir.to_path_buf()]
        } else {
            Self::scan_candidate_files(dir, 0)
        };

        let mut total_logical = 0u64;
        let mut total_before = 0u64;
        let mut total_after = 0u64;
        let mut files_compacted = 0u64;

        for path in &files {
            if ctrl.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }
            ctrl.check_pause();

            // Skip locked files
            if lock_probe.is_file_locked(path) {
                tracing::debug!("Skipping locked file: {}", path.display());
                continue;
            }

            let logical = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let phys_before = compaction_api.get_compressed_size(path).unwrap_or(logical);

            total_logical += logical;
            total_before += phys_before;

            match compaction_api.compact_file(path, algorithm) {
                Ok(res) => {
                    // Check ratio: require at least 10% gain (phys_after < phys_before * 0.90)
                    if res.physical_bytes < (phys_before as f64 * 0.90) as u64 {
                        total_after += res.physical_bytes;
                        files_compacted += 1;
                    } else {
                        // Less than 10% gain or file expanded: roll back file
                        let _ = compaction_api.decompress_file(path);
                        total_after += phys_before;
                    }
                }
                Err(err) => {
                    tracing::warn!("Failed to compact file {}: {}", path.display(), err);
                    total_after += phys_before;
                }
            }

            on_progress(total_logical, files_compacted);
        }

        let mut record = CompactionRecord {
            id: None,
            target_path: dir.to_path_buf(),
            kind: CompactionKind::WofDirectory,
            algorithm: algorithm.as_str().to_string(),
            logical_bytes: total_logical,
            physical_before_bytes: total_before,
            physical_after_bytes: total_after,
            file_count: files_compacted,
            status: "compacted".to_string(),
            updated_at: Utc::now(),
        };

        let id = db.save_compaction_record(&record)?;
        record.id = Some(id);

        Ok(record)
    }

    /// Decompresses all files in a directory back to normal uncompressed state.
    /// Undoes a compaction GreenC performed on `dir`. Refuses folders without a compaction record
    /// (they may hold CompactOS or user-compressed files we must not inflate) and reports every
    /// file that could not be decompressed. Running it again after success is a harmless no-op.
    pub fn decompress_directory(
        dir: &Path,
        db: &GreenCDb,
        compaction_api: &dyn CompactionApi,
        ctrl: &TaskControl,
    ) -> Result<DecompressReport> {
        let Some(mut record) = db.get_compaction_record(dir)? else {
            return Err(GreenCError::PreconditionBlocked {
                operation: "compact undo".to_string(),
                reason: format!(
                    "GreenC has no compaction record for '{}'; refusing to decompress files it did not compress",
                    dir.display()
                ),
            });
        };

        let files = if dir.is_file() {
            vec![dir.to_path_buf()]
        } else {
            let mut all = Vec::new();
            let mut stack = vec![dir.to_path_buf()];
            while let Some(curr) = stack.pop() {
                if let Ok(entries) = fs::read_dir(&curr) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if let Ok(ft) = entry.file_type() {
                            if ft.is_dir() && !ft.is_symlink() {
                                stack.push(p);
                            } else if ft.is_file() {
                                all.push(p);
                            }
                        }
                    }
                }
            }
            all
        };

        let mut report = DecompressReport {
            files_total: files.len() as u64,
            failed: Vec::new(),
        };
        for path in &files {
            if ctrl.is_cancelled() {
                return Err(GreenCError::Cancelled);
            }
            ctrl.check_pause();

            if let Err(err) = compaction_api.decompress_file(path) {
                tracing::warn!("Failed to decompress {}: {}", path.display(), err);
                report.failed.push((path.clone(), err.to_string()));
            }
        }

        record.updated_at = Utc::now();
        if report.is_complete() {
            record.status = "decompressed".to_string();
            record.physical_after_bytes = record.logical_bytes;
        } else {
            record.status = "decompress_incomplete".to_string();
        }
        db.save_compaction_record(&record)?;

        Ok(report)
    }
}
