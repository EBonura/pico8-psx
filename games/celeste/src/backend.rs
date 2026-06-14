//! PSoXide rendering backend for the PICO-8 platform API.
//!
//! Re-implements ccleste's `platform.h` callbacks (spr/map/rectfill/
//! line/circfill/print/pal/camera/mget/fget) on PSoXide's GPU, using
//! immediate-mode draws. PICO-8 is a painter's-order renderer and so is
//! immediate-mode GP0, so call order == layer order, no ordering table.
//!
//! Display model (mirrors the old C++ port): the PICO-8 128x128 image is
//! drawn at 2x (256x256) because the spritesheet is pre-doubled. The
//! 256-wide field is centred in a 320x240 NTSC framebuffer; the 16px
//! vertical overflow is absorbed by `OFS_Y`.

use crate::assets::{
    font::FONT_DATA,
    gfx::GFX_DATA,
    palette::{PICO8_CLUT, PICO8_RGB, TEXT_CLUTS},
    tilemap::{MAP_W, TILEMAP_DATA, TILE_FLAGS},
};
use psx_gpu::{self as gpu};
use psx_hw::gpu::{pack_color, pack_texcoord, pack_vertex, pack_xy};
use psx_io::gpu::{wait_cmd_ready, write_gp0};
use psx_vram::{Clut, TexDepth, Tpage, VramRect, upload_16bpp};

// ---- VRAM layout (off-screen, right of the two 320x240 fb halves) ----
const GFX_TPAGE: Tpage = Tpage::new(640, 0, TexDepth::Bit4); // 256x256 4bpp -> 64 halfwords wide
const FONT_TPAGE: Tpage = Tpage::new(704, 0, TexDepth::Bit4); // 256x170 4bpp
const SPRITE_CLUT: Clut = Clut::new(0, 480); // 16 entries, below both fb halves
const TEXT_CLUT: Clut = Clut::new(0, 481); // one row, re-uploaded per print colour

// ---- Screen transform ----
const SCALE: i16 = 2;
const PLAY_W: i16 = 256; // 128 * SCALE
const OFS_X: i16 = (320 - PLAY_W) / 2; // centre horizontally -> 32
const OFS_Y: i16 = -8; // centre the 256-tall field in 240 (clip 8 top/bottom)

// ---- Mutable PICO-8 draw state ----
static mut CAM_X: i16 = 0;
static mut CAM_Y: i16 = 0;
/// PICO-8 `pal()` colour remap (draw index -> palette index).
static mut PAL: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Upload the spritesheet, font and sprite CLUT to VRAM. Call once after
/// `gpu::init`.
pub fn upload_assets() {
    upload_16bpp(VramRect::new(GFX_TPAGE.x(), GFX_TPAGE.y(), 64, 256), &GFX_DATA);
    upload_16bpp(VramRect::new(FONT_TPAGE.x(), FONT_TPAGE.y(), 64, 170), &FONT_DATA);
    upload_16bpp(VramRect::new(SPRITE_CLUT.x(), SPRITE_CLUT.y(), 16, 1), &PICO8_CLUT);
}

/// Re-apply the gfx tpage as the current draw mode. GP0 0x64 sprites
/// carry no tpage word, so this must precede sprite/map draws each frame.
#[inline]
pub fn begin_sprite_pass() {
    GFX_TPAGE.apply_as_draw_mode();
}

#[inline]
fn sx(px: i16) -> i16 {
    (px - unsafe { CAM_X }) * SCALE + OFS_X
}
#[inline]
fn sy(py: i16) -> i16 {
    (py - unsafe { CAM_Y }) * SCALE + OFS_Y
}

/// Resolve a PICO-8 colour index through the `pal()` remap to RGB888.
#[inline]
fn rgb(c: i32) -> (u8, u8, u8) {
    let idx = unsafe { PAL[(c as usize) & 15] } as usize;
    let e = PICO8_RGB[idx];
    (e[0], e[1], e[2])
}

// --------------------------------------------------------------------
// Sprite / map
// --------------------------------------------------------------------

