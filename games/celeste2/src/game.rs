//! Native Rust port of "Celeste 2: Lani's Trek" (PICO-8) game logic.
//!
//! Prototype-based object engine flattened to an enum-dispatched fixed pool
//! (like the Celeste Classic port). Runs at 60fps with the Celeste 1 method:
//! PICO-8 velocity/impulse magnitudes are kept at face value, accelerations
//! are pre-halved (e.g. PICO-8 0.2 -> 0.1), the per-frame velocity `move`s
//! bake in the x0.5 (one-time position snaps use the `*_exact` variants), and
//! frame-count timers are doubled.
//!
//! Ported: object engine, the player (run/jump/wall-jump + the full grapple
//! state machine), the object types (spikes, grapple pickup, grappler, berry,
//! crumble, bridge, snowball, springboard, checkpoint, spawners), hazards +
//! death + level restart, and a follow camera. Level data is the cart's
//! level-1 tilemap for now; PX9 multi-level streaming layers on top.

#![allow(static_mut_refs)]

use crate::assets::gfx::GFX_DATA;
use crate::assets::levels::{LEVELS, LevelMeta};
use crate::assets::tilemap::TILE_FLAGS;
use pico8::backend::{self, Cart};
use pico8::fixed::{fx, Fix32};
use pico8::rng;
use pico8::sfx;
use pico8::util::{approach, clamp, sign};

#[inline]
fn fi(n: i32) -> Fix32 {
    Fix32::from_int(n)
}
const HALF: Fix32 = fx(0.5);

// Active level dims (set by goto_level from the PX9 LEVELS table).
static mut LVL_W: i32 = 96;
static mut LVL_H: i32 = 16;
static mut LVL_INDEX: i32 = 1;
static mut CUR_MUSIC: i32 = -1;
// Mutable scratch tilemap (largest level is 128x32); the active cart points
// here so spawn-marker tiles can be blanked from the render.
static mut LEVEL_BUF: [u8; 4096] = [0; 4096];

// Input bit layout from lib.rs (PICO-8 button order; jump/grapple last).
pub const IN_LEFT: u8 = 1 << 0;
pub const IN_RIGHT: u8 = 1 << 1;
pub const IN_UP: u8 = 1 << 2;
pub const IN_DOWN: u8 = 1 << 3;
pub const IN_JUMP: u8 = 1 << 4;
pub const IN_GRAPPLE: u8 = 1 << 5;

// ---- input state (mirrors update_input / consume_*_press) ----
static mut RAW: u8 = 0;
static mut INPUT_X: i32 = 0;
static mut AXIS_X: i32 = 0;
static mut AXIS_TURNED: bool = false;
static mut JUMP_HELD: bool = false;
static mut JUMP_PRESSED: i32 = 0;
static mut GRAP_HELD: bool = false;
static mut GRAP_PRESSED: i32 = 0;

#[inline]
fn held(b: u8) -> bool {
    unsafe { RAW & b != 0 }
}

pub fn set_input(mask: u8) {
    unsafe { RAW = mask };
}

fn update_input() {
    unsafe {
        let prev_x = AXIS_X;
        if held(IN_LEFT) {
            if held(IN_RIGHT) {
                if AXIS_TURNED {
                    AXIS_X = prev_x;
                    INPUT_X = prev_x;
                } else {
                    AXIS_TURNED = true;
                    AXIS_X = -prev_x;
                    INPUT_X = -prev_x;
                }
            } else {
                AXIS_TURNED = false;
                AXIS_X = -1;
                INPUT_X = -1;
            }
        } else if held(IN_RIGHT) {
            AXIS_TURNED = false;
            AXIS_X = 1;
            INPUT_X = 1;
        } else {
            AXIS_TURNED = false;
            AXIS_X = 0;
            INPUT_X = 0;
        }

        // Input buffers: PICO-8 sets these to 4 at 30fps; at 60fps the window is
        // 8 frames (a buffer of 4 here halved the real-time grace and made jumps
        // feel unresponsive).
        let jump = held(IN_JUMP);
        JUMP_PRESSED = if jump && !JUMP_HELD { 8 } else if jump { (JUMP_PRESSED - 1).max(0) } else { 0 };
        JUMP_HELD = jump;

        let grap = held(IN_GRAPPLE);
        GRAP_PRESSED = if grap && !GRAP_HELD { 8 } else if grap { (GRAP_PRESSED - 1).max(0) } else { 0 };
        GRAP_HELD = grap;
    }
}

fn consume_jump_press() -> bool {
    unsafe {
        let v = JUMP_PRESSED > 0;
        JUMP_PRESSED = 0;
        v
    }
}
fn consume_grapple_press() -> bool {
    unsafe {
        let v = GRAP_PRESSED > 0;
        GRAP_PRESSED = 0;
        v
    }
}

// ---- audio: PICO-8 psfx(id, off, len[, lock]) with the sfx_timer debounce ----
static mut SFX_TIMER: i32 = 0;
fn psfx(id: i32, off: i32, len: i32) {
    psfx_lock(id, off, len, 0);
}
fn psfx_lock(id: i32, off: i32, len: i32, lock: i32) {
    unsafe {
        if SFX_TIMER <= 0 || lock > 0 {
            sfx::play_range(id, off, len);
            if lock > 0 {
                SFX_TIMER = lock;
            }
        }
    }
}
fn music(n: i32) {
    sfx::music(n, 0, 0);
}

// ---- level access ----
#[inline]
fn tile_at(x: i32, y: i32) -> i32 {
    unsafe {
        if x < 0 || y < 0 || x >= LVL_W || y >= LVL_H {
            return 0;
        }
    }
    backend::mget(x, y)
}
#[inline]
fn solid_tile(x: i32, y: i32) -> bool {
    backend::fget(tile_at(x, y), 1)
}

// ====================================================================
// Object engine
// ====================================================================

#[derive(Clone, Copy, PartialEq)]
enum ObjType {
    None,
    Player,
    GrapplePickup,
    SpikeV,
    SpikeH,
    Snowball,
    Springboard,
    Grappler,
    Bridge,
    Berry,
    Crumble,
    Checkpoint,
    SpawnerR,
    SpawnerL,
}

#[derive(Clone, Copy)]
struct Vec2 {
    x: Fix32,
    y: Fix32,
}
const VZ: Vec2 = Vec2 { x: Fix32::ZERO, y: Fix32::ZERO };

#[derive(Clone, Copy)]
struct Obj {
    exists: bool,
    destroyed: bool,
    otype: ObjType,
    x: Fix32,
    y: Fix32,
    spd: Vec2,
    rem: Vec2,
    hit_x: Fix32,
    hit_y: Fix32,
    hit_w: Fix32,
    hit_h: Fix32,
    facing: i32,
    spr: Fix32,
    flip_x: bool,
    flip_y: bool,
    solid: bool,
    grapple_mode: i32,
    hazard: i32,
    held: bool,
    freeze: i32,
    // generic timer / hp (reused per type)
    timer: i32,
    hp: i32,
    stop: bool,
    falling: bool,
    breaking: bool,
    ox: Fix32,
    oy: Fix32,
    rdir: i32, // spawner direction
    link: usize, // reference to another object (berry.player), MAX = none
    // player
    state: i32,
    t_jump_grace: i32,
    jump_grace_y: Fix32,
    t_var_jump: i32,
    var_jump_speed: Fix32,
    auto_var_jump: bool,
    grapple_x: Fix32,
    grapple_y: Fix32,
    grapple_dir: i32,
    grapple_hit: usize,
    grapple_wave: Fix32,
    grapple_boost: bool,
    t_grapple_cooldown: i32,
    grapple_retract: bool,
    holding: usize,
    t_grapple_jump_grace: i32,
    grapple_jump_grace_y: Fix32,
    t_grapple_pickup: i32,
    wipe_timer: i32,
    oid: i32, // spawn id = level*100 + tx + ty*128 (checkpoint/berry persistence)
    flash: i32, // berry pickup flash-ring timer
}

const NONE: usize = usize::MAX;

const OBJ0: Obj = Obj {
    exists: false,
    destroyed: false,
    otype: ObjType::None,
    x: Fix32::ZERO,
    y: Fix32::ZERO,
    spd: VZ,
    rem: VZ,
    hit_x: Fix32::ZERO,
    hit_y: Fix32::ZERO,
    hit_w: fx(8.0),
    hit_h: fx(8.0),
    facing: 1,
    spr: Fix32::ZERO,
    flip_x: false,
    flip_y: false,
    solid: false,
    grapple_mode: 0,
    hazard: 0,
    held: false,
    freeze: 0,
    timer: 0,
    hp: 0,
    stop: false,
    falling: false,
    breaking: false,
    ox: Fix32::ZERO,
    oy: Fix32::ZERO,
    rdir: 0,
    link: NONE,
    state: 0,
    t_jump_grace: 0,
    jump_grace_y: Fix32::ZERO,
    t_var_jump: 0,
    var_jump_speed: Fix32::ZERO,
    auto_var_jump: false,
    grapple_x: Fix32::ZERO,
    grapple_y: Fix32::ZERO,
    grapple_dir: 0,
    grapple_hit: NONE,
    grapple_wave: Fix32::ZERO,
    grapple_boost: false,
    t_grapple_cooldown: 0,
    grapple_retract: false,
    holding: NONE,
    t_grapple_jump_grace: 0,
    grapple_jump_grace_y: Fix32::ZERO,
    t_grapple_pickup: 0,
    wipe_timer: 0,
    oid: -1,
    flash: 0,
};

const MAX_OBJ: usize = 48;
static mut OBJ: [Obj; MAX_OBJ] = [OBJ0; MAX_OBJ];
static mut PLAYER: usize = NONE;
static mut HAVE_GRAPPLE: bool = false;
// Active checkpoint oid (-1 = none); reset per level. The player respawns here.
static mut LEVEL_CHECKPOINT: i32 = -1;
// Slot of the most-recently-grabbed berry, so grabbing a new one deposits the old.
static mut LAST_BERRY: usize = NONE;
// Deposited-berry oids for the whole run, so collected berries don't respawn or
// re-count on a level restart/re-entry.
static mut COLLECTED: [i32; 32] = [-1; 32];
static mut COLLECTED_N: usize = 0;
static mut FREEZE: i32 = 0;
static mut SHAKE: i32 = 0;
static mut CAM_X: i32 = 0;
static mut CAM_Y: i32 = 0;
static mut CAM_MODE: i32 = 1;
static mut C_OFFSET: i32 = 0; // camera mode 6/7 follow offset
static mut C_FLAG: bool = false; // camera mode 6 latch
static mut FRAMES: i32 = 0; // for time()-driven wobble (and the run timer)
// Run timer + run stats (HH:MM:SS HUD, end score panel). TIMER_F counts rendered
// frames toward a second (FRAMES itself is the time() wobble clock, kept separate).
static mut TIMER_F: i32 = 0;
static mut SECONDS: i32 = 0;
static mut MINUTES: i32 = 0;
static mut BERRY_COUNT: i32 = 0;
static mut DEATH_COUNT: i32 = 0;
static mut SHOW_SCORE: i32 = -1; // <0 = inactive; ramps up on the finale
// Title / level-intro / fade state.
static mut TITLE_FLASH: i32 = i32::MIN; // MIN = not started; counts down once set
static mut LEVEL_INTRO: i32 = 0;
static mut INFADE: i32 = 60; // level-entry wipe (counts up to 60)
static mut SCARF: [Vec2; 5] = [VZ; 5];

// Background particles: drifting snow + parallax clouds (26 each).
#[derive(Clone, Copy)]
struct Cloud {
    x: Fix32,
    y: Fix32,
    s: Fix32,
}
const CLOUD0: Cloud = Cloud { x: Fix32::ZERO, y: Fix32::ZERO, s: Fix32::ZERO };
static mut SNOW: [Vec2; 26] = [VZ; 26];
static mut CLOUDS: [Cloud; 26] = [CLOUD0; 26];

