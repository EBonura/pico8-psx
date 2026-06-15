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
// The phaser (instrument 7) is two triangles a hair apart that beat. We can't do
// that with one static wavetable, so a phaser note also keys a "buddy" voice
// (v + 8, i.e. voices 8..15, otherwise unused) playing a detuned triangle.
const PHASER_BUDDY: usize = 8;

const TICK_INC: i32 = 256;
const TICK_PER_SPEED: i32 = 128;

const MUSIC_LOOP_START: u8 = 0x01;
const MUSIC_LOOP_END: u8 = 0x02;
const MUSIC_STOP: u8 = 0x04;

// Per-note volume (PICO-8 vol 0..7). Scaled so four music voices at max sum
// to ~full-scale instead of clipping (which was adding harsh harmonics): vol 7
// caps at 0x1000 (1/4 of the SPU's 0x4000 range).
const VOL_TABLE: [u16; 8] = [0x0000, 0x0250, 0x0490, 0x06D0, 0x0920, 0x0B60, 0x0DB0, 0x1000];

// --- direct SPU MMIO for hardware noise (instrument 6) ---------------------
// PICO-8's noise is continuous LFSR hiss; a looped sample wavetable just buzzes
// at a pitch, so percussion is lost. The PS1 SPU has a hardware noise generator:
// set a voice's bit in NON (Noise ON) and it outputs LFSR noise instead of its
// sample, clocked by SPUCNT's 6-bit noise rate (bits 8..13). The SDK keeps these
// registers private, so we drive them directly (all our voices are 0..7 < 16, so
// only the low NON word matters).
const NON_REG: u32 = 0x1F80_1D94; // Noise ON, voices 0..15
const SPUCNT_REG: u32 = 0x1F80_1DAA;
static mut NOISE_MASK: u16 = 0;

#[inline]
fn reg_write16(addr: u32, v: u16) {
    unsafe { core::ptr::write_volatile(addr as *mut u16, v) }
}
#[inline]
fn reg_read16(addr: u32) -> u16 {
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}

/// Put voice `v` into (or out of) hardware-noise mode.
unsafe fn set_voice_noise(v: usize, on: bool) {
    let bit = 1u16 << v;
    let was = NOISE_MASK;
    if on {
        NOISE_MASK |= bit;
    } else {
        NOISE_MASK &= !bit;
    }
    if NOISE_MASK != was {
        reg_write16(NON_REG, NOISE_MASK);
    }
}

/// SPUCNT noise-rate field (bits 8..13) for a PICO-8 noise note `pitch` (0..63).
/// On the SPU a higher NoiseShift = HIGHER (brighter) noise frequency, and
/// PICO-8's noise gets brighter with pitch, so map pitch up to shift up.
/// Calibrated so the SPU noise centroid tracks PICO-8's (~2.3kHz at pitch 4 to
/// ~4.7kHz at pitch 60). step (bits 0..1) kept at 2.
fn noise_clock(pitch: i32) -> u16 {
    let shift = (1 + pitch / 7).clamp(1, 12) as u16;
    (shift << 2) | 0x02
}

/// Set the global SPU noise frequency from a noise note's pitch (last writer
/// wins; percussion is normally one voice).
unsafe fn set_noise_freq(pitch: i32) {
    let cnt = reg_read16(SPUCNT_REG) & !0x3F00;
    reg_write16(SPUCNT_REG, cnt | (noise_clock(pitch) << 8));
}

/// Phaser buddy pitch for an SPU pitch + PICO-8 key. zepto8: the two oscillators
/// are detuned less at higher pitch (denom ~97 at c0 .. ~127 at c5), so high notes
/// beat slower instead of warbling. denom = 97 + key/2; buddy = pitch*(denom-1)/denom.
fn phaser_buddy_pitch(spu_pitch: i32, key: i32) -> u16 {
    let denom = 97 + key / 2;
    (spu_pitch * (denom - 1) / denom) as u16
}

