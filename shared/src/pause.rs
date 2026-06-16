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

        // A framed panel over the (frozen) game: black fill, 1px border, then a
        // dark inset so the text reads regardless of what's behind. The FLY row
        // adds one line, so grow the panel when it's shown.
        let extra: i16 = if self.fly { 12 } else { 0 };
        backend::rectfill(17, 29, 110, 97 + extra, 7); // border
        backend::rectfill(18, 30, 109, 96 + extra, 0); // black fill
        backend::rectfill(20, 32, 107, 94 + extra, 1); // dark-blue inset

        print_centered(b"PAUSED", 36, 7);

        self.draw_slider(ROW_SFX, b"SFX", sfx::sfx_volume(), 50);
        self.draw_slider(ROW_MUSIC, b"MUSIC", sfx::music_volume(), 62);

        let mut y: i16 = 78;
        if self.fly {
            let lit = self.sel == ROW_FLY;
            backend::print(b"FLY", 26, y, if lit { 7 } else { 6 });
            let on = crate::debug::fly_enabled();
            backend::print(if on { b"ON" } else { b"OFF" }, 62, y, if on { 10 } else { 5 });
            y += 12;
        }

        let quit_c = if self.sel == self.quit_row() { 7 } else { 6 };
        print_centered(b"QUIT TO MENU", y, quit_c);

        print_centered(b"START RESUMES", y + 10, 5);
    }

    fn draw_slider(&self, row: u8, label: &[u8], vol: u16, y: i16) {
        let lit = self.sel == row;
        let label_c = if lit { 7 } else { 6 };
        backend::print(label, 26, y, label_c);

        // 8-cell bar starting at x=62; filled cells bright, empty cells dark.
        let x0: i16 = 62;
        for i in 0..8u16 {
            let cx = x0 + (i as i16) * 5;
            let c = if i < vol {
                if lit { 10 } else { 6 }
            } else {
                5
            };
            backend::rectfill(cx, y, cx + 3, y + 4, c);
        }
    }
}

/// Centre an ASCII string horizontally on the 128px screen (4px/char) and print.
fn print_centered(s: &[u8], y: i16, c: i32) {
    let w = (s.len() as i16) * 4;
    backend::print(s, 64 - w / 2, y, c);
}
