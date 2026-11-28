//! Tier 5 Installed Application Radar — discovers user-installed apps and ranks uninstall
//! suggestions **least-recently-used & largest first** when drive space is still below goal.
//!
//! Safety guarantees:
//! - Read-only discovery (`#![deny(unsafe_code)]`).
//! - Filters out Windows OS components, drivers, VC++/NET runtimes, and GreenC itself.
//! - Never silently uninstalls; UI launches the official uninstaller or OS Apps settings.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimum app size (`30 MiB`) to be worth suggesting for uninstall.
pub const MIN_SUGGEST_APP_BYTES: u64 = 30 * 1024 * 1024;

/// System/runtime/driver substrings that must NEVER be suggested for uninstall.
const PROTECTED_APP_PATTERNS: &[&str] = &[
    "greenc",
    "microsoft visual c++",
    "microsoft .net",
    "windows sdk",
    "windows driver",
    "microsoft edge webview2",
    "microsoft update health",
    "nvidia graphics driver",
    "nvidia physx",
    "intel(r) graphics",
    "intel(r) management",
    "amd chipset",
    "realtek",
    "bluetooth",
    "wifi",
    "wi-fi",
    "audio driver",
    "security update",
    "hotfix",
    "redistributable",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledAppSuggestion {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub install_location: Option<String>,
    pub size_bytes: u64,
    pub last_used_days: u64,
    pub uninstall_command: Option<String>,
}

/// Returns `true` if `name` / `publisher` matches a critical system, runtime, or driver package.
pub fn is_protected_system_app(name: &str, publisher: &str) -> bool {
    let n = name.trim().to_lowercase();
    let p = publisher.trim().to_lowercase();
    if n.is_empty() {
        return true;
    }
    PROTECTED_APP_PATTERNS
        .iter()
        .any(|pat| n.contains(pat) || (pat.len() > 8 && p.contains(pat)))
}

/// Filters out protected/tiny apps, deduplicates by name, and ranks **least-recently-used first**
/// (`last_used_days` descending) then **largest first** (`size_bytes` descending).
pub fn rank_uninstall_suggestions(
    apps: Vec<InstalledAppSuggestion>,
    limit: usize,
) -> Vec<InstalledAppSuggestion> {
    let mut by_name: std::collections::BTreeMap<String, InstalledAppSuggestion> =
        std::collections::BTreeMap::new();
    for app in apps {
        if app.size_bytes < MIN_SUGGEST_APP_BYTES {
            continue;
        }
        if is_protected_system_app(&app.name, &app.publisher) {
            continue;
        }
        let key = app.name.trim().to_lowercase();
        by_name
            .entry(key)
            .and_modify(|existing| {
                if app.size_bytes > existing.size_bytes {
                    *existing = app.clone();
                }
            })
            .or_insert(app);
    }

    let mut ranked: Vec<InstalledAppSuggestion> = by_name.into_values().collect();
    ranked.sort_by(|a, b| {
        // Group into inactivity buckets (7-day granularity) so genuinely unused heavy apps lead
        let bucket_a = a.last_used_days / 7;
        let bucket_b = b.last_used_days / 7;
        bucket_b
            .cmp(&bucket_a)
            .then_with(|| b.size_bytes.cmp(&a.size_bytes))
            .then_with(|| a.name.cmp(&b.name))
    });
    ranked.truncate(limit);
    ranked
}

/// Estimates days since last activity for an install folder (or falls back to `install_date` YYYYMMDD).
pub fn estimate_app_inactive_days(
    install_location: Option<&str>,
    install_date_yyyymmdd: Option<&str>,
    now: SystemTime,
) -> u64 {
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(loc) = install_location {
        let p = Path::new(loc);
        if p.exists() {
            let mut latest = 0u64;
            if let Ok(m) = std::fs::metadata(p) {
                if let Ok(t) = m.modified() {
                    latest = latest.max(
                        t.duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    );
                }
            }
            if let Ok(entries) = std::fs::read_dir(p) {
                for entry in entries.flatten().take(40) {
                    if let Ok(m) = entry.metadata() {
                        if let Ok(t) = m.modified() {
                            latest = latest.max(
                                t.duration_since(UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0),
                            );
                        }
                    }
                }
            }
            if latest > 0 {
                return now_secs.saturating_sub(latest) / 86_400;
            }
        }
    }

    if let Some(dstr) = install_date_yyyymmdd {
        let trimmed = dstr.trim();
        if trimmed.len() == 8 {
            if let Ok(nd) = chrono::NaiveDate::parse_from_str(trimmed, "%Y%m%d") {
                let today = chrono::Utc::now().date_naive();
                return (today - nd).num_days().max(0) as u64;
            }
        }
    }

    30
}

/// Parses tab-separated Windows PowerShell uninstall inventory rows:
/// `DisplayName\tPublisher\tEstimatedSizeKB\tInstallLocation\tInstallDate\tUninstallString`
pub fn parse_windows_uninstall_tsv(tsv: &str, now: SystemTime) -> Vec<InstalledAppSuggestion> {
    let mut out = Vec::new();
    for (idx, line) in tsv.lines().enumerate() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 6 {
            continue;
        }
        let name = cols[0].trim();
        if name.is_empty() {
            continue;
        }
        let publisher = cols[1].trim().to_string();
        let size_kb: u64 = cols[2].trim().parse().unwrap_or(0);
        let size_bytes = size_kb.saturating_mul(1024);
        let install_location = {
            let s = cols[3].trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };
        let install_date = {
            let s = cols[4].trim();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        };
        let uninstall_command = {
            let s = cols[5].trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        };

        let last_used_days =
            estimate_app_inactive_days(install_location.as_deref(), install_date, now);
        let slug: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();

        out.push(InstalledAppSuggestion {
            id: format!("app_{idx}_{slug}"),
            name: name.to_string(),
            publisher,
            install_location,
            size_bytes,
            last_used_days,
            uninstall_command,
        });
    }
    out
}