#[derive(Clone, Copy)]
struct Channel {
    sfx_id: i32, // -1 = inactive
    note_pos: i32,
    tick: i32,
    vibrato_phase: i32,
    keyed_on: bool,
    stop_at: i32,  // note index to stop at (32 = whole sfx)
    no_loop: bool, // sub-range playback ignores the sfx's loop points
    // Custom (SFX-as-instrument) sub-sequencer: when the current note's custom
    // flag is set, instrument K = one of SFX 0..7 is played as a macro (its own
    // waveform + volume-envelope + pitch over its own ticks) under this note.
    custom_k: i32,    // -1 = not a custom note, else the custom SFX 0..7
    custom_pos: i32,  // position within the custom SFX
    custom_tick: i32, // tick within the custom SFX
    note_pitch: i32,  // the main note's pitch (custom pitch is relative to this)
    note_vol: i32,    // the main note's volume 0..7 (scales the custom envelope)
}
const CH0: Channel = Channel {
    sfx_id: -1,
    note_pos: 0,
    tick: 0,
    vibrato_phase: 0,
    keyed_on: false,
    stop_at: 32,
    no_loop: false,
    custom_k: -1,
    custom_pos: 0,
    custom_tick: 0,
    note_pitch: 0,
    note_vol: 0,
};

static mut AUDIO: AudioData = EMPTY_AUDIO;
static mut CHANNELS: [Channel; 8] = [CH0; 8];
static mut MUSIC_PATTERN: i32 = -1;
static mut MUSIC_LOOP: i32 = -1;
// Pattern length is set by the leftmost length-defining channel (PICO-8 rule),
// NOT by "any channel finished" -- looping channels never finish, so the old
// logic got stuck forever on patterns whose channels all loop.
static mut MUSIC_TICK: i32 = 0; // ticks elapsed in the current pattern
static mut MUSIC_LEN: i32 = 0; // total ticks for the current pattern
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
fn sfx_is_custom(n: u16) -> bool {
    (n >> 15) & 1 != 0 // bit 15: instrument field is a custom SFX (0..7), not an osc
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
        // Also release the phaser buddy (harmless if this note wasn't a phaser).
        Voice::key_off((1 << v) | (1 << (v + PHASER_BUDDY)));
        CHANNELS[v].keyed_on = false;
    }
    set_voice_noise(v, false);
}

/// Program voice `v` from the current step of its custom-instrument SFX: the
/// custom SFX supplies the oscillator, a volume envelope (scaling the main note's
/// volume) and a pitch relative to its own first note (added to the main pitch).
/// `keyon` re-triggers the sample (note start / waveform change); otherwise only
/// volume + pitch are updated, so the envelope sustains without clicks.
unsafe fn custom_set_voice(v: usize, keyon: bool) {
    let ch = CHANNELS[v];
    let k = ch.custom_k as usize;
    let cnote = AUDIO.sfx_notes[k][(ch.custom_pos & 31) as usize];
    let cvol = sfx_vol(cnote);
    if cvol == 0 {
        voice_key_off(v);
        return;
    }
    let cwave = sfx_instr(cnote);
    let base = sfx_pitch(AUDIO.sfx_notes[k][0]);
    let played = (ch.note_pitch + sfx_pitch(cnote) - base).clamp(0, 63);
    let spu_vol = (VOL_TABLE[ch.note_vol as usize] as i32 * cvol / 7) as i16;
    let spu_pitch = get_pitch(played, cwave);
    let voice = Voice::new(v as u8);
    if keyon {
        set_voice_noise(v, cwave == 6);
        if cwave == 6 {
            set_noise_freq(played);
        }
        voice.set_volume(Volume(spu_vol), Volume(spu_vol));
        voice.set_pitch(Pitch::raw(spu_pitch));
        voice.set_start_addr(SpuAddr::new(WAVEFORM_ADDR[(cwave & 7) as usize]));
        Voice::key_on(1 << v);
        CHANNELS[v].keyed_on = true;
    } else {
        if cwave == 6 {
            set_noise_freq(played);
        }
        voice.set_volume(Volume(spu_vol), Volume(spu_vol));
        voice.set_pitch(Pitch::raw(spu_pitch));
    }
}

