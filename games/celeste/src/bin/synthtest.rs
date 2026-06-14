//! Offline synth test disc. Default: plays the noise-pitch-sweep song (song 8)
//! to calibrate the hardware-noise drums against PICO-8. Not shipped.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    celeste::run_synth_song(8);
}
