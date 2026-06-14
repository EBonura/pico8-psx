//! Offline music test disc: plays Celeste's music(0) so the PSX SPU output can
//! be captured and compared, note-aligned, with a PICO-8 recording of the same
//! music. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    loop {
        celeste::run_music_test(0);
    }
}
