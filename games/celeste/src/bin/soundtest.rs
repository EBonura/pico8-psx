//! Offline SFX soundtest disc: plays every Celeste SFX in isolation, each for
//! a fixed window, so the PSX SPU output can be captured
//! (tools/psx-audio-capture) and split at exact offsets to diff against the
//! PICO-8 reference recordings. Not shipped in the game.
//!
//! SOUNDTEST_FRAMES[n] = the play window for SFX n in 60 Hz frames, derived
//! from each SFX's true length (ceil(dur*60)+6 tail, capped at 10s). Keep in
//! sync with the host splitter in tools/compare_sfx.py.

#![no_std]
#![no_main]

extern crate psx_rt;

/// Per-SFX play windows (frames at 60fps). Generated from the cart's SFX
/// durations; see tools/compare_sfx.py which uses the same table to split.
pub const SOUNDTEST_FRAMES: [u16; 63] = [
    38, 38, 54, 38, 70, 54, 54, 38, 54, 54, 261, 516,
    261, 70, 70, 54, 261, 261, 261, 516, 134, 516, 261, 102,
    389, 389, 198, 198, 389, 389, 198, 198, 198, 198, 198, 70,
    198, 261, 198, 261, 261, 134, 516, 261, 134, 261, 261, 261,
    516, 134, 134, 86, 516, 516, 54, 182, 261, 261, 261, 261,
    261, 261, 600,
];

#[no_mangle]
fn main() {
    loop {
        celeste::run_sfx_soundtest(&SOUNDTEST_FRAMES);
    }
}