unsafe fn init_particles() {
    for i in 0..26 {
        SNOW[i] = Vec2 { x: rng::rnd(fi(132)), y: rng::rnd(fi(132)) };
        CLOUDS[i] = Cloud {
            x: rng::rnd(fi(132)),
            y: rng::rnd(fi(132)),
            s: fi(16) + rng::rnd(fi(32)),
        };
    }
}

/// Parallax clouds. Background pass: draw_clouds(1, 0, 1, level.clouds, 26).
/// Foreground fog pass: draw_clouds(1.25, height*8+1, 0, 7, 16) -- pinned to the
/// level's bottom (sy=0). (The PICO-8 clip that flattens each cloud's bottom and
/// the fillp fog dither aren't available on the PSX backend; clouds are full
/// circles and the fog is solid.)
unsafe fn draw_clouds(scale: Fix32, oy: Fix32, sy: Fix32, color: i32, count: usize) {
    for i in 0..count {
        let mut c = CLOUDS[i];
        let s = c.s * scale;
        let x = fi(CAM_X) + (c.x - fi(CAM_X) * fx(0.9)).rem_floor(fi(128) + s) - s / fi(2);
        let y = oy + (fi(CAM_Y) + (c.y - fi(CAM_Y) * fx(0.9)).rem_floor(fi(128) + s / fi(2))) * sy;
        let (xi, yi) = (x.to_int() as i16, y.to_int() as i16);
        backend::circfill(xi, yi, (s / fi(3)).to_int() as i16, color);
        if i % 2 == 0 {
            backend::circfill((x - s / fi(3)).to_int() as i16, yi, (s / fi(5)).to_int() as i16, color);
            backend::circfill((x + s / fi(3)).to_int() as i16, yi, (s / fi(6)).to_int() as i16, color);
        }
        c.x += fi(4 - (i as i32) % 4) * fx(0.25) * HALF;
        CLOUDS[i] = c;
    }
}

/// Apply a level's palette swap (matches the `pal` closures in the level table).
unsafe fn apply_level_pal(pal_id: i32) {
    match pal_id {
        1 => { backend::pal(2, 12); backend::pal(5, 2); }
        2 => { backend::pal(2, 14); backend::pal(5, 2); }
        3 => { backend::pal(2, 1); backend::pal(7, 11); }
        _ => {}
    }
}

#[inline]
unsafe fn level() -> &'static LevelMeta {
    &LEVELS[LVL_INDEX as usize]
}

/// Drifting snow (drawn over the scene).
unsafe fn draw_snow() {
    let t = fi(FRAMES) / fi(60);
    for i in 0..26 {
        let mut s = SNOW[i];
        let px = fi(CAM_X) + (s.x - fi(CAM_X) * HALF).rem_floor(fi(132)) - fi(2);
        let py = fi(CAM_Y) + (s.y - fi(CAM_Y) * HALF).rem_floor(fi(132));
        backend::circfill(px.to_int() as i16, py.to_int() as i16, (i as i32 % 2) as i16, 7);
        s.x += fi(4 - (i as i32) % 4) * HALF;
        s.y += (t * fx(0.25) + fi(i as i32) * fx(0.1)).sin() * HALF; // 60fps: halved (matches x)
        SNOW[i] = s;
    }
}

fn new_slot() -> usize {
    unsafe {
        for i in 0..MAX_OBJ {
            if !OBJ[i].exists {
                return i;
            }
        }
        NONE
    }
}

fn create(otype: ObjType, x: i32, y: i32) -> usize {
    unsafe {
        let i = new_slot();
        if i == NONE {
            return NONE;
        }
        OBJ[i] = OBJ0;
        OBJ[i].exists = true;
        OBJ[i].otype = otype;
        OBJ[i].x = fi(x);
        OBJ[i].y = fi(y);
        OBJ[i].oid = LVL_INDEX * 100 + (x / 8) + (y / 8) * 128;
        OBJ[i].spr = fi(type_spr(otype));
        init_obj(i);
        i
    }
}

unsafe fn is_collected(id: i32) -> bool {
    COLLECTED[..COLLECTED_N].iter().any(|&c| c == id)
}
unsafe fn mark_collected(id: i32) {
    if COLLECTED_N < COLLECTED.len() && !is_collected(id) {
        COLLECTED[COLLECTED_N] = id;
        COLLECTED_N += 1;
    }
}

fn type_spr(t: ObjType) -> i32 {
    match t {
        ObjType::Player => 2,
        ObjType::GrapplePickup => 20,
        ObjType::SpikeV => 36,
        ObjType::SpikeH => 37,
        ObjType::Snowball => 62,
        ObjType::Springboard => 11,
        ObjType::Grappler => 46,
        ObjType::Bridge => 63,
        ObjType::Berry => 21,
        ObjType::Crumble => 19,
        ObjType::Checkpoint => 13,
        ObjType::SpawnerR => 14,
        ObjType::SpawnerL => 15,
        ObjType::None => 0,
    }
}

fn type_of_tile(tile: i32) -> ObjType {
    match tile {
        2 => ObjType::Player,
        20 => ObjType::GrapplePickup,
        36 => ObjType::SpikeV,
        37 => ObjType::SpikeH,
        62 => ObjType::Snowball,
        11 => ObjType::Springboard,
        46 => ObjType::Grappler,
        63 => ObjType::Bridge,
        21 => ObjType::Berry,
        19 => ObjType::Crumble,
        13 => ObjType::Checkpoint,
        14 => ObjType::SpawnerR,
        15 => ObjType::SpawnerL,
        _ => ObjType::None,
    }
}

// --- geometry / collision ---

unsafe fn overlaps(a: usize, b: usize, ox: Fix32, oy: Fix32) -> bool {
    if a == b || !OBJ[b].exists || OBJ[b].destroyed {
        return false;
    }
    let (oa, obb) = (&OBJ[a], &OBJ[b]);
    ox + oa.x + oa.hit_x + oa.hit_w > obb.x + obb.hit_x
        && oy + oa.y + oa.hit_y + oa.hit_h > obb.y + obb.hit_y
        && ox + oa.x + oa.hit_x < obb.x + obb.hit_x + obb.hit_w
        && oy + oa.y + oa.hit_y < obb.y + obb.hit_y + obb.hit_h
}

unsafe fn contains(o: usize, px: Fix32, py: Fix32) -> bool {
    let a = &OBJ[o];
    // snowball has a custom (taller) contains box
    if a.otype == ObjType::Snowball {
        return px >= a.x && px < a.x + fi(8) && py >= a.y - fi(1) && py < a.y + fi(10);
    }
    px >= a.x + a.hit_x
        && px < a.x + a.hit_x + a.hit_w
        && py >= a.y + a.hit_y
        && py < a.y + a.hit_y + a.hit_h
}

unsafe fn check_solid(i: usize, ox: Fix32, oy: Fix32) -> bool {
    let o = &OBJ[i];
    let lx = ((ox + o.x + o.hit_x) / fi(8)).floor_int();
    let rx = ((ox + o.x + o.hit_x + o.hit_w - fi(1)) / fi(8)).floor_int();
    let ty = ((oy + o.y + o.hit_y) / fi(8)).floor_int();
    let by = ((oy + o.y + o.hit_y + o.hit_h - fi(1)) / fi(8)).floor_int();
    let mut tx = lx;
    while tx <= rx {
        let mut tyy = ty;
        while tyy <= by {
            if solid_tile(tx, tyy) {
                return true;
            }
            tyy += 1;
        }
        tx += 1;
    }
    for j in 0..MAX_OBJ {
        if OBJ[j].exists && OBJ[j].solid && j != i && !OBJ[j].destroyed && overlaps(i, j, ox, oy) {
            return true;
        }
    }
    false
}

// On-collide kind: how the moving object reacts to a wall.
#[derive(Clone, Copy, PartialEq)]
enum Collide {
    None,       // pass through (no stop)
    Stop,       // zero remainder + speed (base object)
    Player,     // player corner-correct then stop
    SnowballX,  // snowball: corner-correct over a lip, else hurt, else bounce back
    PullX,      // grappled-object pull: corner-correct around a corner, else end pull
}

unsafe fn move_x(i: usize, amount: Fix32, c: Collide) -> bool {
    do_move(i, amount * HALF, true, c)
}
unsafe fn move_y(i: usize, amount: Fix32, c: Collide) -> bool {
    do_move(i, amount * HALF, false, c)
}
// One-time position snap (full amount, no 60fps halving). Uses Collide::None,
// matching PICO-8's handler-less move_x/move_y: a snap that hits a wall stops
// advancing but must NOT zero speed_x/speed_y. (Collide::Stop would wipe the
// velocity -- e.g. wall_jump sets speed_x=3*dir then snaps move_x(-dir*3) INTO
// the wall, so Stop killed the entire horizontal launch.)
unsafe fn move_x_exact(i: usize, amount: Fix32) {
    do_move(i, amount, true, Collide::None);
}
unsafe fn move_y_exact(i: usize, amount: Fix32) {
    do_move(i, amount, false, Collide::None);
}

unsafe fn do_move(i: usize, amount: Fix32, horiz: bool, c: Collide) -> bool {
    if horiz {
        OBJ[i].rem.x += amount;
    } else {
        OBJ[i].rem.y += amount;
    }
    let r = if horiz { OBJ[i].rem.x } else { OBJ[i].rem.y };
    let mut m = (r + HALF).floor_int();
    if horiz {
        OBJ[i].rem.x -= fi(m);
    } else {
        OBJ[i].rem.y -= fi(m);
    }
    let step = if m > 0 { 1 } else { -1 };
    while m != 0 {
        let blocked = if horiz {
            check_solid(i, fi(step), Fix32::ZERO)
        } else {
            check_solid(i, Fix32::ZERO, fi(step))
        };
        if blocked {
            return on_collide(i, horiz, step, c);
        }
        if horiz {
            OBJ[i].x += fi(step);
        } else {
            OBJ[i].y += fi(step);
        }
        m -= step;
    }
    false
}

unsafe fn on_collide(i: usize, horiz: bool, step: i32, c: Collide) -> bool {
    match c {
        Collide::None => {
            // glide along the wall (no stop) -- still report blocked
            true
        }
        Collide::Stop => {
            if horiz {
                OBJ[i].rem.x = Fix32::ZERO;
                OBJ[i].spd.x = Fix32::ZERO;
            } else {
                OBJ[i].rem.y = Fix32::ZERO;
                OBJ[i].spd.y = Fix32::ZERO;
            }
            true
        }
        Collide::Player => player_on_collide(i, horiz, step),
        Collide::SnowballX => snowball_on_collide_x(i),
        Collide::PullX => pull_on_collide_x(i, step),
    }
}

/// snowball.on_collide_x: try to slide over a small lip (corner_correct); else lose
/// 1 HP (and stop if that kills it); else bounce back off the wall. spd.x is still
/// the pre-collision velocity here, so negating it reverses direction.
unsafe fn snowball_on_collide_x(i: usize) -> bool {
    let s = sign(OBJ[i].spd.x).to_int();
    if corner_correct(i, s, 0, 2, 2, 1, false) {
        return false; // slid over the lip, keep rolling
    }
    if snowball_hurt(i) {
        return true; // lost its last HP -> destroyed
    }
    OBJ[i].spd.x = -OBJ[i].spd.x;
    OBJ[i].rem.x = Fix32::ZERO;
    OBJ[i].freeze = 2; // PICO-8 freeze=1 doubled for 60fps
    psfx(17, 0, 2);
    true
}

/// pull_collide_x: a grappled object being pulled corner-corrects around a snag and
/// keeps coming; only a real wall (corner_correct fails) ends the pull. `step` is
/// sgn(pull amount) == the Lua sgn(target).
unsafe fn pull_on_collide_x(i: usize, step: i32) -> bool {
    !corner_correct(i, step, 0, 4, 2, 0, false)
}

