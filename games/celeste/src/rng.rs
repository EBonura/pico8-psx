//! PICO-8 RNG, ported from ccleste (celeste.cpp:156-205). Must be
//! bit-exact for deterministic object/particle behaviour.

use crate::fixed::Fix32;

static mut SEED_LO: u32 = 0;
static mut SEED_HI: u32 = 1;

/// `pico8_random(max)` — returns an int in `[0, max)`.
pub fn random(max: i32) -> i32 {
    if max == 0 {
        return 0;
    }
    unsafe {
        SEED_HI = ((SEED_HI << 16) | (SEED_HI >> 16)).wrapping_add(SEED_LO);
        SEED_LO = SEED_LO.wrapping_add(SEED_HI);
        (SEED_HI % (max as u32)) as i32
    }
}

/// `pico8_srand(seed)`.
pub fn srand(mut seed: u32) {
    unsafe {
        if seed == 0 {
            SEED_HI = 0x6000_9755;
            seed = 0xdead_beef;
        } else {
            SEED_HI = seed ^ 0xbead_29ba;
        }
        let mut i = 0x20;
        while i > 0 {
            SEED_HI = ((SEED_HI << 16) | (SEED_HI >> 16)).wrapping_add(seed);
            seed = seed.wrapping_add(SEED_HI);
            i -= 1;
        }
        SEED_LO = seed;
    }
}

/// PICO-8 `rnd(max)` for Fix32 — `from_bits(pico8_random(max.n))`.
#[inline]
pub fn rnd(max: Fix32) -> Fix32 {
    Fix32::from_bits(random(max.0))
}
