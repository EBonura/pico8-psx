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
mod atmos;

use assets::cover_bonnie::COVER_BONNIE;
use assets::cover_celeste::COVER_CELESTE;
use assets::cover_celeste2::COVER_CELESTE2;
use assets::palette::PICO8_CLUT;

use pico8::{menusfx, sfx};
use psx_font::{fonts::BASIC, FontAtlas};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_pad::{button, poll_port1, ButtonState};
use psx_vram::{upload_16bpp, Clut, TexDepth, Tpage, VramRect};

// Menu SFX are dedicated CC0 one-shot samples (see pico8::menusfx), NOT in-game
// sounds: SFX_NAV (cursor), SFX_CONFIRM (launch), SFX_TRANSITION (intro -> menu).

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
    atmos::init(); // seed the menu's cloud/particle backdrops
    show_intro(); // Bonnie Studios logo fade -> menu (once, on boot)
    let mut first = true; // play the intro->menu transition only on the first menu
    loop {
        match show_menu(first) {
            0 => celeste::run(),
            1 => celeste2::run(),
            _ => show_credits(),
        }
        first = false;
        // The game/credits clobbered VRAM and left the GPU in its own mode; the
        // next show_menu() re-inits and re-uploads everything.
    }
}

/// Boot intro: fade the Bonnie Studios logo (with "Built with PSoXide") in, hold,
/// fade out, then return to the menu. Any face button skips it.
fn show_intro() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    upload_16bpp(VramRect::new(BONNIE_TPAGE.x(), BONNIE_TPAGE.y(), 32, 128), &COVER_BONNIE);
    upload_cover_clut();
    let font = FontAtlas::upload(&BASIC, FONT_TPAGE, FONT_CLUT);

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

        // brightness 0..0x80 over fade-in / hold / fade-out (0x80 == full / neutral
        // texture modulation, so the logo art reads pure white at the peak).
        let lvl = if frame < FADE_IN {
            frame * 0x80 / FADE_IN
        } else if frame < FADE_IN + HOLD {
            0x80
        } else {
            (TOTAL - frame) * 0x80 / FADE_OUT
        }
        .clamp(0, 0x80) as u8;

        fb.clear(0, 0, 0);
        draw_cover(BONNIE_TPAGE, 112, 34, (lvl, lvl, lvl));
        // "Built with PSoXide" -- cyan->blue gradient with a sweeping sheen, faded by `lvl`.
        let tag = "Built with PSoXide";
        draw_sheen(
            &font,
            SCREEN_CX - text_half(&font, tag),
            150,
            tag,
            (0x68, 0x80, 0x80),
            (0x38, 0x58, 0x80),
            frame,
            lvl as i32,
        );

        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        frame += 1;
    }
}

/// Scrolling credits screen (reached with Select from the menu). Default font,
/// proper capitalisation; any face button returns to the menu.
fn show_credits() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    let font = FontAtlas::upload(&BASIC, FONT_TPAGE, FONT_CLUT);
    sfx::init(celeste::AUDIO);

    // (text, RGB tint). 0x80 == full brightness. Empty lines are spacers.
    const TITLE: (u8, u8, u8) = (0x80, 0x74, 0x30); // warm gold
    const SECT: (u8, u8, u8) = (0x50, 0x50, 0x68); // section header
    const NAME: (u8, u8, u8) = (0x78, 0x78, 0x78); // white-ish
    const HEAD: (u8, u8, u8) = (0x80, 0x80, 0x80); // bright game name
    const LBL: (u8, u8, u8) = (0x58, 0x58, 0x60); // label
    const URL: (u8, u8, u8) = (0x38, 0x58, 0x80); // blue link
    const PSOX: (u8, u8, u8) = (0x60, 0x80, 0x80); // "Built with PSoXide" (gradient marker)
    const DIM: (u8, u8, u8) = (0x48, 0x48, 0x54); // disclaimer
    const LINES: &[(&str, (u8, u8, u8))] = &[
        ("Celeste Classic Collection", TITLE),
        ("", LBL),
        ("PS1 Port by", LBL),
        ("Bonnie Studios", NAME),
        ("bonnie-studios.itch.io", URL),
        ("", LBL),
        ("Built with PSoXide", PSOX),
        ("github.com/EBonura/PSoXide", URL),
        ("", LBL),
        ("The Original Games", SECT),
        ("", LBL),
        ("Celeste Classic   2016", HEAD),
        ("Maddy Thorson   Noel Berry", NAME),
        ("", LBL),
        ("Celeste 2: Lani's Trek   2021", HEAD),
        ("Maddy Thorson   Noel Berry", NAME),
        ("Music   Lena Raine", NAME),
        ("", LBL),
        ("Made with PICO-8", LBL),
        ("by Lexaloffle Games", NAME),
        ("lexaloffle.com/pico-8.php", URL),
        ("", LBL),
        ("Unofficial fan port, free forever", DIM),
        ("All rights to the original creators", DIM),
    ];
    const LINE_H: i16 = 12;
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

        fb.clear(8, 10, 26); // dark navy
        let top = 236 - scroll; // scroll up from below the screen
        for (i, (txt, col)) in LINES.iter().enumerate() {
            if !txt.is_empty() {
                let y = top + i as i16 * LINE_H;
                if y >= -12 && y <= 240 {
                    let x = SCREEN_CX - text_half(&font, txt);
                    // the collection title and game-name headers get gradients
                    if *col == TITLE {
                        ol_gradient(&font, x, y, txt, (0x80, 0x78, 0x3c), (0x60, 0x2a, 0x0c));
                    } else if *col == HEAD {
                        ol_gradient(&font, x, y, txt, (0x80, 0x80, 0x80), (0x3c, 0x60, 0x80));
                    } else if *col == PSOX {
                        draw_sheen(&font, x, y, txt, (0x68, 0x80, 0x80), (0x38, 0x58, 0x80), tick as i32, 0x80);
                    } else {
                        ol_text(&font, x, y, txt, *col);
                    }
                }
            }
        }
        // fixed footer over the scroll
        gpu::draw_quad_flat([(0, 224), (320, 224), (0, 240), (320, 240)], 8, 10, 26);
        let back = "X  Back";
        ol_text(&font, SCREEN_CX - text_half(&font, back), 226, back, (0x50, 0x50, 0x58));

        tick += 1;
        if tick & 1 == 0 {
            scroll += 1; // ~0.5px/frame
        }
        if scroll > content_h + 240 {
            scroll = 0; // loop
        }

        sfx::update();
        gpu::draw_sync();
        wait_vblank();
        fb.swap();
    }
}