/// corner_correct (simplified to the cases the player uses): try to nudge
/// around a blocking corner. `func` = avoid hazards.
unsafe fn corner_correct(i: usize, dx: i32, dy: i32, side: i32, look: i32, only_sign: i32, avoid_hazard: bool) -> bool {
    if dx != 0 {
        let mut k = 1;
        while k <= side {
            let mut s = 1;
            while s >= -1 {
                if s != 0 && s != -only_sign {
                    let off_y = fi(k * s);
                    if !check_solid(i, fi(dx * look), off_y)
                        && (!avoid_hazard || !hazard_check(i, fi(dx), off_y))
                    {
                        OBJ[i].x += fi(dx);
                        OBJ[i].y += off_y;
                        return true;
                    }
                }
                s -= 2;
            }
            k += 1;
        }
    } else if dy != 0 {
        let mut k = 1;
        while k <= side {
            let mut s = 1;
            while s >= -1 {
                if s != 0 && s != -only_sign {
                    let off_x = fi(k * s);
                    if !check_solid(i, off_x, fi(dy * look))
                        && (!avoid_hazard || !hazard_check(i, off_x, fi(dy)))
                    {
                        OBJ[i].x += off_x;
                        OBJ[i].y += fi(dy);
                        return true;
                    }
                }
                s -= 2;
            }
            k += 1;
        }
    }
    false
}

// ====================================================================
// Hazards / death
// ====================================================================

unsafe fn hazard_check(i: usize, ox: Fix32, oy: Fix32) -> bool {
    for j in 0..MAX_OBJ {
        if !OBJ[j].exists || OBJ[j].hazard == 0 || OBJ[j].destroyed {
            continue;
        }
        if overlaps(i, j, ox, oy) {
            let h = OBJ[j].hazard;
            let sy = OBJ[i].spd.y;
            let sx = OBJ[i].spd.x;
            let hit = match h {
                1 => true,
                2 => sy >= Fix32::ZERO,
                3 => sy <= Fix32::ZERO,
                4 => sx <= Fix32::ZERO,
                5 => sx >= Fix32::ZERO,
                _ => false,
            };
            if hit {
                return true;
            }
        }
    }
    false
}

unsafe fn player_die(i: usize) {
    OBJ[i].state = 99;
    FREEZE = 2;
    SHAKE = 5;
    DEATH_COUNT += 1;
    psfx_lock(14, 16, 16, 240);
}

// ====================================================================
// Init per type
// ====================================================================

unsafe fn init_obj(i: usize) {
    match OBJ[i].otype {
        ObjType::Player => {
            OBJ[i].x += fi(4);
            OBJ[i].y += fi(8);
            OBJ[i].hit_x = fi(-3);
            OBJ[i].hit_y = fi(-6);
            OBJ[i].hit_w = fi(6);
            OBJ[i].hit_h = fi(6);
            OBJ[i].spr = fi(2);
            PLAYER = i;
            SCARF = [Vec2 { x: OBJ[i].x, y: OBJ[i].y }; 5];
        }
        ObjType::SpikeV => {
            if !check_solid(i, Fix32::ZERO, fi(1)) {
                OBJ[i].flip_y = true;
                OBJ[i].hazard = 3;
            } else {
                OBJ[i].hit_y = fi(5);
                OBJ[i].hazard = 2;
            }
            OBJ[i].hit_h = fi(3);
        }
        ObjType::SpikeH => {
            if check_solid(i, fi(-1), Fix32::ZERO) {
                OBJ[i].flip_x = true;
                OBJ[i].hazard = 4;
            } else {
                OBJ[i].hit_x = fi(5);
                OBJ[i].hazard = 5;
            }
            OBJ[i].hit_w = fi(3);
        }
        ObjType::Snowball => {
            OBJ[i].grapple_mode = 3;
            OBJ[i].hp = 6;
        }
        ObjType::Springboard => {
            OBJ[i].grapple_mode = 3;
        }
        ObjType::Grappler => {
            OBJ[i].grapple_mode = 2;
            OBJ[i].hit_x = fi(-1);
            OBJ[i].hit_y = fi(-1);
            OBJ[i].hit_w = fi(10);
            OBJ[i].hit_h = fi(10);
        }
        ObjType::Crumble => {
            OBJ[i].solid = true;
            OBJ[i].grapple_mode = 1;
            OBJ[i].ox = OBJ[i].x;
            OBJ[i].oy = OBJ[i].y;
        }
        ObjType::SpawnerR | ObjType::SpawnerL => {
            // PICO-8 spawn period is 32 frames at 30fps -> 64 at 60fps (offset + update).
            OBJ[i].timer = (OBJ[i].x.to_int() / 8) % 64;
            OBJ[i].rdir = if OBJ[i].otype == ObjType::SpawnerR { 1 } else { -1 };
            OBJ[i].spr = fi(-1); // invisible
        }
        _ => {}
    }
}

// ====================================================================
// Player
// ====================================================================

unsafe fn player_on_collide(i: usize, horiz: bool, step: i32) -> bool {
    if horiz {
        if OBJ[i].state == 0 {
            if step == INPUT_X && corner_correct(i, INPUT_X, 0, 2, 2, -1, true) {
                return false;
            }
        } else if OBJ[i].state == 11 {
            if corner_correct(i, OBJ[i].grapple_dir, 0, 4, 2, 0, true) {
                return false;
            }
        }
        OBJ[i].rem.x = Fix32::ZERO;
        OBJ[i].spd.x = Fix32::ZERO;
    } else {
        if step < 0 && corner_correct(i, 0, -1, 2, 1, INPUT_X, true) {
            return false;
        }
        OBJ[i].t_var_jump = 0;
        OBJ[i].rem.y = Fix32::ZERO;
        OBJ[i].spd.y = Fix32::ZERO;
    }
    true
}

unsafe fn player_jump(i: usize) {
    consume_jump_press();
    OBJ[i].state = 0;
    OBJ[i].spd.y = fi(-4);
    OBJ[i].var_jump_speed = fi(-4);
    OBJ[i].spd.x += fi(INPUT_X) * fx(0.2);
    OBJ[i].t_var_jump = 8;
    OBJ[i].t_jump_grace = 0;
    OBJ[i].auto_var_jump = false;
    let snap = OBJ[i].jump_grace_y - OBJ[i].y;
    move_y_exact(i, snap);
    psfx(7, 0, 4);
}

unsafe fn player_wall_jump(i: usize, dir: i32) {
    consume_jump_press();
    OBJ[i].state = 0;
    OBJ[i].spd.y = fi(-3);
    OBJ[i].var_jump_speed = fi(-3);
    OBJ[i].spd.x = fi(3 * dir);
    OBJ[i].t_var_jump = 8;
    OBJ[i].auto_var_jump = false;
    OBJ[i].facing = dir;
    move_x_exact(i, fi(-dir * 3));
    psfx(7, 4, 4);
}

unsafe fn player_grapple_jump(i: usize) {
    consume_jump_press();
    psfx(17, 2, 3);
    OBJ[i].state = 0;
    OBJ[i].t_grapple_jump_grace = 0;
    OBJ[i].spd.y = fi(-3);
    OBJ[i].var_jump_speed = fi(-3);
    OBJ[i].t_var_jump = 8;
    OBJ[i].auto_var_jump = false;
    OBJ[i].grapple_retract = true;
    if OBJ[i].spd.x.abs() > fi(4) {
        OBJ[i].spd.x = sign(OBJ[i].spd.x) * fi(4);
    }
    let snap = OBJ[i].grapple_jump_grace_y - OBJ[i].y;
    move_y_exact(i, snap);
}

unsafe fn start_grapple(i: usize) {
    OBJ[i].state = 10;
    OBJ[i].spd = VZ;
    OBJ[i].rem = VZ;
    OBJ[i].grapple_x = OBJ[i].x;
    OBJ[i].grapple_y = OBJ[i].y - fi(3);
    OBJ[i].grapple_wave = Fix32::ZERO;
    OBJ[i].grapple_retract = false;
    OBJ[i].t_grapple_cooldown = 12; // PICO-8 6, doubled
    OBJ[i].t_var_jump = 0;
    OBJ[i].grapple_dir = if INPUT_X != 0 { INPUT_X } else { OBJ[i].facing };
    OBJ[i].facing = OBJ[i].grapple_dir;
    psfx(8, 0, 5);
}

/// grapple_check: 0 = nothing, 1 = hit, 2 = fail. Sets grapple_hit.
unsafe fn grapple_check(i: usize, x: Fix32, y: Fix32) -> i32 {
    let tx = (x / fi(8)).floor_int();
    let ty = (y / fi(8)).floor_int();
    let tile = tile_at(tx, ty);
    if backend::fget(tile, 1) {
        OBJ[i].grapple_hit = NONE;
        return if backend::fget(tile, 2) { 2 } else { 1 };
    }
    for j in 0..MAX_OBJ {
        if OBJ[j].exists && !OBJ[j].destroyed && OBJ[j].grapple_mode != 0 && contains(j, x, y) {
            OBJ[i].grapple_hit = j;
            return 1;
        }
    }
    0
}

unsafe fn player_bounce(i: usize, bx: Fix32, by: Fix32) {
    OBJ[i].state = 0;
    OBJ[i].spd.y = fi(-4);
    OBJ[i].var_jump_speed = fi(-4);
    OBJ[i].t_var_jump = 8;
    OBJ[i].t_jump_grace = 0;
    OBJ[i].auto_var_jump = true;
    OBJ[i].spd.x += sign(OBJ[i].x - bx) * fx(0.5);
    let snap = by - OBJ[i].y;
    move_y_exact(i, snap);
}

unsafe fn player_spring(i: usize, by: Fix32) {
    consume_jump_press();
    if JUMP_HELD {
        psfx(17, 2, 3);
    } else {
        psfx(17, 0, 2);
    }
    OBJ[i].state = 0;
    OBJ[i].spd.y = fi(-5);
    OBJ[i].var_jump_speed = fi(-5);
    OBJ[i].t_var_jump = 12; // PICO-8 6, doubled
    OBJ[i].t_jump_grace = 0;
    OBJ[i].rem.y = Fix32::ZERO;
    OBJ[i].auto_var_jump = false;
    let sb = OBJ[i].link;
    if sb != NONE {
        OBJ[sb].link = NONE; // springboard.player = nil
    }
    OBJ[i].link = NONE;
    let snap = by - OBJ[i].y;
    move_y_exact(i, snap);
    // break any crumble sitting under the springboard
    if sb != NONE {
        for j in 0..MAX_OBJ {
            if OBJ[j].exists
                && !OBJ[j].destroyed
                && OBJ[j].otype == ObjType::Crumble
                && !OBJ[j].breaking
                && overlaps(sb, j, Fix32::ZERO, fi(4))
            {
                OBJ[j].breaking = true;
                OBJ[j].timer = 0;
                psfx(8, 20, 4);
            }
        }
    }
}

/// True if the player is falling onto `o` from above (snowball/springboard).
unsafe fn bounce_check(i: usize, o: usize) -> bool {
    OBJ[i].spd.y >= Fix32::ZERO && OBJ[i].y - OBJ[i].spd.y < OBJ[o].y + OBJ[o].spd.y + fi(4)
}

unsafe fn snowball_bounce_overlaps(snow: usize, player: usize) -> bool {
    if OBJ[snow].spd.x != Fix32::ZERO {
        OBJ[snow].hit_w = fi(12);
        OBJ[snow].hit_x = fi(-2);
        let r = overlaps(snow, player, Fix32::ZERO, Fix32::ZERO);
        OBJ[snow].hit_w = fi(8);
        OBJ[snow].hit_x = Fix32::ZERO;
        r
    } else {
        overlaps(snow, player, Fix32::ZERO, Fix32::ZERO)
    }
}

unsafe fn obj_on_release(obj: usize, thrown: bool) {
    match OBJ[obj].otype {
        ObjType::Snowball => {
            if !thrown {
                OBJ[obj].stop = true;
            }
            OBJ[obj].timer = 16; // thrown_timer (60fps: PICO-8 8 doubled) -- the
            // window where a just-thrown snowball can't hurt the player
        }
        ObjType::Springboard => {
            if thrown {
                OBJ[obj].timer = 5;
            }
        }
        _ => {}
    }
}

