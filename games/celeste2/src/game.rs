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
use crate::assets::levels::LEVELS;
use crate::assets::tilemap::TILE_FLAGS;
use pico8::backend::{self, Cart};
use pico8::fixed::{fx, Fix32};
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

        let jump = held(IN_JUMP);
        JUMP_PRESSED = if jump && !JUMP_HELD { 4 } else if jump { (JUMP_PRESSED - 1).max(0) } else { 0 };
        JUMP_HELD = jump;

        let grap = held(IN_GRAPPLE);
        GRAP_PRESSED = if grap && !GRAP_HELD { 4 } else if grap { (GRAP_PRESSED - 1).max(0) } else { 0 };
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

// ---- audio: PICO-8 psfx(id, off, len) -> a sub-range of sfx `id` ----
fn psfx(id: i32, off: i32, len: i32) {
    sfx::play_range(id, off, len);
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
};

const MAX_OBJ: usize = 48;
static mut OBJ: [Obj; MAX_OBJ] = [OBJ0; MAX_OBJ];
static mut PLAYER: usize = NONE;
static mut HAVE_GRAPPLE: bool = false;
static mut FREEZE: i32 = 0;
static mut SHAKE: i32 = 0;
static mut CAM_X: i32 = 0;
static mut CAM_Y: i32 = 0;
static mut CAM_MODE: i32 = 1;
static mut FRAMES: i32 = 0; // for time()-driven wobble
static mut SCARF: [Vec2; 5] = [VZ; 5];

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
        OBJ[i].spr = fi(type_spr(otype));
        init_obj(i);
        i
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
    None,    // pass through (no stop)
    Stop,    // zero remainder + speed (base object)
    Player,  // player corner-correct then stop
}

unsafe fn move_x(i: usize, amount: Fix32, c: Collide) -> bool {
    do_move(i, amount * HALF, true, c)
}
unsafe fn move_y(i: usize, amount: Fix32, c: Collide) -> bool {
    do_move(i, amount * HALF, false, c)
}
// One-time position snap (full amount, no 60fps halving).
unsafe fn move_x_exact(i: usize, amount: Fix32) {
    do_move(i, amount, true, Collide::Stop);
}
unsafe fn move_y_exact(i: usize, amount: Fix32) {
    do_move(i, amount, false, Collide::Stop);
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
    }
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
    psfx(14, 16, 16);
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
            OBJ[i].timer = (OBJ[i].x.to_int() / 8) % 32;
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
        OBJ[i].grapple_x = approach(OBJ[i].grapple_x, OBJ[i].x, fi(12));
        OBJ[i].grapple_y = approach(OBJ[i].grapple_y, OBJ[i].y - fi(3), fi(6));
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
                    OBJ[i].freeze = 2;
                    psfx(14, 0, 5);
                    break;
                }
                let new_dist = (OBJ[i].grapple_x - OBJ[i].x).abs();
                if hit == 2 || (hit == 0 && new_dist >= fi(64)) {
                    psfx(if hit == 2 { 7 } else { 14 }, 8, 3);
                    OBJ[i].grapple_retract = true;
                    OBJ[i].freeze = 2;
                    OBJ[i].state = 0;
                    break;
                }
            }
            OBJ[i].grapple_wave = approach(OBJ[i].grapple_wave, fi(1), fx(0.2));
            OBJ[i].spr = fi(3);
            if !grabbed && (!GRAP_HELD || (OBJ[i].y - OBJ[i].grapple_y).abs() > fi(8)) {
                OBJ[i].state = 0;
                OBJ[i].grapple_retract = true;
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
            OBJ[i].grapple_wave = approach(OBJ[i].grapple_wave, Fix32::ZERO, fx(0.6));
            let dead = OBJ[i].grapple_hit != NONE && OBJ[OBJ[i].grapple_hit].destroyed;
            if !GRAP_HELD || dead {
                OBJ[i].state = 0;
                OBJ[i].t_grapple_jump_grace = 4;
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
                    OBJ[i].t_grapple_jump_grace = 4;
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
                if OBJ[i].t_grapple_pickup == 0 {
                    music(39);
                }
                if OBJ[i].t_grapple_pickup == 70 {
                    music(22);
                }
                if OBJ[i].t_grapple_pickup > 80 {
                    OBJ[i].state = 0;
                }
                OBJ[i].t_grapple_pickup += 1;
            }
        }
        99 | 100 => {
            OBJ[i].wipe_timer += 1;
            if OBJ[i].wipe_timer > 20 {
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

    // sprite
    if OBJ[i].state != 11 {
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
                    OBJ[i].freeze = 1;
                    SHAKE = 2;
                    psfx(8, 16, 4);
                }
            }
            ObjType::Berry => {
                if overlaps(i, j, Fix32::ZERO, Fix32::ZERO) && OBJ[j].link == NONE && OBJ[j].timer == 0 && !OBJ[j].stop {
                    OBJ[j].link = i; // collected -> follow player
                    OBJ[j].timer = 0;
                    psfx(7, 12, 4);
                }
            }
            ObjType::Checkpoint => {
                let _ = j;
            }
            _ => {}
        }
    }

    // reached the right edge -> advance to the next level
    if OBJ[i].state < 99 && OBJ[i].x > fi(LVL_W * 8 - 4) {
        OBJ[i].state = 100;
        OBJ[i].wipe_timer = 0;
        return;
    }
    // death / pit-finish
    if OBJ[i].state < 99 && (OBJ[i].y > fi(LVL_H * 8 + 16) || hazard_check(i, Fix32::ZERO, Fix32::ZERO)) {
        if OBJ[i].x > fi(LVL_W * 8 - 64) {
            OBJ[i].state = 100;
            OBJ[i].wipe_timer = -15;
        } else {
            player_die(i);
        }
        return;
    }
    if OBJ[i].y < fi(-16) {
        OBJ[i].y = fi(-16);
        OBJ[i].spd.y = Fix32::ZERO;
    }
}

