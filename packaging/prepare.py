"""Generate build inputs that depend on the app version: the .ico file and the
Windows version resource for the exe. Output goes to build/packaging/.

Run from the repo root: uv run python packaging/prepare.py
"""

from __future__ import annotations

import os
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "build" / "packaging"
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)

sys.path.insert(0, str(ROOT))
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QBuffer, QByteArray, QIODevice  # noqa: E402
from PySide6.QtGui import QGuiApplication  # noqa: E402

from diskguard import __version__  # noqa: E402
from diskguard.config import APP_NAME, AUTHOR, WEBSITE_URL  # noqa: E402


def png_bytes(size: int) -> bytes:
    from diskguard.ui.icons import render_app_icon

    data = QByteArray()
    buffer = QBuffer(data)
    buffer.open(QIODevice.OpenModeFlag.WriteOnly)
    render_app_icon(size).save(buffer, "PNG")
    buffer.close()
    return bytes(data)


def build_ico(images: list[tuple[int, bytes]]) -> bytes:
    """Pack PNG images into a .ico container (PNG entries are valid since Vista)."""
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, png in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    return header + entries + blobs


def version_info(version: str) -> str:
    major, minor, patch = (int(p) for p in version.split("."))
    tup = (major, minor, patch, 0)
    strings = {
        "CompanyName": AUTHOR,
        "FileDescription": APP_NAME,
        "FileVersion": version,
        "InternalName": "GreenC",
        "LegalCopyright": f"© {AUTHOR} - {WEBSITE_URL}",
        "OriginalFilename": "GreenC.exe",
        "ProductName": APP_NAME,
        "ProductVersion": version,
    }
    string_structs = ",\n".join(f"          StringStruct({k!r}, {v!r})" for k, v in strings.items())
    return f"""VSVersionInfo(
  ffi=FixedFileInfo(filevers={tup}, prodvers={tup}, mask=0x3f, flags=0x0, OS=0x40004,
                    fileType=0x1, subtype=0x0, date=(0, 0)),
  kids=[
    StringFileInfo([
      StringTable('040904B0', [
{string_structs}
      ])
    ]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])
  ]
)
"""


def main() -> int:
    app = QGuiApplication.instance() or QGuiApplication([])  # noqa: F841 - needed for QPixmap
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "greenc.ico").write_bytes(build_ico([(s, png_bytes(s)) for s in ICO_SIZES]))
    (OUT / "version_info.txt").write_text(version_info(__version__), encoding="utf-8")
    (OUT / "wizard_small.png").write_bytes(png_bytes(64))
    print(f"Prepared build inputs for v{__version__} in {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
