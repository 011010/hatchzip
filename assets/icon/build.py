#!/usr/bin/env python3
"""Renders the HatchZip icon into every file the packages need.

    python3 -I assets/icon/build.py

Needs rsvg-convert (librsvg) and nothing outside the Python standard library.
The .ico and .icns are just PNGs with a header in front, so they are written
here directly rather than through Pillow or iconutil, which would add a
dependency or need a Mac.

Writes, all committed so packaging never needs librsvg:
    assets/icon/png/hatchzip-<size>.png
    assets/icon/hatchzip.ico      Windows, 16 to 256 px
    assets/icon/hatchzip.icns     macOS, 16 to 512 px at 1x and 2x
    packaging/linux/icons/hicolor/<size>x<size>/apps/<APP_ID>.png
    packaging/linux/icons/hicolor/scalable/apps/<APP_ID>.svg
"""

import shutil
import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
APP_ID = "io.github.xevrion.hatchzip"
SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
# The hicolor theme has no 1024 folder, and nothing on Linux asks for one.
HICOLOR_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
# Type codes from Apple's icns format, all holding PNG data. These are the
# ones iconutil writes for a full iconset; 16 px at 1x has no PNG type, and
# Finder scales the 32 px one down, which this icon's grid keeps sharp.
ICNS_TYPES = [
    (b"ic11", 32),  # 16 pt @2x
    (b"ic12", 64),  # 32 pt @2x
    (b"ic07", 128),
    (b"ic13", 256),  # 128 pt @2x
    (b"ic08", 256),
    (b"ic14", 512),  # 256 pt @2x
    (b"ic09", 512),
    (b"ic10", 1024),  # 512 pt @2x
]


def source_for(size: int) -> Path:
    # The smallest sizes have their own pixel-fitted drawings; see the
    # comments inside each SVG.
    return HERE / {16: "hatchzip-16.svg", 24: "hatchzip-24.svg"}.get(size, "hatchzip.svg")


def render(size: int, out: Path) -> bytes:
    subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(out), str(source_for(size))],
        check=True,
    )
    return out.read_bytes()


def write_ico(pngs: dict[int, bytes], out: Path) -> None:
    # Windows Vista and later read PNG entries at every size, and HatchZip
    # needs Windows 10, so the older bitmap entries aren't worth the code.
    header = struct.pack("<HHH", 0, 1, len(ICO_SIZES))
    offset = len(header) + 16 * len(ICO_SIZES)
    entries, data = b"", b""
    for size in ICO_SIZES:
        png = pngs[size]
        # A width or height of 0 means 256.
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
        data += png
    out.write_bytes(header + entries + data)


def write_icns(pngs: dict[int, bytes], out: Path) -> None:
    body = b"".join(code + struct.pack(">I", 8 + len(pngs[size])) + pngs[size] for code, size in ICNS_TYPES)
    out.write_bytes(b"icns" + struct.pack(">I", 8 + len(body)) + body)


def main() -> int:
    if shutil.which("rsvg-convert") is None:
        print("build.py needs rsvg-convert (librsvg2-tools on Fedora, librsvg2-bin on Debian)", file=sys.stderr)
        return 1
    png_dir = HERE / "png"
    png_dir.mkdir(exist_ok=True)
    pngs = {size: render(size, png_dir / f"hatchzip-{size}.png") for size in SIZES}
    write_ico(pngs, HERE / "hatchzip.ico")
    write_icns(pngs, HERE / "hatchzip.icns")

    hicolor = ROOT / "packaging/linux/icons/hicolor"
    for size in HICOLOR_SIZES:
        dest = hicolor / f"{size}x{size}/apps/{APP_ID}.png"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(pngs[size])
    scalable = hicolor / f"scalable/apps/{APP_ID}.svg"
    scalable.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(HERE / "hatchzip.svg", scalable)
    print(f"wrote {len(SIZES)} PNGs, hatchzip.ico, hatchzip.icns and the hicolor icons")
    return 0


if __name__ == "__main__":
    sys.exit(main())
