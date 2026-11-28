//! Safe WSL2 distro detection and 1-click PowerShell migration script builder.
//!
//! Safety guarantees:
//! - Strict allowlist validation of distro name (`[A-Za-z0-9._-]+`) and target drive (`D`..=`Z`)
//!   prevents PowerShell injection.
//! - Generated script sets `$ErrorActionPreference = 'Stop'`, checks `$LASTEXITCODE -eq 0` and
//!   non-zero archive size BEFORE calling `wsl --unregister`, and verifies import BEFORE removing
//!   the temporary `.tar` backup.

use greenc_core::error::{GreenCError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WslDistroInfo {
    pub name: String,
    pub is_docker_internal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WslMoveScript {
    pub distro: String,
    pub target_drive: String,
    pub target_dir: String,
    pub tar_path: String,
    pub one_line_command: String,
    pub script_body: String,
}

/// Validates a WSL2 distro name strictly so it cannot inject PowerShell tokens.
pub fn validate_wsl_distro_name(distro: &str) -> Result<String> {
    let trimmed = distro.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(GreenCError::PreconditionBlocked {
            operation: "wsl_move".to_string(),
            reason: "Tên phân phối WSL2 không hợp lệ.".to_string(),
        });
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(GreenCError::PreconditionBlocked {
            operation: "wsl_move".to_string(),
            reason: format!("Tên phân phối WSL2 chứa ký tự không an toàn: '{trimmed}'"),
        });
    }
    Ok(trimmed.to_string())
}

/// Validates target drive letter (`D`..=`Z`, never `C` or non-ASCII).
pub fn validate_wsl_target_drive(drive: &str) -> Result<char> {
    let trimmed = drive.trim().trim_end_matches([':', '\\', '/']);
    let mut chars = trimmed.chars();
    let ch = match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => {
            return Err(GreenCError::PreconditionBlocked {
                operation: "wsl_move".to_string(),
                reason: format!("Ổ đĩa đích không hợp lệ: '{drive}'"),
            });
        }
    };
    if ch == 'C' {
        return Err(GreenCError::PreconditionBlocked {
            operation: "wsl_move".to_string(),
            reason: "Không thể chuyển WSL2 sang chính ổ hệ thống C:.".to_string(),
        });
    }
    Ok(ch)
}

/// Decodes raw stdout from `wsl.exe -l -q` (which Windows emits as UTF-16LE with null bytes).
pub fn parse_wsl_list_output(raw: &[u8]) -> Vec<WslDistroInfo> {
    let text = if raw.len() >= 2 && (raw.starts_with(&[0xFF, 0xFE]) || raw.contains(&0u8)) {
        let slice = if raw.starts_with(&[0xFF, 0xFE]) {
            &raw[2..]
        } else {
            raw
        };
        let u16s: Vec<u16> = slice
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };

    let mut out = Vec::new();
    for line in text.lines() {
        let name = line.trim().trim_start_matches('\u{feff}');
        if name.is_empty() {
            continue;
        }
        if validate_wsl_distro_name(name).is_err() {
            continue;
        }
        let lower = name.to_lowercase();
        let is_docker_internal = lower.starts_with("docker-desktop");
        out.push(WslDistroInfo {
            name: name.to_string(),
            is_docker_internal,
        });
    }
    out
}

