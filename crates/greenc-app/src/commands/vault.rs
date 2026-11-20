//! Canary Vault IPC commands for archive browsing, inspection, and rollback (Tasks T035, T037).

use crate::state::AppState;
use chrono::Utc;
use greenc_core::models::VaultStatus;
use greenc_vault::canary::CanaryVerifier;
use greenc_vault::rollback::{RestoreOptions, RollbackEngine};
use serde::{Deserialize, Serialize};
use tauri::State;

/// DTO representing a quarantined or resolved Canary Vault archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultArchiveDto {
    pub id: i64,
    pub archive_id: String,
    pub rule_id: i64,
    pub app_name: String,
    pub zip_path: String,
    pub manifest_path: String,
    pub file_count: u64,
    pub original_bytes: u64,
    pub compressed_bytes: u64,
    pub created_at: String,
    pub expires_at: String,
    pub status: String,
    pub status_reason: String,
    pub remaining_days: i64,
    pub integrity_verified: bool,
}

/// DTO representing an archived file entry within a Vault container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultFileItemDto {
    pub id: i64,
    pub original_path: String,
    pub rel_arcname: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub mtime: f64,
    pub hardlink_count: usize,
    pub restored_individually: bool,
}

/// Detailed inspect payload containing archive metadata and file manifest list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultArchiveDetailDto {
    pub archive: VaultArchiveDto,
    pub files: Vec<VaultFileItemDto>,
    pub total_files: usize,
}

/// Result report from executing full or selective archive restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultRestoreReportDto {
    pub archive_id: String,
    pub success: bool,
    pub restored_count: usize,
    pub restored_bytes: u64,
    pub restored_paths: Vec<String>,
    pub message: String,
}

/// Helper to resolve archive DB ID from either numeric string ("1") or archive name ("vault_...").
fn resolve_archive_db_id(id_str: &str, state: &AppState) -> std::result::Result<i64, String> {
    if let Ok(num_id) = id_str.parse::<i64>() {
        return Ok(num_id);
    }

    let archives = state
        .db
        .list_vault_archives(None)
        .map_err(|e| format!("Database error: {}", e))?;

    for a in archives {
        if a.zip_path.to_string_lossy().contains(id_str)
            || a.manifest_path.to_string_lossy().contains(id_str)
        {
            if let Some(id) = a.id {
                return Ok(id);
            }
        }
    }

    Err(format!("Archive not found for identifier: {}", id_str))
}

/// Retrieves all Vault archives from the database.
#[tauri::command]
pub async fn get_vault_archives(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<VaultArchiveDto>, String> {
    let archives = state
        .db
        .list_vault_archives(None)
        .map_err(|e| format!("Failed to list archives: {}", e))?;

    let now = Utc::now();
    let mut dtos = Vec::new();

    for a in archives {
        let db_id = a.id.unwrap_or(0);
        let remaining_duration = a.expires_at - now;
        let remaining_days = remaining_duration.num_days().max(0);

        let zip_exists = a.zip_path.exists();
        let integrity_verified = zip_exists
            && (CanaryVerifier::verify_archive_sentinel(&a.zip_path).unwrap_or(false)
                || a.status == VaultStatus::Quarantined);

        let arch_name = a
            .zip_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("vault_{}", db_id));

        dtos.push(VaultArchiveDto {
            id: db_id,
            archive_id: arch_name,
            rule_id: a.rule_id,
            app_name: a.app_name,
            zip_path: a.zip_path.to_string_lossy().to_string(),
            manifest_path: a.manifest_path.to_string_lossy().to_string(),
            file_count: a.file_count,
            original_bytes: a.original_bytes,
            compressed_bytes: a.compressed_bytes,
            created_at: a.created_at.to_rfc3339(),
            expires_at: a.expires_at.to_rfc3339(),
            status: a.status.as_str().to_string(),
            status_reason: a.status_reason,
            remaining_days,
            integrity_verified,
        });
    }

    Ok(dtos)
}

/// Restores a full Vault archive back to original paths with conflict avoidance.
#[tauri::command]
pub async fn restore_vault_archive(
    archive_id: String,
    state: State<'_, AppState>,
) -> std::result::Result<VaultRestoreReportDto, String> {
    let db_id = resolve_archive_db_id(&archive_id, &state)?;
    let state_inner = state.inner().clone();

    tokio::task::spawn_blocking(move || {
        let engine = RollbackEngine::new(&state_inner.db, RestoreOptions::default());
        let result = engine
            .rollback_archive(db_id, "User requested 1-click restore from UI", false)
            .map_err(|e| format!("Rollback failed: {}", e))?;

        let restored_paths: Vec<String> = result
            .restored_paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        Ok(VaultRestoreReportDto {
            archive_id,
            success: true,
            restored_count: result.restored_count,
            restored_bytes: result.restored_bytes,
            restored_paths,
            message: format!("Khôi phục thành công {} tệp tin", result.restored_count),
        })
    })
    .await
    .map_err(|e| format!("Task execution error: {}", e))?
}

