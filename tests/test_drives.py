from collections import namedtuple
from pathlib import Path

from greenc.core.drives import (
    DRIVE_FIXED,
    data_root_for,
    drive_of,
    is_on_system_drive,
    list_drives,
    recommend_drive,
)

GB = 1024**3
Usage = namedtuple("Usage", "total used free")

VOLS = {
    "C:\\": (DRIVE_FIXED, "Windows", "NTFS"),
    "D:\\": (DRIVE_FIXED, "Data", "NTFS"),
    "E:\\": (2, "USB", "FAT32"),
    "F:\\": (DRIVE_FIXED, "New Volume", "NTFS"),
    "G:\\": (DRIVE_FIXED, "Old", "exFAT"),
}
SIZES = {
    "C:\\": (237 * GB, 18 * GB),
    "D:\\": (931 * GB, 612 * GB),
    "E:\\": (64 * GB, 60 * GB),
    "F:\\": (465 * GB, 133 * GB),
    "G:\\": (100 * GB, 99 * GB),
}


def _usage(root):
    total, free = SIZES[root]
    return Usage(total, total - free, free)


def test_eligibility_and_recommendation():
    drives = list_drives(
        roots=list(VOLS), probe=lambda r: VOLS[r], usage=_usage, system_root="C:\\"
    )
    by = {d.root: d for d in drives}
    assert by["C:\\"].is_system and by["C:\\"].eligible
    assert not by["E:\\"].eligible and "di động" in by["E:\\"].reason
    assert not by["G:\\"].eligible and "exFAT" in by["G:\\"].reason
    assert by["D:\\"].used == 319 * GB
    assert by["D:\\"].letter == "D:"
    assert recommend_drive(drives).root == "D:\\"


def test_system_drive_recommended_only_when_alone():
    drives = list_drives(
        roots=["C:\\", "E:\\"], probe=lambda r: VOLS[r], usage=_usage, system_root="C:\\"
    )
    assert recommend_drive(drives).root == "C:\\"
    assert recommend_drive([]) is None


def test_unreadable_drive_is_skipped():
    def probe(root):
        if root == "X:\\":
            raise OSError(21, "not ready")
        return VOLS[root]

    drives = list_drives(roots=["C:\\", "X:\\"], probe=probe, usage=_usage, system_root="C:\\")
    assert [d.root for d in drives] == ["C:\\"]


def test_path_helpers():
    assert drive_of("d:\\a\\b") == "D:\\"
    assert is_on_system_drive("C:\\Users\\x", "C:\\")
    assert not is_on_system_drive("D:\\x", "C:\\")
    assert data_root_for("D:\\") == Path("D:\\Green C")


def test_real_machine_lists_system_drive():
    assert any(d.is_system for d in list_drives())


def test_recovery_partition_excluded_and_small_drives_not_recommended():
    vols = {"C:\\": (DRIVE_FIXED, "", "NTFS"), "E:\\": (DRIVE_FIXED, "RECOVERY", "NTFS"),
            "F:\\": (DRIVE_FIXED, "Internal Disk", "NTFS")}
    sizes = {"C:\\": (237 * GB, 10 * GB), "E:\\": (20 * GB, 19 * GB), "F:\\": (465 * GB, 1 * GB)}
    drives = list_drives(roots=list(vols), probe=lambda r: vols[r],
                         usage=lambda r: Usage(sizes[r][0], sizes[r][0] - sizes[r][1], sizes[r][1]),
                         system_root="C:\\")
    by = {d.root: d for d in drives}
    assert not by["E:\\"].eligible and "khôi phục" in by["E:\\"].reason
    assert by["F:\\"].eligible
    # F: has only 1 GB free -> falls back to the drive with the most free space
    assert recommend_drive(drives).root == "C:\\"
