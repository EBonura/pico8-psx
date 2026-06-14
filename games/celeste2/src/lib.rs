//! `celeste2` -- PICO-8 Celeste Classic 2: Lani's Trek, demade natively in
//! Rust for the PlayStation 1 on the PSoXide SDK.
//!
//! Built on the shared `pico8` runtime: this crate supplies Celeste 2's assets
//! (registered as a [`Cart`]) and game logic (`game.rs`). The logic runs at
//! 60fps with PICO-8 velocities kept at face value, accelerations + per-frame
//! moves halved, and frame timers doubled (matching the Celeste 1 port).
//!
//! PHASE A: object engine + player normal-state physics on level 1. Grapple,
//! the other objects, level streaming and audio come next.
//!
//! Exposed as a library so the demo-disc launcher can link it in and call
//! [`run`]; holding Select+Start returns from [`run`] (quit to the launcher).

#![no_std]

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

/// Celeste 2's spritesheet + tilemap as the active PICO-8 cart.
const CART: Cart = Cart {
    gfx: &GFX_DATA,
    tilemap: &TILEMAP_DATA,
    tile_flags: &TILE_FLAGS,
    map_w: MAP_W,
};

/// Celeste 2's PICO-8 sound data (42 music patterns).
const AUDIO: AudioData = AudioData {
    waveform_adpcm: &WAVEFORM_ADPCM,
    waveform_offset: &WAVEFORM_OFFSET,
    sfx_meta: &SFX_META,
    sfx_notes: &SFX_NOTES,
    spu_pitch_table: &SPU_PITCH_TABLE,
    music_patterns: &MUSIC_PATTERNS,
    music_pattern_count: 42,
};

/// Boot Celeste 2 and run its 60fps frame loop until Select+Start is held.
pub fn run() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    backend::upload_assets(CART);
    sfx::init(AUDIO);
    game::init();

    loop {
        let b = poll_port1().buttons;
        if b.is_held(button::SELECT) && b.is_held(button::START) {
            return;
        }

        // PICO-8 buttons: arrows, Cross = jump (btn 4), Circle = grapple (btn 5).
        let mut mask = 0u8;
        if b.is_held(button::LEFT) {
            mask |= game::IN_LEFT;
        }
        if b.is_held(button::RIGHT) {
            mask |= game::IN_RIGHT;
        }
        if b.is_held(button::UP) {
            mask |= game::IN_UP;
        }
        if b.is_held(button::DOWN) {
            mask |= game::IN_DOWN;
        }
        if b.is_held(button::CROSS) {
            mask |= game::IN_JUMP;
        }
        if b.is_held(button::CIRCLE) {
            mask |= game::IN_GRAPPLE;
        }
        game::set_input(mask);

        game::update();

        fb.clear(0, 0, 16); // PICO-8 dark-blue backdrop
        game::draw();
        gpu::draw_sync();
        gpu::vsync();
        fb.swap();
    }
}