unsafe fn player_draw(i: usize) {
    let o = OBJ[i];

    // death fx: an expanding ring of shrinking circles
    if o.state == 99 {
        let e = fi(o.wipe_timer) / fi(10);
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

    // scarf: 5 damped trailing segments (PICO-8 colour 10)
    let t = fi(FRAMES) / fi(60);
    let mut last = Vec2 { x: o.x - fi(o.facing), y: o.y - fi(3) };
    for k in 1..=5 {
        let mut s = SCARF[k - 1];
        s.x += (last.x - s.x - fi(o.facing)) / fx(1.5);
        let ki = fi(k as i32);
        let wob = (ki * fx(0.25) + t).sin() * ki * fx(0.25);
        s.y += ((last.y - s.y) + wob) / fi(2);
        SCARF[k - 1] = s;
        let (sx, sy) = (s.x.to_int() as i16, s.y.to_int() as i16);
        backend::rectfill(sx, sy, sx, sy, 10);
        let mx = ((s.x + last.x) / fi(2)).to_int() as i16;
        let my = ((s.y + last.y) / fi(2)).to_int() as i16;
        backend::rectfill(mx, my, mx, my, 10);
        last = s;
    }

    // grapple rope (simple two-tone line)
    if o.state >= 10 && o.state <= 12 {
        backend::line(o.x.to_int() as i16, (o.y - fi(3)).to_int() as i16, o.grapple_x.to_int() as i16, (o.grapple_y).to_int() as i16, 7);
    }
    if o.grapple_retract {
        backend::line(o.x.to_int() as i16, (o.y - fi(2)).to_int() as i16, o.grapple_x.to_int() as i16, (o.grapple_y + fi(1)).to_int() as i16, 7);
    }
    backend::spr(o.spr.floor_int(), (o.x - fi(4)).to_int() as i16, (o.y - fi(8)).to_int() as i16, o.facing != 1, false);
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
            // snowball wall bounce
            if move_x(i, sx, Collide::Stop) && !snowball_hurt(i) {
                OBJ[i].spd.x = -OBJ[i].spd.x;
                OBJ[i].rem.x = Fix32::ZERO;
            }
            if move_y(i, sy, Collide::Stop) {
                if OBJ[i].spd.y >= fi(4) {
                    OBJ[i].spd.y = fi(-2);
                } else if OBJ[i].spd.y >= fi(1) {
                    OBJ[i].spd.y = fi(-1);
                } else {
                    OBJ[i].spd.y = Fix32::ZERO;
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
            move_x(i, sx, Collide::Stop);
            move_y(i, sy, Collide::Stop);
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
                    OBJ[i].x = OBJ[i].ox;
                    OBJ[i].y = OBJ[i].oy;
                    OBJ[i].breaking = false;
                    OBJ[i].timer = 0;
                    psfx(17, 5, 3);
                }
            }
        }
        ObjType::Berry => {
            if OBJ[i].stop {
                // collected popup
                OBJ[i].timer += 1;
                if OBJ[i].timer > 5 {
                    OBJ[i].y -= fx(0.1);
                }
                if OBJ[i].timer > 60 {
                    OBJ[i].destroyed = true;
                }
            } else if OBJ[i].link != NONE {
                let p = OBJ[i].link;
                OBJ[i].x += (OBJ[p].x - OBJ[i].x) / fi(8);
                OBJ[i].y += (OBJ[p].y - fi(4) - OBJ[i].y) / fi(8);
                let grounded = check_solid(p, Fix32::ZERO, fi(1)) && OBJ[p].state != 99;
                if grounded {
                    OBJ[i].timer += 1;
                } else {
                    OBJ[i].timer = 0;
                }
                if OBJ[i].timer > 6 || OBJ[p].x > fi(LVL_W * 8 - 7) {
                    psfx(8, 8, 8);
                    OBJ[i].stop = true;
                    OBJ[i].timer = 0;
                }
            }
        }
        ObjType::SpawnerR | ObjType::SpawnerL => {
            OBJ[i].timer += 1;
            if OBJ[i].timer >= 32 && (OBJ[i].x.to_int() - 64 - CAM_X).abs() < 128 {
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
unsafe fn camera_target() -> (i32, i32) {
    let px = OBJ[PLAYER].x.to_int();
    let py = OBJ[PLAYER].y.to_int();
    let wlim = (LVL_W * 8 - 128).max(0);
    let hlim = (LVL_H * 8 - 128).max(0);
    let cx = |v: i32| v.max(0).min(wlim);
    let cy = |v: i32| v.max(0).min(hlim);
    match CAM_MODE {
        1 => (if px < 42 { 0 } else { (px - 48).max(40).min(wlim) }, 0),
        2 => (if px < 120 { 0 } else if px > 136 { 128 } else { px - 64 }, cy(py - 64)),
        3 => (cx(px - 56), 0),
        4 => {
            let sx = if px % 128 > 8 && px % 128 < 120 { (px / 128) * 128 + 64 } else { px };
            let sy = if py % 128 > 4 && py % 128 < 124 { (py / 128) * 128 + 64 } else { py };
            (cx(sx - 64), cy(sy - 64))
        }
        5 => (cx(px - 32), 0),
        6 | 7 => (cx(px - 48), 0),
        8 => (0, cy(py - 32)),
        _ => (cx(px - 64), 0),
    }
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
    CAM_X = 0;
    HAVE_GRAPPLE = LVL_INDEX > 2; // levels 1-2: pick the grapple up in-level
    FREEZE = 0;
    SHAKE = 0;
    let mut spawned_player = false;
    for ty in 0..LVL_H {
        for tx in 0..LVL_W {
            let t = type_of_tile(tile_at(tx, ty));
            if t == ObjType::None {
                continue;
            }
            if t == ObjType::Player {
                if spawned_player {
                    continue; // one player at the first spawn tile
                }
                spawned_player = true;
                CAM_X = (tx * 8 - 64).clamp(0, (LVL_W * 8 - 128).max(0));
            }
            create(t, tx * 8, ty * 8);
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
        CUR_MUSIC = -1;
        goto_level(1);
    }
}

pub fn update() {
    unsafe {
        FRAMES += 1;
        update_input();
        if FREEZE > 0 {
            FREEZE -= 1;
            return;
        }
        if SHAKE > 0 {
            SHAKE -= 1;
        }
        // player first, then the rest (snapshot order)
        if PLAYER != NONE && OBJ[PLAYER].exists {
            player_update(PLAYER);
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

pub fn draw() {
    unsafe {
        let shake_y = if SHAKE > 0 { -1 } else { 0 };
        backend::camera(CAM_X as i16, (CAM_Y + shake_y) as i16);
        let cam_col = CAM_X / 8;
        let cam_row = CAM_Y / 8;
        let cols = 18.min(LVL_W - cam_col);
        let rows = 18.min(LVL_H - cam_row);
        backend::map(cam_col, cam_row, (cam_col * 8) as i16, (cam_row * 8) as i16, cols, rows, 0);

        for i in 0..MAX_OBJ {
            let o = OBJ[i];
            if !o.exists || o.destroyed {
                continue;
            }
            if o.otype == ObjType::Player {
                player_draw(i);
            } else if o.spr.floor_int() >= 0 {
                backend::spr(o.spr.floor_int(), o.x.to_int() as i16, o.y.to_int() as i16, o.flip_x, o.flip_y);
            }
        }
    }
}
