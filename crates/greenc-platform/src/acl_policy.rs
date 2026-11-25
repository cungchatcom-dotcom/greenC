//! Shared Windows protected-DACL policy constants and ACL sizing helpers used by
//! `win32::handle::restrict_dir_to_current_user` (Item 8).

/// Win32 `READ_CONTROL` access mask (`0x0002_0000`).
pub const READ_CONTROL_ACCESS: u32 = 0x0002_0000;
/// Win32 `WRITE_DAC` access mask (`0x0004_0000`).
pub const WRITE_DAC_ACCESS: u32 = 0x0004_0000;
/// Required access mask when opening a directory handle to update its protected DACL.
pub const RESTRICT_DIR_HANDLE_ACCESS: u32 = READ_CONTROL_ACCESS | WRITE_DAC_ACCESS;

/// Win32 `WELL_KNOWN_SID_TYPE::WinLocalSystemSid` (`S-1-5-18`, `NT AUTHORITY\SYSTEM`).
pub const WIN_LOCAL_SYSTEM_SID_TYPE: u32 = 22;
/// Win32 `WELL_KNOWN_SID_TYPE::WinBuiltinAdministratorsSid` (`S-1-5-32-544`, `BUILTIN\Administrators`).
pub const WIN_BUILTIN_ADMINISTRATORS_SID_TYPE: u32 = 26;
/// Win32 `SECURITY_MAX_SID_SIZE` (68 bytes).
pub const SECURITY_MAX_SID_SIZE: usize = 68;

/// Win32 `FILE_ALL_ACCESS` mask (`0x001F_01FF`).
pub const FILE_ALL_ACCESS_MASK: u32 = 0x001F_01FF;
/// Win32 `OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE` (`0x01 | 0x02`).
pub const INHERIT_ALL_ACE_FLAGS: u32 = 0x01 | 0x02;

/// Well-known trustee SIDs granted full inherited control alongside the current interactive user
/// so Windows services (backup/AV/VSS) and local administrators never lose access to relocated data.
pub const REQUIRED_EXTRA_WELL_KNOWN_SIDS: &[u32] = &[
    WIN_LOCAL_SYSTEM_SID_TYPE,
    WIN_BUILTIN_ADMINISTRATORS_SID_TYPE,
];

/// Computes the 4-byte-aligned byte size required for a Win32 `ACL` holding one
/// `ACCESS_ALLOWED_ACE` per SID length in `sid_lengths`, plus alignment headroom.
pub fn compute_protected_dacl_size(sid_lengths: &[u32]) -> u32 {
    // sizeof(ACL) = 8; each ACCESS_ALLOWED_ACE header minus trailing DWORD SidStart = 8 bytes.
    let mut total: u32 = 8;
    for &sid_len in sid_lengths {
        let ace_bytes = (8u32.saturating_add(sid_len).saturating_add(3)) & !3;
        total = total.saturating_add(ace_bytes);
    }
    // Add 32 bytes safety headroom and ensure 4-byte alignment.
    total.saturating_add(32).saturating_add(3) & !3
}
