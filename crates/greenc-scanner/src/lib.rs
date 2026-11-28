//! High-speed parallel filesystem walker, Space Lens treemap, disk fullness forecast,
//! write attribution engine, daily delta tracker, app profiles, and orphan leftover radar.

#![deny(unsafe_code)]

pub mod attribution;
pub mod forecast;
pub mod installed_apps;
pub mod orphans;
pub mod profiles;
pub mod space_lens;
pub mod tracker;
pub mod usn_mft;
pub mod walker;
pub mod zalo;

pub use attribution::{
    classify_write_reason, reason_label_key, AttributionEngine, AttributionMatch,
};
pub use forecast::{predict_fullness, BurnRateForecast, ForecastResult};
pub use orphans::{
    OrphanLeftoverCandidate, OrphanLeftoverRadar, DEFAULT_MIN_INACTIVE_DAYS,
    DEFAULT_MIN_ORPHAN_BYTES, PROTECTED_UMBRELLA_FOLDERS,
};
pub use profiles::{discover_dingtalk_profile, discover_zalo_profile, DiscoveredAppProfile};
pub use space_lens::{classify_path_reason, SpaceNode, SquarifiedTreemapGenerator, TreemapRect};
pub use tracker::{DailyDrilldown, DiskTracker, MIN_BUCKET_DELTA_BYTES};
pub use usn_mft::{can_use_usn_scanner, extract_drive_letter, scan_volume_hierarchical};
pub use walker::{
    walk_directory, walk_directory_parallel, walk_directory_parallel_with_progress, FileIdentity,
    WalkProgressCallback,
};
pub use zalo::{
    discover_zalo_installation, extract_drive_label, is_allowed_zalo_partition_cache_name,
    is_allowed_zalo_root_cache_name, is_protected_zalo_entry, is_symlink_or_reparse,
    measure_dir_safe, parse_zalo_version, validate_zalo_config_raw_path, ZaloCacheDirInfo,
    ZaloConfigStatus, ZaloDiscovery, ZaloDiscoveryOptions, ZaloVersionDirInfo,
    MAX_ZALO_CONFIG_BYTES, ZALO_CACHE_MIN_AGE_SECS, ZALO_OLD_VERSION_MIN_AGE_SECS,
    ZALO_PARTITION_CACHE_ALLOWLIST, ZALO_PROCESS_NAMES, ZALO_PROTECTED_DIR_NAMES,
    ZALO_ROOT_CACHE_ALLOWLIST,
};
