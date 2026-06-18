//! Standalone Celeste 2 disc entry point. The game itself lives in the
//! `celeste2` library (`lib.rs`) so the collection launcher can link it in;
//! this thin binary just hands control to it. Quitting (Select+Start)
//! returns from `run`, so here it simply boots again.

#![no_std]
#![no_main]

extern crate psx_rt;

#[no_mangle]
fn main() {
    loop {
        celeste2::run();
    }
}
