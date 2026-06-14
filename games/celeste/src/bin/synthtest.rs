//! Offline synth test disc -- plays song 9 (instruments at pitch 48) to compare
//! high-note timbre against PICO-8. Not shipped.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() { celeste::run_synth_song(9); }
