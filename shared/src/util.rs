//! Generic PICO-8 helper functions shared by every port (the `clamp`/`appr`/
//! `sgn` idioms that show up in nearly every PICO-8 cart). Game logic uses
//! these instead of re-defining them per crate.

use crate::fixed::Fix32;

/// PICO-8 `mid(a, val, b)` style clamp of `val` into `[a, b]`.
#[inline]
pub fn clamp(val: Fix32, a: Fix32, b: Fix32) -> Fix32 {
    a.max(b.min(val))
}

/// Move `val` toward `target` by at most `amount` (PICO-8 carts' `appr`).
#[inline]
pub fn approach(val: Fix32, target: Fix32, amount: Fix32) -> Fix32 {
    if val > target {
        (val - amount).max(target)
    } else {
        (val + amount).min(target)
    }
}

/// PICO-8 `sgn`: -1 / 0 / +1 as a `Fix32`.
#[inline]
pub fn sign(v: Fix32) -> Fix32 {
    if v.0 > 0 {
        Fix32::from_int(1)
    } else if v.0 < 0 {
        Fix32::from_int(-1)
    } else {
        Fix32::ZERO
    }
}
