//! Offline SFX soundtest disc: plays every celeste2 SFX in isolation, each for a
//! FIXED 96-frame (~1.6s) window so the capture splits on a perfectly regular
//! layout (no per-SFX duration variation -> no cumulative drift). The host
//! tools/sfx_bench.py uses the same fixed window. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
pub const SOUNDTEST_FRAMES: [u16; 64] = [96; 64];
#[no_mangle]
fn main() {
    loop {
        celeste2::run_sfx_soundtest(&SOUNDTEST_FRAMES);
    }
}