/// Discovers installed desktop apps on the current OS and returns top `limit` least-used heavy apps.
pub fn discover_unused_apps(limit: usize) -> Vec<InstalledAppSuggestion> {
    let now = SystemTime::now();

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let ps = r#"$paths = @(
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
  'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
Get-ItemProperty $paths -ErrorAction SilentlyContinue |
  Where-Object { $_.DisplayName -and -not $_.SystemComponent -and $_.EstimatedSize -gt 30720 } |
  ForEach-Object {
    $n = ($_.DisplayName -replace "[\r\n\t]", ' ')
    $p = ($_.Publisher -replace "[\r\n\t]", ' ')
    $s = [uint64]$_.EstimatedSize
    $l = ($_.InstallLocation -replace "[\r\n\t]", ' ')
    $d = ($_.InstallDate -replace "[\r\n\t]", ' ')
    $u = ($_.UninstallString -replace "[\r\n\t]", ' ')
    "$n`t$p`t$s`t$l`t$d`t$u"
  }"#;
        let Ok(out) = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", ps])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let parsed = parse_windows_uninstall_tsv(&text, now);
        rank_uninstall_suggestions(parsed, limit)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // On Debian/Ubuntu Linux: query GUI packages with desktop files + dpkg Installed-Size
        let Ok(out) = std::process::Command::new("dpkg-query")
            .args(["-W", "-f=${Package}\t${Installed-Size}\t${Section}\n"])
            .output()
        else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut apps = Vec::new();
        for (idx, line) in text.lines().enumerate() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 3 {
                continue;
            }
            let pkg = cols[0].trim();
            let size_kb: u64 = cols[1].trim().parse().unwrap_or(0);
            let section = cols[2].trim().to_lowercase();
            // Skip core libraries & system sections
            if section.contains("libs")
                || section.contains("kernel")
                || section.contains("admin")
                || section.contains("base")
                || pkg.starts_with("lib")
                || pkg.starts_with("linux-")
            {
                continue;
            }
            let size_bytes = size_kb.saturating_mul(1024);
            if size_bytes < MIN_SUGGEST_APP_BYTES {
                continue;
            }
            let bin_path = format!("/usr/bin/{pkg}");
            let loc = if Path::new(&bin_path).exists() {
                Some(bin_path)
            } else {
                None
            };
            let last_used_days = estimate_app_inactive_days(loc.as_deref(), None, now);
            apps.push(InstalledAppSuggestion {
                id: format!("pkg_{idx}_{pkg}"),
                name: pkg.to_string(),
                publisher: section,
                install_location: loc,
                size_bytes,
                last_used_days,
                uninstall_command: Some(format!("sudo apt remove {pkg}")),
            });
        }
        rank_uninstall_suggestions(apps, limit)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = now;
        let mut apps = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/Applications") {
            for (idx, entry) in entries.flatten().enumerate() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("app") {
                    continue;
                }
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let loc = p.to_string_lossy().to_string();
                let last_used_days = estimate_app_inactive_days(Some(&loc), None, now);
                apps.push(InstalledAppSuggestion {
                    id: format!("mac_{idx}_{name}"),
                    name,
                    publisher: String::new(),
                    install_location: Some(loc),
                    size_bytes: 100 * 1024 * 1024,
                    last_used_days,
                    uninstall_command: None,
                });
            }
        }
        rank_uninstall_suggestions(apps, limit)
    }
}

