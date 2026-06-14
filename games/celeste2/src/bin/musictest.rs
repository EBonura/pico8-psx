//! Offline music test disc: plays Celeste 2's main theme music(2) so the PSX
//! SPU output can be captured and compared, note-aligned, with a PICO-8
//! recording of the same music. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    loop {
        celeste2::run_music_test(2);
    }
}
