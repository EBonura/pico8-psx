//! In-game pause menu (not in the original PICO-8 carts).
//!
//! Pressing Start opens an overlay with two volume sliders (SFX / music), a debug
//! fly toggle, and a "quit to menu" item. The panel/sliders draw through
//! [`crate::backend`] (PICO-8 128-space rects), but the TEXT is rendered in the
//! PSoXide BASIC font, deliberately NOT the PICO-8 typeface, so the menu reads as
//! an add-on rather than part of the original game. Driven one frame at a time by
//! [`Pause::update`] with a 6-bit control mask; the game's loop keeps drawing the
//! frozen game behind it and advancing the SPU, so the volume sliders are live.
//!
//! Control mask bits (edge-detected internally): 0 up, 1 down, 2 left, 3 right,
//! 4 confirm (X), 5 start.

use crate::backend;
use crate::sfx;
use psx_font::{fonts::BASIC, FontAtlas};
use psx_vram::{Clut, TexDepth, Tpage};

pub const UP: u8 = 1 << 0;
pub const DOWN: u8 = 1 << 1;
pub const LEFT: u8 = 1 << 2;
pub const RIGHT: u8 = 1 << 3;
pub const CONFIRM: u8 = 1 << 4;
pub const START: u8 = 1 << 5;

const ROW_SFX: u8 = 0;
const ROW_MUSIC: u8 = 1;
const ROW_FLY: u8 = 2; // debug fly toggle; present only when `fly` is set

// PSX font VRAM (x320, clear of the framebuffers and the games' own tpages;
// re-uploaded each pause-open since the game owns VRAM while it runs).
const FONT_TPAGE: Tpage = Tpage::new(320, 0, TexDepth::Bit4);
const FONT_CLUT: Clut = Clut::new(320, 256);

// psx-font tints (0x80 = full brightness; the glyphs are white, modulated by this).
const T_WHITE: (u8, u8, u8) = (0x80, 0x80, 0x80);
const T_GREY: (u8, u8, u8) = (0x6a, 0x6a, 0x76);
const T_DIM: (u8, u8, u8) = (0x44, 0x44, 0x52);
const T_GREEN: (u8, u8, u8) = (0x20, 0x78, 0x38);

// PICO-8 128-space -> screen, matching backend (camera at 0, SCALE 2, OFS 32/-8).
#[inline]
fn sx(px: i16) -> i16 {
    px * 2 + 32
}
#[inline]
fn sy(py: i16) -> i16 {
    py * 2 - 8
}

/// Outcome of a paused frame.
pub enum Exit {
    /// Close the menu and resume the game.
    Resume,
    /// Leave the game and return to the launcher menu.
    QuitToMenu,
}

pub struct Pause {
    sel: u8,
    prev: u8,
    blip: i32,        // SFX id played when nudging a volume slider, so it's audible
    fly: bool,        // show the debug "FLY" row (toggles pico8::debug fly mode)
    font: FontAtlas,  // PSX (non-PICO-8) typeface for the menu text
}

impl Pause {
    /// `blip` is a short SFX id (from the game's own bank) played on each volume
    /// nudge so the SFX slider gives audible feedback. `fly` adds the debug FLY
    /// row (only games that implement a fly path should pass true). Uploads the
    /// PSX font to VRAM (call once per pause-open, which is what the games do).
    pub fn new(blip: i32, fly: bool) -> Self {
        let font = FontAtlas::upload(&BASIC, FONT_TPAGE, FONT_CLUT);
        Pause { sel: ROW_SFX, prev: 0xFF, blip, fly, font }
    }

    fn row_count(&self) -> u8 {
        if self.fly { 6 } else { 5 }
    }
    fn screen_row(&self) -> u8 {
        if self.fly { 3 } else { 2 }
    }
    fn borders_row(&self) -> u8 {
        self.screen_row() + 1
    }
    fn quit_row(&self) -> u8 {
        self.row_count() - 1
    }

