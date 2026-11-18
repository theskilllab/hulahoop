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

BACKDROP_TOP = (0x16, 0x1B, 0x22)
BACKDROP_BOTTOM = (0x0B, 0x0E, 0x13)
ACCENT = (0x6E, 0xE7, 0xA8)

# The ring is a broken circle: the gap is where the loop stays open.
GAP_START = math.radians(-55.0)
GAP_END = math.radians(-25.0)
RING_RADIUS = 0.265
RING_WIDTH = 0.062
CORNER_RADIUS = 0.222
INSET = 0.035


def _rounded_rect_sd(x: float, y: float, half: float, radius: float) -> float:
    """Signed distance to a rounded square centred on the origin."""
    qx = abs(x) - (half - radius)
    qy = abs(y) - (half - radius)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - radius


def _in_gap(angle: float) -> bool:
    start = GAP_START % (2 * math.pi)
    end = GAP_END % (2 * math.pi)
    angle %= 2 * math.pi
    if start <= end:
        return start <= angle <= end
    return angle >= start or angle <= end


def _pixel(px: float, py: float) -> tuple[int, int, int, int]:
    """Colour of the icon at one point, in unit coordinates from -0.5 to 0.5."""
    half = 0.5 - INSET
    if _rounded_rect_sd(px, py, half, CORNER_RADIUS) > 0.0:
        return (0, 0, 0, 0)

    t = (py + half) / (2 * half)
    t = min(max(t, 0.0), 1.0)
    colour = tuple(
        round(BACKDROP_TOP[i] + (BACKDROP_BOTTOM[i] - BACKDROP_TOP[i]) * t) for i in range(3)
    )

    distance = math.hypot(px, py)
    if abs(distance - RING_RADIUS) <= RING_WIDTH / 2 and not _in_gap(math.atan2(py, px)):
        colour = ACCENT

    return (colour[0], colour[1], colour[2], 255)


def _byte(value: float) -> int:
    return max(0, min(255, round(value)))


def render(size: int) -> bytes:
    """Renders one square size as raw RGBA rows."""
    samples = 4 if size <= 128 else 2
    rows = bytearray()
    step = 1.0 / (size * samples)
    for y in range(size):
        rows.append(0)  # PNG filter: none
        for x in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(samples):
                for sx in range(samples):
                    px = -0.5 + (x * samples + sx + 0.5) * step
                    py = -0.5 + (y * samples + sy + 0.5) * step
                    r, g, b, a = _pixel(px, py)
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            count = samples * samples
            alpha = acc[3] / count
            if alpha < 1.0:
                rows.extend((0, 0, 0, 0))
                continue
            # Colours were accumulated premultiplied by alpha, so dividing by the
            # accumulated alpha recovers the un-premultiplied value.
            rows.extend(
                (
                    _byte(acc[0] / acc[3]),
                    _byte(acc[1] / acc[3]),
                    _byte(acc[2] / acc[3]),
                    _byte(alpha),
                )
            )
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
