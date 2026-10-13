"""Pure-stdlib image utilities: PNG decode, resize, aHash perceptual fingerprint.

Needed because Orion has zero third-party dependencies. Handles the common
case: non-interlaced 8-bit RGB/RGBA/grayscale PNG. That's what grim outputs.
"""

from __future__ import annotations

import struct
import zlib


def decode_png(data: bytes) -> tuple[int, int, int, list[list[list[int]]]]:
    """Decode a non-interlaced 8-bit PNG. Returns (w, h, channels, rows).

    channels: 1 (gray), 2 (gray+alpha), 3 (rgb), 4 (rgba)
    rows: h lists of w pixel-lists ([r,g,b,(a)])
    """
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    pos = 8
    idat = bytearray()
    w = h = bd = ct = None
    palette = None
    while pos < len(data):
        length, typ = struct.unpack(">I4s", data[pos:pos + 8])
        chunk = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if typ == b"IHDR":
            w, h, bd, ct = struct.unpack(">IIBB", chunk[:10])
            if bd != 8:
                raise ValueError(f"unsupported bit depth {bd}")
        elif typ == b"IDAT":
            idat.extend(chunk)
        elif typ == b"PLTE":
            palette = [list(chunk[i:i + 3]) for i in range(0, len(chunk), 3)]
        elif typ == b"IEND":
            break
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ct]
    stride = w * channels
    raw = zlib.decompress(bytes(idat))
    rows = []
    prev = bytearray(stride)
    idx = 0
    for y in range(h):
        ftype = raw[idx]
        line = bytearray(raw[idx + 1:idx + 1 + stride])
        idx += 1 + stride
        if ftype == 1:      # Sub
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif ftype == 2:    # Up
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:    # Average
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:    # Paeth
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                b = prev[i]
                c = prev[i - channels] if i >= channels else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        prev = line
        rows.append(line)
    pixels = []
    for line in rows:
        row = []
        for x in range(w):
            o = x * channels
            if ct == 3:  # palette
                row.append(palette[line[o]])
            else:
                row.append(list(line[o:o + channels]))
            row[-1] = row[-1][:3] if channels in (3, 4, 2) and ct != 3 else row[-1]
        pixels.append(row)
    return w, h, channels, pixels


def to_gray(w: int, h: int, channels: int, pixels) -> list[list[int]]:
    """Convert decoded pixels to a 2-D grayscale array."""
    gray = []
    for y in range(h):
        row = []
        for x in range(w):
            px = pixels[y][x]
            if len(px) >= 3:
                # ITU-R BT.601 luma
                row.append(int(0.299 * px[0] + 0.587 * px[1] + 0.114 * px[2]))
            else:
                row.append(px[0])
        gray.append(row)
    return gray


def box_downscale(gray: list[list[int]], nw: int, nh: int) -> list[list[int]]:
    """Average-pool a grayscale image to nw x nh."""
    h, w = len(gray), len(gray[0])
    out = []
    for y in range(nh):
        y0, y1 = y * h // nh, (y + 1) * h // nh or h
        row = []
        for x in range(nw):
            x0, x1 = x * w // nw, (x + 1) * w // nw or w
            total = count = 0
            for yy in range(y0, max(y0 + 1, y1)):
                for xx in range(x0, max(x0 + 1, x1)):
                    total += gray[yy][xx]
                    count += 1
            row.append(total // max(1, count))
        out.append(row)
    return out


def ahash_gray(gray: list[list[int]]) -> str:
    """64-bit average hash as 16-char hex, from any-size grayscale."""
    small = box_downscale(gray, 8, 8)
    flat = [v for row in small for v in row]
    avg = sum(flat) / len(flat)
    bits = 0
    for v in flat:
        bits = (bits << 1) | (1 if v > avg else 0)
    return f"{bits:016x}"


def png_ahash(data: bytes) -> str:
    """One-shot: PNG bytes -> perceptual hash."""
    w, h, ch, px = decode_png(data)
    return ahash_gray(to_gray(w, h, ch, px))
