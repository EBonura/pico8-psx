//! Offline synth test disc: plays the matched test songs (scale / drums / trio)
//! so the PSX SPU output can be captured and compared, song-by-song, with a
//! PICO-8 recording of the exact same songs. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    celeste::run_synth_test();
}