unsafe fn release_holding(i: usize, obj: usize, sx: Fix32, sy: Fix32, thrown: bool) {
    OBJ[obj].held = false;
    OBJ[obj].spd.x = sx;
    OBJ[obj].spd.y = sy;
    obj_on_release(obj, thrown);
    psfx(7, 24, 6);
    OBJ[i].holding = NONE;
}

unsafe fn player_update(i: usize) {
    let on_ground = check_solid(i, Fix32::ZERO, fi(1));
    if on_ground {
        OBJ[i].t_jump_grace = 8;
        OBJ[i].jump_grace_y = OBJ[i].y;
    } else {
        OBJ[i].t_jump_grace = (OBJ[i].t_jump_grace - 1).max(0);
    }
    OBJ[i].t_grapple_jump_grace = (OBJ[i].t_grapple_jump_grace - 1).max(-1);
    if OBJ[i].t_grapple_cooldown > 0 && OBJ[i].state < 1 {
        OBJ[i].t_grapple_cooldown -= 1;
    }

    // grapple retract animation
    if OBJ[i].grapple_retract {
        // rope retract approach halved for 60fps (PICO-8 12 / 6)
        OBJ[i].grapple_x = approach(OBJ[i].grapple_x, OBJ[i].x, fi(6));
        OBJ[i].grapple_y = approach(OBJ[i].grapple_y, OBJ[i].y - fi(3), fi(3));
        if OBJ[i].grapple_x == OBJ[i].x && OBJ[i].grapple_y == OBJ[i].y - fi(3) {
            OBJ[i].grapple_retract = false;
        }
    }

    match OBJ[i].state {
        0 => {
            if INPUT_X != 0 {
                OBJ[i].facing = INPUT_X;
            }
            // running (accel halved for 60fps)
            let sx = OBJ[i].spd.x;
            let (target, accel) = if sx.abs() > fi(2) && fi(INPUT_X) == sign(sx) {
                (fi(2), fx(0.05))
            } else if on_ground {
                (fi(2), fx(0.4))
            } else if INPUT_X != 0 {
                (fi(2), fx(0.2))
            } else {
                (Fix32::ZERO, fx(0.1))
            };
            OBJ[i].spd.x = approach(OBJ[i].spd.x, fi(INPUT_X) * target, accel);
            // gravity (increments halved)
            if !on_ground {
                let max = if held(IN_DOWN) { fx(5.2) } else { fx(4.4) };
                let g = if OBJ[i].spd.y.abs() < fx(0.2) && JUMP_HELD { fx(0.2) } else { fx(0.4) };
                OBJ[i].spd.y = (OBJ[i].spd.y + g).min(max);
            }
            // variable jump
            if OBJ[i].t_var_jump > 0 {
                if JUMP_HELD || OBJ[i].auto_var_jump {
                    OBJ[i].spd.y = OBJ[i].var_jump_speed;
                    OBJ[i].t_var_jump -= 1;
                } else {
                    OBJ[i].t_var_jump = 0;
                }
            }
            // jump / wall jump / grapple jump
            if JUMP_PRESSED > 0 {
                if OBJ[i].t_jump_grace > 0 {
                    player_jump(i);
                } else if check_solid(i, fi(2), Fix32::ZERO) {
                    player_wall_jump(i, -1);
                } else if check_solid(i, fi(-2), Fix32::ZERO) {
                    player_wall_jump(i, 1);
                } else if OBJ[i].t_grapple_jump_grace > 0 {
                    player_grapple_jump(i);
                }
            }
            // throw / drop a held object
            let hold = OBJ[i].holding;
            if hold != NONE && !GRAP_HELD && !check_solid(hold, Fix32::ZERO, fi(-2)) {
                OBJ[hold].y -= fi(2);
                if held(IN_DOWN) {
                    release_holding(i, hold, fi(2 * OBJ[i].facing), Fix32::ZERO, false);
                } else {
                    release_holding(i, hold, fi(4 * OBJ[i].facing), fi(-1), true);
                }
            }
            // throw grapple
            if HAVE_GRAPPLE && OBJ[i].holding == NONE && OBJ[i].t_grapple_cooldown <= 0 && consume_grapple_press() {
                start_grapple(i);
            }
        }
        10 => {
            // throw grapple: extend the rope, probe for a hit
            let dist = (OBJ[i].grapple_x - OBJ[i].x).abs();
            let amount = (fi(64) - dist).min(fi(6)).max(Fix32::ZERO).to_int();
            let mut grabbed = false;
            let dir = fi(OBJ[i].grapple_dir);
            for _ in 0..amount {
                let mut hit = grapple_check(i, OBJ[i].grapple_x + dir, OBJ[i].grapple_y);
                if hit == 0 {
                    hit = grapple_check(i, OBJ[i].grapple_x + dir, OBJ[i].grapple_y - fi(1));
                }
                if hit == 0 {
                    hit = grapple_check(i, OBJ[i].grapple_x + dir, OBJ[i].grapple_y + fi(1));
                }
                let mode = if OBJ[i].grapple_hit != NONE { OBJ[OBJ[i].grapple_hit].grapple_mode } else { 0 };
                if hit == 0 {
                    OBJ[i].grapple_x += dir; // 60fps: PICO-8 dir*2, halved
                } else if hit == 1 {
                    if mode == 2 {
                        OBJ[i].grapple_x = OBJ[OBJ[i].grapple_hit].x + fi(4);
                        OBJ[i].grapple_y = OBJ[OBJ[i].grapple_hit].y + fi(4);
                    } else if mode == 3 {
                        OBJ[OBJ[i].grapple_hit].held = true;
                        grabbed = true;
                    }
                    OBJ[i].state = if mode == 3 { 12 } else { 11 };
                    OBJ[i].grapple_wave = fi(2);
                    OBJ[i].grapple_boost = false;
                    OBJ[i].freeze = 4; // PICO-8 freeze=2 doubled for 60fps
                    psfx(14, 0, 5);
                    break;
                }
                let new_dist = (OBJ[i].grapple_x - OBJ[i].x).abs();
                if hit == 2 || (hit == 0 && new_dist >= fi(64)) {
                    psfx(if hit == 2 { 7 } else { 14 }, 8, 3);
                    OBJ[i].grapple_retract = true;
                    OBJ[i].freeze = 4; // PICO-8 freeze=2 doubled for 60fps
                    OBJ[i].state = 0;
                    break;
                }
            }
            OBJ[i].grapple_wave = approach(OBJ[i].grapple_wave, fi(1), fx(0.1)); // 60fps: 0.2 halved
            OBJ[i].spr = fi(3);
            if !grabbed && (!GRAP_HELD || (OBJ[i].y - OBJ[i].grapple_y).abs() > fi(8)) {
                OBJ[i].state = 0;
                OBJ[i].grapple_retract = true;
                psfx(-2, 0, 0); // cut the grapple-throw sound short on release
            }
        }
        11 => {
            // attached: boost + swing
            if !OBJ[i].grapple_boost {
                OBJ[i].grapple_boost = true;
                OBJ[i].spd.x = fi(OBJ[i].grapple_dir * 8);
            }
            OBJ[i].spd.x = approach(OBJ[i].spd.x, fi(OBJ[i].grapple_dir * 5), fx(0.125));
            OBJ[i].spd.y = approach(OBJ[i].spd.y, Fix32::ZERO, fx(0.2));
            if OBJ[i].spd.y == Fix32::ZERO {
                if OBJ[i].y - fi(3) > OBJ[i].grapple_y {
                    move_y(i, fx(-0.5), Collide::Stop);
                } else if OBJ[i].y - fi(3) < OBJ[i].grapple_y {
                    move_y(i, fx(0.5), Collide::Stop);
                }
            }
            if OBJ[i].spr != fi(4) && check_solid(i, fi(OBJ[i].grapple_dir), Fix32::ZERO) {
                OBJ[i].spr = fi(4);
                psfx(14, 8, 3);
            }
            if consume_jump_press() {
                if check_solid(i, fi(OBJ[i].grapple_dir * 2), Fix32::ZERO) {
                    player_wall_jump(i, -OBJ[i].grapple_dir);
                } else {
                    OBJ[i].grapple_jump_grace_y = OBJ[i].y;
                    player_grapple_jump(i);
                }
            }
            OBJ[i].grapple_wave = approach(OBJ[i].grapple_wave, Fix32::ZERO, fx(0.3)); // 60fps: 0.6 halved
            let dead = OBJ[i].grapple_hit != NONE && OBJ[OBJ[i].grapple_hit].destroyed;
            if !GRAP_HELD || dead {
                OBJ[i].state = 0;
                OBJ[i].t_grapple_jump_grace = 8; // PICO-8 4 doubled for 60fps
                OBJ[i].grapple_jump_grace_y = OBJ[i].y;
                OBJ[i].grapple_retract = true;
                OBJ[i].facing *= -1;
                if OBJ[i].spd.x.abs() > fi(5) {
                    OBJ[i].spd.x = sign(OBJ[i].spd.x) * fi(5);
                } else if OBJ[i].spd.x.abs() <= fx(0.5) {
                    OBJ[i].spd.x = Fix32::ZERO;
                }
            }
            if sign(OBJ[i].x - OBJ[i].grapple_x) == fi(OBJ[i].grapple_dir) {
                OBJ[i].state = 0;
                if OBJ[i].grapple_hit != NONE && OBJ[OBJ[i].grapple_hit].grapple_mode == 2 {
                    OBJ[i].t_grapple_jump_grace = 8; // PICO-8 4 doubled for 60fps
                    OBJ[i].grapple_jump_grace_y = OBJ[i].y;
                }
                if OBJ[i].spd.x.abs() > fi(5) {
                    OBJ[i].spd.x = sign(OBJ[i].spd.x) * fi(5);
                }
            }
        }
        50 => {
            // grapple-pickup cutscene
            OBJ[i].spd.y = (OBJ[i].spd.y + fx(0.4)).min(fx(4.5));
            OBJ[i].spd.x = approach(OBJ[i].spd.x, Fix32::ZERO, fx(0.1));
            if on_ground {
                // PICO-8 cues at 30fps frames 0/61/70/80; doubled for 60fps so the
                // "got the grapple" jingle plays in full before the level music resumes.
                if OBJ[i].t_grapple_pickup == 0 {
                    music(39);
                }
                if OBJ[i].t_grapple_pickup == 122 {
                    music(-1);
                }
                if OBJ[i].t_grapple_pickup == 140 {
                    music(22);
                }
                if OBJ[i].t_grapple_pickup > 160 {
                    OBJ[i].state = 0;
                }
                OBJ[i].t_grapple_pickup += 1;
            }
        }
        1 => {
            // lift a grappled holdable up to carry height
            let h = OBJ[i].grapple_hit;
            if h == NONE {
                OBJ[i].state = 0;
            } else {
                // per-frame approach halved for 60fps (PICO-8 amount 4)
                OBJ[h].x = approach(OBJ[h].x, OBJ[i].x - fi(4), fi(2));
                OBJ[h].y = approach(OBJ[h].y, OBJ[i].y - fi(14), fi(2));
                if OBJ[h].x == OBJ[i].x - fi(4) && OBJ[h].y == OBJ[i].y - fi(14) {
                    OBJ[i].state = 0;
                    OBJ[i].holding = h;
                }
            }
        }
        2 => {
            // springboard bounce: settle onto it, then spring
            let sb = OBJ[i].link;
            if sb == NONE {
                OBJ[i].state = 0;
            } else {
                // per-frame settle approach halved for 60fps (PICO-8 0.5 / 0.2)
                let at_x = approach(OBJ[i].x, OBJ[sb].x + fi(4), fx(0.25));
                move_x_exact(i, at_x - OBJ[i].x);
                let at_y = approach(OBJ[i].y, OBJ[sb].y + fi(4), fx(0.1));
                move_y_exact(i, at_y - OBJ[i].y);
                if OBJ[sb].spr == fi(11) && OBJ[i].y >= OBJ[sb].y + fi(2) {
                    OBJ[sb].spr = fi(12);
                } else if OBJ[i].y == OBJ[sb].y + fi(4) {
                    let by = OBJ[sb].y + fi(4);
                    player_spring(i, by);
                    OBJ[sb].spr = fi(11);
                }
            }
        }
        12 => {
            // pull a grappled holdable toward the player
            let obj = OBJ[i].grapple_hit;
            if obj == NONE {
                OBJ[i].state = 0;
            } else {
                let dir = OBJ[i].grapple_dir;
                if move_x(obj, fi(-dir * 6), Collide::PullX) {
                    OBJ[i].state = 0;
                    OBJ[i].grapple_retract = true;
                    obj_on_release(obj, dir != 0);
                    OBJ[obj].held = false;
                    return;
                } else {
                    // rope retract halved for 60fps to match the halved object pull (PICO-8 6)
                    OBJ[i].grapple_x = approach(OBJ[i].grapple_x, OBJ[i].x, fi(3));
                }
                if OBJ[obj].y != OBJ[i].y - fi(7) {
                    let d = sign(OBJ[i].y - OBJ[obj].y - fi(7)) * fx(0.5);
                    move_y(obj, d, Collide::Stop);
                }
                OBJ[i].grapple_wave = approach(OBJ[i].grapple_wave, Fix32::ZERO, fx(0.3)); // 60fps: 0.6 halved
                if overlaps(i, obj, Fix32::ZERO, Fix32::ZERO) {
                    OBJ[i].state = 1;
                    psfx(7, 16, 6);
                }
                let off = (OBJ[obj].y - OBJ[i].y + fi(7)).abs() > fi(8)
                    || sign(OBJ[obj].x + fi(4) - OBJ[i].x) == fi(-dir);
                if !GRAP_HELD || off {
                    OBJ[i].state = 0;
                    OBJ[i].grapple_retract = true;
                    release_holding(i, obj, fi(-dir * 5), Fix32::ZERO, true);
                }
            }
        }
        99 | 100 => {
            if OBJ[i].state == 100 {
                OBJ[i].x += fi(1); // stride off-screen during the wipe
                if OBJ[i].wipe_timer == 10 && LVL_INDEX > 1 {
                    psfx(17, 24, 9); // level-finish sound
                }
            }
            OBJ[i].wipe_timer += 1;
            if OBJ[i].wipe_timer > 40 {
                if OBJ[i].state == 99 {
                    restart_level();
                } else {
                    next_level();
                }
            }
            return;
        }
        _ => {}
    }

    // apply movement (velocity moves -> halved inside move_x/move_y)
    let collide = if OBJ[i].state == 99 { Collide::Stop } else { Collide::Player };
    let sx = OBJ[i].spd.x;
    let sy = OBJ[i].spd.y;
    move_x(i, sx, collide);
    move_y(i, sy, collide);

    // carry a held object
    if OBJ[i].holding != NONE {
        let h = OBJ[i].holding;
        OBJ[h].x = OBJ[i].x - fi(4);
        OBJ[h].y = OBJ[i].y - fi(14);
    }

    // sprite
    if OBJ[i].state == 50 && OBJ[i].t_grapple_pickup > 0 {
        OBJ[i].spr = fi(5); // raising the grapple overhead
    } else if OBJ[i].state != 11 {
        if !on_ground {
            OBJ[i].spr = fi(3);
        } else if INPUT_X != 0 {
            OBJ[i].spr += fx(0.25);
            OBJ[i].spr = fi(2) + OBJ[i].spr.rem_floor(fi(2));
        } else {
            OBJ[i].spr = fi(2);
        }
    }

    // object interactions
    for j in 0..MAX_OBJ {
        if !OBJ[j].exists || OBJ[j].destroyed {
            continue;
        }
        match OBJ[j].otype {
            ObjType::GrapplePickup => {
                if overlaps(i, j, Fix32::ZERO, Fix32::ZERO) {
                    OBJ[j].destroyed = true;
                    HAVE_GRAPPLE = true;
                    psfx(7, 12, 4);
                    OBJ[i].state = 50;
                }
            }
            ObjType::Bridge => {
                if !OBJ[j].falling && overlaps(i, j, Fix32::ZERO, Fix32::ZERO) {
                    OBJ[j].falling = true;
                    OBJ[i].freeze = 2; // 60fps: PICO-8 freeze=1 doubled
                    SHAKE = 2;
                    psfx(8, 16, 4);
                }
            }
            ObjType::Berry => {
                if overlaps(i, j, Fix32::ZERO, Fix32::ZERO) && OBJ[j].link == NONE && OBJ[j].timer == 0 && !OBJ[j].stop {
                    OBJ[j].link = i; // collected -> follow player
                    OBJ[j].timer = 0;
                    OBJ[j].flash = 10; // pickup flash ring (60fps: PICO-8 5 doubled)
                    LAST_BERRY = j; // grabbing this deposits any older carried berry
                    psfx(7, 12, 4);
                }
            }
            ObjType::Snowball => {
                if !OBJ[j].held {
                    if bounce_check(i, j) && snowball_bounce_overlaps(j, i) {
                        player_bounce(i, OBJ[j].x + fi(4), OBJ[j].y);
                        psfx(17, 0, 2);
                        OBJ[j].freeze = 2; // 60fps: PICO-8 freeze=1 doubled
                        OBJ[j].spd.y = fi(-1);
                        snowball_hurt(j);
                    } else if OBJ[j].spd.x != Fix32::ZERO
                        && OBJ[j].timer <= 0
                        && overlaps(i, j, Fix32::ZERO, Fix32::ZERO)
                    {
                        player_die(i);
                        return;
                    }
                }
            }
            ObjType::Springboard => {
                if OBJ[i].state != 2
                    && !OBJ[j].held
                    && overlaps(i, j, Fix32::ZERO, Fix32::ZERO)
                    && bounce_check(i, j)
                {
                    OBJ[i].state = 2;
                    OBJ[i].spd = VZ;
                    OBJ[i].t_jump_grace = 0;
                    OBJ[i].rem.y = Fix32::ZERO;
                    OBJ[i].link = j; // springboard ref
                    OBJ[j].link = i; // springboard.player = self
                    move_y_exact(i, OBJ[j].y + fi(4) - OBJ[i].y);
                }
            }
            ObjType::Crumble => {
                if !OBJ[j].breaking {
                    let gd = fi(OBJ[i].grapple_dir);
                    let hit = if OBJ[i].state == 0 {
                        overlaps(i, j, Fix32::ZERO, fi(1))
                    } else if OBJ[i].state == 11 {
                        // grappling into it: check the grapple direction at 3 heights
                        overlaps(i, j, gd, Fix32::ZERO)
                            || overlaps(i, j, gd, fi(3))
                            || overlaps(i, j, gd, fi(-2))
                    } else {
                        false
                    };
                    if hit {
                        OBJ[j].breaking = true;
                        OBJ[j].timer = 0;
                        psfx(8, 20, 4);
                    }
                }
            }
            ObjType::Checkpoint => {
                if LEVEL_CHECKPOINT != OBJ[j].oid && overlaps(i, j, Fix32::ZERO, Fix32::ZERO) {
                    LEVEL_CHECKPOINT = OBJ[j].oid;
                    psfx_lock(8, 24, 6, 40); // PICO-8 lock 20 doubled
                }
            }
            _ => {}
        }
    }

    // death / pit (only level 1 finishes by falling off the right)
    if OBJ[i].state < 99 && (OBJ[i].y > fi(LVL_H * 8 + 16) || hazard_check(i, Fix32::ZERO, Fix32::ZERO)) {
        if LVL_INDEX == 1 && OBJ[i].x > fi(LVL_W * 8 - 64) {
            OBJ[i].state = 100;
            OBJ[i].wipe_timer = -30; // 60fps: PICO-8 -15 doubled
        } else {
            player_die(i);
        }
        return;
    }

    // bounds: clamp top + left; right edge either clamps (right_edge) or advances
    if OBJ[i].y < fi(-16) {
        OBJ[i].y = fi(-16);
        OBJ[i].spd.y = Fix32::ZERO;
    }
    if OBJ[i].x < fi(3) {
        OBJ[i].x = fi(3);
        OBJ[i].spd.x = Fix32::ZERO;
    } else if OBJ[i].x > fi(LVL_W * 8 - 3) {
        if level().right_edge {
            OBJ[i].x = fi(LVL_W * 8 - 3);
            OBJ[i].spd.x = Fix32::ZERO;
        } else {
            OBJ[i].state = 100;
        }
    }

    // level-1 intro bridge music transition
    if CUR_MUSIC == LEVELS[1].music && OBJ[i].x > fi(61 * 8) {
        CUR_MUSIC = 37;
        music(37);
        psfx(17, 24, 9);
    }

    // level-8 ending music + score reveal
    if LVL_INDEX == 8 {
        if CUR_MUSIC != 40 && OBJ[i].y > fi(40) {
            CUR_MUSIC = 40;
            music(40);
        }
        if OBJ[i].y > fi(376) {
            SHOW_SCORE += 1;
        }
        if SHOW_SCORE == 240 {
            // 60fps: PICO-8 120 doubled
            music(38);
        }
    }
}

