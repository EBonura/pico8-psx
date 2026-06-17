//! PSoXide rendering backend for the PICO-8 platform API.
//!
//! Re-implements PICO-8's draw callbacks (spr/map/rectfill/line/circfill/
//! print/pal/camera/mget/fget) on PSoXide's GPU, using immediate-mode draws.
//! PICO-8 is a painter's-order renderer and so is immediate-mode GP0, so call
//! order == layer order, no ordering table.
//!
//! Display model: the PICO-8 128x128 image is drawn at 2x (256x256) because
//! the spritesheet is pre-doubled. The 256-wide field is centred in a 320x240
//! NTSC framebuffer; the 16px vertical overflow is absorbed by `OFS_Y`.
//!
//! The spritesheet and tilemap are game-specific, so they come from a [`Cart`]
//! the game registers via [`set_cart`] / [`upload_assets`]; the font and CLUT
//! are universal PICO-8 data and live in this crate.

use crate::font::FONT_DATA;
use crate::palette::{PICO8_CLUT, PICO8_RGB, TEXT_CLUTS};
use psx_gpu::material::{TextureMaterial, TextureWindow};
use psx_gpu::{self as gpu};
use psx_hw::gpu::{pack_color, pack_texcoord, pack_vertex, pack_xy};
use psx_io::gpu::{wait_cmd_ready, write_gp0};
use psx_vram::{Clut, TexDepth, Tpage, VramRect, upload_16bpp};

/// A game's PICO-8 graphics data: the doubled 256x256 4bpp spritesheet and the
/// tilemap (cells + per-sprite flags). Registered once via [`set_cart`]; the
/// backend's `spr`/`map`/`mget`/`fget` read from the active cart.
#[derive(Clone, Copy)]
pub struct Cart {
    /// 256x256 @ 4bpp spritesheet, 64 halfwords/row (== [u16; 16384]).
    pub gfx: &'static [u16],
    /// Map cells, `map_w` wide.
    pub tilemap: &'static [u8],
    /// Per-sprite flag byte, indexed by sprite id.
    pub tile_flags: &'static [u8],
    /// Map width in cells.
    pub map_w: usize,
}

const EMPTY_CART: Cart = Cart { gfx: &[], tilemap: &[], tile_flags: &[], map_w: 128 };

// ---- VRAM layout (off-screen, right of the framebuffers) ----
const GFX_TPAGE: Tpage = Tpage::new(640, 0, TexDepth::Bit4); // 256x256 4bpp -> 64 halfwords wide
const FONT_TPAGE: Tpage = Tpage::new(704, 0, TexDepth::Bit4); // 256x170 4bpp
const SPRITE_CLUT: Clut = Clut::new(0, 480); // 16 entries, below the framebuffers
const TEXT_CLUT: Clut = Clut::new(0, 481); // one row, re-uploaded per print colour
const FILLP_TPAGE: Tpage = Tpage::new(768, 0, TexDepth::Bit4); // 8x8 dither patterns side-by-side
const FILL_CLUT: Clut = Clut::new(0, 482); // 2 entries (0 = transparent, 1 = fill colour)

// ---- Screen transform ----
const SCALE: i16 = 2;
const PLAY_W: i16 = 256; // 128 * SCALE
const OFS_X: i16 = (320 - PLAY_W) / 2; // centre horizontally -> 32
const OFS_Y: i16 = -8; // centre the 256-tall field in 240 (clip 8 top/bottom)

// ---- Mutable PICO-8 draw state ----
static mut CART: Cart = EMPTY_CART;
static mut CAM_X: i16 = 0;
static mut CAM_Y: i16 = 0;
/// PICO-8 `pal()` colour remap (draw index -> palette index).
static mut PAL: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Register the active cart (spritesheet + map). Call before drawing.
pub fn set_cart(cart: Cart) {
    unsafe { CART = cart };
}

