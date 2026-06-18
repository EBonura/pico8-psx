//! Combined hardware test cart (video + audio), for one burn that validates both
//! the SPU and the GPU work this session:
//!  - VIDEO: the two hardware-suspect GPU primitives in isolation -- the `fillp`
//!    dither (texture-window + transparent CLUT) and the `side_bars` gouraud
//!    gradient -- plus a vivid gouraud band, over a flat field with a slow pan.
//!  - AUDIO: uploads the wavetables through the fixed `upload_adpcm` (DMA) path
//!    and plays a music track, so a clean tune confirms the SPU upload fix on
//!    real hardware. Not shipped in the game.
#![no_std]
#![no_main]
extern crate psx_rt;

use pico8::{backend, sfx};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};

#[inline]
fn wait_vblank() {
    let v = psx_rt::interrupts::vblank_count();
    while psx_rt::interrupts::vblank_count() == v {}
}

#[no_mangle]
fn main() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    psx_rt::interrupts::install_vblank_counter();
    backend::upload_assets(celeste2::CART);
    backend::set_side_preset(1); // Dusk gradient (the one missing on hardware)

    // Audio: wavetable upload via the fixed DMA path, then play a music track so
    // the burn doubles as the SPU-upload check (clean tune = the fix works).
    sfx::init(celeste2::AUDIO);
    sfx::music(0, 0, 0);

    let mut t: i16 = 0;
    loop {
        fb.clear(0, 0, 0);
        // Slow camera pan: on hardware this reveals the screen-locked dither
        // "two colours by position" behaviour (texture-window phase) and lets
        // the gouraud side gradient be checked against the photo.
        backend::camera((t / 8) & 63, 0);
        // Flat colour field over the whole 128-space play area.
        backend::rectfill(0, 0, 127, 127, 1); // dark blue
        // Columns dither (sparse dots), fill colour 8 (red), top half.
        backend::fillp_rect(0, 0, 127, 63, 8, backend::FILLP_COLUMNS);
        // Fog dither (50% checker), fill colour 7 (white), bottom half.
        backend::fillp_rect(0, 64, 127, 127, 7, backend::FILLP_FOG);
        // The side-margin gouraud gradient (the dark Dusk preset -- the feature
        // that's "missing" on hardware).
        backend::side_bars();

        // DIAGNOSTIC: a vivid full-width gouraud band (bright red -> cyan, top
        // to bottom) across the middle. On the burn this disambiguates the side-
        // gradient bug: if THIS bright band also shows as black, gouraud shading
        // is broken on hardware; if it shows clearly but the dark side margins
        // don't, the preset colours are simply too dark to read on a real TV
        // (fix = brighter presets).
        gpu::draw_tri_gouraud(
            [(0, 100), (319, 100), (0, 140)],
            [(255, 40, 40), (255, 40, 40), (40, 220, 255)],
        );
        gpu::draw_tri_gouraud(
            [(319, 100), (0, 140), (319, 140)],
            [(255, 40, 40), (40, 220, 255), (40, 220, 255)],
        );

        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        sfx::update(); // advance the music sequencer each frame
        t = t.wrapping_add(1);
    }
}
