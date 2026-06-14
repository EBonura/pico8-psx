//! `celeste` -- PICO-8 Celeste Classic, demade natively in Rust for the
//! PlayStation 1 on the PSoXide SDK.
//!
//! The game logic (game.rs) is a faithful port of ccleste; the backend
//! (backend.rs) implements PICO-8's draw/input primitives on PSoXide's
//! GPU; the assets are transcoded bit-exact from the original cart.
//!
//! Exposed as a library so the demo-disc launcher can link it in and call
//! [`run`]; the standalone `main` just calls it. Either way, holding
//! Select+Start returns from [`run`] (quit to the launcher).

#![no_std]
#![allow(static_mut_refs)]

pub mod assets;
mod backend;
mod fixed;
mod game;
mod rng;
mod sfx;
mod sin_table;

use psx_gpu::{self as gpu, Resolution, VideoMode, framebuf::FrameBuffer};
use psx_pad::{button, poll_port1};

/// Boot Celeste and run its 60fps frame loop until Select+Start is held.
pub fn run() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    backend::upload_assets();
    sfx::init();

    // Seed the RNG before init (clouds/particles use it), like main.cpp.
    rng::srand(42);
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

        // Freeze frames (dash/orb): hold the last drawn frame on screen by
        // not redrawing or swapping -- exactly the PICO-8 freeze effect.
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

/// Offline SFX soundtest: play SFX `0..frames.len()` one at a time, each for
/// a FIXED number of frames `frames[n]` (so the host can split the captured
/// SPU output at exact offsets), separated by [`SOUNDTEST_GAP_FRAMES`] of
/// silence. Diffed against the PICO-8 reference recordings. Not part of the
/// game; driven by the `soundtest` binary + `tools/psx-audio-capture`.
pub fn run_sfx_soundtest(frames: &[u16]) {
    // gpu::init only to get a 60fps VBlank for sequencer pacing; we never draw.
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    sfx::init();

    let mut n: usize = 0;
    loop {
        // Silence gap (also lets the previous SFX's tail fully decay).
        sfx::play(-1);
        for _ in 0..SOUNDTEST_GAP_FRAMES {
            sfx::update();
            gpu::vsync();
        }

        if n >= frames.len() {
            return; // host capture ends; the bin shim loops us again
        }

        sfx::play(n as i32);
        for _ in 0..frames[n] {
            sfx::update();
            gpu::vsync();
        }
        n += 1;
    }
}