unsafe fn player_draw(i: usize) {
    let o = OBJ[i];

    // death fx: an expanding ring of shrinking circles
    if o.state == 99 {
        let e = fi(o.wipe_timer) / fi(20); // 60fps: PICO-8 /10 doubled
        if e <= fi(1) {
            let dx = clamp(o.x, fi(CAM_X), fi(CAM_X + 128));
            let dy = clamp(o.y - fi(4), Fix32::ZERO, fi(128));
            let r = ((fi(1) - e) * fi(8)).to_int() as i16;
            for k in 0..8 {
                let ang = fi(k) / fi(8);
                let cx = (dx + ang.cos() * fi(32) * e).to_int() as i16;
                let cy = (dy + ang.sin() * fi(32) * e).to_int() as i16;
                backend::circfill(cx, cy, r, 10);
            }
        }
        return;
    }

    // scarf: 5 damped trailing segments (PICO-8 colour 10). The per-frame approach
    // steps are HALVED for 60fps (PICO-8 /1.5 and /2 -> /3 and /4): at 60fps the
    // segments otherwise track the player twice as tightly per real second, so the
    // scarf lagged half as far and barely peeked out from behind the sprite. The
    // 1.5px segment-spacing CLAMP is a position limit, not a per-frame move, so it
    // is unchanged (it still stops fast moves from streaking the scarf into stray
    // pixels across the level).
    let t = fi(FRAMES) / fi(60);
    let mut last = Vec2 { x: o.x - fi(o.facing), y: o.y - fi(3) };
    for k in 1..=5 {
        let mut s = SCARF[k - 1];
        s.x += (last.x - s.x - fi(o.facing)) / fx(3.0);
        let ki = fi(k as i32);
        let wob = (ki * fx(0.25) + t).sin() * ki * fx(0.25);
        s.y += ((last.y - s.y) + wob) / fi(4);
        let dx = s.x - last.x;
        let dy = s.y - last.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > fx(1.5) {
            s.x = last.x + dx / dist * fx(1.5);
            s.y = last.y + dy / dist * fx(1.5);
        }
        SCARF[k - 1] = s;
        let (sx, sy) = (s.x.to_int() as i16, s.y.to_int() as i16);
        backend::rectfill(sx, sy, sx, sy, 10);
        let mx = ((s.x + last.x) / fi(2)).to_int() as i16;
        let my = ((s.y + last.y) / fi(2)).to_int() as i16;
        backend::rectfill(mx, my, mx, my, 10);
        last = s;
    }

    // grapple rope (active grapple: the original's wavy double-line, not a faint
    // straight line). amplitude = 2*grapple_wave, freqs per the cart.
    if o.state >= 10 && o.state <= 12 {
        draw_sine_h(o.x, o.grapple_x, o.y - fi(3), 7, o.grapple_wave * fi(2), fi(6), fx(0.08), 6);
    }
    // retracting grapple: dark underline (1) then white rope (7), as in the original
    if o.grapple_retract {
        backend::line(o.x.to_int() as i16, (o.y - fi(2)).to_int() as i16, o.grapple_x.to_int() as i16, (o.grapple_y + fi(1)).to_int() as i16, 1);
        backend::line(o.x.to_int() as i16, (o.y - fi(3)).to_int() as i16, o.grapple_x.to_int() as i16, (o.grapple_y).to_int() as i16, 7);
    }
    backend::spr(o.spr.floor_int(), (o.x - fi(4)).to_int() as i16, (o.y - fi(8)).to_int() as i16, o.facing != 1, false);

    // grapple-pickup celebration: the hookshot raised overhead + a spinning star burst
    if o.state == 50 && o.t_grapple_pickup > 0 {
        backend::spr(20, (o.x - fi(4)).to_int() as i16, (o.y - fi(18)).to_int() as i16, false, false);
        let ty = o.y - fi(14);
        for k in 0..=16 {
            let ang = t * fi(4) + fi(k) / fi(16);
            let s = ang.sin();
            let c = ang.cos();
            let x0 = (o.x + s * fi(16)).to_int() as i16;
            let y0 = (ty + c * fi(16)).to_int() as i16;
            let x1 = (o.x + s * fi(40)).to_int() as i16;
            let y1 = (ty + c * fi(40)).to_int() as i16;
            backend::line(x0, y0, x1, y1, 7);
        }
    }
}

