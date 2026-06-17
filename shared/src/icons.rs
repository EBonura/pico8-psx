//! PlayStation button-icon atlas: full-colour 15bpp textured quads, used for the
//! control hints (Start/Select in the launcher, Resume/Triangle/Circle in the
//! menus and pause). The source art is downscaled and packed by
//! `tools/gen_icons.py` into [`icons_data`]; this module uploads it and draws it.
//!
//! The atlas lives at VRAM (576, 0), in the gap between the menu font (x320..576)
//! and the covers / in-game gfx tpages (x640+), so it is clear of both the
//! launcher's and the games' VRAM. Re-upload on each screen that draws icons
//! (the running game owns VRAM), exactly like the fonts.

use psx_gpu::{self as gpu};
use psx_vram::{upload_16bpp, TexDepth, Tpage, VramRect};

pub use crate::icons_data::Cell;
pub use crate::icons_data::{CIRCLE, CROSS, SELECT, START, TRIANGLE};
use crate::icons_data::{ATLAS, ATLAS_H, ATLAS_W};

const ICON_TPAGE: Tpage = Tpage::new(576, 0, TexDepth::Bit15);

/// Upload the icon atlas to VRAM (call once per menu/pause open).
pub fn upload() {
    upload_16bpp(VramRect::new(576, 0, ATLAS_W as u16, ATLAS_H as u16), &ATLAS);
}

/// Draw `cell` at screen `(x, y)`, native size. Returns its width (for layout).
pub fn draw(cell: &Cell, x: i16, y: i16) -> i16 {
    let (u, v, w, h) = (cell.u, cell.v, cell.w, cell.h);
    let verts = [(x, y), (x + w as i16, y), (x, y + h as i16), (x + w as i16, y + h as i16)];
    let uvs = [(u, v), (u + w, v), (u, v + h), (u + w, v + h)];
    // 15bpp: clut word ignored; texel 0x0000 is transparent.
    gpu::draw_quad_textured(verts, uvs, 0, ICON_TPAGE.uv_tpage_word(0), (0x80, 0x80, 0x80));
    w as i16
}

/// Native width of an icon cell.
#[inline]
pub fn width(cell: &Cell) -> i16 {
    cell.w as i16
}
