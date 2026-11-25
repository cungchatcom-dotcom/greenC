//! ACL policy & Windows native DACL tests for `restrict_dir_to_current_user` (Item 8).
#![cfg_attr(windows, allow(unsafe_code))]

use greenc_platform::acl_policy::{
    compute_protected_dacl_size, FILE_ALL_ACCESS_MASK, INHERIT_ALL_ACE_FLAGS, READ_CONTROL_ACCESS,
    REQUIRED_EXTRA_WELL_KNOWN_SIDS, RESTRICT_DIR_HANDLE_ACCESS, SECURITY_MAX_SID_SIZE,
    WIN_BUILTIN_ADMINISTRATORS_SID_TYPE, WIN_LOCAL_SYSTEM_SID_TYPE, WRITE_DAC_ACCESS,
};

#[test]
fn item8_acl_policy_requires_read_control_write_dac_and_system_and_admins() {
    assert_eq!(
        RESTRICT_DIR_HANDLE_ACCESS,
        READ_CONTROL_ACCESS | WRITE_DAC_ACCESS,
        "directory handle for ACL update MUST be opened with READ_CONTROL | WRITE_DAC"
    );
    assert_eq!(RESTRICT_DIR_HANDLE_ACCESS, 0x0006_0000);
    assert_eq!(FILE_ALL_ACCESS_MASK, 0x001F_01FF);
    assert_eq!(INHERIT_ALL_ACE_FLAGS, 0x03);
    assert!(
        REQUIRED_EXTRA_WELL_KNOWN_SIDS.contains(&WIN_LOCAL_SYSTEM_SID_TYPE),
        "protected DACL must grant full control to NT AUTHORITY\\SYSTEM"
    );
    assert!(
        REQUIRED_EXTRA_WELL_KNOWN_SIDS.contains(&WIN_BUILTIN_ADMINISTRATORS_SID_TYPE),
        "protected DACL must grant full control to BUILTIN\\Administrators"
    );
    const { assert!(SECURITY_MAX_SID_SIZE >= 68) };

    // 3 SIDs: current user (28B), SYSTEM (12B), BUILTIN\Administrators (16B)
    let size = compute_protected_dacl_size(&[28, 12, 16]);
    assert_eq!(size % 4, 0, "Win32 ACL buffer MUST be 4-byte aligned");
    // sizeof(ACL)=8 + 3 * (sizeof(ACCESS_ALLOWED_ACE)-4 = 8) + (28+12+16) = 8 + 24 + 56 = 88
    assert!(
        size >= 88,
        "ACL buffer size {size} too small for 3 ACEs + SIDs"
    );
}

