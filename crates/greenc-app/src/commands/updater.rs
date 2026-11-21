//! GitHub Releases auto-update & manual update checker and installer launcher.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::AppHandle;

const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/cungchatcom-dotcom/greenC/releases/latest";
const ALLOWED_DOWNLOAD_PREFIX: &str =
    "https://github.com/cungchatcom-dotcom/greenC/releases/download/";
const RELEASES_PAGE_URL: &str = "https://github.com/cungchatcom-dotcom/greenC/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckDto {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
    pub download_url: Option<String>,
    pub asset_name: Option<String>,
    pub release_notes: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseResponse {
    tag_name: String,
    html_url: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Parses a `vX.Y.Z` or `X.Y.Z` semver string into `(major, minor, patch)`.
pub fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let clean = version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V');
    let core = clean.split('-').next().unwrap_or(clean);
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// Returns `true` if `latest` is strictly newer than `current`.
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    match (parse_semver(current), parse_semver(latest)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    }
}

fn select_platform_asset(assets: &[GithubAsset]) -> Option<(&str, &str)> {
    #[cfg(target_os = "windows")]
    let matcher = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".exe") && lower.contains("setup")
    };

    #[cfg(target_os = "macos")]
    let matcher = |name: &str| name.to_ascii_lowercase().ends_with(".dmg");

    #[cfg(target_os = "linux")]
    let matcher = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".appimage") || lower.ends_with(".deb")
    };

    assets
        .iter()
        .find(|a| matcher(&a.name))
        .map(|a| (a.name.as_str(), a.browser_download_url.as_str()))
}

/// Queries GitHub Releases for the latest GreenC release and compares it with `CARGO_PKG_VERSION`.
#[tauri::command]
pub async fn check_for_updates() -> std::result::Result<UpdateCheckDto, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(format!("GreenC/{current_version}"))
        .build()
        .map_err(|e| format!("Không thể khởi tạo kết nối kiểm tra cập nhật: {e}"))?;

    let resp = client
        .get(GITHUB_LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Không thể kết nối máy chủ cập nhật: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Máy chủ cập nhật trả về mã lỗi {}",
            resp.status().as_u16()
        ));
    }

    let release: GithubReleaseResponse = resp
        .json()
        .await
        .map_err(|e| format!("Dữ liệu phiên bản mới không hợp lệ: {e}"))?;

    let latest_version = release
        .tag_name
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();
    let has_update = is_newer_version(&current_version, &latest_version);
    let (asset_name, download_url) = select_platform_asset(&release.assets)
        .map(|(n, u)| (Some(n.to_string()), Some(u.to_string())))
        .unwrap_or((None, None));

    let notes = release
        .body
        .unwrap_or_default()
        .lines()
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(UpdateCheckDto {
        current_version,
        latest_version,
        has_update,
        release_url: release
            .html_url
            .unwrap_or_else(|| RELEASES_PAGE_URL.to_string()),
        download_url,
        asset_name,
        release_notes: notes,
        published_at: release.published_at,
    })
}

/// Downloads the official GitHub Release installer to the system temp directory and launches it.
/// On Windows, exits the running GreenC process shortly after launching the NSIS installer so the
/// binary can be upgraded cleanly without file-in-use errors.
#[tauri::command]
pub async fn download_and_install_update(
    download_url: String,
    asset_name: String,
    app: AppHandle,
) -> std::result::Result<String, String> {
    if !download_url.starts_with(ALLOWED_DOWNLOAD_PREFIX) {
        return Err("Liên kết tải bản cập nhật không thuộc kho phát hành chính thức.".to_string());
    }

    let safe_name = std::path::Path::new(&asset_name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && !n.contains(".."))
        .ok_or_else(|| "Tên tệp cập nhật không hợp lệ.".to_string())?;

    let current_version = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent(format!("GreenC/{current_version}"))
        .build()
        .map_err(|e| format!("Không thể khởi tạo trình tải cập nhật: {e}"))?;

    let resp = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("Không thể tải bộ cài mới: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Tải bộ cài thất bại (HTTP {})", resp.status().as_u16()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Lỗi đọc dữ liệu bộ cài: {e}"))?;

    let update_dir = std::env::temp_dir().join("greenc-update");
    std::fs::create_dir_all(&update_dir)
        .map_err(|e| format!("Không thể tạo thư mục tạm cho bản cập nhật: {e}"))?;
    let dest_path = update_dir.join(safe_name);
    std::fs::write(&dest_path, &bytes)
        .map_err(|e| format!("Không thể lưu tệp cài đặt mới: {e}"))?;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(&dest_path)
            .spawn()
            .map_err(|e| format!("Không thể khởi chạy bộ cài mới: {e}"))?;
        let app_clone = app.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            app_clone.exit(0);
        });
    }

    #[cfg(target_os = "macos")]
    {
        let _ = app;
        std::process::Command::new("open")
            .arg(&dest_path)
            .spawn()
            .map_err(|e| format!("Không thể mở tệp .dmg mới: {e}"))?;
    }

    #[cfg(target_os = "linux")]
    {
        let _ = app;
        let _ = std::process::Command::new("xdg-open")
            .arg(&update_dir)
            .spawn();
    }

    Ok(dest_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison_handles_v_prefix_and_ordering() {
        assert!(is_newer_version("2.1.0", "2.1.1"));
        assert!(is_newer_version("2.1.0", "v2.2.0"));
        assert!(is_newer_version("2.1.0", "3.0.0"));
        assert!(!is_newer_version("2.1.0", "2.1.0"));
        assert!(!is_newer_version("2.1.0", "v2.0.9"));
        assert!(!is_newer_version("2.1.0", "invalid"));
    }
}
