//! In-game pause menu (not in the original PICO-8 carts).
//!
//! Pressing Start opens an overlay with two volume sliders (SFX / music) and a
//! "quit to menu" item. The menu is platform-agnostic: it owns only its state,
//! draws through [`crate::backend`] (rectfill/print), and is driven one frame at
//! a time by [`Pause::update`] with a 6-bit control mask. The game's own frame
//! loop keeps drawing the frozen game behind it and advancing the SPU, so the
//! music plays on and the volume sliders are heard live.
//!
//! Control mask bits (edge-detected internally): 0 up, 1 down, 2 left, 3 right,
//! 4 confirm (X), 5 start.

use crate::backend;
use crate::sfx;

pub const UP: u8 = 1 << 0;
pub const DOWN: u8 = 1 << 1;
pub const LEFT: u8 = 1 << 2;
pub const RIGHT: u8 = 1 << 3;
pub const CONFIRM: u8 = 1 << 4;
pub const START: u8 = 1 << 5;

const ROW_SFX: u8 = 0;
const ROW_MUSIC: u8 = 1;
const ROW_FLY: u8 = 2; // debug fly toggle; present only when `fly` is set

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
    blip: i32,  // SFX id played when nudging a volume slider, so it's audible
    fly: bool,  // show the debug "FLY" row (toggles pico8::debug fly mode)
}

impl Pause {
    /// `blip` is a short SFX id (from the game's own bank) played on each volume
    /// nudge so the SFX slider gives audible feedback. `fly` adds the debug FLY
    /// row (only games that implement a fly path should pass true).
    pub fn new(blip: i32, fly: bool) -> Self {
        Pause { sel: ROW_SFX, prev: 0xFF, blip, fly }
    }

    fn row_count(&self) -> u8 {
        if self.fly { 4 } else { 3 }
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
        if self.fly && self.sel == ROW_FLY && dir != 0 {
            crate::debug::set_fly(dir > 0); // right = on, left = off
        } else if dir != 0 {
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

        if pressed & CONFIRM != 0 {
            if self.fly && self.sel == ROW_FLY {
                crate::debug::toggle_fly();
            } else if self.sel == self.quit_row() {
                return Some(Exit::QuitToMenu);
            }
        }
        None
    }

    /// Draw the overlay (call after the game's own draw, before the buffer swap).
    /// Coordinates are PICO-8 128-space; the caller should have camera at (0,0).
    pub fn draw(&self) {
        backend::camera(0, 0);
        let fly = self.fly;

        // row + panel geometry (grows by one row + a hint line when fly is shown)
        let sfx_y = 42i16;
        let music_y = 54i16;
        let fly_y = 66i16;
        let quit_y = if fly { 82 } else { 70 };
        let footer_y = quit_y + 13;
        let hint_y = footer_y + 9;
        let bottom = if fly { hint_y + 8 } else { footer_y + 8 };

        // framed panel: white border, black fill, a lavender title bar
        backend::rectfill(12, 22, 115, bottom, 7);
        backend::rectfill(13, 23, 114, bottom - 1, 0);
        backend::rectfill(13, 23, 114, 33, 13);
        print_centered(b"PAUSED", 25, 7);

        // selection highlight bar + cursor
        let sel_y = match self.sel {
            ROW_SFX => sfx_y,
            ROW_MUSIC => music_y,
            r if fly && r == ROW_FLY => fly_y,
            _ => quit_y,
        };
        backend::rectfill(15, sel_y - 2, 112, sel_y + 6, 1);
        backend::print(b">", 17, sel_y, 7);

        self.draw_slider(ROW_SFX, b"SFX", sfx::sfx_volume(), sfx_y);
        self.draw_slider(ROW_MUSIC, b"MUSIC", sfx::music_volume(), music_y);

        if fly {
            let lit = self.sel == ROW_FLY;
            draw_tri(26, fly_y, 11); // green PS1-triangle icon
            backend::print(b"FLY", 34, fly_y, if lit { 7 } else { 6 });
            let on = crate::debug::fly_enabled();
            backend::print(if on { b"ON" } else { b"OFF" }, 92, fly_y, if on { 11 } else { 5 });
        }

        let quit_c = if self.sel == self.quit_row() { 7 } else { 6 };
        backend::print(b"QUIT TO MENU", 26, quit_y, quit_c);

        print_centered(b"START = RESUME", footer_y, 6);
        if fly {
            // "HOLD <tri> TO FLY" -- spell out the activation button
            backend::print(b"HOLD", 38, hint_y, 5);
            draw_tri(58, hint_y, 11);
            backend::print(b"TO FLY", 66, hint_y, 5);
        }
    }

    fn draw_slider(&self, row: u8, label: &[u8], vol: u16, y: i16) {
        let lit = self.sel == row;
        backend::print(label, 26, y, if lit { 7 } else { 6 });
        // a 40px track with a filled portion and a handle at the fill end
        let tx: i16 = 54;
        let tw: i16 = 40;
        backend::rectfill(tx, y + 1, tx + tw, y + 3, 5); // track
        let fw = (vol as i16) * tw / 8;
        if fw > 0 {
            backend::rectfill(tx, y + 1, tx + fw, y + 3, if lit { 11 } else { 3 });
        }
        let kx = tx + fw;
        backend::rectfill(kx - 1, y - 1, kx + 1, y + 5, if lit { 7 } else { 6 }); // handle
    }
}

/// Centre an ASCII string horizontally on the 128px screen (4px/char) and print.
fn print_centered(s: &[u8], y: i16, c: i32) {
    let w = (s.len() as i16) * 4;
    backend::print(s, 64 - w / 2, y, c);
}

/// A small upward-pointing filled triangle (~5x4 px) -- the PS1 Triangle button
/// icon. Drawn as a quad with a doubled last vertex so it collapses to a tri.
fn draw_tri(x: i16, y: i16, c: i32) {
    backend::quad([(x + 2, y), (x, y + 4), (x + 4, y + 4), (x + 4, y + 4)], c);
}