#[cfg(windows)]
#[test]
fn item8_windows_restrict_dir_grants_user_system_admins_and_blocks_inheritance() {
    use greenc_platform::restrict_dir_to_current_user;
    use std::os::windows::ffi::OsStrExt;
    use tempfile::TempDir;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::{
        EqualSid, GetKernelObjectSecurity, GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR,
        PSID, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const DACL_SECURITY_INFO: u32 = 0x0000_0004;
    const SE_DACL_PROTECTED: u16 = 0x1000;
    const ACL_SIZE_INFORMATION_CLASS: u32 = 2;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const WIN_WORLD_SID: u32 = 1;
    const WIN_AUTHENTICATED_USER_SID: u32 = 17;
    const WIN_BUILTIN_USERS_SID: u32 = 27;

    #[repr(C)]
    struct AclSizeInformation {
        ace_count: u32,
        acl_bytes_in_use: u32,
        acl_bytes_free: u32,
    }

    #[repr(C)]
    struct AceHeader {
        ace_type: u8,
        ace_flags: u8,
        ace_size: u16,
    }

    #[repr(C)]
    struct AccessAllowedAce {
        header: AceHeader,
        mask: u32,
        sid_start: u32,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn CreateWellKnownSid(
            well_known_sid_type: u32,
            domain_sid: PSID,
            p_sid: *mut std::ffi::c_void,
            cb_sid: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorControl(
            p_sd: PSECURITY_DESCRIPTOR,
            p_control: *mut u16,
            lpdw_revision: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorDacl(
            p_sd: PSECURITY_DESCRIPTOR,
            lpb_dacl_present: *mut i32,
            p_dacl: *mut *mut std::ffi::c_void,
            lpb_dacl_defaulted: *mut i32,
        ) -> i32;
        fn GetAclInformation(
            p_acl: *const std::ffi::c_void,
            p_acl_information: *mut std::ffi::c_void,
            n_acl_information_length: u32,
            dw_acl_information_class: u32,
        ) -> i32;
        fn GetAce(
            p_acl: *const std::ffi::c_void,
            dw_ace_index: u32,
            p_ace: *mut *mut std::ffi::c_void,
        ) -> i32;
    }

    let make_well_known = |sid_type: u32| -> Vec<u32> {
        let mut words = vec![0u32; SECURITY_MAX_SID_SIZE.div_ceil(4)];
        let mut cb = (words.len() * 4) as u32;
        let ok = unsafe {
            CreateWellKnownSid(
                sid_type,
                PSID::default(),
                words.as_mut_ptr().cast(),
                &mut cb,
            )
        };
        assert_ne!(ok, 0, "CreateWellKnownSid({sid_type}) failed");
        words
    };

    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("zalo_private_acl");
    std::fs::create_dir_all(&target).unwrap();

    restrict_dir_to_current_user(&target).expect("restrict_dir_to_current_user must succeed");

    // Query current user SID from process token
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.unwrap();
    let mut tok_len = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut tok_len) };
    let mut tok_buf = vec![0u8; tok_len as usize];
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(tok_buf.as_mut_ptr().cast()),
            tok_len,
            &mut tok_len,
        )
    }
    .unwrap();
    let _ = unsafe { CloseHandle(token) };
    let current_sid = unsafe { std::ptr::read_unaligned(tok_buf.as_ptr().cast::<TOKEN_USER>()) }
        .User
        .Sid;

    let system_sid_buf = make_well_known(WIN_LOCAL_SYSTEM_SID_TYPE);
    let admins_sid_buf = make_well_known(WIN_BUILTIN_ADMINISTRATORS_SID_TYPE);
    let users_sid_buf = make_well_known(WIN_BUILTIN_USERS_SID);
    let auth_sid_buf = make_well_known(WIN_AUTHENTICATED_USER_SID);
    let world_sid_buf = make_well_known(WIN_WORLD_SID);

    let system_sid = PSID(system_sid_buf.as_ptr() as *mut _);
    let admins_sid = PSID(admins_sid_buf.as_ptr() as *mut _);
    let users_sid = PSID(users_sid_buf.as_ptr() as *mut _);
    let auth_sid = PSID(auth_sid_buf.as_ptr() as *mut _);
    let world_sid = PSID(world_sid_buf.as_ptr() as *mut _);

    // Open target dir with READ_CONTROL to inspect kernel DACL
    let wide: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            READ_CONTROL_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .unwrap();
    assert_ne!(handle, INVALID_HANDLE_VALUE);

    let mut needed = 0u32;
    let _ = unsafe {
        GetKernelObjectSecurity(
            handle,
            DACL_SECURITY_INFO,
            PSECURITY_DESCRIPTOR::default(),
            0,
            &mut needed,
        )
    };
    assert!(needed > 0);
    let mut sd_buf = vec![0u8; needed as usize];
    let sd_ptr = PSECURITY_DESCRIPTOR(sd_buf.as_mut_ptr().cast());
    unsafe { GetKernelObjectSecurity(handle, DACL_SECURITY_INFO, sd_ptr, needed, &mut needed) }
        .unwrap();
    let _ = unsafe { CloseHandle(handle) };

    // 1. DACL must be protected (no inherited entries from parent drive)
    let mut control = 0u16;
    let mut rev = 0u32;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(sd_ptr, &mut control, &mut rev) },
        0
    );
    assert_ne!(
        control & SE_DACL_PROTECTED,
        0,
        "DACL must have SE_DACL_PROTECTED set"
    );

    // 2. Inspect every ACE in the DACL
    let mut dacl_present = 0i32;
    let mut dacl_defaulted = 0i32;
    let mut dacl_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    assert_ne!(
        unsafe {
            GetSecurityDescriptorDacl(
                sd_ptr,
                &mut dacl_present,
                &mut dacl_ptr,
                &mut dacl_defaulted,
            )
        },
        0
    );
    assert_ne!(dacl_present, 0);
    assert!(!dacl_ptr.is_null());

    let mut acl_info = AclSizeInformation {
        ace_count: 0,
        acl_bytes_in_use: 0,
        acl_bytes_free: 0,
    };
    assert_ne!(
        unsafe {
            GetAclInformation(
                dacl_ptr,
                (&mut acl_info as *mut AclSizeInformation).cast(),
                std::mem::size_of::<AclSizeInformation>() as u32,
                ACL_SIZE_INFORMATION_CLASS,
            )
        },
        0
    );
    assert!(
        acl_info.ace_count >= 2 && acl_info.ace_count <= 3,
        "expected 2..=3 ACEs (current user + SYSTEM + Administrators, deduplicated), got {}",
        acl_info.ace_count
    );

    let mut has_user = false;
    let mut has_system = false;
    let mut has_admins = false;

    for idx in 0..acl_info.ace_count {
        let mut ace_raw: *mut std::ffi::c_void = std::ptr::null_mut();
        assert_ne!(unsafe { GetAce(dacl_ptr, idx, &mut ace_raw) }, 0);
        let ace = unsafe { &*(ace_raw as *const AccessAllowedAce) };
        assert_eq!(ace.header.ace_type, ACCESS_ALLOWED_ACE_TYPE);
        assert_eq!(
            ace.header.ace_flags & (INHERIT_ALL_ACE_FLAGS as u8),
            INHERIT_ALL_ACE_FLAGS as u8,
            "ACE must inherit to child objects and containers"
        );
        assert_eq!(
            ace.mask & FILE_ALL_ACCESS_MASK,
            FILE_ALL_ACCESS_MASK,
            "ACE must grant FILE_ALL_ACCESS"
        );
        let ace_sid = PSID((&ace.sid_start as *const u32) as *mut _);
        if unsafe { EqualSid(ace_sid, current_sid) }.is_ok() {
            has_user = true;
        }
        if unsafe { EqualSid(ace_sid, system_sid) }.is_ok() {
            has_system = true;
        }
        if unsafe { EqualSid(ace_sid, admins_sid) }.is_ok() {
            has_admins = true;
        }
        assert!(
            unsafe { EqualSid(ace_sid, users_sid) }.is_err(),
            "BUILTIN\\Users MUST NOT have access"
        );
        assert!(
            unsafe { EqualSid(ace_sid, auth_sid) }.is_err(),
            "Authenticated Users MUST NOT have access"
        );
        assert!(
            unsafe { EqualSid(ace_sid, world_sid) }.is_err(),
            "Everyone MUST NOT have access"
        );
    }

    assert!(has_user, "DACL must grant full control to current user");
    assert!(has_system, "DACL must grant full control to SYSTEM");
    assert!(
        has_admins,
        "DACL must grant full control to BUILTIN\\Administrators"
    );
}
