//! PICO-8 audio on the PS1 SPU: synthesises PICO-8's sfx/music by keying SPU
//! voices over the game's pre-rendered instrument waveforms. Voices 0-3 =
//! music channels, 4-7 = SFX. Voice index == channel index throughout.
//!
//! The sound data (waveforms, sfx note tables, music patterns, pitch table) is
//! game-specific and supplied via [`AudioData`] at [`init`]; the engine itself
//! is universal PICO-8 behaviour.

use psx_spu as spu;
use spu::{Adsr, Pitch, SpuAddr, Voice, Volume};

/// A game's PICO-8 sound data, generated alongside its other assets.
#[derive(Clone, Copy)]
pub struct AudioData {
    /// ADPCM-encoded instrument waveforms (the 8 PICO-8 waveforms), uploaded
    /// to SPU RAM at [`SPU_WAVEFORM_BASE`].
    pub waveform_adpcm: &'static [u8],
    /// Byte offset of each of the 8 waveforms within `waveform_adpcm`.
    pub waveform_offset: &'static [u16; 8],
    /// Per-SFX metadata: `[speed, loop_start, loop_end, _]`.
    pub sfx_meta: &'static [[u8; 4]],
    /// Per-SFX 32 note words (pitch|instr|vol|effect bit-packed).
    pub sfx_notes: &'static [[u16; 32]],
    /// PICO-8 key (0..63) -> SPU pitch.
    pub spu_pitch_table: &'static [u16; 64],
    /// Music patterns: `[flags, ch0, ch1, ch2, ch3, _, _, _]`.
    pub music_patterns: &'static [[u8; 8]],
    /// Number of valid music patterns (the rest of `music_patterns` is unused).
    pub music_pattern_count: i32,
}

const EMPTY_AUDIO: AudioData = AudioData {
    waveform_adpcm: &[],
    waveform_offset: &[0; 8],
    sfx_meta: &[],
    sfx_notes: &[],
    spu_pitch_table: &[0; 64],
    music_patterns: &[],
    music_pattern_count: 0,
};

const SPU_WAVEFORM_BASE: u32 = 0x1000;
const NUM_SFX_VOICES: usize = 4;
const SFX_VOICE_BASE: usize = 4;

const TICK_INC: i32 = 256;
const TICK_PER_SPEED: i32 = 128;

const MUSIC_LOOP_START: u8 = 0x01;
const MUSIC_LOOP_END: u8 = 0x02;
const MUSIC_STOP: u8 = 0x04;

const VOL_TABLE: [u16; 8] = [0x0000, 0x0800, 0x1000, 0x1800, 0x2000, 0x2800, 0x3000, 0x3800];

#[derive(Clone, Copy)]
struct Channel {
    sfx_id: i32, // -1 = inactive
    note_pos: i32,
    tick: i32,
    vibrato_phase: i32,
    keyed_on: bool,
}
const CH0: Channel = Channel { sfx_id: -1, note_pos: 0, tick: 0, vibrato_phase: 0, keyed_on: false };

static mut AUDIO: AudioData = EMPTY_AUDIO;
static mut CHANNELS: [Channel; 8] = [CH0; 8];
static mut MUSIC_PATTERN: i32 = -1;
static mut MUSIC_LOOP: i32 = -1;
static mut SFX_NEXT_VOICE: usize = 0;
static mut WAVEFORM_ADDR: [u32; 8] = [0; 8]; // byte addresses in SPU RAM

// ---- note decode ----
#[inline]
fn sfx_pitch(n: u16) -> i32 {
    (n & 0x3F) as i32
}
#[inline]
fn sfx_instr(n: u16) -> i32 {
    ((n >> 6) & 0x7) as i32
}
#[inline]
fn sfx_vol(n: u16) -> i32 {
    ((n >> 9) & 0x7) as i32
}
#[inline]
fn sfx_effect(n: u16) -> i32 {
    ((n >> 12) & 0x7) as i32
}

#[inline]
fn get_pitch(key: i32, instr: i32) -> u16 {
    let mut p = unsafe { AUDIO.spu_pitch_table }[(key & 63) as usize];
    if instr == 6 {
        p >>= 2; // noise: quarter the pitch
    }
    p
}

unsafe fn voice_key_off(v: usize) {
    if CHANNELS[v].keyed_on {
        Voice::key_off(1 << v);
        CHANNELS[v].keyed_on = false;
    }
}

unsafe fn start_channel_note(v: usize) {
    let ch = CHANNELS[v];
    let note = AUDIO.sfx_notes[ch.sfx_id as usize][ch.note_pos as usize];
    let vol = sfx_vol(note);
    if vol == 0 {
        voice_key_off(v);
        return;
    }
    let spu_vol = VOL_TABLE[vol as usize] as i16;
    let spu_pitch = get_pitch(sfx_pitch(note), sfx_instr(note));
    let addr = WAVEFORM_ADDR[(sfx_instr(note) & 7) as usize];
    voice_key_off(v);
    let voice = Voice::new(v as u8);
    voice.set_volume(Volume(spu_vol), Volume(spu_vol));
    voice.set_pitch(Pitch::raw(spu_pitch));
    voice.set_start_addr(SpuAddr::new(addr));
    Voice::key_on(1 << v);
    CHANNELS[v].keyed_on = true;
}