/// Safely parses a Windows `UninstallString` into `(executable, args)`.
/// Rejects shell metacharacters (`&`, `|`, `;`, `>`, `<`, `` ` ``, `$`, newlines) to prevent
/// command injection; callers should fall back to opening OS Apps settings when `None`.
pub fn parse_safe_uninstall_command(raw: &str) -> Option<(String, Vec<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed
        .chars()
        .any(|c| matches!(c, '&' | '|' | ';' | '>' | '<' | '`' | '$' | '\r' | '\n'))
    {
        return None;
    }

    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in trimmed.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if in_quotes {
        return None;
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    if tokens.is_empty() {
        return None;
    }
    let exe = tokens.remove(0);
    if exe.is_empty() {
        return None;
    }
    Some((exe, tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_safe_uninstall_commands_and_rejects_shell_injection() {
        assert_eq!(
            parse_safe_uninstall_command(r#""C:\Program Files\My App\unins000.exe" /NORESTART"#),
            Some((
                r"C:\Program Files\My App\unins000.exe".to_string(),
                vec!["/NORESTART".to_string()]
            ))
        );
        assert_eq!(
            parse_safe_uninstall_command("MsiExec.exe /I{1234-5678}"),
            Some(("MsiExec.exe".to_string(), vec!["/I{1234-5678}".to_string()]))
        );
        assert_eq!(parse_safe_uninstall_command("unins.exe && calc.exe"), None);
        assert_eq!(parse_safe_uninstall_command("unins.exe | whoami"), None);
        assert_eq!(parse_safe_uninstall_command(""), None);
    }

    #[test]
    fn filters_system_components_and_ranks_least_used_largest_first() {
        let tsv = "\
Microsoft Visual C++ 2022 Redistributable\tMicrosoft\t80000\t\t20240101\tmsiexec /x 1
GreenC\tCungChat\t90000\t\t20240101\tunins000.exe
TinyTool\tAcme\t5000\t\t20230101\tunins.exe
OldHeavyGame\tStudioA\t4500000\t\t20240201\tC:\\Games\\Old\\unins.exe
RecentEditor\tStudioB\t2000000\t\t20261125\tC:\\Apps\\Ed\\unins.exe
OldMediumApp\tStudioC\t600000\t\t20240201\tC:\\Apps\\Med\\unins.exe
";
        let now = SystemTime::now();
        let parsed = parse_windows_uninstall_tsv(tsv, now);
        let ranked = rank_uninstall_suggestions(parsed, 10);

        let names: Vec<&str> = ranked.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["OldHeavyGame", "OldMediumApp", "RecentEditor"]);
    }
}
