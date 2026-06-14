//! Offline synth test disc. Default: plays the instruments song (song 7) in
//! isolation so each of the 8 instruments can be compared, aligned by onset,
//! against the PICO-8 reference. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    celeste::run_synth_song(7);
}