/// Raw 16x16 textured-rect blit (GP0 0x64). Tpage must already be set.
#[inline]
fn blit16(x: i16, y: i16, u0: u8, v0: u8, clut_word: u16) {
    wait_cmd_ready();
    write_gp0(0x6400_0000 | pack_color(0x80, 0x80, 0x80));
    write_gp0(pack_vertex(x, y));
    write_gp0(pack_texcoord(u0, v0, clut_word));
    write_gp0(pack_xy(16, 16));
}

/// PICO-8 `spr()`. Draws 8x8 PICO-8 sprite `n` (16x16 in the doubled
/// sheet) at PICO-8 `(x,y)`. Non-flipped uses GP0 0x64; flipped uses a
/// textured quad with swapped UVs.
pub fn spr(n: i32, x: i16, y: i16, flip_x: bool, flip_y: bool) {
    if n < 0 {
        return;
    }
    let u0 = ((n % 16) * 16) as u8;
    let v0 = ((n / 16) * 16) as u8;
    let px = sx(x);
    let py = sy(y);
    let clut_word = SPRITE_CLUT.uv_clut_word();

    if !flip_x && !flip_y {
        begin_sprite_pass();
        blit16(px, py, u0, v0, clut_word);
        return;
    }

    // Flipped: textured quad, corners TL,TR,BL,BR; swap UV per axis.
    let (ul, ur) = if flip_x { (u0 + 16, u0) } else { (u0, u0 + 16) };
    let (vt, vb) = if flip_y { (v0 + 16, v0) } else { (v0, v0 + 16) };
    let verts = [(px, py), (px + 16, py), (px, py + 16), (px + 16, py + 16)];
    let uvs = [(ul, vt), (ur, vt), (ul, vb), (ur, vb)];
    gpu::draw_quad_textured(verts, uvs, clut_word, GFX_TPAGE.uv_tpage_word(0), (0x80, 0x80, 0x80));
}

/// `mget(x,y)` — raw map fetch (NOT camera/room relative).
#[inline]
pub fn mget(x: i32, y: i32) -> i32 {
    if x < 0 || y < 0 || x >= MAP_W as i32 {
        return 0;
    }
    let i = x as usize + y as usize * MAP_W;
    if i >= TILEMAP_DATA.len() {
        return 0;
    }
    TILEMAP_DATA[i] as i32
}

/// `fget(t,f)` — tile flag bit `f` of tile `t`.
#[inline]
pub fn fget(t: i32, f: i32) -> bool {
    if t < 0 || t as usize >= TILE_FLAGS.len() {
        return false;
    }
    (TILE_FLAGS[t as usize] >> f) & 1 != 0
}

/// PICO-8 `map()`. Draws map cells `[mx,mx+mw) x [my,my+mh)` at screen
/// `(tx,ty)`, filtered by `mask` exactly as the C++ port did
/// (`main.cpp:284-289`): 0 = all; 4 = flags==4 exactly; else flag bit
/// `mask-1`.
pub fn map(mx: i32, my: i32, tx: i16, ty: i16, mw: i32, mh: i32, mask: i32) {
    begin_sprite_pass();
    let clut_word = SPRITE_CLUT.uv_clut_word();
    for j in 0..mh {
        for i in 0..mw {
            let t = mget(mx + i, my + j);
            if mask != 0 {
                if t == 0 {
                    continue;
                }
                let flags = TILE_FLAGS.get(t as usize).copied().unwrap_or(0) as i32;
                let keep = if mask == 4 { flags == 4 } else { flags & (1 << (mask - 1)) != 0 };
                if !keep {
                    continue;
                }
            }
            let u0 = ((t % 16) * 16) as u8;
            let v0 = ((t / 16) * 16) as u8;
            let px = sx(tx + (i as i16) * 8);
            let py = sy(ty + (j as i16) * 8);
            blit16(px, py, u0, v0, clut_word);
        }
    }
}

// --------------------------------------------------------------------
// Flat shapes
// --------------------------------------------------------------------