/// Upload the opaque-black PICO-8 CLUT used by the covers + logo.
fn upload_cover_clut() {
    let mut clut = PICO8_CLUT;
    clut[0] = 0x0421; // RGB555 (1,1,1) -- opaque, reads as black
    upload_16bpp(VramRect::new(COVER_CLUT.x(), COVER_CLUT.y(), 16, 1), &clut);
}

/// Show the cover menu and block until the player picks something.
/// Returns 0 = Celeste, 1 = Celeste 2, 2 = credits (Select).
fn show_menu(first: bool) -> usize {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    let font = upload_menu_vram();
    sfx::init(celeste::AUDIO); // inits the SPU + the SFX-volume state
    menusfx::init(); // upload the dedicated CC0 menu sample bank
    // intro -> menu uses the same confirm sound as launching a game; returning from
    // a game/credits gets the softer reveal.
    menusfx::play(if first { menusfx::SFX_CONFIRM } else { menusfx::SFX_TRANSITION });

    let mut sel: usize = 0;
    let mut frame = 0i32; // animation clock (starfield drift, glow pulse)

    // Dissolve in from black: covers the intro -> menu reveal and returning from a game.
    for k in 0..FADE_FRAMES {
        draw_menu_scene(&mut fb, &font, sel, frame);
        fade_quad((255 - 255 * k / FADE_FRAMES) as u8);
        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        frame = frame.wrapping_add(1);
    }

    // Seed `prev` with whatever is held now so a button still down from the screen we
    // came from doesn't read as a fresh press (e.g. the held X from "quit to menu").
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
            menusfx::play(menusfx::SFX_NAV); // cursor moved
        }
        if pressed(button::SELECT) {
            menusfx::play(menusfx::SFX_NAV);
            return 2; // open the credits screen
        }
        if pressed(button::CROSS) || pressed(button::START) {
            // Launch sound, then dissolve to black before the game boots (this also
            // gives the sound time to be heard before the SPU is clobbered).
            menusfx::play(menusfx::SFX_CONFIRM);
            fade_out(&mut fb, &font, sel, &mut frame);
            return sel;
        }
        prev = b;

        draw_menu_scene(&mut fb, &font, sel, frame);
        sfx::update(); // keep the SPU sequencer ticking
        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        frame = frame.wrapping_add(1);
    }
}

/// Number of frames for the menu dissolve in/out.
const FADE_FRAMES: i32 = 16;

