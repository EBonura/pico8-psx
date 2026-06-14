//! `celeste2` -- PICO-8 Celeste Classic 2: Lani's Trek, demade natively
//! in Rust for the PlayStation 1 on the PSoXide SDK.
//!
//! PHASE 1 (current): asset + render bring-up. Uploads the real Celeste 2
//! spritesheet/CLUT/map to VRAM and renders an actual room's tilemap,
//! proving the asset pipeline and the PICO-8 -> PSoXide draw backend.
//! Assets are extracted from the standalone level-1 cart (ExOK/Celeste2,
//! `1.p8`) by `tools/p8_to_rust.py`. The game logic (fixed-point physics,
//! the grappling hook, the object engine, the streamed levels) is ported
//! on top of this backend in later phases.
//!
//! Exposed as a library so the demo-disc launcher can link it in and call
//! [`run`]; the standalone `main` just calls it. Either way, holding
//! Select+Start returns from [`run`] (quit to the launcher).

#![no_std]

pub mod assets;
mod backend;

use psx_gpu::{self as gpu, Resolution, VideoMode, framebuf::FrameBuffer};
use psx_pad::{button, poll_port1};

/// Boot Celeste 2 and run its frame loop until Select+Start is held.
pub fn run() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    backend::upload_assets();

    loop {
        let pad = poll_port1().buttons;
        if pad.is_held(button::SELECT) && pad.is_held(button::START) {
            return;
        }

        // PICO-8 dark-blue backdrop around the centred 256x256 field.
        fb.clear(0, 0, 16);

        // Level 1's opening screen: at camera (0,0) the game draws map
        // cells (0..16, 0..16) -- sky on top, the trailhead ground below.
        // mask 0 draws every tile in one pass for the bring-up smoke test.
        backend::map(0, 0, 0, 0, 16, 16, 0);

        gpu::draw_sync();
        gpu::vsync();
        fb.swap();
    }
}