// ====================================================================
// Object updates
// ====================================================================

unsafe fn snowball_hurt(i: usize) -> bool {
    OBJ[i].hp -= 1;
    if OBJ[i].hp <= 0 {
        psfx(8, 16, 4);
        OBJ[i].destroyed = true;
        return true;
    }
    false
}

unsafe fn obj_update(i: usize) {
    if OBJ[i].freeze > 0 {
        OBJ[i].freeze -= 1;
        return;
    }
    match OBJ[i].otype {
        ObjType::Snowball => {
            if OBJ[i].held {
                return;
            }
            OBJ[i].timer -= 1; // thrown_timer
            if OBJ[i].stop {
                OBJ[i].spd.x = approach(OBJ[i].spd.x, Fix32::ZERO, fx(0.125));
                if OBJ[i].spd.x == Fix32::ZERO {
                    OBJ[i].stop = false;
                }
            } else if OBJ[i].spd.x != Fix32::ZERO {
                OBJ[i].spd.x = approach(OBJ[i].spd.x, sign(OBJ[i].spd.x) * fi(2), fx(0.05));
            }
            if !check_solid(i, Fix32::ZERO, fi(1)) {
                OBJ[i].spd.y = approach(OBJ[i].spd.y, fi(4), fx(0.2));
            }
            let sx = OBJ[i].spd.x;
            let sy = OBJ[i].spd.y;
            // snowball wall collision (snowball_on_collide_x): corner-correct over a
            // small lip, else hurt, else bounce back.
            move_x(i, sx, Collide::SnowballX);
            if move_y(i, sy, Collide::Stop) {
                // Bounce off the floor using the PRE-collision speed (Collide::Stop
                // zeroed spd.y). PICO-8's tiers were >=4 -> -2, >=1 -> -1, else 0,
                // but the >=1 -> -1 tier perfectly restitutes at the 1px/frame
                // boundary: at 30fps the pixel-quantised fall lost enough energy to
                // settle, but at 60fps's finer timestep the snowball returns at +1
                // and re-bounces forever. Use a half-speed bounce that rests below
                // 2 instead -- 4 -> -2, 2 -> -1 still match the cart, and it decays
                // (return ~2 -> -1 -> return ~1 -> rest) so it settles in a bounce
                // or two like the original.
                if sy >= fi(2) {
                    OBJ[i].spd.y = -(sy * fx(0.5));
                    psfx(17, 0, 2);
                } else {
                    OBJ[i].spd.y = Fix32::ZERO; // too slow (or a ceiling hit) -> rest
                }
                OBJ[i].rem.y = Fix32::ZERO;
            }
            if OBJ[i].y > fi(LVL_H * 8 + 24) {
                OBJ[i].destroyed = true;
            }
        }
        ObjType::Springboard => {
            if OBJ[i].held {
                return;
            }
            if check_solid(i, Fix32::ZERO, fi(1)) {
                OBJ[i].spd.x = approach(OBJ[i].spd.x, Fix32::ZERO, fx(0.5));
            } else {
                OBJ[i].spd.x = approach(OBJ[i].spd.x, Fix32::ZERO, fx(0.1));
                OBJ[i].spd.y = approach(OBJ[i].spd.y, fi(4), fx(0.2));
            }
            let sx = OBJ[i].spd.x;
            let sy = OBJ[i].spd.y;
            // A thrown springboard BOUNCES off walls/floor (not a dead stop):
            // on_collide_x -> speed_x *= -0.2; on_collide_y -> speed_y *= -0.4 when
            // falling >= 2, plus speed_x *= 0.5 (PICO-8 springboard.on_collide_*).
            if move_x(i, sx, Collide::Stop) {
                OBJ[i].spd.x = sx * fx(-0.2);
                OBJ[i].freeze = 2; // PICO-8 freeze=1 doubled for 60fps
            }
            if move_y(i, sy, Collide::Stop) {
                if sy < Fix32::ZERO {
                    OBJ[i].spd.y = Fix32::ZERO;
                } else {
                    OBJ[i].spd.y = if sy >= fi(2) { sy * fx(-0.4) } else { Fix32::ZERO };
                    OBJ[i].spd.x = OBJ[i].spd.x * fx(0.5);
                }
            }
            // carry the player riding it
            if OBJ[i].link != NONE {
                let p = OBJ[i].link;
                move_y(p, OBJ[i].spd.y, Collide::Stop);
            }
            if OBJ[i].y > fi(LVL_H * 8 + 24) {
                OBJ[i].destroyed = true;
            }
        }
        ObjType::Bridge => {
            if OBJ[i].falling {
                OBJ[i].y += fx(1.5); // PICO-8 +3/frame, halved
            }
        }
        ObjType::Crumble => {
            if OBJ[i].breaking {
                OBJ[i].timer += 1;
                if OBJ[i].timer > 20 {
                    OBJ[i].x = fi(-32);
                    OBJ[i].y = fi(-32);
                }
                if OBJ[i].timer > 180 {
                    // move back, but only respawn if nothing overlaps the slot
                    OBJ[i].x = OBJ[i].ox;
                    OBJ[i].y = OBJ[i].oy;
                    let mut clear = true;
                    for j in 0..MAX_OBJ {
                        if overlaps(i, j, Fix32::ZERO, Fix32::ZERO) {
                            clear = false;
                            break;
                        }
                    }
                    if clear {
                        OBJ[i].breaking = false;
                        OBJ[i].timer = 0;
                        psfx(17, 5, 3);
                    } else {
                        OBJ[i].x = fi(-32);
                        OBJ[i].y = fi(-32);
                    }
                }
            }
        }
        ObjType::Berry => {
            if OBJ[i].stop {
                // collected popup
                OBJ[i].timer += 1;
                if OBJ[i].timer > 10 {
                    // 60fps: PICO-8 5 doubled
                    OBJ[i].y -= fx(0.1);
                }
                if OBJ[i].timer > 60 {
                    OBJ[i].destroyed = true;
                }
            } else if OBJ[i].link != NONE {
                let p = OBJ[i].link;
                // per-frame approach is halved for 60fps (PICO-8 ran this at 30)
                OBJ[i].x += (OBJ[p].x - OBJ[i].x) / fi(8) * HALF;
                OBJ[i].y += (OBJ[p].y - fi(4) - OBJ[i].y) / fi(8) * HALF;
                OBJ[i].flash -= 1;
                let grounded = check_solid(p, Fix32::ZERO, fi(1)) && OBJ[p].state != 99;
                if grounded {
                    OBJ[i].timer += 1;
                } else {
                    OBJ[i].timer = 0;
                }
                // deposit when settled, at the level edge, or when a newer berry was grabbed
                if OBJ[i].timer > 6 || OBJ[p].x > fi(LVL_W * 8 - 7) || LAST_BERRY != i {
                    psfx_lock(8, 8, 8, 40);
                    OBJ[i].stop = true;
                    OBJ[i].timer = 0;
                    BERRY_COUNT += 1;
                    mark_collected(OBJ[i].oid); // don't respawn/re-count on restart
                }
            }
        }
        ObjType::SpawnerR | ObjType::SpawnerL => {
            OBJ[i].timer += 1;
            if OBJ[i].timer >= 64 && (OBJ[i].x.to_int() - 64 - CAM_X).abs() < 128 {
                OBJ[i].timer = 0;
                let s = create(ObjType::Snowball, OBJ[i].x.to_int(), OBJ[i].y.to_int() - 8);
                if s != NONE {
                    OBJ[s].spd.x = fi(OBJ[i].rdir * 2);
                    OBJ[s].spd.y = fi(4);
                }
                psfx(17, 5, 3);
            }
        }
        _ => {}
    }
}

// ====================================================================
// Camera
// ====================================================================

/// The cart's 8 per-level camera modes (barrier/stateful refinements omitted).
/// Shrink/grow the x-target so the camera can't cross a level barrier column.
unsafe fn camera_x_barrier(tile_x: i32, px: i32, tx: &mut i32) {
    let bx = tile_x * 8;
    if px < bx - 8 {
        *tx = (*tx).min(bx - 128);
    } else if px > bx + 8 {
        *tx = (*tx).max(bx);
    }
}

unsafe fn camera_target() -> (i32, i32) {
    let px = OBJ[PLAYER].x.to_int();
    let py = OBJ[PLAYER].y.to_int();
    let lv = level();
    let wlim = LVL_W * 8 - 128;
    let hlim = LVL_H * 8 - 128;
    // The PICO-8 modes use max(min(wlim, ..)) -- ceiling only, no floor -- except
    // modes 1/4/5/7/8 which clamp the floor; replicated below.
    let mut tx;
    let mut ty = 0;
    match CAM_MODE {
        1 => {
            tx = if px < 42 { 0 } else { (px - 48).max(40).min(wlim) };
        }
        2 => {
            tx = if px < 120 { 0 } else if px > 136 { 128 } else { px - 64 };
            ty = (py - 64).min(hlim);
        }
        3 => {
            tx = (px - 56).min(wlim);
            if lv.barrier_x >= 0 {
                camera_x_barrier(lv.barrier_x, px, &mut tx);
            }
            ty = if py < lv.barrier_y * 8 + 3 { 0 } else { lv.barrier_y * 8 };
        }
        4 => {
            let sx = if px % 128 > 8 && px % 128 < 120 { (px / 128) * 128 + 64 } else { px };
            let sy = if py % 128 > 4 && py % 128 < 124 { (py / 128) * 128 + 64 } else { py };
            tx = (sx - 64).min(wlim);
            ty = (sy - 64).min(hlim);
        }
        5 => {
            tx = (px - 32).min(wlim);
        }
        6 => {
            if px > 848 {
                C_OFFSET = 48;
            } else if px < 704 {
                C_FLAG = false;
                C_OFFSET = 32;
            } else if px > 808 {
                C_FLAG = true;
                C_OFFSET = 96;
            }
            tx = (px - C_OFFSET).min(wlim);
            if lv.barrier_x >= 0 {
                camera_x_barrier(lv.barrier_x, px, &mut tx);
            }
            if C_FLAG {
                tx = tx.max(672);
            }
        }
        7 => {
            if px > 420 {
                if px < 436 {
                    C_OFFSET = 32 + px - 420;
                } else if px > 808 {
                    C_OFFSET = 48 - (px - 808).min(16);
                } else {
                    C_OFFSET = 48;
                }
            } else {
                C_OFFSET = 32;
            }
            tx = (px - C_OFFSET).max(0).min(wlim);
        }
        8 => {
            tx = 0;
            ty = (py - 32).max(0).min(hlim);
        }
        _ => {
            tx = (px - 64).min(wlim);
        }
    }
    (tx, ty)
}

unsafe fn update_camera() {
    if PLAYER == NONE {
        return;
    }
    let (tx, ty) = camera_target();
    CAM_X += (tx - CAM_X).clamp(-3, 3); // PICO-8 5px/frame, ~halved for 60fps
    CAM_Y += (ty - CAM_Y).clamp(-3, 3);
}