/// Draw one full menu frame (backdrop, covers, tracer, title, labels, hint) into
/// the back buffer. The caller adds any fade overlay, then presents.
fn draw_menu_scene(fb: &mut FrameBuffer, font: &FontAtlas, sel: usize, frame: i32) {
    fb.clear(0, 0, 0);
    atmos::draw(sel, frame);

    let (t0, t1) = if sel == 0 {
        ((0x80, 0x80, 0x80), (0x48, 0x48, 0x48))
    } else {
        ((0x48, 0x48, 0x48), (0x80, 0x80, 0x80))
    };
    draw_cover(COVER1_TPAGE, COVER1_X, COVER_Y, t0);
    draw_cover(COVER2_TPAGE, COVER2_X, COVER_Y, t1);

    let sel_x = if sel == 0 { COVER1_X } else { COVER2_X };
    atmos::draw_tracer(sel_x, COVER_Y, COVER_W, frame);

    let title = "Celeste Classic Collection";
    draw_sheen(
        font,
        SCREEN_CX - text_half(font, title),
        20,
        title,
        (0x80, 0x78, 0x3c),
        (0x60, 0x2a, 0x0c),
        frame,
        0x80,
    );

    let icy_top = (0x80, 0x80, 0x80);
    let icy_bot = (0x3c, 0x60, 0x80);
    let dim = (0x44, 0x44, 0x4c);
    if sel == 0 {
        ol_gradient(font, CENTER1 - text_half(font, "Celeste"), 166, "Celeste", icy_top, icy_bot);
        ol_text(font, CENTER2 - text_half(font, "Celeste 2"), 166, "Celeste 2", dim);
    } else {
        ol_text(font, CENTER1 - text_half(font, "Celeste"), 166, "Celeste", dim);
        ol_gradient(font, CENTER2 - text_half(font, "Celeste 2"), 166, "Celeste 2", icy_top, icy_bot);
    }

    let hint2 = "Select: Credits";
    ol_text(font, SCREEN_CX - text_half(font, hint2), 212, hint2, (0x48, 0x48, 0x58));
}

/// Full-screen subtractive grey quad: `background - (g,g,g)`, a linear darken.
fn fade_quad(g: u8) {
    use psx_gpu::material::BlendMode;
    gpu::draw_tri_flat_blended([(0, 0), (320, 0), (0, 240)], g, g, g, BlendMode::Subtract);
    gpu::draw_tri_flat_blended([(320, 0), (0, 240), (320, 240)], g, g, g, BlendMode::Subtract);
}

/// Dissolve the menu to black over `FADE_FRAMES` (when leaving for a game).
fn fade_out(fb: &mut FrameBuffer, font: &FontAtlas, sel: usize, frame: &mut i32) {
    for k in 1..=FADE_FRAMES {
        draw_menu_scene(fb, font, sel, *frame);
        fade_quad((255 * k / FADE_FRAMES) as u8);
        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        *frame = frame.wrapping_add(1);
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

/// Black 1px outline (4 diagonals) behind text for readability over the backdrop.
fn outline(font: &FontAtlas, x: i16, y: i16, s: &str) {
    for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
        font.draw_text(x + dx, y + dy, s, (0, 0, 0));
    }
}

/// Outlined flat text.
fn ol_text(font: &FontAtlas, x: i16, y: i16, s: &str, tint: (u8, u8, u8)) {
    outline(font, x, y, s);
    font.draw_text(x, y, s, tint);
}

/// Outlined gradient text.
fn ol_gradient(font: &FontAtlas, x: i16, y: i16, s: &str, top: (u8, u8, u8), bot: (u8, u8, u8)) {
    outline(font, x, y, s);
    font.draw_text_gradient(x, y, s, top, bot);
}

/// Draw `text` with a top->bottom base gradient PLUS a white "sheen" highlight
/// that sweeps left->right over time: each glyph is lerped toward white by how
/// close it is to a moving head, so a band of brightness travels across the word.
fn draw_sheen(
    font: &FontAtlas,
    mut x: i16,
    y: i16,
    text: &str,
    top: (u8, u8, u8),
    bot: (u8, u8, u8),
    frame: i32,
    bright: i32, // white-point / brightness cap (0x80 = full; lower fades the whole thing)
) {
    outline(font, x, y, text); // black border behind the whole word, for readability
    let span = text.chars().count() as i32 + 18; // word length + a gap before repeat
    let head = (frame / 2).rem_euclid(span); // highlight position, advances 1 char / 2 frames
    let mix = |c: (u8, u8, u8), t: i32| -> (u8, u8, u8) {
        let f = |v: u8| {
            let base = v as i32 * bright / 0x80; // dim the base colour by the fade
            (base + (bright - base) * t / 18) as u8 // lerp toward `bright` near the head
        };
        (f(c.0), f(c.1), f(c.2))
    };
    for (i, ch) in text.char_indices() {
        let glyph = &text[i..i + ch.len_utf8()];
        let t = (18 - (i as i32 - head).abs() * 6).max(0); // 0..18, peaks at the head
        font.draw_text_gradient(x, y, glyph, mix(top, t), mix(bot, t));
        x += font.text_width(glyph) as i16; // monospace advance for this glyph
    }
}