/// Register `cart` and upload its spritesheet plus the universal font and
/// sprite CLUT to VRAM. Call once after `gpu::init`.
pub fn upload_assets(cart: Cart) {
    set_cart(cart);
    upload_16bpp(VramRect::new(GFX_TPAGE.x(), GFX_TPAGE.y(), 64, 256), cart.gfx);
    upload_16bpp(VramRect::new(FONT_TPAGE.x(), FONT_TPAGE.y(), 64, 170), &FONT_DATA);
    upload_16bpp(VramRect::new(SPRITE_CLUT.x(), SPRITE_CLUT.y(), 16, 1), &PICO8_CLUT);
    upload_fillp();
}

/// Upload just the universal PICO-8 font (and reset the colour remap), so a
/// screen with no cart -- e.g. the launcher's intro/credits -- can use [`print`]
/// for PICO-8-font text. Coordinates are PICO-8 128-space (scaled 2x, centred);
/// set `camera(0, 0)` first. Call once after `gpu::init`.
pub fn upload_font() {
    upload_16bpp(VramRect::new(FONT_TPAGE.x(), FONT_TPAGE.y(), 64, 170), &FONT_DATA);
    pal_reset();
}

/// Re-apply the gfx tpage as the current draw mode. GP0 0x64 sprites carry no
/// tpage word, so this must precede sprite/map draws each frame.
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

/// A 16x16 primitive at screen `(x,y)` is fully outside the 320x240 frame.
/// The GP0 vertex word is 11-bit signed, so a primitive at screen x >= 1024
/// wraps to the left edge: a wide level's far-right objects (e.g. celeste2
/// level-3 spikes at col 123) reappear as "floating" sprites on the left when
/// the camera sits near the level's left edge. PICO-8's spr()/map() clip to the
/// screen; we must cull here so the wrap can never happen.
#[inline]
fn offscreen16(x: i16, y: i16) -> bool {
    x <= -16 || x >= 320 || y <= -16 || y >= 240
}

/// Raw 16x16 textured-rect blit (GP0 0x64). Tpage must already be set.
#[inline]
fn blit16(x: i16, y: i16, u0: u8, v0: u8, clut_word: u16) {
    if offscreen16(x, y) {
        return;
    }
    wait_cmd_ready();
    write_gp0(0x6400_0000 | pack_color(0x80, 0x80, 0x80));
    write_gp0(pack_vertex(x, y));
    write_gp0(pack_texcoord(u0, v0, clut_word));
    write_gp0(pack_xy(16, 16));
}

/// PICO-8 `spr()`. Draws 8x8 PICO-8 sprite `n` (16x16 in the doubled sheet) at
/// PICO-8 `(x,y)`. Non-flipped uses GP0 0x64; flipped uses a textured quad
/// with swapped UVs.
pub fn spr(n: i32, x: i16, y: i16, flip_x: bool, flip_y: bool) {
    if n < 0 {
        return;
    }
    let u0 = ((n % 16) * 16) as u8;
    let v0 = ((n / 16) * 16) as u8;
    let px = sx(x);
    let py = sy(y);
    if offscreen16(px, py) {
        return; // cull before the GP0 11-bit vertex can wrap onto the screen
    }
    let clut_word = SPRITE_CLUT.uv_clut_word();

    if !flip_x && !flip_y {
        begin_sprite_pass();
        blit16(px, py, u0, v0, clut_word);
        return;
    }

    // Flipped: textured quad, corners TL,TR,BL,BR; swap UV per axis.
    // The far UV edge is u0/v0 + 16, but UVs are u8: a last-column sprite (u0=240,
    // e.g. the smoke's final frame 31) would wrap 256 -> 0 and the quad would
    // sample the whole texture row as vertical multicolour garbage. Clamp the far
    // edge to 255 (the last texel) so it stays inside the sprite's tile.
    let u_hi = (u0 as u16 + 16).min(255) as u8;
    let v_hi = (v0 as u16 + 16).min(255) as u8;
    let (ul, ur) = if flip_x { (u_hi, u0) } else { (u0, u_hi) };
    let (vt, vb) = if flip_y { (v_hi, v0) } else { (v0, v_hi) };
    let verts = [(px, py), (px + 16, py), (px, py + 16), (px + 16, py + 16)];
    let uvs = [(ul, vt), (ur, vt), (ul, vb), (ur, vb)];
    gpu::draw_quad_textured(verts, uvs, clut_word, GFX_TPAGE.uv_tpage_word(0), (0x80, 0x80, 0x80));
}

