//! PICO-8 button model: `btn()` (held) and `btnp()` (pressed, with auto-repeat).
//!
//! Buttons are PICO-8's six, by bit: 0 left, 1 right, 2 up, 3 down, 4 O (jump),
//! 5 X (dash/grapple). Call [`set_buttons`] once per frame with the current
//! 6-bit pad mask BEFORE game logic, then use [`btn`] / [`btnp`] exactly like
//! the Lua equivalents.
//!
//! `btnp` matches PICO-8: true on the frame a button is first pressed, then --
//! if held -- again after a 15-frame delay, repeating every 4 frames. Plus one
//! launcher-safe rule: a button that is already held when the cart starts (e.g.
//! the Cross still down from the menu that launched us) is suppressed by `btnp`
//! until it is released and pressed again. Prime that with [`prime`].

static mut STATE: u8 = 0; // buttons held this frame
static mut HOLD: [u16; 8] = [0; 8]; // frames each button has been held (0 = up)
static mut IGNORE: u8 = 0; // buttons held at prime; btnp-suppressed until released

// PICO-8 default auto-repeat (in the cart's frames).
const REPEAT_DELAY: u16 = 15;
const REPEAT_INTERVAL: u16 = 4;

/// Seed the held state from the current pad so buttons already down when the
/// cart starts don't read as a fresh `btnp`. Call once before the frame loop.
pub fn prime(mask: u8) {
    unsafe {
        STATE = mask;
        IGNORE = mask;
        for b in 0..8 {
            HOLD[b] = if mask & (1 << b) != 0 { 1 } else { 0 };
        }
    }
}

/// Latch this frame's 6-bit button mask. Call once per frame before game logic.
pub fn set_buttons(mask: u8) {
    unsafe {
        IGNORE &= mask; // a suppressed button clears once it's released
        STATE = mask;
        for b in 0..8 {
            if mask & (1 << b) != 0 {
                HOLD[b] = HOLD[b].saturating_add(1);
            } else {
                HOLD[b] = 0;
            }
        }
    }
}

/// PICO-8 `btn(b)`: is button `b` held this frame.
#[inline]
pub fn btn(b: i32) -> bool {
    unsafe { STATE & (1 << (b & 7)) != 0 }
}

/// PICO-8 `btnp(b)`: pressed this frame, or an auto-repeat tick. Suppressed for
/// buttons that were already held when the cart started (see [`prime`]).
#[inline]
pub fn btnp(b: i32) -> bool {
    let i = (b & 7) as usize;
    unsafe {
        if IGNORE & (1 << i) != 0 {
            return false;
        }
        let h = HOLD[i];
        h == 1 || (h > REPEAT_DELAY && (h - REPEAT_DELAY - 1) % REPEAT_INTERVAL == 0)
    }
}
