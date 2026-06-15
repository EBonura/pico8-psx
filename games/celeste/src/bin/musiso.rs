//! Offline per-instrument isolation disc: plays Celeste's main theme music(0)
//! as full mix then each channel soloed, so the host can split the capture and
//! compare each instrument (drums/lead/bass) to a channel-soloed PICO-8
//! recording. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;
#[no_mangle]
fn main() {
    loop {
        celeste::run_music_iso(0);
    }
}
