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
//! the Cross still down from the menu that launched us) is suppressed until it
//! is released and pressed again. Prime that with [`prime`].
//!
//! The bookkeeping (edges, hold counting, auto-repeat, handoff suppression) is
//! the SDK's [`PadTracker`]; this module just keeps PICO-8's bit-indexed API
//! and repeat cadence on top of it.

use psx_pad::PadTracker;

static mut TRACKER: PadTracker = PadTracker::new();

// PICO-8 default auto-repeat (in the cart's frames).
const REPEAT_DELAY: u8 = 15;
const REPEAT_INTERVAL: u8 = 4;

/// Seed the held state from the current pad so buttons already down when the
/// cart starts don't read as a fresh `btnp`. Call once before the frame loop.
pub fn prime(mask: u8) {
    unsafe {
        TRACKER.update(mask as u16);
        TRACKER.prime();
    }
}

/// Latch this frame's 6-bit button mask. Call once per frame before game logic.
pub fn set_buttons(mask: u8) {
    unsafe { TRACKER.update(mask as u16) }
}

/// PICO-8 `btn(b)`: is button `b` held this frame.
#[inline]
pub fn btn(b: i32) -> bool {
    unsafe { TRACKER.is_held(1 << (b & 7)) }
}

/// PICO-8 `btnp(b)`: pressed this frame, or an auto-repeat tick. Suppressed for
/// buttons that were already held when the cart started (see [`prime`]).
#[inline]
pub fn btnp(b: i32) -> bool {
    unsafe { TRACKER.repeats(1 << (b & 7), REPEAT_DELAY, REPEAT_INTERVAL) }
}
