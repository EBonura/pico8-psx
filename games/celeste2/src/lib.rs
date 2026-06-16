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
    MUSIC_PATTERNS, SFX_META, SFX_NOTES, SPU_PITCH_TABLE, WAVEFORM_ADPCM, WAVEFORM_ADPCM_LONG,
    WAVEFORM_OFFSET, WAVEFORM_OFFSET_LONG,
};
use assets::gfx::GFX_DATA;
use assets::tilemap::{MAP_W, TILEMAP_DATA, TILE_FLAGS};
use pico8::backend::{self, Cart};
use pico8::pause::{self, Exit, Pause};
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
    waveform_adpcm_long: &WAVEFORM_ADPCM_LONG,
    waveform_offset_long: &WAVEFORM_OFFSET_LONG,
    sfx_meta: &SFX_META,
    sfx_notes: &SFX_NOTES,
    spu_pitch_table: &SPU_PITCH_TABLE,
    music_patterns: &MUSIC_PATTERNS,
    music_pattern_count: 42,
};

/// Wait for the next real VBlank IRQ. The SDK's `gpu::vsync()` busy-waits a fixed
/// 242 hblanks (~15.4ms) instead of syncing to the display, leaving almost no
/// per-frame compute budget; the VBlank IRQ counter gives the full ~16.6ms frame.
#[inline]
fn wait_vblank() {
    let v = psx_rt::interrupts::vblank_count();
    while psx_rt::interrupts::vblank_count() == v {}
}

/// Boot Celeste 2 and run its 60fps frame loop until Select+Start is held.
pub fn run() {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);

    backend::upload_assets(CART);
    sfx::init(AUDIO);
    game::init();

    // Drive audio off real VBlanks so the music keeps tempo when rendering can't
    // hold 60fps (see celeste1).
    psx_rt::interrupts::install_vblank_counter();
    let mut last_vb = psx_rt::interrupts::vblank_count();
    let mut prev_start = true; // require a fresh press before the first pause

    loop {
        let b = poll_port1().buttons;
        if b.is_held(button::SELECT) && b.is_held(button::START) {
            return;
        }

        // Start alone (a fresh press, no Select) opens the pause menu.
        let start = b.is_held(button::START);
        if start && !prev_start {
            if run_pause(&mut fb) {
                return; // player chose "quit to menu"
            }
            last_vb = psx_rt::interrupts::vblank_count(); // don't count paused vblanks
            prev_start = true; // wait for release before it can pause again
            continue;
        }
        prev_start = start;

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
        wait_vblank();
        fb.swap();

        // Advance the music/SFX by the VBlanks actually elapsed (real-time tempo).
        let vb = psx_rt::interrupts::vblank_count();
        let mut elapsed = vb.wrapping_sub(last_vb);
        last_vb = vb;
        if elapsed == 0 {
            elapsed = 1;
        } else if elapsed > 4 {
            elapsed = 4;
        }
        for _ in 0..elapsed {
            sfx::update();
        }
    }
}

/// Pause overlay: freeze the game, show the volume/quit menu, and keep the SPU
/// advancing so the music plays on at the chosen volume. Returns true if the
/// player picked "quit to menu". `psfx(7,..)` (jump) is the SFX-slider blip.
fn run_pause(fb: &mut FrameBuffer) -> bool {
    let mut menu = Pause::new(7);
    loop {
        let b = poll_port1().buttons;
        let mut m = 0u8;
        if b.is_held(button::UP) {
            m |= pause::UP;
        }
        if b.is_held(button::DOWN) {
            m |= pause::DOWN;
        }
        if b.is_held(button::LEFT) {
            m |= pause::LEFT;
        }
        if b.is_held(button::RIGHT) {
            m |= pause::RIGHT;
        }
        if b.is_held(button::CROSS) {
            m |= pause::CONFIRM;
        }
        if b.is_held(button::START) {
            m |= pause::START;
        }
        match menu.update(m) {
            Some(Exit::Resume) => return false,
            Some(Exit::QuitToMenu) => return true,
            None => {}
        }

        fb.clear(0, 0, 16);
        game::draw(); // frozen game behind the overlay
        menu.draw();
        gpu::draw_sync();
        wait_vblank();
        fb.swap();
        sfx::update(); // keep music/SFX alive (and audible at the new volume)
    }
}

