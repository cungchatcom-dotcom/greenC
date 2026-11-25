//! Per-drive capacity quotas: the basis for warnings, automatic backup and automatic cleanup.

use crate::state::AppState;
use chrono::Utc;
use greenc_core::models::{DriveQuota, QuotaLevel, RegenerableCacheMode};
use greenc_platform::traits::Win32VolumeApi;
use greenc_platform::volume::NativeWin32VolumeApi;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveQuotaInputDto {
    pub id: Option<i64>,
    pub drive_root: String,
    /// `0` = use the whole drive capacity.
    pub limit_bytes: u64,
    pub warn_ratio: Option<f64>,
    pub clean_ratio: Option<f64>,
    pub target_ratio: Option<f64>,
    pub auto_clean: Option<bool>,
    pub auto_backup: Option<bool>,
    pub backup_dir: Option<String>,
    pub enabled: Option<bool>,
    #[serde(default)]
    pub regenerable_cache_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveQuotaStatusDto {
    pub id: i64,
    pub drive_root: String,
    pub limit_bytes: u64,
    pub effective_limit_bytes: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub usage_ratio: f64,
    pub warn_ratio: f64,
    pub clean_ratio: f64,
    pub target_ratio: f64,
    pub auto_clean: bool,
    pub auto_backup: bool,
    pub backup_dir: Option<String>,
    pub enabled: bool,
    pub regenerable_cache_mode: String,
    pub level: String,
    pub bytes_to_reclaim: u64,
    pub available: bool,
}

pub use greenc_core::models::normalize_drive_root;

/// Builds the status DTO for a quota. Takes the volume API so it can be tested without real drives.
pub fn quota_status(quota: &DriveQuota, volume: &dyn Win32VolumeApi) -> DriveQuotaStatusDto {
    let probe = volume.get_disk_free_space(Path::new(&quota.drive_root));
    let (total, free, available) = match probe {
        Ok((t, f)) if t > 0 => (t, f, true),
        _ => (0, 0, false),
    };
    let level = if available {
        quota.level(total, free)
    } else {
        QuotaLevel::Ok
    };
    DriveQuotaStatusDto {
        id: quota.id.unwrap_or(0),
        drive_root: quota.drive_root.clone(),
        limit_bytes: quota.limit_bytes,
        effective_limit_bytes: quota.effective_limit(total),
        total_bytes: total,
        free_bytes: free,
        used_bytes: total.saturating_sub(free),
        usage_ratio: quota.usage_ratio(total, free),
        warn_ratio: quota.warn_ratio,
        clean_ratio: quota.clean_ratio,
        target_ratio: quota.target_ratio,
        auto_clean: quota.auto_clean,
        auto_backup: quota.auto_backup,
        backup_dir: quota
            .backup_dir
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        enabled: quota.enabled,
        regenerable_cache_mode: quota.regenerable_cache_mode.as_str().to_string(),
        level: level.as_str().to_string(),
        bytes_to_reclaim: quota.bytes_to_reclaim(total, free),
        available,
    }
}

/// Validates the input and turns it into a normalized [`DriveQuota`].
pub fn build_quota(input: &DriveQuotaInputDto) -> Result<DriveQuota, String> {
    let root = normalize_drive_root(&input.drive_root)
        .ok_or_else(|| "Ổ đĩa không hợp lệ (ví dụ: C:\\ hoặc /)".to_string())?;

    let mut q = DriveQuota::new(root.clone());
    q.id = input.id;
    q.limit_bytes = input.limit_bytes;
    if let Some(v) = input.warn_ratio {
        q.warn_ratio = v;
    }
    if let Some(v) = input.clean_ratio {
        q.clean_ratio = v;
    }
    if let Some(v) = input.target_ratio {
        q.target_ratio = v;
    }
    q.auto_clean = input.auto_clean.unwrap_or(false);
    q.auto_backup = input.auto_backup.unwrap_or(false);
    q.enabled = input.enabled.unwrap_or(true);
    q.regenerable_cache_mode = match input.regenerable_cache_mode.as_deref() {
        Some(raw) => RegenerableCacheMode::from_str_strict(raw)
            .ok_or_else(|| format!("Chế độ cache tái tạo không hợp lệ: '{raw}'"))?,
        None => RegenerableCacheMode::Vault,
    };
    q.backup_dir = input
        .backup_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    q.validate_backup()?;
    Ok(q.normalized())
}

#[tauri::command]
pub async fn list_drive_quotas(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<DriveQuotaStatusDto>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let volume = NativeWin32VolumeApi;
        let quotas = db
            .list_drive_quotas(false)
            .map_err(|e| format!("Lỗi tải hạn mức ổ đĩa: {e}"))?;
        Ok(quotas.iter().map(|q| quota_status(q, &volume)).collect())
    })
    .await
    .map_err(|e| format!("List drive quotas join error: {e}"))?
}

