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
    changed: bool,    // a PERSISTED setting was touched; save to card on close
    font: FontAtlas,  // PSX (non-PICO-8) typeface for the menu text
}

impl Pause {
    /// `blip` is a short SFX id (from the game's own bank) played on each volume
    /// nudge so the SFX slider gives audible feedback. `fly` adds the debug FLY
    /// row (only games that implement a fly path should pass true). Uploads the
    /// PSX font to VRAM (call once per pause-open, which is what the games do).
    pub fn new(blip: i32, fly: bool) -> Self {
        let font = FontAtlas::upload(&BASIC, FONT_TPAGE, FONT_CLUT);
        crate::icons::upload();
        Pause { sel: ROW_SFX, prev: 0xFF, blip, fly, changed: false, font }
    }

    fn row_count(&self) -> u8 {
        if self.fly { 7 } else { 6 }
    }
    fn pixel_row(&self) -> u8 {
        if self.fly { 3 } else { 2 }
    }
    fn screen_row(&self) -> u8 {
        self.pixel_row() + 1
    }
    fn borders_row(&self) -> u8 {
        self.pixel_row() + 2
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
            // Closing after a settings change is the save point (never per
            // slider tick; card writes are slow). No-op without a card.
            if self.changed {
                crate::save::save();
            }
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
            } else if self.sel == self.pixel_row() {
                backend::set_pixel_scale(if dir > 0 { 2 } else { 1 });
                self.changed = true;
                crate::menusfx::play(crate::menusfx::SFX_NAV);
            } else if self.sel == self.screen_row() && backend::pixel_scale() != 1 {
                backend::set_screen_follow(dir > 0); // right = follow, left = centre
                crate::menusfx::play(crate::menusfx::SFX_NAV);
            } else if self.sel == self.borders_row() {
                let n = backend::side_preset_count() as i32;
                let cur = backend::side_preset() as i32;
                backend::set_side_preset((cur + dir).rem_euclid(n) as u8);
                self.changed = true;
                crate::menusfx::play(crate::menusfx::SFX_NAV);
            } else {
                match self.sel {
                    ROW_SFX => {
                        let v = (sfx::sfx_volume() as i32 + dir).clamp(0, 8) as u16;
                        sfx::set_sfx_volume(v);
                        self.changed = true;
                        sfx::play(self.blip);
                    }
                    ROW_MUSIC => {
                        let v = (sfx::music_volume() as i32 + dir).clamp(0, 8) as u16;
                        sfx::set_music_volume(v);
                        self.changed = true;
                    }
                    _ => {}
                }
            }
        }

        if pressed & CONFIRM != 0 {
            if self.fly && self.sel == ROW_FLY {
                crate::debug::toggle_fly();
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.pixel_row() {
                backend::set_pixel_scale(if backend::pixel_scale() == 2 { 1 } else { 2 });
                self.changed = true;
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.screen_row() && backend::pixel_scale() != 1 {
                backend::set_screen_follow(!backend::screen_follow());
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.borders_row() {
                let n = backend::side_preset_count();
                backend::set_side_preset((backend::side_preset() + 1) % n);
                self.changed = true;
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
            } else if self.sel == self.quit_row() {
                crate::menusfx::play(crate::menusfx::SFX_CONFIRM);
                // Quit-to-menu is also a save point for pending changes.
                if self.changed {
                    crate::save::save();
                }
                return Some(Exit::QuitToMenu);
            }
        }
        None
    }

    /// Draw the overlay (call after the game's own draw, before the buffer swap).
    /// Coordinates are PICO-8 128-space; the caller should have camera at (0,0).
    pub fn draw(&self) {
        // The overlay positions text at a fixed 2x and uses the fixed-size PSX font,
        // so render it at 2x regardless of the game's pixel scale; restore at the end
        // (the frozen game behind is redrawn at its own scale next frame).
        let game_scale = backend::pixel_scale();
        backend::set_pixel_scale(2); // also centres V_OFS at the 2x centre (-8)
        backend::camera(0, 0);
        backend::pal_reset(); // the frozen game may have left a pal() remap; the
                              // overlay must draw with the canonical PICO-8 palette
        let fly = self.fly;
        let scale1x = game_scale == 1; // 1x never clips -> the Screen row is greyed

        // Row + panel geometry, sized to the rows and centred vertically (py 64).
        let n_rows = self.row_count() as i16;
        const ROW_H: i16 = 11; // row pitch
        const HEAD: i16 = 19; // panel top -> first row (title bar + gap)
        const FOOT_GAP: i16 = 12; // last row -> "Resume" footer
        let h = HEAD + (n_rows - 1) * ROW_H + FOOT_GAP + 10;
        let top = 64 - h / 2;
        let bottom = top + h;
        let row_y = |p: u8| top + HEAD + p as i16 * ROW_H;
        let sfx_y = row_y(ROW_SFX);
        let music_y = row_y(ROW_MUSIC);
        let fly_y = row_y(ROW_FLY);
        let pixel_y = row_y(self.pixel_row());
        let screen_y = row_y(self.screen_row());
        let borders_y = row_y(self.borders_row());
        let quit_y = row_y(self.quit_row());
        let footer_y = quit_y + FOOT_GAP;

        // framed panel: white border, black fill, a lavender title bar
        backend::rectfill(12, top, 115, bottom, 7);
        backend::rectfill(13, top + 1, 114, bottom - 1, 0);
        backend::rectfill(13, top + 1, 114, top + 11, 13);
        // "Paused" with a white -> lavender gradient (matches the title bar)
        self.text_center_gradient(top + 5, "Paused", (0x80, 0x80, 0x80), (0x52, 0x46, 0x74));

        // selection highlight bar + cursor
        let sel_y = match self.sel {
            ROW_SFX => sfx_y,
            ROW_MUSIC => music_y,
            r if fly && r == ROW_FLY => fly_y,
            r if r == self.pixel_row() => pixel_y,
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
            // the Triangle button it maps to, beside the label (matches the launcher
            // Settings Fly row, so no separate "Hold Triangle to Fly" hint is needed)
            let fw = self.font.text_width("Fly") as i16;
            crate::icons::draw(&crate::icons::TRIANGLE, sx(28) + fw + 4, sy(fly_y) - 3);
            let on = crate::debug::fly_enabled();
            self.text(92, fly_y, if on { "On" } else { "Off" }, if on { T_GREEN } else { T_DIM });
        }

        // pixel scale toggle (1x native vs 2x doubled)
        {
            let lit = self.sel == self.pixel_row();
            self.text(28, pixel_y, "Pixel", if lit { T_WHITE } else { T_GREY });
            let label = if game_scale == 1 { "1x" } else { "2x" };
            self.text(74, pixel_y, label, if lit { T_WHITE } else { T_GREY });
        }

        // screen mode toggle (vertical pan vs classic centre) -- greyed at 1x (no clip)
        {
            let lit = self.sel == self.screen_row();
            let lbl_t = if scale1x {
                T_DIM
            } else if lit {
                T_WHITE
            } else {
                T_GREY
            };
            self.text(28, screen_y, "Screen", lbl_t);
            let val = if scale1x {
                "--"
            } else if backend::screen_follow() {
                "Follow"
            } else {
                "Center"
            };
            self.text(74, screen_y, val, lbl_t);
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

        // footer: the Start button icon + "Resume" (centred as a group)
        let resume = "Resume";
        let rw = self.font.text_width(resume) as i16; // screen px
        let iw = crate::icons::width(&crate::icons::START);
        let gap = 5;
        let x0 = 160 - (iw + gap + rw) / 2; // screen x
        crate::icons::draw(&crate::icons::START, x0, sy(footer_y) - 3);
        let tx = x0 + iw + gap;
        self.outline(tx, sy(footer_y), resume);
        self.font.draw_text(tx, sy(footer_y), resume, T_GREY);

        backend::set_pixel_scale(game_scale); // restore the game's scale
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

    /// Centre with a top->bottom colour gradient (PSoXide gouraud-textured text).
    fn text_center_gradient(&self, py: i16, s: &str, top: (u8, u8, u8), bottom: (u8, u8, u8)) {
        let x = 160 - self.font.text_width(s) as i16 / 2;
        self.outline(x, sy(py), s);
        self.font.draw_text_gradient(x, sy(py), s, top, bottom);
    }
}