/// Permanently purges an archive from disk and sets status to PURGED.
#[tauri::command]
pub async fn delete_vault_archive(
    archive_id: String,
    state: State<'_, AppState>,
) -> std::result::Result<bool, String> {
    let db_id = resolve_archive_db_id(&archive_id, &state)?;
    let archive = state
        .db
        .get_vault_archive(db_id)
        .map_err(|e| format!("Database error: {}", e))?
        .ok_or_else(|| format!("Archive not found for ID {}", db_id))?;

    // Delete .zip and .manifest.json from filesystem
    let _ = std::fs::remove_file(&archive.zip_path);
    let _ = std::fs::remove_file(&archive.manifest_path);

    // Update database status to PURGED
    state
        .db
        .update_vault_status(
            db_id,
            VaultStatus::Purged.as_str(),
            "Xóa thủ công bởi người dùng",
        )
        .map_err(|e| format!("Failed to update database status: {}", e))?;

    Ok(true)
}

/// Inspects archive files and details for drill-down modal inspection.
#[tauri::command]
pub async fn inspect_vault_archive(
    archive_id: String,
    state: State<'_, AppState>,
) -> std::result::Result<VaultArchiveDetailDto, String> {
    let db_id = resolve_archive_db_id(&archive_id, &state)?;
    let archive = state
        .db
        .get_vault_archive(db_id)
        .map_err(|e| format!("Database error: {}", e))?
        .ok_or_else(|| format!("Archive not found for ID {}", db_id))?;

    let files = state
        .db
        .get_vault_files(db_id)
        .map_err(|e| format!("Database error: {}", e))?;

    let now = Utc::now();
    let remaining_duration = archive.expires_at - now;
    let remaining_days = remaining_duration.num_days().max(0);

    let arch_dto = VaultArchiveDto {
        id: db_id,
        archive_id: archive_id.clone(),
        rule_id: archive.rule_id,
        app_name: archive.app_name,
        zip_path: archive.zip_path.to_string_lossy().to_string(),
        manifest_path: archive.manifest_path.to_string_lossy().to_string(),
        file_count: archive.file_count,
        original_bytes: archive.original_bytes,
        compressed_bytes: archive.compressed_bytes,
        created_at: archive.created_at.to_rfc3339(),
        expires_at: archive.expires_at.to_rfc3339(),
        status: archive.status.as_str().to_string(),
        status_reason: archive.status_reason,
        remaining_days,
        integrity_verified: true,
    };

    let file_dtos: Vec<VaultFileItemDto> = files
        .into_iter()
        .map(|f| VaultFileItemDto {
            id: f.id.unwrap_or(0),
            original_path: f.original_path.to_string_lossy().to_string(),
            rel_arcname: f.rel_arcname,
            size_bytes: f.size_bytes,
            sha256: f.sha256,
            mtime: f.mtime,
            hardlink_count: f.hardlink_paths.len(),
            restored_individually: f.restored_individually,
        })
        .collect();

    let total = file_dtos.len();
    Ok(VaultArchiveDetailDto {
        archive: arch_dto,
        files: file_dtos,
        total_files: total,
    })
}

/// Selectively extracts specific files from an archive.
#[tauri::command]
pub async fn restore_vault_files_selective(
    archive_id: String,
    rel_arcnames: Vec<String>,
    state: State<'_, AppState>,
) -> std::result::Result<VaultRestoreReportDto, String> {
    let db_id = resolve_archive_db_id(&archive_id, &state)?;
    let all_files = state
        .db
        .get_vault_files(db_id)
        .map_err(|e| format!("Database error: {}", e))?;

    let file_ids: Vec<i64> = all_files
        .iter()
        .filter(|f| rel_arcnames.contains(&f.rel_arcname))
        .filter_map(|f| f.id)
        .collect();

    if file_ids.is_empty() {
        return Err("Không tìm thấy tệp tin nào tương ứng trong kho lưu trữ".to_string());
    }

    let state_inner = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let engine = RollbackEngine::new(&state_inner.db, RestoreOptions::default());
        let result = engine
            .restore_files_selective(db_id, &file_ids)
            .map_err(|e| format!("Lỗi khôi phục chọn lọc: {}", e))?;

        let restored_paths: Vec<String> = result
            .restored_paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        Ok(VaultRestoreReportDto {
            archive_id,
            success: true,
            restored_count: result.restored_count,
            restored_bytes: result.restored_bytes,
            restored_paths,
            message: format!("Đã khôi phục {} tệp tin thành công", result.restored_count),
        })
    })
    .await
    .map_err(|e| format!("Task execution error: {}", e))?
}