/// PICO-8 `rectfill(x,y,x2,y2,c)` — inclusive, camera-relative.
pub fn rectfill(x: i16, y: i16, x2: i16, y2: i16, c: i32) {
    let (lx, rx) = if x <= x2 { (x, x2) } else { (x2, x) };
    let (ty, by) = if y <= y2 { (y, y2) } else { (y2, y) };
    let x0 = sx(lx);
    let y0 = sy(ty);
    let x1 = sx(rx + 1); // inclusive -> +1 px (then *2 in transform)
    let y1 = sy(by + 1);
    let (r, g, b) = rgb(c);
    gpu::draw_quad_flat([(x0, y0), (x1, y0), (x0, y1), (x1, y1)], r, g, b);
}

/// PICO-8 `line(x,y,x2,y2,c)`.
pub fn line(x: i16, y: i16, x2: i16, y2: i16, c: i32) {
    let (r, g, b) = rgb(c);
    gpu::draw_line_mono(sx(x), sy(y), sx(x2), sy(y2), r, g, b);
}

/// PICO-8 `circfill(x,y,r,c)` — span-filled from horizontal rows.
pub fn circfill(cx: i16, cy: i16, radius: i16, c: i32) {
    if radius < 0 {
        return;
    }
    let (r, g, b) = rgb(c);
    let mut dy = -radius;
    while dy <= radius {
        // half-width at this row (integer circle)
        let mut dx = 0;
        while dx * dx + dy * dy <= radius * radius {
            dx += 1;
        }
        dx -= 1;
        let x0 = sx(cx - dx);
        let x1 = sx(cx + dx + 1);
        let y0 = sy(cy + dy);
        let y1 = sy(cy + dy + 1);
        gpu::draw_quad_flat([(x0, y0), (x1, y0), (x0, y1), (x1, y1)], r, g, b);
        dy += 1;
    }
}

// --------------------------------------------------------------------
// Text
// --------------------------------------------------------------------

/// PICO-8 `print(str,x,y,c)` — 4px advance per char, font from the font
/// tpage with a per-colour CLUT.
pub fn print(s: &[u8], x: i16, y: i16, c: i32) {
    // Select the CLUT for this colour and upload it (one row).
    let clut_idx = (unsafe { PAL[(c as usize) & 15] }) as usize;
    upload_16bpp(VramRect::new(TEXT_CLUT.x(), TEXT_CLUT.y(), 16, 1), &TEXT_CLUTS[clut_idx & 15]);
    let clut_word = TEXT_CLUT.uv_clut_word();
    FONT_TPAGE.apply_as_draw_mode();

    let mut cx = x;
    for &ch in s {
        let ci = (ch & 0x7F) as i32;
        let u0 = ((ci % 16) * 16) as u8;
        let v0 = ((ci / 16) * 16) as u8;
        // 8x8 glyph (doubled), drawn at the doubled screen position.
        let px = sx(cx);
        let py = sy(y);
        wait_cmd_ready();
        write_gp0(0x6400_0000 | pack_color(0x80, 0x80, 0x80));
        write_gp0(pack_vertex(px, py));
        write_gp0(pack_texcoord(u0, v0, clut_word));
        write_gp0(pack_xy(16, 16));
        cx += 4; // PICO-8 4px advance
    }
}

// --------------------------------------------------------------------
// State
// --------------------------------------------------------------------

/// PICO-8 `camera(x,y)`.
pub fn camera(x: i16, y: i16) {
    unsafe {
        CAM_X = x;
        CAM_Y = y;
    }
}

/// PICO-8 `pal(a,b)` — remap draw colour `a` to `b`.
pub fn pal(a: i32, b: i32) {
    unsafe {
        PAL[(a as usize) & 15] = (b & 15) as u8;
    }
}

/// PICO-8 `pal()` — reset the colour remap.
pub fn pal_reset() {
    unsafe {
        let mut i = 0u8;
        while (i as usize) < 16 {
            PAL[i as usize] = i;
            i += 1;
        }
    }
}
