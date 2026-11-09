from pathlib import Path
from diskguard.models import WriteEvent, QuotaRule, VaultArchive, FileManifestItem
from diskguard.storage.database import DiskGuardDB


def test_daily_snapshots_and_net_change(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "test.db")
    s1 = db.upsert_daily_snapshot("C:\\", "2026-11-07", total_bytes=100_000, used_bytes=50_000, free_bytes=50_000)
    assert s1.net_change_bytes == 0

    s2 = db.upsert_daily_snapshot("C:\\", "2026-11-08", total_bytes=100_000, used_bytes=58_000, free_bytes=42_000)
    assert s2.net_change_bytes == 8_000

    # Updating the same day recalculates against previous day
    s2_updated = db.upsert_daily_snapshot("C:\\", "2026-11-08", total_bytes=100_000, used_bytes=62_000, free_bytes=38_000)
    assert s2_updated.net_change_bytes == 12_000

    snaps = db.get_daily_snapshots("C:\\", limit_days=7)
    assert len(snaps) == 2
    assert snaps[0].snapshot_date == "2026-11-07"
    assert snaps[1].snapshot_date == "2026-11-08"


def test_write_events_and_day_drilldown(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "test.db")
    db.record_write_event(
        WriteEvent(
            event_date="2026-11-09",
            timestamp="2026-11-09T10:00:00",
            drive_letter="F:\\",
            folder_path=r"F:\Zalo Data\media\1988\ZaloDownloads\resource",
            file_path=r"F:\Zalo Data\media\1988\ZaloDownloads\resource\abc\fileNoise\123",
            delta_bytes=250 * 1024 * 1024,
            process_name="Zalo.exe",
            pid=4204,
            service_name="",
            service_display_name="Zalo Desktop App",
            reason_code="MEDIA_CHAT",
            reason_detail="Media & File đính kèm phát sinh khi chat Zalo",
        )
    )
    db.record_write_event(
        WriteEvent(
            event_date="2026-11-09",
            timestamp="2026-11-09T11:00:00",
            drive_letter="F:\\",
            folder_path=r"F:\Zalo Data\media\1988\ZaloDownloads\resource",
            file_path=r"F:\Zalo Data\media\1988\ZaloDownloads\resource\abc\fileNoise\456",
            delta_bytes=150 * 1024 * 1024,
            process_name="Zalo.exe",
            pid=4204,
            service_name="",
            service_display_name="Zalo Desktop App",
            reason_code="MEDIA_CHAT",
            reason_detail="Media & File đính kèm phát sinh khi chat Zalo",
        )
    )

    folders = db.get_day_folder_breakdown("F:\\", "2026-11-09")
    assert len(folders) == 1
    assert folders[0]["folder_path"] == r"F:\Zalo Data\media\1988\ZaloDownloads\resource"
    assert folders[0]["total_delta_bytes"] == 400 * 1024 * 1024
    assert folders[0]["reason_code"] == "MEDIA_CHAT"

    actors = db.get_day_actor_breakdown("F:\\", "2026-11-09")
    assert len(actors) == 1
    assert actors[0]["process_name"] == "Zalo.exe"
    assert actors[0]["total_delta_bytes"] == 400 * 1024 * 1024


def test_quota_rules_and_vault_archives(tmp_path: Path):
    db = DiskGuardDB(tmp_path / "test.db")
    rule = QuotaRule(
        id=None,
        app_name="Zalo PC Media",
        process_name="Zalo.exe",
        service_name="",
        target_paths=[r"F:\Zalo Data\media"],
        max_bytes=2 * 1024**3,
        watermark_ratio=0.85,
        retention_days=7,
        min_file_age_seconds=60,
        exclude_patterns=["*.db", "*.rescache"],
        protected_files=[],
        enabled=True,
    )
    rule_id = db.save_quota_rule(rule)
    assert rule_id > 0

    db.add_protected_files(rule_id, [r"F:\Zalo Data\media\important.dat"])
    loaded_rule = db.get_quota_rule(rule_id)
    assert loaded_rule is not None
    assert r"F:\Zalo Data\media\important.dat" in loaded_rule.protected_files

    archive = VaultArchive(
        id=None,
        rule_id=rule_id,
        app_name="Zalo PC Media",
        zip_path=str(tmp_path / "vault_1.zip"),
        manifest_path=str(tmp_path / "vault_1.manifest.json"),
        file_count=1,
        original_bytes=1024 * 1024,
        compressed_bytes=512 * 1024,
        created_at="2026-11-09T12:00:00",
        expires_at="2026-11-16T12:00:00",
        status="QUARANTINED",
        status_reason="Đang theo dõi an toàn 7 ngày",
    )
    manifest_item = FileManifestItem(
        original_path=r"F:\Zalo Data\media\fileNoise\hash1",
        rel_arcname="item_0001.bin",
        size_bytes=1024 * 1024,
        sha256="deadbeef",
        mtime=1700000000.0,
        atime=1700000000.0,
        hardlink_paths=[
            r"F:\Zalo Data\media\fileNoise\hash1",
            r"F:\Zalo Data\media\resource\123\fileNoise\msg1",
        ],
    )
    archive_id = db.create_vault_archive(archive, [manifest_item])
    assert archive_id > 0

    files = db.get_vault_files(archive_id)
    assert len(files) == 1
    assert len(files[0].hardlink_paths) == 2

    db.update_vault_status(archive_id, "ROLLED_BACK", "Khôi phục theo yêu cầu")
    updated_arch = db.get_vault_archive(archive_id)
    assert updated_arch is not None
    assert updated_arch.status == "ROLLED_BACK"