unsafe fn apply_effects(v: usize) {
    let ch = CHANNELS[v];
    if ch.sfx_id < 0 {
        return;
    }
    let note = AUDIO.sfx_notes[ch.sfx_id as usize][ch.note_pos as usize];
    let effect = sfx_effect(note);
    let pitch_key = sfx_pitch(note);
    let instr = sfx_instr(note);
    let vol = sfx_vol(note);
    if vol == 0 || effect == 0 {
        return;
    }
    let base_pitch = get_pitch(pitch_key, instr) as i32;
    let speed = AUDIO.sfx_meta[ch.sfx_id as usize][0] as i32;
    let mut total = speed * TICK_PER_SPEED;
    if total < 1 {
        total = 1;
    }
    let t = ch.tick;
    let voice = Voice::new(v as u8);
    match effect {
        1 => {
            // slide
            let next_pos = ch.note_pos + 1;
            if next_pos < 32 {
                let nn = AUDIO.sfx_notes[ch.sfx_id as usize][next_pos as usize];
                let target = get_pitch(sfx_pitch(nn), sfx_instr(nn)) as i32;
                let mut p = base_pitch + ((target - base_pitch) * t) / total;
                p = p.clamp(1, 0x3FFF);
                voice.set_pitch(Pitch::raw(p as u16));
            }
        }
        2 => {
            // vibrato
            CHANNELS[v].vibrato_phase += 16;
            let phase = CHANNELS[v].vibrato_phase & 0xFF;
            let m = if phase < 64 {
                phase
            } else if phase < 192 {
                128 - phase
            } else {
                phase - 256
            };
            let mut p = base_pitch + (m * base_pitch) / 2048;
            p = p.clamp(1, 0x3FFF);
            voice.set_pitch(Pitch::raw(p as u16));
        }
        3 => {
            // drop
            let mut p = base_pitch * (total - t) / total;
            if p < 0 {
                p = 0;
            }
            voice.set_pitch(Pitch::raw(p as u16));
        }
        4 => {
            // fade in
            let vv = (VOL_TABLE[vol as usize] as u32 * t as u32 / total as u32) as i16;
            voice.set_volume(Volume(vv), Volume(vv));
        }
        5 => {
            // fade out
            let vv = (VOL_TABLE[vol as usize] as u32 * (total - t) as u32 / total as u32) as i16;
            voice.set_volume(Volume(vv), Volume(vv));
        }
        6 => {
            // arp fast
            let step = (t / 4) % 3;
            let off = if step == 0 { 0 } else if step == 1 { 4 } else { 7 };
            voice.set_pitch(Pitch::raw(get_pitch((pitch_key + off) & 63, instr)));
        }
        7 => {
            // arp slow
            let step = (t / 8) % 3;
            let off = if step == 0 { 0 } else if step == 1 { 4 } else { 7 };
            voice.set_pitch(Pitch::raw(get_pitch((pitch_key + off) & 63, instr)));
        }
        _ => {}
    }
}

unsafe fn advance_channel(v: usize) {
    if CHANNELS[v].sfx_id < 0 {
        return;
    }
    let mut speed = AUDIO.sfx_meta[CHANNELS[v].sfx_id as usize][0] as i32;
    if speed < 1 {
        speed = 1;
    }
    let threshold = speed * TICK_PER_SPEED;
    CHANNELS[v].tick += TICK_INC;
    while CHANNELS[v].tick >= threshold {
        CHANNELS[v].tick -= threshold;
        CHANNELS[v].note_pos += 1;
        CHANNELS[v].vibrato_phase = 0;
        let meta = AUDIO.sfx_meta[CHANNELS[v].sfx_id as usize];
        let loop_end = meta[2] as i32;
        let loop_start = meta[1] as i32;
        if loop_end > 0 && CHANNELS[v].note_pos >= loop_end {
            CHANNELS[v].note_pos = loop_start;
        }
        if CHANNELS[v].note_pos >= 32 {
            CHANNELS[v].sfx_id = -1;
            voice_key_off(v);
            return;
        }
        start_channel_note(v);
    }
    apply_effects(v);
}

