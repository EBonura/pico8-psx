//! pico8-psx demo disc -- the headline artifact.
//!
//! One bootable PS1 image that opens to a cover menu showing the real
//! PICO-8 cart labels for each demade game; pick one with the D-pad and
//! press X to launch it. Each game is linked in as a library and exposes
//! `run()`, which boots the game and returns when the player holds
//! Select+Start -- dropping back here to the menu.
//!
//! Rendering: the 128x128 cart labels live in their own off-screen 4bpp
//! tpages (x >= 768, clear of the framebuffers and the games' own VRAM)
//! and are blitted as scaled textured quads; menu text uses psx-font's
//! BASIC atlas. Because each game clobbers VRAM while it runs, the menu
//! re-uploads all of its own VRAM every time it is shown.

#![no_std]
#![no_main]

extern crate psx_rt;

mod assets;

use assets::cover_celeste::COVER_CELESTE;
use assets::cover_celeste2::COVER_CELESTE2;
use assets::palette::PICO8_CLUT;

use psx_font::{fonts::BASIC, FontAtlas};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_pad::{button, poll_port1, ButtonState};
use psx_vram::{upload_16bpp, Clut, TexDepth, Tpage, VramRect};

// ---- VRAM layout -----------------------------------------------------
// The two 320x240 framebuffers stack vertically (x 0..320, y 0..240 and
// y 240..480), so everything here lives at x >= 320, clear of them.
//
// The font atlas goes at x=320 because `FontAtlas::upload` internally
// builds a VramRect of width `MAX_ATLAS_W_TEXELS` (256 texels) at the
// tpage origin -- so the font tpage X must be <= 768 or that rect's
// (x + 256) overflows VRAM width (1024) and panics. The covers then sit
// past that reserved strip, at 768 / 832. Tpage X must be a multiple of
// 64; Y is 0. (Games re-upload their own VRAM, so sharing these columns
// with the in-game tpages is fine -- only one runs at a time.)
const FONT_TPAGE: Tpage = Tpage::new(320, 0, TexDepth::Bit4); // reserves x 320..576
const COVER1_TPAGE: Tpage = Tpage::new(768, 0, TexDepth::Bit4); // celeste, x 768..800
const COVER2_TPAGE: Tpage = Tpage::new(832, 0, TexDepth::Bit4); // celeste2, x 832..864
const FONT_CLUT: Clut = Clut::new(320, 256); // psx-font's 2-entry CLUT
const COVER_CLUT: Clut = Clut::new(768, 256); // 16 entries, opaque-black variant

// ---- Menu geometry (320x240 screen) ----------------------------------
const COVER_SRC: u8 = 128; // source label is 128x128
const COVER_W: i16 = 96; // on-screen size
const COVER_H: i16 = 96;
const COVER_Y: i16 = 60;
const COVER1_X: i16 = 44; // centre 92
const COVER2_X: i16 = 180; // centre 228
const CENTER1: i16 = COVER1_X + COVER_W / 2;
const CENTER2: i16 = COVER2_X + COVER_W / 2;
const SCREEN_CX: i16 = 160;

#[no_mangle]
fn main() {
    loop {
        match show_menu() {
            0 => celeste::run(),
            _ => celeste2::run(),
        }
        // The game clobbered VRAM and left the GPU in its own mode; the
        // next show_menu() re-inits and re-uploads everything.
    }
}

