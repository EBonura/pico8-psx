//! pico8-psx demo disc -- the headline artifact.
//!
//! Boot flow: a fading Bonnie Studios intro (logo + "Built with PSoXIDE") ->
//! a cover menu showing the real PICO-8 cart labels for each demade game. Pick
//! one with the D-pad and press X to launch it, or press Select for a scrolling
//! credits screen. Each game is linked in as a library and exposes `run()`,
//! which boots the game and returns when the player holds Select+Start --
//! dropping back here to the menu.
//!
//! The menu uses psx-font's BASIC atlas; the intro/credits use the shared
//! PICO-8 font (`pico8::backend`).
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

use assets::cover_bonnie::COVER_BONNIE;
use assets::cover_celeste::COVER_CELESTE;
use assets::cover_celeste2::COVER_CELESTE2;
use assets::palette::PICO8_CLUT;

use pico8::{backend, sfx};
use psx_font::{fonts::BASIC, FontAtlas};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_pad::{button, poll_port1, ButtonState};
use psx_vram::{upload_16bpp, Clut, TexDepth, Tpage, VramRect};

// Menu sound effects, borrowed from Celeste's bank (see celeste::AUDIO): sfx 2 is
// a crisp 0.1s blip for moving the cursor, sfx 3 the 0.4s dash "whoosh" for launch.
const MENU_MOVE: i32 = 2;
const MENU_SELECT: i32 = 3;

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
const BONNIE_TPAGE: Tpage = Tpage::new(896, 0, TexDepth::Bit4); // intro logo, x 896..928
const FONT_CLUT: Clut = Clut::new(320, 256); // psx-font's 2-entry CLUT
const COVER_CLUT: Clut = Clut::new(768, 256); // 16 entries, opaque-black variant

// The intro/credits screens render text in the shared PICO-8 font (pico8::backend,
// FONT tpage at x=704, clear of the covers) -- coordinates are PICO-8 128-space.

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

/// Wait for the next real VBlank IRQ (the SDK's `gpu::vsync()` busy-waits a fixed
/// 242 hblanks ~= 15.4ms instead of syncing to the display).
#[inline]
fn wait_vblank() {
    let v = psx_rt::interrupts::vblank_count();
    while psx_rt::interrupts::vblank_count() == v {}
}

#[no_mangle]
fn main() {
    psx_rt::interrupts::install_vblank_counter();
    show_intro(); // Bonnie Studios logo fade -> menu (once, on boot)
    loop {
        match show_menu() {
            0 => celeste::run(),
            1 => celeste2::run(),
            _ => show_credits(),
        }
        // The game/credits clobbered VRAM and left the GPU in its own mode; the
        // next show_menu() re-inits and re-uploads everything.
    }
}

/// Boot intro: fade the Bonnie Studios logo (with "BUILT WITH PSoXIDE") in, hold,
/// fade out, then return to the menu. Any face button skips it.
fn show_intro() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    upload_16bpp(VramRect::new(BONNIE_TPAGE.x(), BONNIE_TPAGE.y(), 32, 128), &COVER_BONNIE);
    upload_cover_clut();
    backend::upload_font();

    const FADE_IN: i32 = 32;
    const HOLD: i32 = 74;
    const TOTAL: i32 = 150;
    const FADE_OUT: i32 = TOTAL - FADE_IN - HOLD;

    let any = |b: ButtonState| {
        b.is_held(button::CROSS) || b.is_held(button::CIRCLE) || b.is_held(button::START)
    };
    let mut prev = poll_port1().buttons;
    let mut frame = 0i32;
    while frame < TOTAL {
        let b = poll_port1().buttons;
        if frame > 8 && any(b) && !any(prev) {
            break; // fresh press skips
        }
        prev = b;

        // logo brightness 0..0x80 over fade-in / hold / fade-out
        let lvl = if frame < FADE_IN {
            frame * 0x80 / FADE_IN
        } else if frame < FADE_IN + HOLD {
            0x80
        } else {
            (TOTAL - frame) * 0x80 / FADE_OUT
        }
        .clamp(0, 0x80) as u8;

        fb.clear(0, 0, 0);
        draw_cover(BONNIE_TPAGE, 112, 38, (lvl, lvl, lvl));
        // tagline below, faded through PICO-8 greys (0->1->13->6) as the logo rises.
        backend::camera(0, 0);
        let c = if lvl < 0x18 {
            0
        } else if lvl < 0x40 {
            1
        } else if lvl < 0x68 {
            13
        } else {
            6
        };
        pprint_center(b"built with psoxide", 80, c);

        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        frame += 1;
    }
}

