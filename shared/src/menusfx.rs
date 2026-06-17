//! One-shot UI sound effects for the menus, separate from the PICO-8 music/SFX
//! engine. The sounds are CC0 (Kenney "Interface Sounds", public domain),
//! resampled to 22050 Hz mono and stored as one-shot PS1 SPU ADPCM (see
//! `tools/gen_menu_sfx.py`). They play on dedicated voices 16..18 -- outside the
//! 0..15 the [`crate::sfx`] engine uses -- so the launcher and the in-game pause
//! overlay can fire UI sounds without disturbing the game's audio.

use psx_spu as spu;
use spu::{Adsr, Pitch, SpuAddr, Voice, Volume};

include!("menu_sfx_data.rs"); // MENU_SFX_ADPCM, MENU_SFX, SFX_NAV / CONFIRM / TRANSITION

const SPU_BASE: u32 = 0x4000; // clear of the PICO-8 waveforms (0x1000..~0x1900)
const PITCH_22K: u16 = 0x0800; // a 22050 Hz sample on the 44100 Hz SPU
const VOL_BASE: i16 = 0x2800; // pre-scale level (scaled by the SFX-volume setting)
const VOICES: [u8; 3] = [16, 17, 18]; // round-robin so quick sounds don't cut each other

static mut NEXT: usize = 0;

/// Upload the menu SFX bank to SPU RAM. Call once after `crate::sfx::init`
/// (which does `spu::init`); re-call if the SPU is re-initialised (e.g. the
/// launcher after returning from a game).
pub fn init() {
    spu::upload_adpcm(SpuAddr::new(SPU_BASE), &MENU_SFX_ADPCM);
}

/// Fire menu sound `id` (`SFX_NAV` / `SFX_CONFIRM` / `SFX_TRANSITION`). Volume is
/// scaled by the shared SFX-volume setting, so the pause slider affects it.
pub fn play(id: usize) {
    if id >= MENU_SFX.len() {
        return;
    }
    unsafe {
        let off = MENU_SFX[id].0;
        let v = VOICES[NEXT % VOICES.len()];
        NEXT = NEXT.wrapping_add(1);
        let vol = (VOL_BASE as i32 * crate::sfx::sfx_volume() as i32 / 8) as i16;
        let voice = Voice::new(v);
        voice.set_volume(Volume(vol), Volume(vol));
        voice.set_pitch(Pitch::raw(PITCH_22K));
        voice.set_start_addr(SpuAddr::new(SPU_BASE + off));
        // instant attack, full sustain; the ADPCM end flag (0x01) mutes the voice.
        voice.set_adsr(Adsr { lower: 0x000F, upper: 0x0000 });
        Voice::key_on(1 << v);
    }
}