/// Step a channel's custom-instrument macro one frame (independent of the main
/// note's tick). The macro runs at the custom SFX's own speed and loops.
unsafe fn advance_custom(v: usize) {
    if CHANNELS[v].sfx_id < 0 {
        CHANNELS[v].custom_k = -1;
        return;
    }
    if CHANNELS[v].custom_k < 0 {
        return;
    }
    let k = CHANNELS[v].custom_k as usize;
    let meta = AUDIO.sfx_meta[k];
    let speed = (meta[0] as i32).max(1);
    let threshold = speed * TICK_PER_SPEED;
    CHANNELS[v].custom_tick += TICK_INC;
    let mut stepped = false;
    while CHANNELS[v].custom_tick >= threshold {
        CHANNELS[v].custom_tick -= threshold;
        CHANNELS[v].custom_pos += 1;
        let ls = meta[1] as i32;
        let le = meta[2] as i32;
        if le > 0 && CHANNELS[v].custom_pos >= le {
            CHANNELS[v].custom_pos = ls; // loop the sustain region while held
        } else if CHANNELS[v].custom_pos >= 32 {
            CHANNELS[v].custom_pos = 31;
        }
        stepped = true;
    }
    if stepped {
        custom_set_voice(v, false);
    }
    custom_apply_effect(v);
}

/// Per-frame effect for the current custom-instrument step (its own notes carry
/// vibrato/fades -- e.g. the sustain of celeste2's pad instruments wobbles).
unsafe fn custom_apply_effect(v: usize) {
    let ch = CHANNELS[v];
    let k = ch.custom_k as usize;
    let cnote = AUDIO.sfx_notes[k][(ch.custom_pos & 31) as usize];
    let eff = sfx_effect(cnote);
    let cvol = sfx_vol(cnote);
    if cvol == 0 || eff == 0 {
        return;
    }
    let cwave = sfx_instr(cnote);
    let base = sfx_pitch(AUDIO.sfx_notes[k][0]);
    let played = (ch.note_pitch + sfx_pitch(cnote) - base).clamp(0, 63);
    let base_pitch = get_pitch(played, cwave) as i32;
    let base_vol = VOL_TABLE[ch.note_vol as usize] as i32 * cvol / 7;
    let total = (AUDIO.sfx_meta[k][0] as i32 * TICK_PER_SPEED).max(1);
    let t = ch.custom_tick;
    let voice = Voice::new(v as u8);
    match eff {
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
            let p = (base_pitch + (m * base_pitch) / 2048).clamp(1, 0x3FFF);
            voice.set_pitch(Pitch::raw(p as u16));
        }
        4 => {
            // fade in over the custom row
            let vv = (base_vol * t / total) as i16;
            voice.set_volume(Volume(vv), Volume(vv));
        }
        5 => {
            // fade out over the custom row
            let vv = (base_vol * (total - t) / total) as i16;
            voice.set_volume(Volume(vv), Volume(vv));
        }
        _ => {}
    }
}

