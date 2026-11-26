//! OS release identifier and build/major number for diagnostics.

/// Classifies a Windows build number as `"windows11"` (build >= 22000) or `"windows10"`.
pub const fn classify_windows(build: u32) -> &'static str {
    if build >= 22_000 {
        "windows11"
    } else {
        "windows10"
    }
}

/// Parses a macOS product version string such as `"14.5"` or `"15.0.1"` into its major version.
#[allow(dead_code)]
fn parse_macos_major(version: &str) -> u32 {
    version
        .trim()
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

/// Parses a little-endian UTF-16 registry `REG_SZ` byte buffer into a Windows build number.
#[allow(dead_code)]
fn parse_windows_build_utf16le(bytes: &[u8]) -> u32 {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    text.trim_end_matches('\0')
        .trim()
        .parse::<u32>()
        .unwrap_or(0)
}

/// Returns `(os_name, build_or_major)` matching the diagnostics data contract:
/// - Windows: `("windows11" | "windows10", build_number)` (read without admin privileges)
/// - macOS: `("macos", major_version)`
/// - Linux / other: `("linux", 0)`
pub fn os_release() -> (&'static str, u32) {
    #[cfg(target_os = "windows")]
    {
        let build = read_windows_build_number().unwrap_or(0);
        (classify_windows(build), build)
    }

    #[cfg(target_os = "macos")]
    {
        let major = read_macos_major_version().unwrap_or(0);
        ("macos", major)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        ("linux", 0)
    }
}

#[cfg(target_os = "windows")]
fn read_windows_build_number() -> Option<u32> {
    use crate::power_session::OwnedRegKey;
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_SZ,
        REG_VALUE_TYPE,
    };

    let subkey: Vec<u16> = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut raw_key = HKEY::default();
    // SAFETY: Read-only registry open on standard CurrentVersion key (accessible without admin).
    let open_err = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut raw_key,
        )
    };
    if open_err.is_err() {
        return None;
    }
    let reg_key = OwnedRegKey::new(raw_key)?;

    let value_name: Vec<u16> = "CurrentBuildNumber".encode_utf16().chain(Some(0)).collect();
    let mut val_type = REG_VALUE_TYPE::default();
    let mut data_len: u32 = 0;
    // SAFETY: Probes byte length of `CurrentBuildNumber`.
    let probe_err = unsafe {
        RegQueryValueExW(
            reg_key.raw(),
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut val_type),
            None,
            Some(&mut data_len),
        )
    };
    if probe_err.is_err() || val_type != REG_SZ || data_len == 0 || data_len > 256 {
        return None;
    }

    let mut buf = vec![0u8; data_len as usize];
    // SAFETY: `buf` has capacity `data_len` bytes and receives the UTF-16LE REG_SZ value.
    let read_err = unsafe {
        RegQueryValueExW(
            reg_key.raw(),
            PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut val_type),
            Some(buf.as_mut_ptr()),
            Some(&mut data_len),
        )
    };
    if read_err.is_err() || val_type != REG_SZ {
        return None;
    }

    let used = (data_len as usize).min(buf.len());
    Some(parse_windows_build_utf16le(&buf[..used]))
}

#[cfg(target_os = "macos")]
fn read_macos_major_version() -> Option<u32> {
    use std::ffi::CStr;

    let mut buf = [0u8; 64];
    let mut len: libc::size_t = buf.len();
    // SAFETY: `buf` is a writable stack buffer of length `len` and name is a NUL-terminated C string.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buf.as_mut_ptr().cast::<libc::c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 {
        return None;
    }
    let slice = &buf[..len.min(buf.len())];
    let text = CStr::from_bytes_until_nul(slice)
        .ok()
        .and_then(|s| s.to_str().ok())
        .or_else(|| std::str::from_utf8(slice).ok())?;
    Some(parse_macos_major(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_macos_major_handles_standard_and_malformed_strings() {
        assert_eq!(parse_macos_major("14.5"), 14);
        assert_eq!(parse_macos_major("15.0.1\n"), 15);
        assert_eq!(parse_macos_major("13"), 13);
        assert_eq!(parse_macos_major(""), 0);
        assert_eq!(parse_macos_major("abc"), 0);
    }

    #[test]
    fn parse_windows_build_utf16le_decodes_reg_sz() {
        let wide: Vec<u8> = "22631\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_windows_build_utf16le(&wide), 22631);

        let win10: Vec<u8> = "19045".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse_windows_build_utf16le(&win10), 19045);
        assert_eq!(
            classify_windows(parse_windows_build_utf16le(&win10)),
            "windows10"
        );
        assert_eq!(
            classify_windows(parse_windows_build_utf16le(&wide)),
            "windows11"
        );
    }
}
