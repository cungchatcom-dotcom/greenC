import os
from pathlib import Path

from greenc.core.relocation.base import MOVING_SUFFIX
from greenc.core.relocation.fs_ops import is_reparse_point, junction_target_matches, remove_tree
from greenc.models import RELOCATION_DONE, RELOCATION_FAILED, RELOCATION_MOVING, RelocationRecord
from greenc.storage.database import GreenCDB


def recover_interrupted(db: GreenCDB) -> list[RelocationRecord]:
    """Finish or roll back junction moves interrupted by a crash or power loss."""
    fixed: list[RelocationRecord] = []
    for rec in db.list_relocations(status=RELOCATION_MOVING):
        src, target = Path(rec.source_path), Path(rec.target_path)
        moving = Path(rec.source_path + MOVING_SUFFIX)
        if target.is_dir() and junction_target_matches(src, target):
            if moving.exists():
                remove_tree(moving)
            rec.status = RELOCATION_DONE
            rec.detail = "Đã hoàn tất sau khi ứng dụng khởi động lại"
        else:
            if is_reparse_point(src):
                os.rmdir(src)
            if moving.exists() and not src.exists():
                os.rename(moving, src)
            if src.is_dir() and target.exists():
                remove_tree(target)
            rec.status = RELOCATION_FAILED
            rec.detail = "Bị gián đoạn, đã khôi phục về vị trí cũ"
        db.save_relocation(rec)
        fixed.append(rec)
    return fixed