unsafe fn start_channel_note(v: usize) {
    let ch = CHANNELS[v];
    let note = AUDIO.sfx_notes[ch.sfx_id as usize][ch.note_pos as usize];
    let vol = sfx_vol(note);
    if vol == 0 {
        voice_key_off(v);
        CHANNELS[v].custom_k = -1;
        return;
    }
    let instr = sfx_instr(note);
    if sfx_is_custom(note) {
        // Custom instrument: play SFX `instr` as a macro under this note.
        CHANNELS[v].custom_k = instr;
        CHANNELS[v].custom_pos = 0;
        CHANNELS[v].custom_tick = 0;
        CHANNELS[v].vibrato_phase = 0;
        CHANNELS[v].note_pitch = sfx_pitch(note);
        CHANNELS[v].note_vol = vol;
        voice_key_off(v);
        custom_set_voice(v, true);
        return;
    }
    CHANNELS[v].custom_k = -1;
    // Hardware noise: a 0.70 base (it reads ~1.4x hotter than a sample voice),
    // times a pitch-loudness factor -- PICO-8's noise gets ~2.6x louder from low
    // to high pitch (measured), so a flat level made the drums all the same.
    let spu_vol = if instr == 6 {
        let base = VOL_TABLE[vol as usize] as i32 * 45 / 64;
        let p = sfx_pitch(note);
        let pfac = 54 + (p * p) / 37; // /64: ~0.85 (low) .. ~2.4 (pitch 60)
        ((base * pfac) / 64) as i16
    } else {
        VOL_TABLE[vol as usize] as i16
    };
    let spu_pitch = get_pitch(sfx_pitch(note), instr);
    let addr = WAVEFORM_ADDR[(instr & 7) as usize];
    voice_key_off(v);
    // Instrument 6 = hardware LFSR noise (real hiss); all others = sample voice.
    set_voice_noise(v, instr == 6);
    if instr == 6 {
        set_noise_freq(sfx_pitch(note));
    }
    let voice = Voice::new(v as u8);
    if instr == 7 {
        // Phaser = two triangles (instrument 0) a hair apart (109/110), summed 2:1.
        // PICO-8's phaser is triangle-like (fundamental + odd harmonics) with the
        // two oscillators beating; a single static wavetable can't sweep, so we
        // key a detuned triangle buddy alongside the primary triangle.
        let tri = WAVEFORM_ADDR[0];
        let va = (spu_vol as i32 * 2 / 3) as i16;
        let vb = (spu_vol as i32 / 3) as i16;
        voice.set_volume(Volume(va), Volume(va));
        voice.set_pitch(Pitch::raw(spu_pitch));
        voice.set_start_addr(SpuAddr::new(tri));
        let buddy = v + PHASER_BUDDY;
        let bv = Voice::new(buddy as u8);
        bv.set_volume(Volume(vb), Volume(vb));
        bv.set_pitch(Pitch::raw(phaser_buddy_pitch(spu_pitch as i32, sfx_pitch(note))));
        bv.set_start_addr(SpuAddr::new(tri));
        Voice::key_on((1 << v) | (1 << buddy));
        CHANNELS[v].keyed_on = true;
        return;
    }
    voice.set_volume(Volume(spu_vol), Volume(spu_vol));
    voice.set_pitch(Pitch::raw(spu_pitch));
    voice.set_start_addr(SpuAddr::new(addr));
    Voice::key_on(1 << v);
    CHANNELS[v].keyed_on = true;
}

