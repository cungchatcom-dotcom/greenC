//! Keeps Vault archive records pointing at ZIPs that actually exist.
//!
//! Vault folders move: the project was renamed (`C:\DiskGuard\vault` → `C:\Green C\Vault`), users
//! change the data root, legacy databases are imported with their old absolute paths. A record whose
//! ZIP is not where it says would promise a restore that can only fail. This pass looks for the same
//! file name in the known Vault folders and re-points the record, or marks it `MISSING` when the ZIP
//! is gone everywhere.

use crate::db::GreenCDb;
use crate::settings::AppSettings;
use greenc_core::error::Result;
use greenc_core::models::VaultStatus;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VaultRepairReport {
    /// (archive id, old zip path, new zip path)
    pub relocated: Vec<(i64, PathBuf, PathBuf)>,
    /// Archives whose ZIP was found nowhere; now `MISSING`.
    pub missing: Vec<i64>,
}

/// Vault folders worth searching on this machine: the configured one, the default one and the
/// folders used by earlier releases (DiskGuard, "Green C", GreenC).
pub fn default_vault_search_dirs(db: &GreenCDb) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(Some(root)) = AppSettings::new(db).get_data_root() {
        dirs.push(root.join("Vault"));
    }
    dirs.push(greenc_core::config::default_vault_dir());
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(&local).join("Green C").join("Vault"));
    }
    for legacy in [
        r"C:\Green C\Vault",
        r"C:\GreenC\Vault",
        r"C:\DiskGuard\vault",
    ] {
        dirs.push(PathBuf::from(legacy));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.to_string_lossy().to_lowercase()));
    dirs
}

fn sibling_manifest(zip: &Path) -> Option<PathBuf> {
    let stem = zip.file_stem()?.to_string_lossy().to_string();
    Some(zip.with_file_name(format!("{stem}.manifest.json")))
}

/// Re-points quarantined archives whose ZIP moved into one of `search_dirs`, and marks those whose
/// ZIP exists nowhere as [`VaultStatus::Missing`]. Finished archives (restored, purged) are history
/// and left untouched. Idempotent.
pub fn repair_vault_paths(db: &GreenCDb, search_dirs: &[PathBuf]) -> Result<VaultRepairReport> {
    let mut report = VaultRepairReport::default();
    for archive in db.list_vault_archives(Some(VaultStatus::Quarantined.as_str()))? {
        let Some(id) = archive.id else { continue };
        if archive.zip_path.exists() {
            continue;
        }
        let Some(name) = archive.zip_path.file_name() else {
            continue;
        };

        let found = search_dirs
            .iter()
            .map(|d| d.join(name))
            .find(|candidate| candidate.is_file());

        match found {
            Some(new_zip) => {
                let new_manifest = sibling_manifest(&new_zip)
                    .filter(|m| m.exists())
                    .unwrap_or_else(|| {
                        new_zip
                            .parent()
                            .unwrap_or(Path::new(""))
                            .join(archive.manifest_path.file_name().unwrap_or_default())
                    });
                db.update_vault_paths(id, &new_zip, &new_manifest)?;
                info!(
                    "Vault archive {id} re-pointed: {} -> {}",
                    archive.zip_path.display(),
                    new_zip.display()
                );
                report
                    .relocated
                    .push((id, archive.zip_path.clone(), new_zip));
            }
            None => {
                db.update_vault_status(
                    id,
                    VaultStatus::Missing.as_str(),
                    &format!(
                        "Gói zip không còn trên đĩa ({}); không thể khôi phục",
                        archive.zip_path.display()
                    ),
                )?;
                warn!(
                    "Vault archive {id} marked MISSING: {} not found in any Vault folder",
                    archive.zip_path.display()
                );
                report.missing.push(id);
            }
        }
    }
    Ok(report)
}

/// Startup hook used by the app and the CLI.
pub fn repair_vault_paths_default(db: &GreenCDb) -> Result<VaultRepairReport> {
    repair_vault_paths(db, &default_vault_search_dirs(db))
}