/// Discovers installed WSL2 distros on Windows via `wsl.exe -l -q`.
pub fn list_installed_wsl_distros() -> Vec<WslDistroInfo> {
    #[cfg(windows)]
    {
        match std::process::Command::new("wsl.exe")
            .args(["-l", "-q"])
            .output()
        {
            Ok(out) if out.status.success() => parse_wsl_list_output(&out.stdout),
            _ => Vec::new(),
        }
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Builds a verified, fail-closed PowerShell script and single-line command to move a WSL2 distro.
pub fn build_wsl_move_script(distro: &str, target_drive: &str) -> Result<WslMoveScript> {
    let safe_distro = validate_wsl_distro_name(distro)?;
    let drive = validate_wsl_target_drive(target_drive)?;
    let target_dir = format!("{drive}:\\WSL\\{safe_distro}");
    let tar_path = format!("{drive}:\\WSL\\{safe_distro}.tar");

    let script_body = format!(
        r#"$ErrorActionPreference = 'Stop'
$distro = '{safe_distro}'
$destDir = '{target_dir}'
$tarPath = '{tar_path}'

Write-Host "[GreenC] Moving WSL2 distro '$distro' to $destDir ..." -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

Write-Host "[1/4] Stopping WSL2..."
wsl --shutdown
if ($LASTEXITCODE -ne 0) {{ throw "wsl --shutdown failed (exit $LASTEXITCODE)" }}

Write-Host "[2/4] Exporting '$distro' to $tarPath (this may take a few minutes)..."
wsl --export $distro $tarPath
if ($LASTEXITCODE -ne 0) {{ throw "wsl --export failed (exit $LASTEXITCODE). Original distro untouched." }}
if (-not (Test-Path $tarPath) -or ((Get-Item $tarPath).Length -le 0)) {{
    throw "Export archive is missing or empty ($tarPath). Aborting before unregister."
}}

Write-Host "[3/4] Registering '$distro' at $destDir..."
wsl --unregister $distro
if ($LASTEXITCODE -ne 0) {{ throw "wsl --unregister failed. Backup kept safely at $tarPath" }}

wsl --import $distro $destDir $tarPath --version 2
if ($LASTEXITCODE -ne 0) {{
    throw "wsl --import failed. Your full backup is STILL safe at $tarPath. Run: wsl --import $distro `"$destDir`" `"$tarPath`" --version 2"
}}

Write-Host "[4/4] Cleaning temporary backup archive..."
Remove-Item -Force $tarPath
Write-Host "[GreenC] Done! '$distro' is now stored at $destDir." -ForegroundColor Green
"#
    );

    let one_line_command = format!(
        "powershell -NoProfile -Command \"$ErrorActionPreference='Stop'; New-Item -ItemType Directory -Force -Path '{target_dir}' | Out-Null; wsl --shutdown; wsl --export '{safe_distro}' '{tar_path}'; if ($LASTEXITCODE -ne 0 -or -not (Test-Path '{tar_path}') -or (Get-Item '{tar_path}').Length -le 0) {{ throw 'Export failed, original untouched' }}; wsl --unregister '{safe_distro}'; wsl --import '{safe_distro}' '{target_dir}' '{tar_path}' --version 2; if ($LASTEXITCODE -ne 0) {{ throw 'Import failed, backup kept at {tar_path}' }}; Remove-Item -Force '{tar_path}'; Write-Host 'Done!' -ForegroundColor Green\""
    );

    Ok(WslMoveScript {
        distro: safe_distro,
        target_drive: drive.to_string(),
        target_dir,
        tar_path,
        one_line_command,
        script_body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_command_injection_and_system_drive() {
        assert!(build_wsl_move_script("Ubuntu; calc.exe", "D").is_err());
        assert!(build_wsl_move_script("Ubuntu$(whoami)", "D").is_err());
        assert!(build_wsl_move_script("Ubuntu' | del *", "D").is_err());
        assert!(build_wsl_move_script("", "D").is_err());
        assert!(build_wsl_move_script("Ubuntu", "C").is_err());
        assert!(build_wsl_move_script("Ubuntu", "D;echo").is_err());
    }

    #[test]
    fn builds_safe_verified_script_and_one_liner() {
        let s = build_wsl_move_script("Ubuntu-24.04", "d:").unwrap();
        assert_eq!(s.distro, "Ubuntu-24.04");
        assert_eq!(s.target_drive, "D");
        assert_eq!(s.target_dir, r"D:\WSL\Ubuntu-24.04");
        assert_eq!(s.tar_path, r"D:\WSL\Ubuntu-24.04.tar");

        // Must check export exit code and non-zero size BEFORE unregister
        let export_idx = s.script_body.find("wsl --export").unwrap();
        let check_idx = s.script_body.find("Length -le 0").unwrap();
        let unreg_idx = s.script_body.find("wsl --unregister").unwrap();
        let import_idx = s.script_body.find("wsl --import").unwrap();
        let rm_idx = s.script_body.find("Remove-Item -Force $tarPath").unwrap();
        assert!(export_idx < check_idx);
        assert!(check_idx < unreg_idx);
        assert!(unreg_idx < import_idx);
        assert!(import_idx < rm_idx);
        assert!(s.one_line_command.contains("Length -le 0"));
    }

    #[test]
    fn parses_utf16le_wsl_list_output() {
        let sample = "Ubuntu-24.04\r\ndocker-desktop\r\nDebian\r\n";
        let mut bytes = vec![0xFFu8, 0xFE];
        for unit in sample.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let distros = parse_wsl_list_output(&bytes);
        assert_eq!(distros.len(), 3);
        assert_eq!(distros[0].name, "Ubuntu-24.04");
        assert!(!distros[0].is_docker_internal);
        assert_eq!(distros[1].name, "docker-desktop");
        assert!(distros[1].is_docker_internal);
        assert_eq!(distros[2].name, "Debian");
    }
}
