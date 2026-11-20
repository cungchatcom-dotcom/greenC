//! Native Windows API abstractions, RAII safe handles, and testable trait boundaries.

#![allow(unsafe_code)]

pub mod handle;
pub mod known_folders;
pub mod mutex;
pub mod power_session;
pub mod reparse;
pub mod restart_mgr;
pub mod traits;
pub mod volume;
pub mod wof_compact;

pub use handle::{
    ensure_not_protected_system_path, is_file_locked, path_to_wide, query_file_identity,
    OwnedHandle, Win32FileIdentity,
};
pub use known_folders::{
    parse_guid, FOLDERID_DESKTOP, FOLDERID_DOCUMENTS, FOLDERID_DOWNLOADS, FOLDERID_MUSIC,
    FOLDERID_PICTURES, FOLDERID_VIDEOS,
};
pub use mutex::GreenCAppMutex;
pub use power_session::{
    is_autostart_enabled_native, query_power_session_native, set_autostart_registry_native,
    MockPowerSessionApi, NativePowerSessionApi, OwnedRegKey, PowerSessionApi, PowerSessionState,
};
pub use reparse::{
    FSCTL_DELETE_REPARSE_POINT, FSCTL_GET_REPARSE_POINT, FSCTL_SET_REPARSE_POINT,
    IO_REPARSE_TAG_MOUNT_POINT, MAXIMUM_REPARSE_DATA_BUFFER_SIZE,
};
pub use restart_mgr::{
    get_processes_locking_files_native, map_pids_to_services_native, query_service_running_native,
    LockingProcessInfo, MockRestartManagerApi, NativeRestartManagerApi, OwnedServiceHandle,
    RestartManagerApi, RmSessionGuard,
};
pub use traits::*;
pub use volume::{
    evaluate_drive_eligibility, get_system_drive_root, is_process_elevated, list_drives_native,
    probe_volume_usn_capability, recommend_drive, recommend_drive_info, recommend_drive_letter,
    RecommendedDrive, ThreadErrorModeGuard, UsnCapabilityStatus, DRIVE_CDROM, DRIVE_FIXED,
    DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE, DRIVE_UNKNOWN,
};
pub use wof_compact::{
    FileProviderExternalInfoV1, WofExternalInfo, WofFileCompressionInfo,
    COMPRESSION_FORMAT_DEFAULT, COMPRESSION_FORMAT_LZNT1, COMPRESSION_FORMAT_NONE,
    FILE_PROVIDER_COMPRESSION_LZX, FILE_PROVIDER_COMPRESSION_XPRESS16K,
    FILE_PROVIDER_COMPRESSION_XPRESS4K, FILE_PROVIDER_COMPRESSION_XPRESS8K,
    FILE_PROVIDER_CURRENT_VERSION, WOF_CURRENT_VERSION, WOF_PROVIDER_FILE,
};