/// Offline SFX test: play `sfx(id)` repeatedly (id once, ~3s gap) so the host can
/// capture it and compare to a PICO-8 recording of the same SFX. Not the game.
pub fn run_sfx_test(id: i32) {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    sfx::init(AUDIO);
    loop {
        sfx::play(id);
        for _ in 0..240 {
            sfx::update();
            gpu::vsync();
        }
    }
}

/// Silent gap (frames) between SFX in the soundtest; the host splits the capture
/// on these. Keep in sync with the splitter in tools/compare_sfx.py.
pub const SOUNDTEST_GAP_FRAMES: u32 = 18;

/// Offline SFX soundtest: play SFX `0..frames.len()` one at a time, each for a
/// FIXED window `frames[n]` (so the host can split the captured SPU output at
/// exact offsets), separated by [`SOUNDTEST_GAP_FRAMES`] of silence. Diffed
/// against the PICO-8 reference recordings (audio-ref/celeste2/sfx). Advances at
/// the real 60Hz vblank like the game (gpu::vsync busy-waits ~65fps, which ran the
/// SFX ~8% fast vs the PICO-8 references). Not part of the game.
pub fn run_sfx_soundtest(frames: &[u16]) {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    sfx::init(AUDIO);
    psx_rt::interrupts::install_vblank_counter();
    let mut n: usize = 0;
    loop {
        sfx::play(-1);
        for _ in 0..SOUNDTEST_GAP_FRAMES {
            sfx::update();
            wait_vblank();
        }
        if n >= frames.len() {
            return;
        }
        sfx::play(n as i32);
        for _ in 0..frames[n] {
            sfx::update();
            wait_vblank();
        }
        n += 1;
    }
}

/// Offline music test: play `music(pattern)` and run the sequencer forever, so
/// the host can capture the exact same song the cart plays and compare it,
/// note-aligned, with a PICO-8 recording of `music(pattern)`. Not part of the
/// game; driven by the `musictest` binary + `tools/psx-audio-capture`.
pub fn run_music_test(pattern: i32) {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    sfx::init(AUDIO);
    // Advance one sequencer step per REAL vblank (60Hz) like the game -- gpu::vsync()
    // busy-waits 242 hblanks (~65fps), which played the music ~8% fast vs PICO-8.
    psx_rt::interrupts::install_vblank_counter();
    sfx::music(pattern, 0, 0);
    loop {
        sfx::update();
        wait_vblank();
    }
}

/// Offline per-instrument isolation: play `pattern` five times in fixed windows --
/// full mix, then each music channel 0..3 SOLO'd (others muted) -- separated by
/// silence, so the host can split the capture and compare each instrument against
/// a channel-soloed PICO-8 recording. Not part of the game.
pub fn run_music_iso(pattern: i32) {
    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    sfx::init(AUDIO);
    psx_rt::interrupts::install_vblank_counter();
    // mute masks: 0=full, then solo ch0/1/2/3 (mute the other three of the low 4).
    let masks = [0u8, 0x0E, 0x0D, 0x0B, 0x07];
    let mut i = 0;
    loop {
        sfx::set_music_mute(masks[i]);
        sfx::music(pattern, 0, 0);
        for _ in 0..420 {
            sfx::update();
            wait_vblank();
        }
        sfx::music(-1, 0, 0);
        sfx::set_music_mute(0);
        for _ in 0..72 {
            sfx::update();
            wait_vblank();
        }
        i += 1;
        if i >= masks.len() {
            return;
        }
    }
}
