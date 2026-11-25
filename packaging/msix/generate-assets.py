#!/usr/bin/env python3
"""Cross-platform generator for MSIX Store logo PNG assets from icon.png (Pillow).

Produces the exact asset set referenced by packaging/msix/AppxManifest.xml.in
without committing duplicate binary PNGs into the repository.
"""

from __future__ import annotations

import argparse
from pathlib import Path
from PIL import Image

ASSET_SPECS: list[tuple[str, int, int, float]] = [
    ("StoreLogo.png", 50, 50, 0.04),
    ("Square44x44Logo.png", 44, 44, 0.04),
    ("Square71x71Logo.png", 71, 71, 0.08),
    ("Square150x150Logo.png", 150, 150, 0.12),
    ("Wide310x150Logo.png", 310, 150, 0.16),
    ("Square310x310Logo.png", 310, 310, 0.16),
]


def generate_assets(source_icon: Path, output_dir: Path) -> list[Path]:
    if not source_icon.is_file():
        raise FileNotFoundError(f"Source icon not found: {source_icon}")

    output_dir.mkdir(parents=True, exist_ok=True)
    generated: list[Path] = []

    with Image.open(source_icon) as raw:
        src = raw.convert("RGBA")
        for filename, width, height, pad in ASSET_SPECS:
            canvas = Image.new("RGBA", (width, height), (0, 0, 0, 0))
            min_side = min(width, height)
            draw_side = max(1, round(min_side * (1.0 - 2.0 * pad)))
            resized = src.resize((draw_side, draw_side), Image.Resampling.LANCZOS)
            offset_x = (width - draw_side) // 2
            offset_y = (height - draw_side) // 2
            canvas.paste(resized, (offset_x, offset_y), resized)

            out_path = output_dir / filename
            canvas.save(out_path, format="PNG", optimize=True)
            generated.append(out_path)
            print(f"Generated {filename} ({width}x{height})")

    return generated


def main() -> None:
    repo_root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description="Generate MSIX Store PNG assets")
    parser.add_argument(
        "--source",
        type=Path,
        default=repo_root / "crates" / "greenc-app" / "icons" / "icon.png",
        help="Path to 512x512 RGBA PNG source icon",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=repo_root / "packaging" / "msix" / "Assets",
        help="Output directory for generated Assets PNGs",
    )
    args = parser.parse_args()
    generate_assets(args.source, args.out)


if __name__ == "__main__":
    main()
