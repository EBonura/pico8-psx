//! Offline per-instrument isolation disc: plays Celeste 2's main theme music(2)
//! as full mix then each channel soloed, so the host can split the capture and
//! compare each instrument (bass/lead/drums) to a channel-soloed PICO-8
//! recording. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    loop {
        celeste2::run_music_iso(2);
    }
}