    /// Advance the menu one frame given the current control mask. Returns
    /// `Some(..)` when the menu should close.
    pub fn update(&mut self, mask: u8) -> Option<Exit> {
        let pressed = mask & !self.prev; // rising edges only
        self.prev = mask;

        let count = self.row_count();
        if pressed & START != 0 {
            return Some(Exit::Resume);
        }
        if pressed & (UP | DOWN) != 0 {
            crate::menusfx::play(crate::menusfx::SFX_NAV);
        }
        if pressed & UP != 0 {
            self.sel = (self.sel + count - 1) % count;
        }
        if pressed & DOWN != 0 {
            self.sel = (self.sel + 1) % count;
        }

        let dir: i32 = if pressed & RIGHT != 0 {
            1
        } else if pressed & LEFT != 0 {
            -1
        } else {
            0
        };
        if dir != 0 {
            if self.fly && self.sel == ROW_FLY {
                crate::debug::set_fly(dir > 0); // right = on, left = off
                crate::menusfx::play(crate::menusfx::SFX_NAV);
            } else if self.sel == self.screen_row() {
                backend::set_screen_follow(dir > 0); // right = follow, left = centre
                crate::menusfx::play(crate::menusfx::SFX_NAV);
            } else if self.sel == self.borders_row() {
                let n = backend::side_preset_count() as i32;
                let cur = backend::side_preset() as i32;
                backend::set_side_preset((cur + dir).rem_euclid(n) as u8);
                crate::menusfx::play(crate::menusfx::SFX_NAV);
            } else {
                match self.sel {
                    ROW_SFX => {
                        let v = (sfx::sfx_volume() as i32 + dir).clamp(0, 8) as u16;
                        sfx::set_sfx_volume(v);
                        sfx::play(self.blip);
                    }
                    ROW_MUSIC => {
                        let v = (sfx::music_volume() as i32 + dir).clamp(0, 8) as u16;
                        sfx::set_music_volume(v);
                    }
                    _ => {}
                }
            }
        }

        if pressed & CONFIRM != 0 {
            if self.fly && self.sel == ROW_FLY {
                crate::debug::toggle_fly();
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.screen_row() {
                backend::set_screen_follow(!backend::screen_follow());
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.borders_row() {
                let n = backend::side_preset_count();
                backend::set_side_preset((backend::side_preset() + 1) % n);
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.quit_row() {
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
                return Some(Exit::QuitToMenu);
            }
        }
        None
    }

    /// Draw the overlay (call after the game's own draw, before the buffer swap).
    /// Coordinates are PICO-8 128-space; the caller should have camera at (0,0).
    pub fn draw(&self) {
        backend::camera(0, 0);
        backend::center_screen(); // overlay is centred; ignore the game's follow-pan
        let fly = self.fly;

        // row + panel geometry (grows by one row + a hint line when fly is shown)
        let sfx_y = 42i16;
        let music_y = 54i16;
        let fly_y = 66i16;
        let screen_y = if fly { 78 } else { 66 };
        let borders_y = screen_y + 12;
        let quit_y = borders_y + 12;
        let footer_y = quit_y + 13;
        let hint_y = footer_y + 9;
        let bottom = if fly { hint_y + 8 } else { footer_y + 8 };

        // framed panel: white border, black fill, a lavender title bar
        backend::rectfill(12, 22, 115, bottom, 7);
        backend::rectfill(13, 23, 114, bottom - 1, 0);
        backend::rectfill(13, 23, 114, 33, 13);
        // "Paused" with a white -> lavender gradient (matches the title bar)
        self.text_center_gradient(27, "Paused", (0x80, 0x80, 0x80), (0x52, 0x46, 0x74));

        // selection highlight bar + cursor
        let sel_y = match self.sel {
            ROW_SFX => sfx_y,
            ROW_MUSIC => music_y,
            r if fly && r == ROW_FLY => fly_y,
            r if r == self.screen_row() => screen_y,
            r if r == self.borders_row() => borders_y,
            _ => quit_y,
        };
        backend::rectfill(15, sel_y - 2, 112, sel_y + 6, 1);
        self.text(18, sel_y, ">", T_WHITE);

        self.draw_slider(ROW_SFX, "SFX", sfx::sfx_volume(), sfx_y);
        self.draw_slider(ROW_MUSIC, "Music", sfx::music_volume(), music_y);

        if fly {
            let lit = self.sel == ROW_FLY;
            self.text(28, fly_y, "Fly", if lit { T_WHITE } else { T_GREY });
            let on = crate::debug::fly_enabled();
            self.text(92, fly_y, if on { "On" } else { "Off" }, if on { T_GREEN } else { T_DIM });
        }

        // screen mode toggle (vertical pan vs classic centre)
        {
            let lit = self.sel == self.screen_row();
            self.text(28, screen_y, "Screen", if lit { T_WHITE } else { T_GREY });
            let follow = backend::screen_follow();
            self.text(74, screen_y, if follow { "Follow" } else { "Center" }, if lit { T_WHITE } else { T_GREY });
        }

        // side-margin gradient preset
        {
            let lit = self.sel == self.borders_row();
            self.text(28, borders_y, "Borders", if lit { T_WHITE } else { T_GREY });
            let name = backend::side_preset_name(backend::side_preset());
            self.text(74, borders_y, name, if lit { T_WHITE } else { T_GREY });
        }

        let quit_t = if self.sel == self.quit_row() { T_WHITE } else { T_GREY };
        self.text(28, quit_y, "Quit to Menu", quit_t);

        self.text_center(footer_y, "Start = Resume", T_GREY);
        if fly && crate::debug::fly_enabled() {
            // shown only while fly is active: "Hold <tri> to Fly"
            self.text(40, hint_y, "Hold", T_DIM);
            draw_tri(59, hint_y, 11);
            self.text(67, hint_y, "to Fly", T_DIM);
        }
    }

    fn draw_slider(&self, row: u8, label: &str, vol: u16, y: i16) {
        let lit = self.sel == row;
        self.text(28, y, label, if lit { T_WHITE } else { T_GREY });
        // a 40px track with a filled portion and a handle at the fill end
        let tx: i16 = 56;
        let tw: i16 = 38;
        backend::rectfill(tx, y + 1, tx + tw, y + 3, 5); // track
        let fw = (vol as i16) * tw / 8;
        if fw > 0 {
            backend::rectfill(tx, y + 1, tx + fw, y + 3, if lit { 11 } else { 3 });
        }
        let kx = tx + fw;
        backend::rectfill(kx - 1, y - 1, kx + 1, y + 5, if lit { 7 } else { 6 }); // handle
    }

    /// Black 1px outline (4 diagonals) behind text at screen `(x, y)`, so labels
    /// stay readable over the frozen game / coloured panel bars.
    fn outline(&self, x: i16, y: i16, s: &str) {
        for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            self.font.draw_text(x + dx, y + dy, s, (0, 0, 0));
        }
    }

    /// Draw text in the PSX font at PICO-8 128-space `(px, py)` (converted to screen).
    fn text(&self, px: i16, py: i16, s: &str, tint: (u8, u8, u8)) {
        let (x, y) = (sx(px), sy(py));
        self.outline(x, y, s);
        self.font.draw_text(x, y, s, tint);
    }

    /// Horizontally centre PSX-font text on the screen at 128-space row `py`.
    fn text_center(&self, py: i16, s: &str, tint: (u8, u8, u8)) {
        let x = 160 - self.font.text_width(s) as i16 / 2;
        self.outline(x, sy(py), s);
        self.font.draw_text(x, sy(py), s, tint);
    }

    /// Centre with a top->bottom colour gradient (PSoXide gouraud-textured text).
    fn text_center_gradient(&self, py: i16, s: &str, top: (u8, u8, u8), bottom: (u8, u8, u8)) {
        let x = 160 - self.font.text_width(s) as i16 / 2;
        self.outline(x, sy(py), s);
        self.font.draw_text_gradient(x, sy(py), s, top, bottom);
    }
}

/// A small upward-pointing filled triangle (~5x4 px) -- the PS1 Triangle button
/// icon. Drawn as a quad with a doubled last vertex so it collapses to a tri.
fn draw_tri(x: i16, y: i16, c: i32) {
    backend::quad([(x + 2, y), (x, y + 4), (x + 4, y + 4), (x + 4, y + 4)], c);
}
