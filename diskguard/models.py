from dataclasses import dataclass, field


@dataclass
class DailySnapshot:
    id: int | None
    drive_letter: str
    snapshot_date: str
    total_bytes: int
    used_bytes: int
    free_bytes: int
    net_change_bytes: int
    updated_at: str


@dataclass
class WriteEvent:
    event_date: str
    timestamp: str
    drive_letter: str
    folder_path: str
    file_path: str
    delta_bytes: int
    process_name: str
    pid: int
    service_name: str
    service_display_name: str
    reason_code: str
    reason_detail: str
    id: int | None = None


@dataclass
class QuotaRule:
    id: int | None
    app_name: str
    process_name: str
    service_name: str
    target_paths: list[str]
    max_bytes: int
    watermark_ratio: float = 0.85
    retention_days: int = 7
    min_file_age_seconds: int = 60
    exclude_patterns: list[str] = field(default_factory=list)
    protected_files: list[str] = field(default_factory=list)
    enabled: bool = True
    created_at: str = ""


@dataclass
class FileManifestItem:
    original_path: str
    rel_arcname: str
    size_bytes: int
    sha256: str
    mtime: float
    atime: float
    hardlink_paths: list[str] = field(default_factory=list)
    id: int | None = None
    archive_id: int | None = None


@dataclass
class VaultArchive:
    id: int | None
    rule_id: int
    app_name: str
    zip_path: str
    manifest_path: str
    file_count: int
    original_bytes: int
    compressed_bytes: int
    created_at: str
    expires_at: str
    status: str  # QUARANTINED | ROLLED_BACK | PURGED
    status_reason: str
    resolved_at: str = ""

