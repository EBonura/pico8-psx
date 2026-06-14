#!/usr/bin/env python3
"""Convert a 128x128 PICO-8 cart-label PNG into a PS1 4bpp cover texture.

The launcher menu shows each game's real PICO-8 cart label as its cover.
Cart labels are PICO-8 screenshots, so every pixel already maps to one of
the 16 PICO-8 palette colours; this quantises each pixel to the nearest
palette index and packs it 4bpp (low nibble = leftmost pixel), the same
format the in-game spritesheets use, so the menu can upload it with the
shared PICO8_CLUT.

Usage:
    python3 tools/png_to_cover.py <label.png> <out.rs> <ARRAY_NAME>
"""

import os
import sys

PICO8_RGB = [
    (0, 0, 0), (29, 43, 83), (126, 37, 83), (0, 135, 81),
    (171, 82, 54), (95, 87, 79), (194, 195, 199), (255, 241, 232),
    (255, 0, 77), (255, 163, 0), (255, 236, 39), (0, 228, 54),
    (41, 173, 255), (131, 118, 156), (255, 119, 168), (255, 204, 170),
]


def nearest(r, g, b):
    best, bi = 1 << 30, 0
    for i, (pr, pg, pb) in enumerate(PICO8_RGB):
        d = (r - pr) ** 2 + (g - pg) ** 2 + (b - pb) ** 2
        if d < best:
            best, bi = d, i
    return bi


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    png, out, name = sys.argv[1], sys.argv[2], sys.argv[3]
    from PIL import Image
    im = Image.open(png).convert("RGB").resize((128, 128))
    px = im.load()

    words = []
    for y in range(128):
        for x in range(0, 128, 4):
            p = [nearest(*px[x + k, y]) for k in range(4)]
            words.append(p[0] | (p[1] << 4) | (p[2] << 8) | (p[3] << 12))

    os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
    with open(out, "w") as f:
        f.write(f"// Auto-generated from {os.path.basename(png)} by "
                "tools/png_to_cover.py. Do not edit by hand.\n\n")
        f.write(f"// 128x128 @ 4bpp (32 halfwords/row), PICO8_CLUT palette.\n")
        f.write(f"pub static {name}: [u16; {len(words)}] = [\n")
        for i in range(0, len(words), 32):
            f.write("    " + ", ".join(f"0x{w:04X}" for w in words[i:i + 32]) + ",\n")
        f.write("];\n")
    print(f"Wrote {name} ({len(words)} halfwords) to {out}")


if __name__ == "__main__":
    main()
