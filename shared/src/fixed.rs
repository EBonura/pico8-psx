//! PICO-8 16.16 fixed-point, ported from ccleste's `_fix32`
//! (celeste.cpp:32-119). Bit-exact arithmetic is required or the
//! physics diverge from the original.

use crate::sin_table::SIN_TBL;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

pub const FACTOR: i32 = 1 << 16;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fix32(pub i32);

/// Fix32 from a (compile-time) number literal — matches C's
/// `_fix32(double)` which truncates `x * 65536` toward zero.
pub const fn fx(x: f64) -> Fix32 {
    Fix32((x * 65536.0) as i32)
}

impl Fix32 {
    pub const ZERO: Fix32 = Fix32(0);

    #[inline]
    pub const fn from_bits(n: i32) -> Self {
        Fix32(n)
    }
    #[inline]
    pub const fn from_int(i: i32) -> Self {
        Fix32(i.wrapping_mul(FACTOR))
    }
    /// C `(int)` cast: truncate toward zero.
    #[inline]
    pub fn to_int(self) -> i32 {
        self.0 / FACTOR
    }
    /// Low 16 bits (fractional part), as C `frac()`.
    #[inline]
    pub fn frac(self) -> u16 {
        self.0 as u16
    }
    /// `_fix32_floor`: arithmetic floor, as Fix32.
    #[inline]
    pub fn floor(self) -> Fix32 {
        Fix32(self.0 & !0xFFFF)
    }
    /// floor as an integer.
    #[inline]
    pub fn floor_int(self) -> i32 {
        self.0 >> 16
    }
    #[inline]
    pub fn abs(self) -> Fix32 {
        if self.0 >= 0 { self } else { Fix32(self.0.wrapping_neg()) }
    }
    #[inline]
    pub fn min(self, b: Fix32) -> Fix32 {
        if self.0 < b.0 { self } else { b }
    }
    #[inline]
    pub fn max(self, b: Fix32) -> Fix32 {
        if self.0 > b.0 { self } else { b }
    }
    /// `_fix32_mod`: `((a % b) + b) % b`.
    #[inline]
    pub fn rem_floor(self, b: Fix32) -> Fix32 {
        Fix32(((self.0 % b.0) + b.0) % b.0)
    }
    /// PICO-8 `sin` (note: turns, and negated — matches `_fix32_sin`).
    pub fn sin(self) -> Fix32 {
        let mut index = ((self.0.wrapping_add(0x4002) >> 2) & 0x3FFF) as i32;
        if index > 0x1FFF {
            index = 0x4000 - index;
        }
        if index < 0x1000 {
            Fix32::from_bits(SIN_TBL[index as usize])
        } else {
            Fix32::from_bits(-SIN_TBL[(0x2000 - index) as usize])
        }
    }
    /// PICO-8 `cos(x) = -sin(x + 0.25)`.
    #[inline]
    pub fn cos(self) -> Fix32 {
        -((self + fx(0.25)).sin())
    }
}

impl Add for Fix32 {
    type Output = Fix32;
    #[inline]
    fn add(self, b: Fix32) -> Fix32 {
        Fix32(self.0.wrapping_add(b.0))
    }
}
impl Sub for Fix32 {
    type Output = Fix32;
    #[inline]
    fn sub(self, b: Fix32) -> Fix32 {
        Fix32(self.0.wrapping_sub(b.0))
    }
}
impl Neg for Fix32 {
    type Output = Fix32;
    #[inline]
    fn neg(self) -> Fix32 {
        Fix32(self.0.wrapping_neg())
    }
}
impl Mul for Fix32 {
    type Output = Fix32;
    #[inline]
    fn mul(self, b: Fix32) -> Fix32 {
        Fix32(((self.0 as i64 * b.0 as i64) / FACTOR as i64) as i32)
    }
}
impl Div for Fix32 {
    type Output = Fix32;
    #[inline]
    fn div(self, b: Fix32) -> Fix32 {
        // PICO-8 decomp'd division (celeste.cpp:70-76).
        if b.0 == 0 {
            return Fix32::from_int(if self.0 > 0 { FACTOR } else { -FACTOR });
        }
        if b.frac() == 0 && b.to_int() > 0 {
            let bi = b.0 as i64 / FACTOR as i64;
            return Fix32((self.0 as i64 / bi) as i32);
        }
        Fix32(((self.0 as i64 * FACTOR as i64) / b.0 as i64) as i32)
    }
}
impl AddAssign for Fix32 {
    #[inline]
    fn add_assign(&mut self, b: Fix32) {
        *self = *self + b;
    }
}
impl SubAssign for Fix32 {
    #[inline]
    fn sub_assign(&mut self, b: Fix32) {
        *self = *self - b;
    }
}