unsafe fn apply_effects(v: usize) {
    let ch = CHANNELS[v];
    if ch.sfx_id < 0 || ch.custom_k >= 0 {
        return; // custom-instrument notes are driven by advance_custom instead
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
    let phaser = instr == 7;
    // Effects set a new pitch OR volume; route both through the phaser's detuned
    // buddy voice too, or e.g. a fade-out leaves the buddy ringing (muffles the
    // percussion -- celeste1's kick is a phaser at pitch 1 with fade-out).
    let mut set_pitch: Option<i32> = None;
    let mut set_vol: Option<i32> = None;
    match effect {
        1 => {
            // slide (portamento): glide from the PREVIOUS note's pitch to this one.
            let from = if ch.note_pos > 0 {
                let pn = AUDIO.sfx_notes[ch.sfx_id as usize][(ch.note_pos - 1) as usize];
                get_pitch(sfx_pitch(pn), instr) as i32
            } else {
                base_pitch
            };
            set_pitch = Some(from + ((base_pitch - from) * t) / total);
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
            set_pitch = Some(base_pitch + (m * base_pitch) / 2048);
        }
        3 => {
            // drop
            set_pitch = Some((base_pitch * (total - t) / total).max(0));
        }
        4 => {
            // fade in
            set_vol = Some(VOL_TABLE[vol as usize] as i32 * t / total);
        }
        5 => {
            // fade out
            set_vol = Some(VOL_TABLE[vol as usize] as i32 * (total - t) / total);
        }
        6 | 7 => {
            // arpeggio: cycle the 4 notes of the current group (pos & ~3 .. +3),
            // holding each 4 (fast) or 8 (slow) PICO-8 ticks -- halved if speed<=8.
            let hold_base = if effect == 6 { 4 } else { 8 };
            let hold = if speed <= 8 { (hold_base / 2).max(1) } else { hold_base };
            let gtick = ch.note_pos * speed + t / TICK_PER_SPEED;
            let idx = (gtick / hold) % 4;
            let g = ((ch.note_pos & !3) + idx).clamp(0, 31);
            let an = AUDIO.sfx_notes[ch.sfx_id as usize][g as usize];
            set_pitch = Some(get_pitch(sfx_pitch(an), instr) as i32);
        }
        _ => {}
    }
    if let Some(p) = set_pitch {
        let p = p.clamp(1, 0x3FFF);
        voice.set_pitch(Pitch::raw(p as u16));
        if phaser {
            Voice::new((v + PHASER_BUDDY) as u8)
                .set_pitch(Pitch::raw(phaser_buddy_pitch(p, pitch_key)));
        }
    }
    if let Some(vv) = set_vol {
        if phaser {
            voice.set_volume(Volume((vv * 2 / 3) as i16), Volume((vv * 2 / 3) as i16));
            Voice::new((v + PHASER_BUDDY) as u8)
                .set_volume(Volume((vv / 3) as i16), Volume((vv / 3) as i16));
        } else {
            voice.set_volume(Volume(vv as i16), Volume(vv as i16));
        }
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
        if !CHANNELS[v].no_loop && loop_end > 0 && CHANNELS[v].note_pos >= loop_end {
            CHANNELS[v].note_pos = loop_start;
        }
        if CHANNELS[v].note_pos >= CHANNELS[v].stop_at {
            CHANNELS[v].sfx_id = -1;
            voice_key_off(v);
            return;
        }
        start_channel_note(v);
    }
    apply_effects(v);
    advance_custom(v);
}

/// Total ticks the current pattern lasts. PICO-8 rule: the leftmost active
/// channel whose SFX is *length-defining* sets it -- that's a non-looping sfx
/// (loop_end 0) or one whose loop spans the full 32 rows (loop_end >= 32).
/// Genuine sub-loops (0 < loop_end < 32) repeat to fill the pattern and are
/// skipped. Length = rows (32, or the LEN marker loop_start) * speed.
unsafe fn music_pattern_len() -> i32 {
    let pat = AUDIO.music_patterns[MUSIC_PATTERN as usize];
    let mut fallback = 0;
    for c in 0..4 {
        let chan = pat[1 + c];
        if chan & 0x80 != 0 {
            continue; // disabled channel
        }
        let meta = AUDIO.sfx_meta[(chan & 0x3F) as usize];
        let speed = (meta[0] as i32).max(1);
        let loop_start = meta[1] as i32;
        let loop_end = meta[2] as i32;
        if fallback == 0 {
            fallback = 32 * speed * TICK_PER_SPEED;
        }
        if loop_end > 0 && loop_end < 32 {
            continue; // a sub-loop never defines length
        }
        let rows = if loop_end == 0 && loop_start > 0 { loop_start } else { 32 };
        return rows * speed * TICK_PER_SPEED;
    }
    if fallback > 0 { fallback } else { 32 * TICK_PER_SPEED }
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
        CHANNELS[c].stop_at = 32;
        CHANNELS[c].no_loop = false;
        start_channel_note(c);
    }
    MUSIC_TICK = 0;
    MUSIC_LEN = music_pattern_len();
}

// ---- public API ----

/// Initialise the SPU and load the game's [`AudioData`]. Call once after boot.
pub fn init(audio: AudioData) {
    unsafe {
        AUDIO = audio;
        spu::init();
        // Full main volume; per-note levels (VOL_TABLE) keep a 4-voice mix
        // from clipping. (The emulator's SPU output is pre-main-volume.)
        spu::set_main_volume(Volume(0x3FFF), Volume(0x3FFF));
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
            MUSIC_TICK += TICK_INC;
            if MUSIC_TICK >= MUSIC_LEN {
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

/// PICO-8 `sfx(id)` (id < 0 stops all SFX voices). Plays the whole sfx.
pub fn play(id: i32) {
    play_range(id, 0, 32);
}

/// PICO-8 `sfx(id, _, offset, length)`: play notes `[offset, offset+length)`
/// of sfx `id` on a free SFX voice (looping disabled for sub-ranges).
pub fn play_range(id: i32, offset: i32, length: i32) {
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
        CHANNELS[v].note_pos = offset.clamp(0, 31);
        CHANNELS[v].tick = 0;
        CHANNELS[v].vibrato_phase = 0;
        CHANNELS[v].stop_at = (offset + length).clamp(1, 32);
        CHANNELS[v].no_loop = length < 32; // sub-range plays once
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
