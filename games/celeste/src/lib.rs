//! `celeste` -- PICO-8 Celeste Classic, demade natively in Rust for the
//! PlayStation 1 on the PSoXide SDK.
//!
//! The game logic (game.rs) is a faithful port of ccleste. The PICO-8 runtime
//! (draw/input/audio backend, fixed-point, RNG) lives in the shared `pico8`
//! crate; this crate supplies Celeste's own assets and wires them in as the
//! active [`Cart`] / [`AudioData`].
//!
//! Exposed as a library so the demo-disc launcher can link it in and call
//! [`run`]; the standalone `main` just calls it. Holding Select+Start returns
//! from [`run`] (quit to the launcher).

#![no_std]
#![allow(static_mut_refs)]

pub mod assets;
mod game;

use assets::audio_data::{
    MUSIC_PATTERNS, SFX_META, SFX_NOTES, SPU_PITCH_TABLE, WAVEFORM_ADPCM, WAVEFORM_OFFSET,
};
use assets::gfx::GFX_DATA;
use assets::tilemap::{MAP_W, TILEMAP_DATA, TILE_FLAGS};
use pico8::backend::{self, Cart};
use pico8::sfx::{self, AudioData};
use psx_gpu::{self as gpu, Resolution, VideoMode, framebuf::FrameBuffer};
use psx_pad::{button, poll_port1};

/// Celeste's spritesheet + tilemap as the active PICO-8 cart.
const CART: Cart = Cart {
    gfx: &GFX_DATA,
    tilemap: &TILEMAP_DATA,
    tile_flags: &TILE_FLAGS,
    map_w: MAP_W,
};

/// Celeste's PICO-8 sound data (42 music patterns).
const AUDIO: AudioData = AudioData {
    waveform_adpcm: &WAVEFORM_ADPCM,
    waveform_offset: &WAVEFORM_OFFSET,
    sfx_meta: &SFX_META,
    sfx_notes: &SFX_NOTES,
    spu_pitch_table: &SPU_PITCH_TABLE,
    music_patterns: &MUSIC_PATTERNS,
    music_pattern_count: 42,
};

/// Boot Celeste and run its 60fps frame loop until Select+Start is held.
pub fn run() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    backend::upload_assets(CART);
    sfx::init(AUDIO);

    // Seed the RNG before init (clouds/particles use it), like main.cpp.
    pico8::rng::srand(42);
    game::init();

    loop {
        // Map the pad to PICO-8's 6 buttons: arrows, Cross=jump, Circle=dash.
        let b = poll_port1().buttons;

        // Quit to the launcher: Select+Start held together.
        if b.is_held(button::SELECT) && b.is_held(button::START) {
            return;
        }

        let mut mask = 0u8;
        if b.is_held(button::LEFT) {
            mask |= 1 << 0;
        }
        if b.is_held(button::RIGHT) {
            mask |= 1 << 1;
        }
        if b.is_held(button::UP) {
            mask |= 1 << 2;
        }
        if b.is_held(button::DOWN) {
            mask |= 1 << 3;
        }
        if b.is_held(button::CROSS) {
            mask |= 1 << 4;
        }
        if b.is_held(button::CIRCLE) {
            mask |= 1 << 5;
        }
        game::set_input(mask);

        game::update();
        sfx::update();

        // Freeze frames (dash/orb): hold the last drawn frame on screen by not
        // redrawing or swapping -- exactly the PICO-8 freeze effect.
        if game::freeze() > 0 {
            gpu::vsync();
            continue;
        }

        fb.clear(0, 0, 0);
        game::draw();
        gpu::draw_sync();
        gpu::vsync();
        fb.swap();
    }
}

/// Number of silent frames played between SFX in the soundtest. Doubles as
/// the split marker for the host capture (a clear gap between clips).
pub const SOUNDTEST_GAP_FRAMES: u32 = 18; // ~0.3s

/// Offline SFX soundtest: play SFX `0..frames.len()` one at a time, each for a
/// FIXED number of frames `frames[n]` (so the host can split the captured SPU
/// output at exact offsets), separated by [`SOUNDTEST_GAP_FRAMES`] of silence.
/// Diffed against the PICO-8 reference recordings. Not part of the game;
/// driven by the `soundtest` binary + `tools/psx-audio-capture`.
pub fn run_sfx_soundtest(frames: &[u16]) {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    sfx::init(AUDIO);

    let mut n: usize = 0;
    loop {
        sfx::play(-1);
        for _ in 0..SOUNDTEST_GAP_FRAMES {
            sfx::update();
            gpu::vsync();
        }

        if n >= frames.len() {
            return;
        }

        sfx::play(n as i32);
        for _ in 0..frames[n] {
            sfx::update();
            gpu::vsync();
        }
        n += 1;
    }
}
