//! Collection settings on the PS1 memory card (via `psx-mc`).
//!
//! One tiny save file persists the player options that outlive a session:
//! SFX volume, music volume, pixel scale (1x/2x) and the borders preset.
//! (Berry/death counters are per-run in both carts and reset by the games
//! themselves, so they are deliberately not saved.)
//!
//! Policy, sized for a shipped product:
//! - [`load`] runs once at boot. A missing, unformatted or foreign card is
//!   never an error: every failure path silently keeps the defaults.
//! - [`save`] runs when a settings menu closes with a change pending, never
//!   per slider tick (card writes are slow). It compares against the last
//!   payload synced with the card and skips the write when nothing differs.
//! - The card is formatted only when it affirmatively reports unformatted
//!   AND the player is saving; read errors never trigger a format.

use crate::{backend, sfx};
use psx_mc::{Card, HardwareCard, Slot};

/// BIOS file name (region + product code + label). PS1 names cap at 20
/// chars, so the label is CELSTCC1 (CELESTE CLASSIC COLLECTION, slot 1).
const FILE_NAME: &str = "BESLES-00000CELSTCC1";
/// Human-readable title shown by the console's memory-card manager.
const TITLE: &str = "CELESTE COLLECTION";

/// Payload magic + layout version. Bump the digit if the layout changes.
const MAGIC: [u8; 4] = *b"CCS1";
/// magic(4) + sfx vol + music vol + pixel scale + borders preset.
const PAYLOAD_LEN: usize = 8;

/// The payload as last synced with the card (loaded or written), so [`save`]
/// can skip the slow write when the live settings already match. `None` until
/// a load or write succeeds.
static mut SYNCED: Option<[u8; PAYLOAD_LEN]> = None;

/// The live settings, encoded as the on-card payload.
fn snapshot() -> [u8; PAYLOAD_LEN] {
    let mut p = [0u8; PAYLOAD_LEN];
    p[0..4].copy_from_slice(&MAGIC);
    p[4] = sfx::sfx_volume() as u8;
    p[5] = sfx::music_volume() as u8;
    p[6] = backend::pixel_scale() as u8;
    p[7] = backend::side_preset();
    p
}

/// Load the settings file and apply it. Call once at boot, before the menu.
/// Any failure (no card, unformatted, no file, bad payload) keeps defaults.
pub fn load() {
    let mut card = Card::new(HardwareCard::new(Slot::One));
    let mut buf = [0u8; 128];
    let Ok(len) = card.read(FILE_NAME, &mut buf) else {
        return;
    };
    if len < PAYLOAD_LEN || buf[0..4] != MAGIC {
        return;
    }
    // Every setter clamps/wraps on its own, so a stale payload from a future
    // layout can nudge a setting but never wedge the game.
    sfx::set_sfx_volume(buf[4] as u16); // clamps to 0..=8
    sfx::set_music_volume(buf[5] as u16); // clamps to 0..=8
    backend::set_pixel_scale(buf[6] as i16); // clamps to 1|2
    backend::set_side_preset(buf[7]); // wraps % preset count
    unsafe { SYNCED = Some(snapshot()) };
}

/// Write the settings file if the live settings differ from the card's.
/// Call when a settings menu closes after a change; failures are silent
/// (the settings simply stay RAM-only for this session).
pub fn save() {
    let snap = snapshot();
    if unsafe { SYNCED } == Some(snap) {
        return;
    }
    let mut card = Card::new(HardwareCard::new(Slot::One));
    match card.is_formatted() {
        Ok(true) => {}
        // Fresh card + an explicit player save: the one case we format.
        Ok(false) => {
            if card.format().is_err() {
                return;
            }
        }
        // No card / unreadable: don't format, don't insist.
        Err(_) => return,
    }
    if card.write(FILE_NAME, TITLE, &snap).is_ok() {
        unsafe { SYNCED = Some(snap) };
    }
}