/// Show the cover menu and block until the player launches a game.
/// Returns the chosen game index (0 = Celeste, 1 = Celeste 2).
fn show_menu() -> usize {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    let font = upload_menu_vram();

    let mut sel: usize = 0;
    let mut prev = ButtonState::from_bits(0);

    loop {
        let b = poll_port1().buttons;
        let pressed = |m: u16| b.is_held(m) && !prev.is_held(m);

        if pressed(button::LEFT) {
            sel = 0;
        }
        if pressed(button::RIGHT) {
            sel = 1;
        }
        if pressed(button::CROSS) || pressed(button::START) {
            return sel;
        }
        prev = b;

        // PICO-8 dark-navy backdrop.
        fb.clear(13, 20, 40);

        // Selection frame: a filled rect just larger than the chosen
        // cover. The covers are drawn with an opaque-black CLUT (see
        // upload_menu_vram), so only the 5px border shows around them.
        let (fx, _) = if sel == 0 { (COVER1_X, CENTER1) } else { (COVER2_X, CENTER2) };
        gpu::draw_quad_flat(
            [
                (fx - 5, COVER_Y - 5),
                (fx + COVER_W + 5, COVER_Y - 5),
                (fx - 5, COVER_Y + COVER_H + 5),
                (fx + COVER_W + 5, COVER_Y + COVER_H + 5),
            ],
            255,
            236,
            39, // PICO-8 yellow
        );

        // Covers: the selected one full-bright, the other dimmed.
        let (t0, t1) = if sel == 0 {
            ((0x80, 0x80, 0x80), (0x48, 0x48, 0x48))
        } else {
            ((0x48, 0x48, 0x48), (0x80, 0x80, 0x80))
        };
        draw_cover(COVER1_TPAGE, COVER1_X, COVER_Y, t0);
        draw_cover(COVER2_TPAGE, COVER2_X, COVER_Y, t1);

        // Title, per-cover labels, and the controls hint.
        let title = "PICO-8  PSX";
        font.draw_text(SCREEN_CX - text_half(&font, title), 22, title, (0x80, 0x80, 0x80));

        let (l0, l1) = if sel == 0 {
            ((0x80, 0x80, 0x80), (0x40, 0x40, 0x40))
        } else {
            ((0x40, 0x40, 0x40), (0x80, 0x80, 0x80))
        };
        font.draw_text(CENTER1 - text_half(&font, "CELESTE"), 166, "CELESTE", l0);
        font.draw_text(CENTER2 - text_half(&font, "CELESTE 2"), 166, "CELESTE 2", l1);

        let hint = "D-PAD  SELECT     X  PLAY";
        font.draw_text(SCREEN_CX - text_half(&font, hint), 212, hint, (0x60, 0x60, 0x60));

        gpu::draw_sync();
        gpu::vsync();
        fb.swap();
    }
}

/// Upload the two cover textures, their opaque-black CLUT, and the font
/// atlas. Called every time the menu is (re-)entered, since a launched
/// game overwrites VRAM. Returns the freshly-uploaded font atlas.
fn upload_menu_vram() -> FontAtlas {
    // 128x128 @ 4bpp == 32 halfwords/row.
    upload_16bpp(VramRect::new(COVER1_TPAGE.x(), COVER1_TPAGE.y(), 32, 128), &COVER_CELESTE);
    upload_16bpp(VramRect::new(COVER2_TPAGE.x(), COVER2_TPAGE.y(), 32, 128), &COVER_CELESTE2);

    // The cart labels use PICO-8 colour 0 (black) for their borders. In
    // VRAM a 4bpp texel whose CLUT entry is 0x0000 is transparent, which
    // would punch holes in the labels; swap entry 0 for a near-black
    // opaque value so the covers render solid like the real cart.
    let mut clut = PICO8_CLUT;
    clut[0] = 0x0421; // RGB555 (1,1,1) -- opaque, reads as black
    upload_16bpp(VramRect::new(COVER_CLUT.x(), COVER_CLUT.y(), 16, 1), &clut);

    FontAtlas::upload(&BASIC, FONT_TPAGE, FONT_CLUT)
}

/// Blit a 128x128 cover, scaled to COVER_W x COVER_H, at screen (x, y).
fn draw_cover(tpage: Tpage, x: i16, y: i16, tint: (u8, u8, u8)) {
    let verts = [(x, y), (x + COVER_W, y), (x, y + COVER_H), (x + COVER_W, y + COVER_H)];
    let uvs = [(0, 0), (COVER_SRC, 0), (0, COVER_SRC), (COVER_SRC, COVER_SRC)];
    gpu::draw_quad_textured(verts, uvs, COVER_CLUT.uv_clut_word(), tpage.uv_tpage_word(0), tint);
}

/// Half the pixel width of `s` in the BASIC font, for horizontal centring.
#[inline]
fn text_half(font: &FontAtlas, s: &str) -> i16 {
    (font.text_width(s) / 2) as i16
}
