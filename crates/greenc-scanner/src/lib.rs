//! High-speed parallel filesystem walker, Space Lens treemap, disk fullness forecast,
//! write attribution engine, daily delta tracker, app profiles, and orphan leftover radar.

#![deny(unsafe_code)]

pub mod attribution;
pub mod forecast;
pub mod orphans;
pub mod profiles;
pub mod space_lens;
pub mod tracker;
pub mod usn_mft;
pub mod walker;

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
