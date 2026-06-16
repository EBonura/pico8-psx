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
const ROW_QUIT: u8 = 2;
const ROW_COUNT: u8 = 3;

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
    blip: i32, // SFX id played when nudging a volume slider, so it's audible
}

impl Pause {
    /// `blip` is a short SFX id (from the game's own bank) played on each volume
    /// nudge so the SFX slider gives audible feedback.
    pub fn new(blip: i32) -> Self {
        Pause { sel: ROW_SFX, prev: 0xFF, blip }
    }

    /// Advance the menu one frame given the current control mask. Returns
    /// `Some(..)` when the menu should close.
    pub fn update(&mut self, mask: u8) -> Option<Exit> {
        let pressed = mask & !self.prev; // rising edges only
        self.prev = mask;

        if pressed & START != 0 {
            return Some(Exit::Resume);
        }
        if pressed & UP != 0 {
            self.sel = (self.sel + ROW_COUNT - 1) % ROW_COUNT;
        }
        if pressed & DOWN != 0 {
            self.sel = (self.sel + 1) % ROW_COUNT;
        }

        let dir: i32 = if pressed & RIGHT != 0 {
            1
        } else if pressed & LEFT != 0 {
            -1
        } else {
            0
        };
        if dir != 0 {
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

        if pressed & CONFIRM != 0 && self.sel == ROW_QUIT {
            return Some(Exit::QuitToMenu);
        }
        None
    }

    /// Draw the overlay (call after the game's own draw, before the buffer swap).
    /// Coordinates are PICO-8 128-space; the caller should have camera at (0,0).
    pub fn draw(&self) {
        backend::camera(0, 0);

        // A framed panel over the (frozen) game: black fill, 1px border, then a
        // dark inset so the text reads regardless of what's behind.
        backend::rectfill(17, 29, 110, 97, 7); // border
        backend::rectfill(18, 30, 109, 96, 0); // black fill
        backend::rectfill(20, 32, 107, 94, 1); // dark-blue inset

        print_centered(b"PAUSED", 36, 7);

        self.draw_slider(ROW_SFX, b"SFX", sfx::sfx_volume(), 50);
        self.draw_slider(ROW_MUSIC, b"MUSIC", sfx::music_volume(), 62);

        let quit_c = if self.sel == ROW_QUIT { 7 } else { 6 };
        print_centered(b"QUIT TO MENU", 78, quit_c);

        print_centered(b"START RESUMES", 88, 5);
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
