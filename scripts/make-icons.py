#!/usr/bin/env python3
"""Draws the Hulahoop app icon and writes every format the bundlers ask for.

The macOS bundler needs a real `.icns` (it refuses to build a `.app` without one),
Windows wants a `.ico`, and Tauri's own config lists PNGs as well. Everything is
generated from the same drawing so the sizes never drift apart.

Only the standard library is used: PNGs are written by hand and the icon containers
are just length-prefixed chunks around those PNGs.
"""

from __future__ import annotations

import math
import struct
import sys
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "apps/desktop/src-tauri/icons"

# The mark from the prototype: a white rounded tile with the brand dot in the
# middle. The dot is 53% of the tile's width, and its colour never changes with
# the theme, so the icon reads the same in Finder and in the dock.
TILE = (0xFF, 0xFF, 0xFF)
DOT = (0x6F, 0x86, 0xD6)
# Apple's macOS icon grid: on a 1024 canvas the tile is 824 wide with a 100
# margin, and its corners have a radius of about 185. A tile that fills the
# whole canvas looks oversized next to other apps, and macOS 26 shrinks such
# an icon into a grey tile of its own.
INSET = 100 / 1024
TILE_WIDTH = 1.0 - 2 * INSET
CORNER_RADIUS = 185.4 / 824 * TILE_WIDTH
DOT_RADIUS = 0.53 / 2 * TILE_WIDTH


def _rounded_rect_sd(x: float, y: float, half: float, radius: float) -> float:
    """Signed distance to a rounded square centred on the origin."""
    qx = abs(x) - (half - radius)
    qy = abs(y) - (half - radius)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - radius


def _coverage(distance: float, pixel: float) -> float:
    """How much of a pixel a shape covers, from its signed distance at the pixel's
    centre. A one-pixel ramp across the edge gives every size a smooth outline;
    sampling a few points per pixel left only four or five shades and a jagged rim."""
    return max(0.0, min(1.0, 0.5 - distance / pixel))


def _byte(value: float) -> int:
    return max(0, min(255, round(value)))


def render(size: int) -> bytes:
    """Renders one square size as raw RGBA rows."""
    rows = bytearray()
    pixel = 1.0 / size
    half = 0.5 - INSET
    for y in range(size):
        rows.append(0)  # PNG filter: none
        py = -0.5 + (y + 0.5) * pixel
        for x in range(size):
            px = -0.5 + (x + 0.5) * pixel
            tile = _coverage(_rounded_rect_sd(px, py, half, CORNER_RADIUS), pixel)
            if tile <= 0.0:
                rows.extend((0, 0, 0, 0))
                continue
            dot = _coverage(math.hypot(px, py) - DOT_RADIUS, pixel)
            # The dot sits wholly inside the tile, so its edge blends dot into
            # tile colour and the tile's edge alone sets transparency.
            colour = [TILE[i] * (1.0 - dot) + DOT[i] * dot for i in range(3)]
            rows.extend((*(_byte(c) for c in colour), _byte(tile * 255)))
    return bytes(rows)


def png(size: int) -> bytes:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    body = zlib.compress(render(size), 9)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", body) + chunk(b"IEND", b"")


def icns(payloads: dict[bytes, bytes]) -> bytes:
    entries = b"".join(
        kind + struct.pack(">I", len(data) + 8) + data for kind, data in payloads.items()
    )
    return b"icns" + struct.pack(">I", len(entries) + 8) + entries


def ico(payloads: dict[int, bytes]) -> bytes:
    entries = b""
    offset = 6 + 16 * len(payloads)
    blobs = b""
    for size, data in payloads.items():
        entries += struct.pack(
            "<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset
        )
        blobs += data
        offset += len(data)
    return struct.pack("<HHH", 0, 1, len(payloads)) + entries + blobs


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    written: list[str] = []

    for size, name in (
        (32, "32x32.png"),
        (128, "128x128.png"),
        (256, "128x128@2x.png"),
        (1024, "icon.png"),
    ):
        (OUT / name).write_bytes(png(size))
        written.append(name)

    icns_types = {
        b"icp4": 16,
        b"icp5": 32,
        b"icp6": 64,
        b"ic07": 128,
        b"ic08": 256,
        b"ic09": 512,
        b"ic10": 1024,
        b"ic11": 32,
        b"ic12": 64,
        b"ic13": 256,
        b"ic14": 512,
    }
    cache: dict[int, bytes] = {}
    payloads: dict[bytes, bytes] = {}
    for kind, size in icns_types.items():
        if size not in cache:
            cache[size] = png(size)
        payloads[kind] = cache[size]
    (OUT / "icon.icns").write_bytes(icns(payloads))
    written.append("icon.icns")

    ico_sizes = (16, 32, 48, 64, 128, 256)
    for size in ico_sizes:
        if size not in cache:
            cache[size] = png(size)
    (OUT / "icon.ico").write_bytes(ico({size: cache[size] for size in ico_sizes}))
    written.append("icon.ico")

    print("wrote " + ", ".join(written) + f" in {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
