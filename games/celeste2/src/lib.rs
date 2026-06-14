//! `celeste2` -- PICO-8 Celeste Classic 2: Lani's Trek, demade natively in
//! Rust for the PlayStation 1 on the PSoXide SDK.
//!
//! PHASE 1 (current): asset + render bring-up on the shared `pico8` runtime.
//! Registers Celeste 2's spritesheet/map as the active [`Cart`] and renders an
//! actual room's tilemap. The game logic (fixed-point physics, the grappling
//! hook, the object engine, the PX9-streamed levels) is ported on top next.
//!
//! Exposed as a library so the demo-disc launcher can link it in and call
//! [`run`]; the standalone `main` just calls it. Holding Select+Start returns
//! from [`run`] (quit to the launcher).

#![no_std]

pub mod assets;

use assets::gfx::GFX_DATA;
use assets::tilemap::{MAP_W, TILEMAP_DATA, TILE_FLAGS};
use pico8::backend::{self, Cart};
use psx_gpu::{self as gpu, Resolution, VideoMode, framebuf::FrameBuffer};
use psx_pad::{button, poll_port1};

/// Celeste 2's spritesheet + tilemap as the active PICO-8 cart.
const CART: Cart = Cart {
    gfx: &GFX_DATA,
    tilemap: &TILEMAP_DATA,
    tile_flags: &TILE_FLAGS,
    map_w: MAP_W,
};

/// Boot Celeste 2 and run its frame loop until Select+Start is held.
pub fn run() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    backend::upload_assets(CART);

    loop {
        let pad = poll_port1().buttons;
        if pad.is_held(button::SELECT) && pad.is_held(button::START) {
            return;
        }

        // PICO-8 dark-blue backdrop around the centred 256x256 field.
        fb.clear(0, 0, 16);

        // Level 1's opening screen: at camera (0,0) draw map cells (0..16).
        backend::map(0, 0, 0, 0, 16, 16, 0);

        gpu::draw_sync();
        gpu::vsync();
        fb.swap();
    }
}