/// `mget(x,y)` -- raw map fetch (NOT camera/room relative).
#[inline]
pub fn mget(x: i32, y: i32) -> i32 {
    let cart = unsafe { CART };
    if x < 0 || y < 0 || x >= cart.map_w as i32 {
        return 0;
    }
    let i = x as usize + y as usize * cart.map_w;
    if i >= cart.tilemap.len() {
        return 0;
    }
    cart.tilemap[i] as i32
}

/// `fget(t,f)` -- tile flag bit `f` of tile `t`.
#[inline]
pub fn fget(t: i32, f: i32) -> bool {
    let cart = unsafe { CART };
    if t < 0 || t as usize >= cart.tile_flags.len() {
        return false;
    }
    (cart.tile_flags[t as usize] >> f) & 1 != 0
}

/// PICO-8 `map()`. Draws map cells `[mx,mx+mw) x [my,my+mh)` at screen
/// `(tx,ty)`, filtered by `mask`: 0 = all; 4 = flags==4 exactly; else flag bit
/// `mask-1`. PICO-8 `map()` never draws sprite 0 (treated as empty).
pub fn map(mx: i32, my: i32, tx: i16, ty: i16, mw: i32, mh: i32, mask: i32) {
    begin_sprite_pass();
    let clut_word = SPRITE_CLUT.uv_clut_word();
    let cart = unsafe { CART };
    for j in 0..mh {
        for i in 0..mw {
            let t = mget(mx + i, my + j);
            if t == 0 {
                continue;
            }
            if mask != 0 {
                let flags = cart.tile_flags.get(t as usize).copied().unwrap_or(0) as i32;
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

/// PICO-8 `rectfill(x,y,x2,y2,c)` -- inclusive, camera-relative.
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

// ---- fillp (PICO-8 fill patterns / dither) -------------------------------
// PICO-8's `fillp` overlays a screen-fixed 4x4 dither over fills. The PS1 has no
// such mode, so we tile a tiny 8x8 pattern (2x2 copies of the 4x4) via the GPU's
// texture window: the pattern's set texels draw the fill colour, unset texels map
// to a transparent CLUT entry so the background shows through. UVs follow screen
// position, so the pattern stays screen-locked as the camera scrolls, like the cart.

/// fillp pattern ids (index into FILLP_VALUES; the cart's 16-bit 4x4 patterns).
pub const FILLP_COLUMNS: usize = 0; // sparse dots (background pillars)
pub const FILLP_FOG: usize = 1; // 50% checkerboard (fog)
pub const FILLP_CRUMBLE: usize = 2; // checkerboard, opposite phase (crumble crack)
const FILLP_VALUES: [u16; 3] =
    [0b0000_1000_0000_0010, 0b0101_1010_0101_1010, 0b1010_0101_1010_0101];

/// Build an 8x8 4bpp tile from a cart 4x4 fillp pattern: texel 1 where the bit is
/// set (drawn), 0 elsewhere (transparent). 16 halfwords (2 wide x 8 tall).
fn build_pattern(p: u16) -> [u16; 16] {
    let mut out = [0u16; 16];
    let mut y = 0usize;
    while y < 8 {
        let mut hw = [0u16; 2];
        let mut x = 0usize;
        while x < 8 {
            let bit = 15 - ((y % 4) * 4 + (x % 4));
            if (p >> bit) & 1 != 0 {
                hw[x / 4] |= 1u16 << ((x % 4) * 4);
            }
            x += 1;
        }
        out[y * 2] = hw[0];
        out[y * 2 + 1] = hw[1];
        y += 1;
    }
    out
}

/// Upload the dither patterns side-by-side in FILLP_TPAGE (pattern i at U = i*8).
/// Called from [`upload_assets`]; re-run per game boot since VRAM is shared.
fn upload_fillp() {
    let mut i = 0;
    while i < FILLP_VALUES.len() {
        let tile = build_pattern(FILLP_VALUES[i]);
        upload_16bpp(VramRect::new(FILLP_TPAGE.x() + (i as u16) * 2, FILLP_TPAGE.y(), 2, 8), &tile);
        i += 1;
    }
}

/// Set the GPU texture window (GP0 0xE2). Masks/offsets are in 8-pixel units.
#[inline]
unsafe fn set_tex_window(mask_x: u32, mask_y: u32, off_x: u32, off_y: u32) {
    wait_cmd_ready();
    write_gp0(
        0xE200_0000
            | (mask_x & 0x1F)
            | ((mask_y & 0x1F) << 5)
            | ((off_x & 0x1F) << 10)
            | ((off_y & 0x1F) << 15),
    );
}

/// Upload the fill CLUT (1 = colour, 0 = transparent), select the pattern's tpage
/// as the draw mode (GP0 0x2C polys sample the draw-mode tpage), and return the
/// material to draw dithered quads with. The window MUST travel in the material:
/// draw_quad_textured_material re-emits it, overwriting a standalone GP0 0xE2.
/// Callers must `set_tex_window(0,0,0,0)` after their draws to restore sprites/map.
unsafe fn fillp_material(c: i32, pattern: usize) -> TextureMaterial {
    let col = PICO8_CLUT[(PAL[(c as usize) & 15] as usize) & 15];
    upload_16bpp(VramRect::new(FILL_CLUT.x(), FILL_CLUT.y(), 2, 1), &[0u16, col]);
    FILLP_TPAGE.apply_as_draw_mode();
    let win = TextureWindow::power_of_two_tile((pattern as u8) * 8, 0, 8, 8); // 8x8 tile @ pattern*8
    TextureMaterial::opaque(FILL_CLUT.uv_clut_word(), FILLP_TPAGE.uv_tpage_word(0), (0x80, 0x80, 0x80))
        .with_texture_window(win)
}

/// PICO-8 `fillp(pattern) rectfill(x0,y0,x1,y1,c)`: a dithered rectangle. The
/// pattern is screen-fixed, so the rect is clamped to the visible camera window
/// and the pattern phase follows screen position. `pattern` is one of FILLP_*.
pub fn fillp_rect(x0: i16, y0: i16, x1: i16, y1: i16, c: i32, pattern: usize) {
    let (lx0, lx1) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let (ly0, ly1) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    unsafe {
        // clamp to the 128px camera window (fillp is screen-space)
        let lx = lx0.max(CAM_X);
        let rx = (lx1 + 1).min(CAM_X + 128);
        let ty = ly0.max(CAM_Y);
        let by = (ly1 + 1).min(CAM_Y + 128);
        if rx <= lx || by <= ty {
            return;
        }
        let mat = fillp_material(c, pattern);
        let (u0, v0) = ((lx - CAM_X) as u8, (ty - CAM_Y) as u8);
        let (u1, v1) = ((rx - CAM_X) as u8, (by - CAM_Y) as u8);
        let verts = [(sx(lx), sy(ty)), (sx(rx), sy(ty)), (sx(lx), sy(by)), (sx(rx), sy(by))];
        let uvs = [(u0, v0), (u1, v0), (u0, v1), (u1, v1)];
        gpu::draw_quad_textured_material(verts, uvs, mat);
        set_tex_window(0, 0, 0, 0); // reset the window so sprites/map sample fully
    }
}

/// PICO-8 `fillp(pattern) circfill(cx,cy,r,c)`: a dithered filled circle, drawn as
/// dithered 1px scanlines (same midpoint span walk as [`circfill`]). Used for the
/// fog clouds. Screen-fixed dither, clamped to the camera window per scanline.
pub fn fillp_circfill(cx: i16, cy: i16, radius: i16, c: i32, pattern: usize) {
    if radius < 0 {
        return;
    }
    unsafe {
        let mat = fillp_material(c, pattern);
        let (camx, camy) = (CAM_X, CAM_Y);
        let span = |dx: i32, yy: i16| {
            if yy < camy || yy >= camy + 128 {
                return;
            }
            let lx = (cx - dx as i16).max(camx);
            let rx = (cx + dx as i16 + 1).min(camx + 128);
            if rx <= lx {
                return;
            }
            let (u0, u1, v) = ((lx - camx) as u8, (rx - camx) as u8, (yy - camy) as u8);
            let verts =
                [(sx(lx), sy(yy)), (sx(rx), sy(yy)), (sx(lx), sy(yy + 1)), (sx(rx), sy(yy + 1))];
            let uvs = [(u0, v), (u1, v), (u0, v.wrapping_add(1)), (u1, v.wrapping_add(1))];
            gpu::draw_quad_textured_material(verts, uvs, mat);
        };
        let r2 = radius as i32 * radius as i32;
        let mut dx = radius as i32;
        let mut dy = 0i32;
        while dy <= radius as i32 {
            while dx * dx + dy * dy > r2 {
                dx -= 1;
            }
            span(dx, cy + dy as i16);
            if dy != 0 {
                span(dx, cy - dy as i16);
            }
            dy += 1;
        }
        set_tex_window(0, 0, 0, 0);
    }
}

/// PICO-8 `line(x,y,x2,y2,c)`.
pub fn line(x: i16, y: i16, x2: i16, y2: i16, c: i32) {
    let (r, g, b) = rgb(c);
    gpu::draw_line_mono(sx(x), sy(y), sx(x2), sy(y2), r, g, b);
}

/// Flat-filled quad with arbitrary corners, in PICO-8 128-space. Corner order is a
/// strip (v0,v1,v2,v3), same as `rectfill`. Use for diagonal shapes that GP0 lines
/// render unreliably on hardware-accelerated PS1 emulators (e.g. the grapple-pickup
/// rays drawn as thin quads).
pub fn quad(p: [(i16, i16); 4], c: i32) {
    let (r, g, b) = rgb(c);
    gpu::draw_quad_flat(
        [
            (sx(p[0].0), sy(p[0].1)),
            (sx(p[1].0), sy(p[1].1)),
            (sx(p[2].0), sy(p[2].1)),
            (sx(p[3].0), sy(p[3].1)),
        ],
        r,
        g,
        b,
    );
}

/// PICO-8 `circfill(x,y,r,c)` -- one 1px span per row, drawn symmetrically about
/// the centre. `dx` is tracked monotonically downward as `dy` grows (it only ever
/// decreases), so the total span work is O(r), not the naive O(r^2). This is hot
/// (clouds/hair/particles issue dozens per frame) but with the real VBlank sync
/// there's ample budget, so spans stay 1px for exact circles.
pub fn circfill(cx: i16, cy: i16, radius: i16, c: i32) {
    if radius < 0 {
        return;
    }
    let (r, g, b) = rgb(c);
    let r2 = radius as i32 * radius as i32;
    let row = |dx: i32, yy: i16| {
        let x0 = sx(cx - dx as i16);
        let x1 = sx(cx + dx as i16 + 1);
        let y0 = sy(yy);
        let y1 = sy(yy + 1);
        gpu::draw_quad_flat([(x0, y0), (x1, y0), (x0, y1), (x1, y1)], r, g, b);
    };
    let mut dx = radius as i32;
    let mut dy = 0i32;
    while dy <= radius as i32 {
        while dx * dx + dy * dy > r2 {
            dx -= 1;
        }
        row(dx, cy + dy as i16);
        if dy != 0 {
            row(dx, cy - dy as i16);
        }
        dy += 1;
    }
}

/// PICO-8 `circ(x,y,r,c)` -- 1px outline (midpoint circle), each point a 2x2
/// screen dot. Used for the berry pickup flash ring.
pub fn circ(cx: i16, cy: i16, radius: i16, c: i32) {
    if radius < 0 {
        return;
    }
    let (r, g, b) = rgb(c);
    let dot = |x: i16, y: i16| {
        let x0 = sx(x);
        let y0 = sy(y);
        gpu::draw_quad_flat([(x0, y0), (x0 + 2, y0), (x0, y0 + 2), (x0 + 2, y0 + 2)], r, g, b);
    };
    let mut x = radius as i32;
    let mut y = 0i32;
    let mut err = 0i32;
    while x >= y {
        for (ox, oy) in [(x, y), (y, x), (-x, y), (-y, x), (x, -y), (y, -x), (-x, -y), (-y, -x)] {
            dot(cx + ox as i16, cy + oy as i16);
        }
        y += 1;
        if err <= 0 {
            err += 2 * y + 1;
        }
        if err > 0 {
            x -= 1;
            err -= 2 * x + 1;
        }
    }
}

// --------------------------------------------------------------------
// Text
// --------------------------------------------------------------------

/// PICO-8 `print(str,x,y,c)` -- 4px advance per char, font from the font tpage
/// with a per-colour CLUT.
pub fn print(s: &[u8], x: i16, y: i16, c: i32) {
    let clut_idx = (unsafe { PAL[(c as usize) & 15] }) as usize;
    upload_16bpp(VramRect::new(TEXT_CLUT.x(), TEXT_CLUT.y(), 16, 1), &TEXT_CLUTS[clut_idx & 15]);
    let clut_word = TEXT_CLUT.uv_clut_word();
    FONT_TPAGE.apply_as_draw_mode();

    let mut cx = x;
    for &ch in s {
        let ci = (ch & 0x7F) as i32;
        let u0 = ((ci % 16) * 16) as u8;
        let v0 = ((ci / 16) * 16) as u8;
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

/// Re-upload the sprite CLUT so textured draws (spr/map) honour the current
/// `pal()` remap -- entry i = the palette colour PAL[i] points at. circfill/
/// rectfill already remap via PAL directly; sprites read this CLUT, so e.g.
/// Madeline's hair (colour 8) only turns blue on dash once this is in sync.
fn sync_sprite_clut() {
    let mut c = [0u16; 16];
    let mut i = 0usize;
    while i < 16 {
        c[i] = PICO8_CLUT[(unsafe { PAL[i] } as usize) & 15];
        i += 1;
    }
    upload_16bpp(VramRect::new(SPRITE_CLUT.x(), SPRITE_CLUT.y(), 16, 1), &c);
}

/// Wait for the GPU to finish the queued draws. Call before a palette change
/// that must not retro-actively affect already-issued sprite draws (the sprite
/// CLUT is shared VRAM, so an unset/reset would otherwise recolour them).
pub fn flush() {
    gpu::draw_sync();
}

/// PICO-8 `pal(a,b)` -- remap draw colour `a` to `b`.
pub fn pal(a: i32, b: i32) {
    unsafe {
        PAL[(a as usize) & 15] = (b & 15) as u8;
    }
    sync_sprite_clut();
}

/// PICO-8 `pal()` -- reset the colour remap.
pub fn pal_reset() {
    unsafe {
        let mut i = 0u8;
        while (i as usize) < 16 {
            PAL[i as usize] = i;
            i += 1;
        }
    }
    sync_sprite_clut();
}