unsafe fn snap_camera() {
    if PLAYER == NONE {
        return;
    }
    let (tx, ty) = camera_target();
    CAM_X = tx;
    CAM_Y = ty;
}

// ====================================================================
// Lifecycle
// ====================================================================

/// Load a level: point the active cart at its PX9-decompressed tilemap, set
/// the dims, start its music, and spawn its objects.
unsafe fn goto_level(index: i32) {
    let idx = index.clamp(1, (LEVELS.len() - 1) as i32);
    LVL_INDEX = idx;
    let m = &LEVELS[idx as usize];
    LEVEL_CHECKPOINT = -1; // checkpoints don't carry across levels
    // Titled levels show a 60-frame intro card (doubled to 120 for 60fps).
    LEVEL_INTRO = if m.title.is_empty() { 0 } else { 120 };
    if idx == 2 {
        psfx(17, 8, 16);
    }
    LVL_W = m.width;
    LVL_H = m.height;
    CAM_MODE = m.camera_mode;
    // Copy the level into a mutable scratch tilemap (like the cart's PX9 dest
    // buffer) so restart_level can blank object-spawn tiles from the render.
    let n = m.tiles.len().min(LEVEL_BUF.len());
    LEVEL_BUF[..n].copy_from_slice(&m.tiles[..n]);
    // Switch the tilemap the backend reads (gfx + flags unchanged, already in VRAM).
    backend::set_cart(Cart {
        gfx: &GFX_DATA,
        tilemap: &LEVEL_BUF,
        tile_flags: &TILE_FLAGS,
        map_w: m.width as usize,
    });
    if CUR_MUSIC != m.music {
        CUR_MUSIC = m.music;
        music(m.music);
    }
    restart_level();
}

unsafe fn next_level() {
    let next = if LVL_INDEX + 1 >= LEVELS.len() as i32 { 1 } else { LVL_INDEX + 1 };
    goto_level(next);
}

/// Spawn the current level's objects from its tilemap.
unsafe fn restart_level() {
    OBJ = [OBJ0; MAX_OBJ];
    PLAYER = NONE;
    LAST_BERRY = NONE;
    CAM_X = 0;
    CAM_Y = 0;
    C_OFFSET = 0;
    C_FLAG = false;
    INFADE = 0; // restart the level-entry wipe
    SFX_TIMER = 0;
    HAVE_GRAPPLE = LVL_INDEX > 2; // levels 1-2: pick the grapple up in-level
    FREEZE = 0;
    SHAKE = 0;
    // Re-copy the level into the scratch tilemap each restart (the previous
    // pass blanked spawn tiles in place; without this, a respawn finds none).
    let m = &LEVELS[LVL_INDEX as usize];
    let n = m.tiles.len().min(LEVEL_BUF.len());
    LEVEL_BUF[..n].copy_from_slice(&m.tiles[..n]);
    let mut spawned_player = false;
    for ty in 0..LVL_H {
        for tx in 0..LVL_W {
            let t = type_of_tile(tile_at(tx, ty));
            if t == ObjType::None {
                continue;
            }
            let oid = LVL_INDEX * 100 + tx + ty * 128;
            // already-collected berries don't respawn (or re-count)
            if t == ObjType::Berry && is_collected(oid) {
                continue;
            }
            if t == ObjType::Player {
                // with a checkpoint active, skip the level's start spawn; the
                // active checkpoint spawns the player instead.
                if spawned_player || LEVEL_CHECKPOINT >= 0 {
                    continue;
                }
                spawned_player = true;
                CAM_X = (tx * 8 - 64).clamp(0, (LVL_W * 8 - 128).max(0));
            }
            create(t, tx * 8, ty * 8);
            // if this is the active checkpoint, spawn the player on it
            if t == ObjType::Checkpoint && oid == LEVEL_CHECKPOINT && !spawned_player {
                create(ObjType::Player, tx * 8, ty * 8);
                spawned_player = true;
                CAM_X = (tx * 8 - 64).clamp(0, (LVL_W * 8 - 128).max(0));
            }
            // Blank the spawn tile so the map doesn't draw it under the object.
            let bi = (tx + ty * LVL_W) as usize;
            if bi < LEVEL_BUF.len() {
                LEVEL_BUF[bi] = 0;
            }
        }
    }
    snap_camera();
}

pub fn init() {
    unsafe {
        rng::srand(42);
        init_particles();
        FRAMES = 0;
        TIMER_F = 0;
        SECONDS = 0;
        MINUTES = 0;
        BERRY_COUNT = 0;
        DEATH_COUNT = 0;
        COLLECTED_N = 0; // fresh run: no berries collected yet
        LEVEL_CHECKPOINT = -1;
        SHOW_SCORE = 0;
        TITLE_FLASH = i32::MIN;
        LEVEL_INTRO = 0;
        INFADE = 120; // 60fps: "fade complete" sentinel (PICO-8 60 doubled)
        SHAKE = 0;
        FREEZE = 0;
        CAM_X = 0;
        CAM_Y = 0;
        // Start on the titlescreen (level 0); its music is shared with level 1.
        LVL_INDEX = 0;
        CUR_MUSIC = 38;
        music(38);
    }
}

pub fn update() {
    unsafe {
        FRAMES += 1; // time() wobble clock -- always advances

        // titlescreen
        if LVL_INDEX == 0 {
            if TITLE_FLASH != i32::MIN {
                TITLE_FLASH -= 1;
                if TITLE_FLASH < -60 {
                    goto_level(1);
                }
            } else if held(IN_JUMP) || held(IN_GRAPPLE) {
                TITLE_FLASH = 100; // PICO-8 50 frames doubled for 60fps
                sfx::play(22);
            }
            return;
        }

        // level intro card
        if LEVEL_INTRO > 0 {
            LEVEL_INTRO -= 1;
            if LEVEL_INTRO == 0 {
                psfx(17, 24, 9);
            }
            return;
        }

        // normal-level timers
        SFX_TIMER = (SFX_TIMER - 1).max(0);
        if SHAKE > 0 {
            SHAKE -= 1;
        }
        INFADE = (INFADE + 1).min(120); // 60fps: PICO-8 fade timer doubled
        if LVL_INDEX != 8 {
            TIMER_F += 1;
            if TIMER_F >= 60 {
                TIMER_F = 0;
                SECONDS += 1;
            }
            if SECONDS >= 60 {
                SECONDS = 0;
                MINUTES += 1;
            }
        }

        update_input();
        if FREEZE > 0 {
            FREEZE -= 1;
            return;
        }
        // player first, then the rest (snapshot order). Honor the player's per-object
        // freeze like PICO-8's object loop does (skip + decrement while freeze > 0):
        // the grapple sets freeze = 2 on latch/miss for a brief hitstop.
        if PLAYER != NONE && OBJ[PLAYER].exists {
            if OBJ[PLAYER].freeze > 0 {
                OBJ[PLAYER].freeze -= 1;
            } else {
                player_update(PLAYER);
            }
        }
        for i in 0..MAX_OBJ {
            if OBJ[i].exists && OBJ[i].otype != ObjType::Player {
                obj_update(i);
            }
        }
        // compact destroyed
        for i in 0..MAX_OBJ {
            if OBJ[i].exists && OBJ[i].destroyed {
                OBJ[i].exists = false;
                if i == PLAYER {
                    PLAYER = NONE;
                }
            }
        }
        update_camera();
    }
}

/// Sparse-dithered background pillars (level.columns). The backend has no fillp,
/// so approximate the dither with thin parallax-scrolled vertical streaks.
unsafe fn draw_columns(lv: &LevelMeta) {
    let par = CAM_X / 10; // camera_x * 0.1 parallax
    let y1 = (lv.height * 8) as i16;
    let mut x = 0;
    while x < lv.width {
        let tx = (x * 8 + par) as i16;
        let w = ((x % 2) * 8 + 8) as i16;
        let mut sx = tx;
        while sx < tx + w {
            backend::line(sx, 0, sx, y1, lv.columns);
            sx += 4;
        }
        x += 1 + x % 7;
    }
}

pub fn draw() {
    unsafe {
        if LVL_INDEX == 0 {
            draw_title();
            return;
        }
        if LEVEL_INTRO > 0 {
            draw_intro();
            return;
        }
        let lv = level();

        // camera with PICO-8's random both-axis shake
        let (jx, jy) = if SHAKE > 0 {
            (rng::rnd(fi(5)).to_int() - 2, rng::rnd(fi(5)).to_int() - 2)
        } else {
            (0, 0)
        };
        backend::camera((CAM_X + jx) as i16, (CAM_Y + jy) as i16);

        // cls(level.bg): fill the playfield (plus a margin) with the bg colour
        backend::rectfill((CAM_X - 20) as i16, (CAM_Y - 20) as i16, (CAM_X + 148) as i16, (CAM_Y + 148) as i16, lv.bg);

        // background clouds in the level's colour
        draw_clouds(fi(1), Fix32::ZERO, fi(1), lv.clouds, 26);

        // background pillars
        if lv.columns >= 0 {
            draw_columns(lv);
        }

        // tilemap (base pass)
        let cam_col = CAM_X / 8;
        let cam_row = CAM_Y / 8;
        let cols = 18.min(LVL_W - cam_col);
        let rows = 18.min(LVL_H - cam_row);
        backend::map(cam_col, cam_row, (cam_col * 8) as i16, (cam_row * 8) as i16, cols, rows, 0);
        // per-level palette swap: overdraw the flag-7 tiles with the swap applied
        if lv.pal_id != 0 {
            backend::flush(); // let the base tiles finish with the unswapped CLUT
            apply_level_pal(lv.pal_id);
            for y in cam_row..(cam_row + rows) {
                for x in cam_col..(cam_col + cols) {
                    let tile = tile_at(x, y);
                    if tile != 0 && backend::fget(tile, 0) && backend::fget(tile, 7) {
                        backend::spr(tile, (x * 8) as i16, (y * 8) as i16, false, false);
                    }
                }
            }
            backend::flush(); // finish the swapped tiles before resetting the CLUT
            backend::pal_reset();
        }

        // score panel
        if SHOW_SCORE > 210 {
            // 60fps: PICO-8 105 doubled
            draw_score();
        }

        // objects (player drawn last)
        for i in 0..MAX_OBJ {
            let o = OBJ[i];
            if !o.exists || o.destroyed || o.otype == ObjType::Player {
                continue;
            }
            draw_object(i);
        }
        if PLAYER != NONE && OBJ[PLAYER].exists {
            player_draw(PLAYER);
        }

        // drifting snow
        draw_snow();

        // foreground fog, pinned to the level's bottom edge
        if lv.fogmode != 0 {
            draw_clouds(fx(1.25), fi(LVL_H * 8 + 1), Fix32::ZERO, 7, 16);
        }

        // screen wipes (level finish / entry)
        draw_wipes();

        // run timer HUD (hidden briefly during the entry fade)
        if INFADE < 90 {
            draw_time((CAM_X + 4) as i16, (CAM_Y + 4) as i16);
        }

        // Cover the 32px screen margins each side (the 128px playfield is drawn 2x =
        // 256 wide, centred in 320). Reset the camera first so the borders stay fixed.
        backend::camera(0, 0);
        backend::rectfill(-20, -20, 1, 148, 0);
        backend::rectfill(127, -20, 148, 148, 0);
    }
}

// ---- draw helpers ----

/// `print` centred on `cx` (PICO-8 font is 4px/char).
unsafe fn print_center(text: &[u8], cx: i16, y: i16, c: i32) {
    let x = cx - (text.len() as i16 * 4 - 1) / 2;
    backend::print(text, x, y, c);
}

/// PICO-8 `pset` -- a single pixel (the backend has no pset; a 1px rectfill is it).
#[inline]
unsafe fn pset(x: i16, y: i16, c: i32) {
    backend::rectfill(x, y, x, y, c);
}

