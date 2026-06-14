#!/usr/bin/env python3
"""Convert a PICO-8 cart (.p8) into the PSoXide asset Rust modules used by
this collection: `gfx.rs` (spritesheet) and `tilemap.rs` (map + flags).

PICO-8 stores graphics 4bpp and the PSoXide backend draws the 128x128
PICO-8 image pre-doubled to 256x256 (each pixel 2x2), exactly mirroring
the original ccleste-derived Celeste assets. The `font.rs` and
`palette.rs` modules are universal PICO-8 data and are shared verbatim
between games, so this tool does not regenerate them.

Usage:
    python3 tools/p8_to_rust.py <cart.p8> <out_dir>

Emits <out_dir>/gfx.rs and <out_dir>/tilemap.rs.
"""

import os
import sys


def parse_sections(path):
    """Return {name: [lines]} for the __gfx__/__gff__/__map__ sections."""
    sections = {}
    cur = None
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if line.startswith("__") and line.endswith("__"):
                cur = line.strip("_")
                sections[cur] = []
            elif cur is not None:
                sections[cur].append(line)
    return sections


def gfx_pixels(gfx_lines):
    """Decode __gfx__ into a 128x128 grid of palette indices (0-15).

    Each character is one pixel; rows/cols beyond the supplied data are 0.
    """
    grid = [[0] * 128 for _ in range(128)]
    for y, line in enumerate(gfx_lines[:128]):
        for x, ch in enumerate(line[:128]):
            grid[y][x] = int(ch, 16)
    return grid


def map_bytes(sections):
    """Reconstruct the full 128x64 PICO-8 map (8192 bytes).

    Rows 0..31 come from __map__ (0x2000 region). Rows 32..63 share memory
    with the bottom half of the spritesheet (0x1000), so they are read back
    out of the decoded gfx pixels two-per-byte, low nibble first.
    """
    data = bytearray(8192)

    # Top half: __map__, two hex chars per tile.
    for y, line in enumerate(sections.get("map", [])[:32]):
        for x in range(min(128, len(line) // 2)):
            data[x + y * 128] = int(line[2 * x : 2 * x + 2], 16)

    # Bottom half: shared with spritesheet rows 64..127. The shared region
    # is a raw byte view of RAM 0x1000 + (y-32)*128 + x; as spritesheet bytes,
    # address a holds pixels (2*(a%64), 2*(a%64)+1) of gfx row a//64.
    px = gfx_pixels(sections.get("gfx", []))
    for y in range(32, 64):
        for x in range(128):
            a = 0x1000 + (y - 32) * 128 + x
            row = a // 64
            col = (a % 64) * 2
            lo = px[row][col]
            hi = px[row][col + 1]
            data[x + y * 128] = lo | (hi << 4)
    return data


def tile_flags(sections):
    """Decode __gff__ into 256 per-sprite flag bytes."""
    flags = bytearray(256)
    raw = "".join(sections.get("gff", []))
    for i in range(min(256, len(raw) // 2)):
        flags[i] = int(raw[2 * i : 2 * i + 2], 16)
    return flags


def double_and_pack(px):
    """Double the 128x128 pixel grid to 256x256 and pack to 4bpp u16s.

    PS1 4bpp packs 4 pixels per halfword, low nibble = leftmost pixel.
    Returns a flat list of 16384 ints (256 rows x 64 halfwords).
    """
    words = []
    for y in range(128):
        drow = []
        for x in range(128):
            drow.append(px[y][x])
            drow.append(px[y][x])
        for _ in range(2):  # each source row drawn twice
            for x in range(0, 256, 4):
                w = (drow[x] & 0xF) | ((drow[x + 1] & 0xF) << 4) \
                    | ((drow[x + 2] & 0xF) << 8) | ((drow[x + 3] & 0xF) << 12)
                words.append(w)
    return words


HEADER = ("// Auto-generated from {cart} by tools/p8_to_rust.py. "
          "Do not edit by hand.\n")


def write_gfx(out_dir, cart, words):
    with open(os.path.join(out_dir, "gfx.rs"), "w") as f:
        f.write(HEADER.format(cart=cart))
        f.write("\n// 256x256 @ 4bpp (64 halfwords/row). Upload to a Bit4 tpage.\n")
        f.write(f"pub static GFX_DATA: [u16; {len(words)}] = [\n")
        for i in range(0, len(words), 64):
            chunk = words[i : i + 64]
            f.write("    " + ", ".join(f"0x{w:04X}" for w in chunk) + ",\n")
        f.write("];\n")


def write_tilemap(out_dir, cart, data, flags):
    with open(os.path.join(out_dir, "tilemap.rs"), "w") as f:
        f.write(HEADER.format(cart=cart))
        f.write("\n// PICO-8 map: 128 wide. mget(x,y) = TILEMAP_DATA[x + y*128].\n")
        f.write("pub const MAP_W: usize = 128;\n")
        f.write(f"pub static TILEMAP_DATA: [u8; {len(data)}] = [\n")
        for i in range(0, len(data), 32):
            chunk = data[i : i + 32]
            f.write("    " + ", ".join(f"0x{b:02X}" for b in chunk) + ",\n")
        f.write("];\n\n")
        f.write("// Per-sprite flag byte (PICO-8 fget): bit f of sprite t.\n")
        f.write(f"pub static TILE_FLAGS: [u8; {len(flags)}] = [\n")
        for i in range(0, len(flags), 16):
            chunk = flags[i : i + 16]
            f.write("    " + ", ".join(f"0x{b:02X}" for b in chunk) + ",\n")
        f.write("];\n")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    cart, out_dir = sys.argv[1], sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    sections = parse_sections(cart)
    cart_name = os.path.basename(cart)

    px = gfx_pixels(sections.get("gfx", []))
    write_gfx(out_dir, cart_name, double_and_pack(px))
    write_tilemap(out_dir, cart_name, map_bytes(sections), tile_flags(sections))
    print(f"Wrote gfx.rs and tilemap.rs to {out_dir} from {cart_name}")


if __name__ == "__main__":
    main()
