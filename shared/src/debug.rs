//! Debug toggles shared between the in-game pause menu and the games.
//!
//! Just "fly mode" for now: when enabled (from the pause menu), holding the
//! game's fly button moves the player freely with no gravity, collision, or
//! death, for exploring levels. The game reads [`fly_enabled`]; the menu flips
//! it. The flag is process-global and persists across pause/resume.

static mut FLY: bool = false;

/// Is debug fly mode enabled?
#[inline]
pub fn fly_enabled() -> bool {
    unsafe { FLY }
}

/// Enable or disable debug fly mode.
#[inline]
pub fn set_fly(on: bool) {
    unsafe { FLY = on }
}

/// Flip debug fly mode.
#[inline]
pub fn toggle_fly() {
    unsafe { FLY = !FLY }
}