/// Scrolling credits screen (reached with Select from the menu). Renders in the
/// PICO-8 font; any face button returns to the menu.
fn show_credits() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    backend::upload_font();
    sfx::init(celeste::AUDIO);

    // (text, PICO-8 colour). Empty lines are spacers. Rendered all-caps by the font.
    const LINES: &[(&[u8], i32)] = &[
        (b"celeste classic collection", 10),
        (b"", 0),
        (b"ps1 port by", 6),
        (b"bonnie studios", 7),
        (b"bonnie-studios.itch.io", 12),
        (b"", 0),
        (b"built with psoxide", 6),
        (b"github.com/ebonura/psoxide", 12),
        (b"", 0),
        (b"-- original games --", 5),
        (b"", 0),
        (b"celeste classic   2016", 7),
        (b"maddy thorson  noel berry", 6),
        (b"", 0),
        (b"celeste 2 lani's trek  2021", 7),
        (b"maddy thorson  noel berry", 6),
        (b"music   lena raine", 6),
        (b"", 0),
        (b"made with pico-8", 7),
        (b"by lexaloffle games", 6),
        (b"lexaloffle.com/pico-8.php", 12),
        (b"", 0),
        (b"unofficial fan port", 5),
        (b"free forever", 5),
        (b"all rights to the original", 5),
        (b"creators", 5),
    ];
    const LINE_H: i16 = 7;
    let content_h = LINES.len() as i16 * LINE_H;

    let any = |b: ButtonState| {
        b.is_held(button::CROSS) || b.is_held(button::CIRCLE) || b.is_held(button::START)
    };
    let mut prev = poll_port1().buttons; // void a button still held from the menu
    let mut scroll = 0i16;
    let mut tick = 0u32;
    loop {
        let b = poll_port1().buttons;
        if any(b) && !any(prev) {
            return; // fresh press exits
        }
        prev = b;

        fb.clear(0, 0, 16); // dark navy
        backend::camera(0, 0);
        let top = 132 - scroll; // scroll up from below the screen
        let mut i: i16 = 0;
        for (txt, col) in LINES {
            if !txt.is_empty() {
                let y = top + i * LINE_H;
                if y >= -7 && y <= 130 {
                    pprint_center(txt, y, *col);
                }
            }
            i += 1;
        }
        // fixed footer over the scroll
        backend::rectfill(0, 120, 127, 128, 0);
        pprint_center(b"x  back", 121, 5);

        tick += 1;
        if tick & 1 == 0 {
            scroll += 1; // ~0.5px/frame
        }
        if scroll > content_h + 134 {
            scroll = 0; // loop
        }

        sfx::update();
        gpu::draw_sync();
        wait_vblank();
        fb.swap();
    }
}

/// Centre an all-caps string on the 128px PICO-8 screen (4px/char) and print it.
#[inline]
fn pprint_center(s: &[u8], y: i16, c: i32) {
    backend::print(s, 64 - (s.len() as i16) * 2, y, c);
}

/// Upload the opaque-black PICO-8 CLUT used by the covers + logo.
fn upload_cover_clut() {
    let mut clut = PICO8_CLUT;
    clut[0] = 0x0421; // RGB555 (1,1,1) -- opaque, reads as black
    upload_16bpp(VramRect::new(COVER_CLUT.x(), COVER_CLUT.y(), 16, 1), &clut);
}

/// Show the cover menu and block until the player picks something.
/// Returns 0 = Celeste, 1 = Celeste 2, 2 = credits (Select).
fn show_menu() -> usize {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    let font = upload_menu_vram();
    sfx::init(celeste::AUDIO); // menu blips run off Celeste's sound bank

    let mut sel: usize = 0;
    // Seed `prev` with whatever is held right now so a button still down from the
    // screen we came from doesn't read as a fresh press. Without this, pressing X
    // for "quit to menu" in a game carries the held X into the menu and instantly
    // re-launches the first game. A held button is voided until released + pressed.
    let mut prev = poll_port1().buttons;

    loop {
        let b = poll_port1().buttons;
        let pressed = |m: u16| b.is_held(m) && !prev.is_held(m);

        let old_sel = sel;
        if pressed(button::LEFT) {
            sel = 0;
        }
        if pressed(button::RIGHT) {
            sel = 1;
        }
        if sel != old_sel {
            sfx::play(MENU_MOVE); // cursor moved
        }
        if pressed(button::SELECT) {
            sfx::play(MENU_MOVE);
            return 2; // open the credits screen
        }
        if pressed(button::CROSS) || pressed(button::START) {
            // Play the launch blip and hold a few frames so it's heard before the
            // game boots and clobbers the SPU.
            sfx::play(MENU_SELECT);
            for _ in 0..18 {
                sfx::update();
                wait_vblank();
            }
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
        font.draw_text(SCREEN_CX - text_half(&font, hint), 210, hint, (0x60, 0x60, 0x60));

        let hint2 = "SELECT  CREDITS";
        font.draw_text(SCREEN_CX - text_half(&font, hint2), 222, hint2, (0x48, 0x48, 0x58));

        sfx::update(); // advance the SPU sequencer so menu blips play out
        gpu::draw_sync();
        wait_vblank();
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

    // The cart labels use PICO-8 colour 0 (black) for their borders; the opaque-black
    // CLUT keeps them solid instead of punching transparent holes.
    upload_cover_clut();

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