/// PICO-8 `draw_sine_h` -- a horizontal wavy line from x0 to x1 at height `y`,
/// each lit pixel sitting on a colour-1 underline, with the wave faded in/out at
/// the ends and vertical gaps between samples filled so the rope reads as one
/// continuous strand. Used for the active grapple rope. Faithful port of the cart.
unsafe fn draw_sine_h(
    x0: Fix32,
    x1: Fix32,
    y: Fix32,
    col: i32,
    amplitude: Fix32,
    time_freq: Fix32,
    x_freq: Fix32,
    fade_x_dist: i32,
) {
    let t = fi(FRAMES) / fi(60); // time()
    let yi = y.floor_int() as i16;
    pset(x0.floor_int() as i16, yi, col);
    pset(x1.floor_int() as i16, yi, col);

    let x_sign: i32 = if x1.0 >= x0.0 { 1 } else { -1 };
    let x_max = (x1 - x0).abs().to_int() - 1;
    let mut last_y = y;
    let mut i = 1;
    while i <= x_max {
        let fade = if i <= fade_x_dist {
            fi(i) / fi(fade_x_dist + 1)
        } else if i > x_max - fade_x_dist + 1 {
            fi(x_max + 1 - i) / fi(fade_x_dist + 1)
        } else {
            fi(1)
        };
        let ax = (x0.to_int() + i * x_sign) as i16;
        let ay = y + (t * time_freq + fi(i) * x_freq).sin() * amplitude * fade;
        pset(ax, (ay + fi(1)).floor_int() as i16, 1);
        pset(ax, ay.floor_int() as i16, col);

        // fill the vertical gap back to the previous sample so the rope is solid
        let step = if ay.0 > last_y.0 { 1 } else if ay.0 < last_y.0 { -1 } else { 0 };
        let mut fy = ay;
        while step != 0 && (fy - last_y).abs() > fi(1) {
            fy = fy - fi(step);
            pset(ax - x_sign as i16, (fy + fi(1)).floor_int() as i16, 1);
            pset(ax - x_sign as i16, fy.floor_int() as i16, col);
        }
        last_y = ay;
        i += 1;
    }
}

/// Outline rectangle (the backend only has filled rects).
unsafe fn rect_outline(x0: i16, y0: i16, x1: i16, y1: i16, c: i32) {
    backend::line(x0, y0, x1, y0, c);
    backend::line(x0, y1, x1, y1, c);
    backend::line(x0, y0, x0, y1, c);
    backend::line(x1, y0, x1, y1, c);
}

/// Two-digit decimal into `buf[0..2]`.
fn two_digits(n: i32, buf: &mut [u8]) {
    buf[0] = b'0' + ((n / 10) % 10) as u8;
    buf[1] = b'0' + (n % 10) as u8;
}

/// `draw_time`: the HH:MM:SS run timer.
unsafe fn draw_time(x: i16, y: i16) {
    let m = MINUTES % 60;
    let h = MINUTES / 60;
    backend::rectfill(x, y, x + 32, y + 6, 0);
    let mut buf = [b':'; 8];
    two_digits(h, &mut buf[0..2]);
    two_digits(m, &mut buf[3..5]);
    two_digits(SECONDS, &mut buf[6..8]);
    backend::print(&buf, x + 1, y + 1, 7);
}

/// A sprite bobbing on `sin(time())*2` (grapple pickup + uncollected berry).
unsafe fn draw_bob(n: i32, x: i16, y: Fix32) {
    if n < 0 {
        return;
    }
    let bob = ((fi(FRAMES) / fi(60)).sin() * fi(2)).to_int() as i16;
    backend::spr(n, x, y.to_int() as i16 + bob, true, false);
}

/// Sprite 13 (the checkpoint flag) as 8x8 PICO-8 palette indices, [row][col];
/// 0 = transparent. Column 0 is the pole (colour 4); columns 1-7 rows 0-3 are the
/// flag fabric (colour 2).
const FLAG13: [[u8; 8]; 8] = [
    [4, 2, 2, 2, 2, 2, 2, 2],
    [4, 2, 2, 2, 2, 2, 2, 2],
    [4, 2, 2, 2, 2, 2, 2, 2],
    [4, 2, 2, 2, 2, 2, 2, 2],
    [2, 0, 0, 0, 0, 0, 0, 0],
    [4, 0, 0, 0, 0, 0, 0, 0],
    [4, 0, 0, 0, 0, 0, 0, 0],
    [4, 0, 0, 0, 0, 0, 0, 0],
];

/// Active-checkpoint waving flag, an ad-hoc pixel replica of the PICO-8 sspr
/// effect: column 0 (the pole) is static; columns 1-7 (the fabric) are recoloured
/// 2->11 and offset per-column by `sin(-time()*2 + col*0.25) * (col-1)*0.2`, so the
/// free end ripples more than the pole side. Drawn pixel-by-pixel via rectfill.
unsafe fn draw_active_flag(x: i16, y: i16) {
    let t = fi(FRAMES) / fi(60);
    for col in 0..8i32 {
        let off = if col == 0 {
            0
        } else {
            ((-(t * fi(2)) + fi(col) * fx(0.25)).sin() * fi(col - 1) * fx(0.2)).to_int()
        } as i16;
        for row in 0..8usize {
            let mut c = FLAG13[row][col as usize] as i32;
            if c == 0 {
                continue; // transparent
            }
            if col > 0 && c == 2 {
                c = 11; // pal(2,11) on the fabric columns only
            }
            let px = x + col as i16;
            let py = y + off + row as i16;
            backend::rectfill(px, py, px, py, c);
        }
    }
}

/// Per-object custom draw (snowball shadow, berry bob/popup, crumble crack, ...).
unsafe fn draw_object(i: usize) {
    let o = OBJ[i];
    let n = o.spr.floor_int();
    let x = o.x.to_int() as i16;
    let y = o.y.to_int() as i16;
    match o.otype {
        ObjType::Snowball => {
            if n < 0 {
                return;
            }
            // 1px drop-shadow: the sprite with colour 7 -> 1, then the real sprite.
            backend::flush();
            backend::pal(7, 1);
            backend::spr(n, x, y + 1, o.flip_x, o.flip_y);
            backend::flush();
            backend::pal_reset();
            backend::spr(n, x, y, o.flip_x, o.flip_y);
        }
        ObjType::GrapplePickup => draw_bob(n, x, o.y),
        ObjType::Berry => {
            if !o.stop {
                draw_bob(n, x, o.y); // bobbing berry
                if o.flash > 0 {
                    // pickup flash: expanding white ring + a solid flash over the berry
                    backend::circ(x + 4, y + 4, (o.flash * 3) as i16, 7);
                    backend::circfill(x + 4, y + 4, 5, 7);
                }
            } else {
                // "1000" score popup, floating up, blinking 7/14 over an 8 shadow
                let c = if o.timer % 4 < 2 { 7 } else { 14 };
                backend::print(b"1000", x - 4, y + 1, 8);
                backend::print(b"1000", x - 4, y, c);
            }
        }
        ObjType::Checkpoint => {
            if LEVEL_CHECKPOINT == o.oid {
                draw_active_flag(x, y); // per-column waving flag (ad-hoc sspr replica)
            } else if n >= 0 {
                backend::spr(n, x, y, o.flip_x, o.flip_y);
            }
        }
        ObjType::Crumble => {
            if n >= 0 {
                backend::spr(n, x, y, o.flip_x, o.flip_y);
            }
            // "about to break" crack: PICO-8 dithers the tile; approximate with a
            // coarse colour-1 checker (timer 2 doubled to 4 for 60fps).
            if o.breaking && o.timer > 4 {
                let mut dy = 0i16;
                while dy < 8 {
                    let mut dx = ((dy / 2) % 2) * 2;
                    while dx < 8 {
                        backend::rectfill(x + dx, y + dy, x + dx + 1, y + dy + 1, 1);
                        dx += 4;
                    }
                    dy += 2;
                }
            }
        }
        _ => {
            if n >= 0 {
                backend::spr(n, x, y, o.flip_x, o.flip_y);
            }
        }
    }
}

/// Titlescreen (level_index 0): logo, border, credits, snow, flash transition.
unsafe fn draw_title() {
    backend::camera(0, 0);
    backend::rectfill(-20, -20, 148, 148, 0); // cls(0)

    // flash transition: remap the whole palette to a flat colour as we strobe out
    let flashing = TITLE_FLASH != i32::MIN;
    let mut swapped = false;
    if flashing {
        let c = if TITLE_FLASH > 20 {
            if TITLE_FLASH % 20 < 10 { 7 } else { 10 }
        } else if TITLE_FLASH > 10 {
            2
        } else if TITLE_FLASH > 0 {
            1
        } else {
            0
        };
        if c < 10 {
            backend::flush();
            for k in 1..=15 {
                backend::pal(k, c);
            }
            swapped = true;
        }
    }

    // logo: sspr(64,32, 64,32, 36,32) -> the 8x4 sprite block at sheet (col 8,row 4)
    for ty in 0..4 {
        for tx in 0..8 {
            let nn = (4 + ty) * 16 + (8 + tx);
            backend::spr(nn, (36 + tx * 8) as i16, (32 + ty * 8) as i16, false, false);
        }
    }
    rect_outline(0, 0, 127, 127, 7);
    print_center(b"lANI'S tREK", 64, 68, 14);
    print_center(b"a game by", 64, 80, 1);
    print_center(b"maddy thorson", 64, 87, 5);
    print_center(b"noel berry", 64, 94, 5);
    print_center(b"lena raine", 64, 101, 5);
    draw_snow();

    if swapped {
        backend::flush();
        backend::pal_reset();
    }
}

/// Level intro card (level_intro > 0): "level N" + the level title.
unsafe fn draw_intro() {
    backend::camera(0, 0);
    backend::rectfill(-20, -20, 148, 148, 0); // cls(0)
    draw_time(4, 4);
    if LVL_INDEX != 8 {
        let mut buf = [0u8; 8];
        let pre = b"level ";
        buf[..6].copy_from_slice(pre);
        two_digits(LVL_INDEX - 2, &mut buf[6..8]);
        // drop a leading zero for single-digit level numbers
        if buf[6] == b'0' {
            print_center(&[buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[7]], 64, 64 - 8, 7);
        } else {
            print_center(&buf, 64, 64 - 8, 7);
        }
    }
    print_center(level().title.as_bytes(), 64, 64, 7);
}

/// End-of-game score panel (berries / time / deaths).
unsafe fn draw_score() {
    backend::rectfill(34, 392, 98, 434, 1);
    backend::rectfill(32, 390, 96, 432, 0);
    rect_outline(32, 390, 96, 432, 7);
    backend::spr(21, 44, 396, false, false);
    let mut bn = [b'X', b' ', 0, 0];
    two_digits(BERRY_COUNT, &mut bn[2..4]);
    backend::print(&bn, 56, 398, 7);
    backend::spr(87, 44, 408, false, false);
    draw_time(56, 408);
    backend::spr(71, 44, 420, false, false);
    let mut dn = [b'X', b' ', 0, 0];
    two_digits(DEATH_COUNT, &mut dn[2..4]);
    backend::print(&dn, 56, 421, 7);
}

/// Wavy screen wipes: out-wipe on level finish, in-wipe (infade) on entry.
unsafe fn draw_wipes() {
    let wave = |i: i32, e: Fix32| -> i32 {
        (fi(191) * e - fi(32) + (fi(i) * fx(0.2)).sin() * fi(16) + fi(127 - i) * fx(0.25)).to_int()
    };
    if PLAYER != NONE && OBJ[PLAYER].exists && OBJ[PLAYER].wipe_timer > 10 {
        let e = fi(OBJ[PLAYER].wipe_timer - 10) / fi(24); // 60fps: -5/12 doubled
        for i in 0..128 {
            let s = wave(i, e);
            backend::rectfill(CAM_X as i16, (CAM_Y + i) as i16, (CAM_X + s) as i16, (CAM_Y + i) as i16, 0);
        }
    }
    if INFADE < 30 {
        let e = fi(INFADE) / fi(24); // 60fps: PICO-8 /12 doubled
        for i in 0..128 {
            let s = wave(i, e);
            backend::rectfill((CAM_X + s) as i16, (CAM_Y + i) as i16, (CAM_X + 128) as i16, (CAM_Y + i) as i16, 0);
        }
    }
}