#[tauri::command]
pub async fn save_drive_quota(
    quota: DriveQuotaInputDto,
    state: State<'_, AppState>,
) -> std::result::Result<DriveQuotaStatusDto, String> {
    let explicit_mode = quota.regenerable_cache_mode.is_some();
    let mut model = build_quota(&quota)?;
    model.created_at = Utc::now();
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        if !explicit_mode {
            if let Ok(existing_list) = db.list_drive_quotas(false) {
                if let Some(existing) = existing_list
                    .into_iter()
                    .find(|q| q.drive_root.eq_ignore_ascii_case(&model.drive_root))
                {
                    model.regenerable_cache_mode = existing.regenerable_cache_mode;
                }
            }
        }
        let id = db
            .save_drive_quota(&model)
            .map_err(|e| format!("Không thể lưu hạn mức: {e}"))?;
        model.id = Some(id);
        Ok(quota_status(&model, &NativeWin32VolumeApi))
    })
    .await
    .map_err(|e| format!("Save drive quota join error: {e}"))?
}

#[tauri::command]
pub async fn delete_drive_quota(
    quota_id: i64,
    state: State<'_, AppState>,
) -> std::result::Result<bool, String> {
    state
        .db
        .delete_drive_quota(quota_id)
        .map(|()| true)
        .map_err(|e| format!("Không thể xóa hạn mức: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use greenc_core::models::DriveInfo;
    use greenc_platform::traits::MockVolumeApi;

    fn input(root: &str) -> DriveQuotaInputDto {
        DriveQuotaInputDto {
            id: None,
            drive_root: root.to_string(),
            limit_bytes: 0,
            warn_ratio: None,
            clean_ratio: None,
            target_ratio: None,
            auto_clean: None,
            auto_backup: None,
            backup_dir: None,
            enabled: None,
            regenerable_cache_mode: None,
        }
    }

    #[test]
    fn normalizes_drive_roots() {
        assert_eq!(normalize_drive_root("c"), Some("C:\\".to_string()));
        assert_eq!(normalize_drive_root(" d: "), Some("D:\\".to_string()));
        assert_eq!(normalize_drive_root("E:\\"), Some("E:\\".to_string()));
        assert_eq!(normalize_drive_root("/"), Some("/".to_string()));
        assert_eq!(normalize_drive_root("/home/"), Some("/home".to_string()));
        assert_eq!(normalize_drive_root("C:\\Users"), None);
        assert_eq!(normalize_drive_root("1:"), None);
        assert_eq!(normalize_drive_root(""), None);
    }

    #[test]
    fn rejects_bad_input_and_same_drive_backup() {
        assert!(build_quota(&input("C:\\Users")).is_err());

        let mut bad = input("C");
        bad.auto_backup = Some(true);
        assert!(build_quota(&bad).is_err(), "backup needs a folder");

        bad.backup_dir = Some("C:\\bk".to_string());
        assert!(
            build_quota(&bad).is_err(),
            "backup on the protected drive is refused"
        );

        bad.backup_dir = Some("D:\\bk".to_string());
        let q = build_quota(&bad).unwrap();
        assert_eq!(q.drive_root, "C:\\");
        assert!(q.auto_backup);
    }

    #[test]
    fn status_reports_level_and_unavailable_drives() {
        let gb = 1024u64 * 1024 * 1024;
        let volume = MockVolumeApi::new(vec![DriveInfo {
            root: PathBuf::from("C:\\"),
            drive_letter: 'C',
            label: "Windows".to_string(),
            filesystem: "NTFS".to_string(),
            total_bytes: 100 * gb,
            free_bytes: 5 * gb,
            is_system: true,
            is_eligible: false,
            ineligible_reason: None,
        }]);
        let q = DriveQuota::new("C:\\");
        let st = quota_status(&q, &volume);
        assert_eq!(st.level, "critical");
        assert!(st.available);
        assert_eq!(st.bytes_to_reclaim, 10 * gb);
    }
}