unsafe fn music_advance_pattern() {
    if MUSIC_PATTERN < 0 || MUSIC_PATTERN >= AUDIO.music_pattern_count {
        MUSIC_PATTERN = -1;
        return;
    }
    let pat = AUDIO.music_patterns[MUSIC_PATTERN as usize];
    if pat[0] & MUSIC_LOOP_START != 0 {
        MUSIC_LOOP = MUSIC_PATTERN;
    }
    for c in 0..4 {
        let chan = pat[1 + c];
        if chan & 0x80 != 0 {
            if CHANNELS[c].sfx_id >= 0 {
                CHANNELS[c].sfx_id = -1;
                voice_key_off(c);
            }
            continue;
        }
        CHANNELS[c].sfx_id = (chan & 0x3F) as i32;
        CHANNELS[c].note_pos = 0;
        CHANNELS[c].tick = 0;
        CHANNELS[c].vibrato_phase = 0;
        start_channel_note(c);
    }
}

unsafe fn music_any_channel_done() -> bool {
    let pat = AUDIO.music_patterns[MUSIC_PATTERN as usize];
    for c in 0..4 {
        if pat[1 + c] & 0x80 != 0 {
            continue;
        }
        if CHANNELS[c].sfx_id < 0 {
            return true;
        }
    }
    false
}

// ---- public API ----

/// Initialise the SPU and load the game's [`AudioData`]. Call once after boot.
pub fn init(audio: AudioData) {
    unsafe {
        AUDIO = audio;
        spu::init();
        spu::set_main_volume(Volume(0x3800), Volume(0x3800));
        for v in 0..24u8 {
            let voice = Voice::new(v);
            voice.set_volume(Volume(0), Volume(0));
            voice.set_pitch(Pitch::raw(0));
            voice.set_start_addr(SpuAddr::new(0));
            voice.set_adsr(Adsr { lower: 0x000F, upper: 0x0000 });
        }
        spu::upload_adpcm(SpuAddr::new(SPU_WAVEFORM_BASE), audio.waveform_adpcm);
        for w in 0..8 {
            WAVEFORM_ADDR[w] = SPU_WAVEFORM_BASE + audio.waveform_offset[w] as u32;
        }
        CHANNELS = [CH0; 8];
        MUSIC_PATTERN = -1;
        MUSIC_LOOP = -1;
    }
}

/// Advance the sequencer one frame. Call every game frame.
pub fn update() {
    unsafe {
        if MUSIC_PATTERN >= 0 {
            for c in 0..4 {
                advance_channel(c);
            }
            if music_any_channel_done() {
                let flags = AUDIO.music_patterns[MUSIC_PATTERN as usize][0];
                if flags & MUSIC_STOP != 0 {
                    MUSIC_PATTERN = -1;
                    for c in 0..4 {
                        CHANNELS[c].sfx_id = -1;
                        voice_key_off(c);
                    }
                } else if flags & MUSIC_LOOP_END != 0 {
                    MUSIC_PATTERN = if MUSIC_LOOP >= 0 { MUSIC_LOOP } else { 0 };
                    music_advance_pattern();
                } else {
                    MUSIC_PATTERN += 1;
                    if MUSIC_PATTERN >= AUDIO.music_pattern_count {
                        MUSIC_PATTERN = -1;
                    } else {
                        music_advance_pattern();
                    }
                }
            }
        }
        for s in 0..NUM_SFX_VOICES {
            advance_channel(SFX_VOICE_BASE + s);
        }
    }
}

/// PICO-8 `sfx(id)` (id < 0 stops all).
pub fn play(id: i32) {
    unsafe {
        if id < 0 || id >= 64 {
            for s in 0..NUM_SFX_VOICES {
                CHANNELS[SFX_VOICE_BASE + s].sfx_id = -1;
                voice_key_off(SFX_VOICE_BASE + s);
            }
            return;
        }
        let mut slot = usize::MAX;
        for s in 0..NUM_SFX_VOICES {
            if CHANNELS[SFX_VOICE_BASE + s].sfx_id < 0 {
                slot = s;
                break;
            }
        }
        if slot == usize::MAX {
            slot = SFX_NEXT_VOICE;
            SFX_NEXT_VOICE = (SFX_NEXT_VOICE + 1) % NUM_SFX_VOICES;
        }
        let v = SFX_VOICE_BASE + slot;
        CHANNELS[v].sfx_id = id;
        CHANNELS[v].note_pos = 0;
        CHANNELS[v].tick = 0;
        CHANNELS[v].vibrato_phase = 0;
        start_channel_note(v);
    }
}

/// PICO-8 `music(pattern, fade, mask)` (pattern < 0 stops).
pub fn music(pattern: i32, _fade: i32, _mask: i32) {
    unsafe {
        if pattern < 0 {
            MUSIC_PATTERN = -1;
            for c in 0..4 {
                CHANNELS[c].sfx_id = -1;
                voice_key_off(c);
            }
            return;
        }
        if pattern >= AUDIO.music_pattern_count {
            return;
        }
        MUSIC_PATTERN = pattern;
        MUSIC_LOOP = -1;
        music_advance_pattern();
    }
}
