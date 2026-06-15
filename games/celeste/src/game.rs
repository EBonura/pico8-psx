//! Native Rust port of ccleste (PICO-8 Celeste Classic game logic),
//! from psx-lab/src/celeste/celeste.cpp. Mirrors the C memory model
//! (`static mut` globals + raw-pointer objects) so the loading-jank,
//! the kill-player dummy-copy, and the slot re-iteration behave
//! bit-for-bit like the original.

#![allow(clippy::needless_return)]

use core::ptr::addr_of_mut;
use pico8::backend;
use pico8::fixed::{fx, Fix32};
use pico8::rng;
use pico8::sfx;

const MAX_OBJECTS: usize = 30;
const FRUIT_COUNT: usize = 30;

#[inline]
fn fi(n: i32) -> Fix32 {
    Fix32::from_int(n)
}

// ---- input (k_left..k_dash bitmask, set each frame by main) ----
const K_LEFT: i32 = 0;
const K_RIGHT: i32 = 1;
const K_UP: i32 = 2;
const K_DOWN: i32 = 3;
const K_JUMP: i32 = 4;
const K_DASH: i32 = 5;

pub fn set_input(mask: u8) {
    pico8::input::set_buttons(mask);
}
fn p8btn(b: i32) -> bool {
    pico8::input::btn(b)
}
fn p8btnp(b: i32) -> bool {
    pico8::input::btnp(b)
}

// ---- PICO-8 platform wrappers (int-cast args, like the C macros) ----
fn p8spr(s: i32, x: i32, y: i32, fx_: bool, fy: bool) {
    backend::spr(s, x as i16, y as i16, fx_, fy);
}
fn p8map(mx: i32, my: i32, tx: i32, ty: i32, mw: i32, mh: i32, mask: i32) {
    backend::map(mx, my, tx as i16, ty as i16, mw, mh, mask);
}
fn p8rectfill(x: i32, y: i32, x2: i32, y2: i32, c: i32) {
    backend::rectfill(x as i16, y as i16, x2 as i16, y2 as i16, c);
}
fn p8circfill(x: i32, y: i32, r: i32, c: i32) {
    backend::circfill(x as i16, y as i16, r as i16, c);
}
fn p8line(x: i32, y: i32, x2: i32, y2: i32, c: i32) {
    backend::line(x as i16, y as i16, x2 as i16, y2 as i16, c);
}
fn p8print(s: &[u8], x: i32, y: i32, c: i32) {
    backend::print(s, x as i16, y as i16, c);
}
fn p8camera(x: i32, y: i32) {
    backend::camera(x as i16, y as i16);
}
fn p8pal(a: i32, b: i32) {
    backend::pal(a, b);
}
fn p8pal_reset() {
    backend::pal_reset();
}
fn p8mget(x: i32, y: i32) -> i32 {
    backend::mget(x, y)
}
fn p8fget(t: i32, f: i32) -> bool {
    backend::fget(t, f)
}

// ---- value types ----
#[derive(Clone, Copy)]
struct Vec2 {
    x: Fix32,
    y: Fix32,
}
const VZERO: Vec2 = Vec2 { x: Fix32::ZERO, y: Fix32::ZERO };

#[derive(Clone, Copy)]
struct VecI {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy)]
struct Hitbox {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy)]
struct Hair {
    x: Fix32,
    y: Fix32,
    size: Fix32,
    is_last: bool,
}
const HAIR0: Hair = Hair { x: Fix32::ZERO, y: Fix32::ZERO, size: Fix32::ZERO, is_last: false };

#[derive(Clone, Copy)]
struct Particle {
    active: bool,
    x: Fix32,
    y: Fix32,
    s: Fix32,
    spd: Fix32,
    off: Fix32,
    c: Fix32,
    h: Fix32,
    t: Fix32,
    spd2: Vec2,
}
const PARTICLE0: Particle = Particle {
    active: false,
    x: Fix32::ZERO,
    y: Fix32::ZERO,
    s: Fix32::ZERO,
    spd: Fix32::ZERO,
    off: Fix32::ZERO,
    c: Fix32::ZERO,
    h: Fix32::ZERO,
    t: Fix32::ZERO,
    spd2: VZERO,
};

#[derive(Clone, Copy)]
struct Cloud {
    x: Fix32,
    y: Fix32,
    spd: Fix32,
    w: Fix32,
}
const CLOUD0: Cloud = Cloud { x: Fix32::ZERO, y: Fix32::ZERO, spd: Fix32::ZERO, w: Fix32::ZERO };

// ---- object type table (order == tile-match order in load_room) ----
#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjType {
    Player,
    PlayerSpawn,
    Spring,
    Balloon,
    Smoke,
    Platform,
    FallFloor,
    Fruit,
    FlyFruit,
    FakeWall,
    Key,
    Chest,
    Lifeup,
    Message,
    BigChest,
    Orb,
    Flag,
    RoomTitle,
}
use ObjType::*;

const OBJ_ORDER: [ObjType; 18] = [
    Player, PlayerSpawn, Spring, Balloon, Smoke, Platform, FallFloor, Fruit, FlyFruit, FakeWall, Key,
    Chest, Lifeup, Message, BigChest, Orb, Flag, RoomTitle,
];

fn type_tile(t: ObjType) -> i32 {
    match t {
        Player => -1,
        PlayerSpawn => 1,
        Spring => 18,
        Balloon => 22,
        Smoke => -1,
        Platform => -1,
        FallFloor => 23,
        Fruit => 26,
        FlyFruit => 28,
        FakeWall => 64,
        Key => 8,
        Chest => 20,
        Lifeup => -1,
        Message => 86,
        BigChest => 96,
        Orb => -1,
        Flag => 118,
        RoomTitle => -1,
    }
}
fn type_if_not_fruit(t: ObjType) -> bool {
    matches!(t, Fruit | FlyFruit | FakeWall | Key | Chest)
}

// ---- the fat object struct (union-of-all-kinds, like the C OBJ) ----
#[derive(Clone, Copy)]
struct Obj {
    active: bool,
    id: i16,
    type_: ObjType,
    collideable: bool,
    solids: bool,
    spr: Fix32,
    flip_x: bool,
    flip_y: bool,
    x: Fix32,
    y: Fix32,
    hitbox: Hitbox,
    spd: Vec2,
    rem: Vec2,
    // player
    p_jump: bool,
    p_dash: bool,
    grace: i32,
    jbuffer: i32,
    djump: i32,
    dash_time: i32,
    dash_effect_time: i16,
    dash_target: Vec2,
    dash_accel: Vec2,
    spr_off: Fix32,
    was_on_ground: bool,
    hair: [Hair; 5],
    // player_spawn
    state: i32,
    delay: i32,
    target: Vec2,
    // spring
    hide_in: i32,
    hide_for: i32,
    // balloon
    timer: i32,
    offset: Fix32,
    start: Fix32,
    // fruit
    off: Fix32,
    // fly_fruit
    fly: bool,
    step: Fix32,
    sfx_delay: i32,
    // lifeup
    duration: i32,
    flash: Fix32,
    // platform
    last: Fix32,
    dir: Fix32,
    // message
    index: Fix32,
    off2: VecI,
    // big chest
    particles: [Particle; 50],
    particle_count: i32,
    // flag
    score: i32,
    show: bool,
}

const OBJ0: Obj = Obj {
    active: false,
    id: 0,
    type_: Player,
    collideable: true,
    solids: true,
    spr: Fix32::ZERO,
    flip_x: false,
    flip_y: false,
    x: Fix32::ZERO,
    y: Fix32::ZERO,
    hitbox: Hitbox { x: 0, y: 0, w: 8, h: 8 },
    spd: VZERO,
    rem: VZERO,
    p_jump: false,
    p_dash: false,
    grace: 0,
    jbuffer: 0,
    djump: 0,
    dash_time: 0,
    dash_effect_time: 0,
    dash_target: VZERO,
    dash_accel: VZERO,
    spr_off: Fix32::ZERO,
    was_on_ground: false,
    hair: [HAIR0; 5],
    state: 0,
    delay: 0,
    target: VZERO,
    hide_in: 0,
    hide_for: 0,
    timer: 0,
    offset: Fix32::ZERO,
    start: Fix32::ZERO,
    off: Fix32::ZERO,
    fly: false,
    step: Fix32::ZERO,
    sfx_delay: 0,
    duration: 0,
    flash: Fix32::ZERO,
    last: Fix32::ZERO,
    dir: Fix32::ZERO,
    index: Fix32::ZERO,
    off2: VecI { x: 0, y: 0 },
    particles: [PARTICLE0; 50],
    particle_count: 0,
    score: 0,
    show: false,
};

// ---- global game state (mirrors celeste.cpp file-scope statics) ----
static mut OBJECTS: [Obj; MAX_OBJECTS] = [OBJ0; MAX_OBJECTS];
static mut NEXT_ID: i16 = 0;
static mut PLAYER_DUMMY: Obj = OBJ0;

static mut ROOM: VecI = VecI { x: 0, y: 0 };
static mut FREEZE: i32 = 0;
static mut SHAKE: i32 = 0;
static mut WILL_RESTART: bool = false;
static mut DELAY_RESTART: i32 = 0;
static mut GOT_FRUIT: [bool; FRUIT_COUNT] = [false; FRUIT_COUNT];
static mut HAS_DASHED: bool = false;
static mut SFX_TIMER: i32 = 0;
static mut HAS_KEY: bool = false;
static mut PAUSE_PLAYER: bool = false;
static mut FLASH_BG: bool = false;
static mut MUSIC_TIMER: i32 = 0;
static mut NEW_BG: bool = false;
static mut FRAMES: i32 = 0;
static mut SECONDS: i32 = 0;
static mut MINUTES: i16 = 0;
static mut DEATHS: i32 = 0;
static mut MAX_DJUMP: i32 = 1;
static mut START_GAME: bool = false;
static mut START_GAME_FLASH: i32 = 0;

static mut CLOUDS: [Cloud; 17] = [CLOUD0; 17];
static mut PARTICLES: [Particle; 25] = [PARTICLE0; 25];
static mut DEAD_PARTICLES: [Particle; 8] = [PARTICLE0; 8];

pub fn freeze() -> i32 {
    unsafe { FREEZE }
}

// ---- small helpers (celeste.cpp:1877-1893) ----
fn clamp(val: Fix32, a: Fix32, b: Fix32) -> Fix32 {
    a.max(b.min(val))
}
fn appr(val: Fix32, target: Fix32, amount: Fix32) -> Fix32 {
    if val > target {
        (val - amount).max(target)
    } else {
        (val + amount).min(target)
    }
}
fn sign(v: Fix32) -> Fix32 {
    if v.0 > 0 {
        fi(1)
    } else if v.0 < 0 {
        fi(-1)
    } else {
        fi(0)
    }
}
fn maybe() -> bool {
    rng::rnd(fi(1)) < fx(0.5)
}

// ---- world queries (celeste.cpp:1895-1934) ----
fn tile_at(x: i32, y: i32) -> i32 {
    unsafe { p8mget(ROOM.x * 16 + x, ROOM.y * 16 + y) }
}
fn tile_flag_at(x: i32, y: i32, w: i32, h: i32, flag: i32) -> bool {
    let mut i = (x / 8).max(0);
    while i <= ((x + w - 1) / 8).min(15) {
        let mut j = (y / 8).max(0);
        while j <= ((y + h - 1) / 8).min(15) {
            if p8fget(tile_at(i, j), flag) {
                return true;
            }
            j += 1;
        }
        i += 1;
    }
    false
}
fn solid_at(x: i32, y: i32, w: i32, h: i32) -> bool {
    tile_flag_at(x, y, w, h, 0)
}
fn ice_at(x: i32, y: i32, w: i32, h: i32) -> bool {
    tile_flag_at(x, y, w, h, 4)
}
fn spikes_at(x: Fix32, y: Fix32, w: i32, h: i32, xspd: Fix32, yspd: Fix32) -> bool {
    let wf = fi(w);
    let hf = fi(h);
    let mut i = fi(0).max((x / fx(8.0)).floor()).to_int();
    while fi(i) <= fi(15).min((x + wf - fi(1)) / fx(8.0)) {
        let mut j = fi(0).max((y / fx(8.0)).floor()).to_int();
        while fi(j) <= fi(15).min((y + hf - fi(1)) / fx(8.0)) {
            let tile = tile_at(i, j);
            if tile == 17 && ((y + hf - fi(1)).rem_floor(fi(8)) >= fi(6) || y + hf == fi(j * 8 + 8)) && yspd >= fi(0) {
                return true;
            } else if tile == 27 && (y).rem_floor(fi(8)) <= fi(2) && yspd <= fi(0) {
                return true;
            } else if tile == 43 && (x).rem_floor(fi(8)) <= fi(2) && xspd <= fi(0) {
                return true;
            } else if tile == 59 && ((x + wf - fi(1)).rem_floor(fi(8)) >= fi(6) || x + wf == fi(i * 8 + 8)) && xspd >= fi(0) {
                return true;
            }
            j += 1;
        }
        i += 1;
    }
    false
}

// ---- object engine (celeste.cpp:474-598, 1474-1547) ----
unsafe fn objp(i: usize) -> *mut Obj {
    addr_of_mut!(OBJECTS[i])
}

unsafe fn obj_is_solid(o: *mut Obj, ox: Fix32, oy: Fix32) -> bool {
    if oy > fi(0) && !obj_check(o, Platform, ox, fi(0)) && obj_check(o, Platform, ox, oy) {
        return true;
    }
    let hb = (*o).hitbox;
    solid_at(
        ((*o).x + fi(hb.x) + ox).to_int(),
        ((*o).y + fi(hb.y) + oy).to_int(),
        hb.w,
        hb.h,
    ) || obj_check(o, FallFloor, ox, oy)
        || obj_check(o, FakeWall, ox, oy)
}
unsafe fn obj_is_ice(o: *mut Obj, ox: Fix32, oy: Fix32) -> bool {
    let hb = (*o).hitbox;
    ice_at(((*o).x + fi(hb.x) + ox).to_int(), ((*o).y + fi(hb.y) + oy).to_int(), hb.w, hb.h)
}
unsafe fn obj_collide(o: *mut Obj, ty: ObjType, ox: Fix32, oy: Fix32) -> *mut Obj {
    for i in 0..MAX_OBJECTS {
        let other = objp(i);
        if other == o {
            continue;
        }
        if (*other).active
            && (*other).type_ == ty
            && (*other).collideable
        {
            let oh = (*other).hitbox;
            let th = (*o).hitbox;
            if (*other).x + fi(oh.x) + fi(oh.w) > (*o).x + fi(th.x) + ox
                && (*other).y + fi(oh.y) + fi(oh.h) > (*o).y + fi(th.y) + oy
                && (*other).x + fi(oh.x) < (*o).x + fi(th.x) + fi(th.w) + ox
                && (*other).y + fi(oh.y) < (*o).y + fi(th.y) + fi(th.h) + oy
            {
                return other;
            }
        }
    }
    core::ptr::null_mut()
}
unsafe fn obj_check(o: *mut Obj, ty: ObjType, ox: Fix32, oy: Fix32) -> bool {
    !obj_collide(o, ty, ox, oy).is_null()
}
unsafe fn obj_move(o: *mut Obj, ox: Fix32, oy: Fix32) {
    (*o).rem.x += ox * fx(0.5);
    let amount = ((*o).rem.x + fx(0.5)).floor();
    (*o).rem.x -= amount;
    obj_move_x(o, amount, fi(0));

    (*o).rem.y += oy * fx(0.5);
    let amount = ((*o).rem.y + fx(0.5)).floor();
    (*o).rem.y -= amount;
    obj_move_y(o, amount);
}
unsafe fn obj_move_x(o: *mut Obj, amount: Fix32, start: Fix32) {
    if (*o).solids {
        let step = sign(amount);
        let mut i = start;
        while i <= amount.abs() {
            if !obj_is_solid(o, step, fi(0)) {
                (*o).x += step;
            } else {
                (*o).spd.x = fi(0);
                (*o).rem.x = fi(0);
                break;
            }
            i += fi(1);
        }
    } else {
        (*o).x += amount;
    }
}
unsafe fn obj_move_y(o: *mut Obj, amount: Fix32) {
    if (*o).solids {
        let step = sign(amount);
        let mut i = 0;
        while fi(i) <= amount.abs() {
            if !obj_is_solid(o, fi(0), step) {
                (*o).y += step;
            } else {
                (*o).spd.y = fi(0);
                (*o).rem.y = fi(0);
                break;
            }
            i += 1;
        }
    } else {
        (*o).y += amount;
    }
}

unsafe fn init_object(ty: ObjType, x: Fix32, y: Fix32) -> *mut Obj {
    if type_if_not_fruit(ty) && GOT_FRUIT[level_index() as usize] {
        return core::ptr::null_mut();
    }
    let mut slot = usize::MAX;
    for i in 0..MAX_OBJECTS {
        if !OBJECTS[i].active {
            slot = i;
            break;
        }
    }
    if slot == usize::MAX {
        return core::ptr::null_mut();
    }
    let o = objp(slot);
    *o = OBJ0;
    (*o).active = true;
    (*o).id = NEXT_ID;
    NEXT_ID = NEXT_ID.wrapping_add(1);
    (*o).type_ = ty;
    (*o).collideable = true;
    (*o).solids = true;
    (*o).spr = fi(type_tile(ty));
    (*o).flip_x = false;
    (*o).flip_y = false;
    (*o).x = x;
    (*o).y = y;
    (*o).hitbox = Hitbox { x: 0, y: 0, w: 8, h: 8 };
    (*o).spd = VZERO;
    (*o).rem = VZERO;
    dispatch_init(o);
    o
}

unsafe fn destroy_object(o: *mut Obj) {
    // shift later slots left (loading-jank emulation)
    let base = objp(0);
    let mut p = o;
    while p.add(1) < base.add(MAX_OBJECTS) {
        *p = *p.add(1);
        p = p.add(1);
    }
    (*base.add(MAX_OBJECTS - 1)).active = false;
}

// ---- hair (celeste.cpp:849-877) ----
unsafe fn create_hair(o: *mut Obj) {
    for i in 0..=4 {
        (*o).hair[i] = Hair {
            x: (*o).x,
            y: (*o).y,
            size: fi(1).max(fi(2).min(fi(3 - i as i32))),
            is_last: i == 4,
        };
    }
}
fn set_hair_color(djump: i32) {
    let c = if djump == 1 {
        8
    } else if djump == 2 {
        7 + ((fi(unsafe { FRAMES }) / fx(6.0)).to_int() % 2) * 4
    } else {
        12
    };
    p8pal(8, c);
}
unsafe fn draw_hair(o: *mut Obj, facing: i32) {
    let mut last_x = (*o).x + fi(4) - fi(facing * 2);
    let mut last_y = (*o).y + if p8btn(K_DOWN) { fi(4) } else { fi(3) };
    let mut i = 0;
    loop {
        let h = addr_of_mut!((*o).hair[i]);
        i += 1;
        (*h).x += (last_x - (*h).x) / fx(3.0);
        (*h).y += (last_y + fx(0.5) - (*h).y) / fx(3.0);
        p8circfill((*h).x.to_int(), (*h).y.to_int(), (*h).size.to_int(), 8);
        last_x = (*h).x;
        last_y = (*h).y;
        if (*h).is_last {
            break;
        }
    }
}
fn unset_hair_color() {
    p8pal(8, 8);
}

fn psfx(num: i32) {
    if unsafe { SFX_TIMER } <= 0 {
        sfx::play(num);
    }
}

// ---- player (celeste.cpp:604-841) ----
unsafe fn player_init(this: *mut Obj) {
    (*this).p_jump = false;
    (*this).p_dash = false;
    (*this).grace = 0;
    (*this).jbuffer = 0;
    (*this).djump = MAX_DJUMP;
    (*this).dash_time = 0;
    (*this).dash_effect_time = 0;
    (*this).dash_target = VZERO;
    (*this).dash_accel = VZERO;
    (*this).hitbox = Hitbox { x: 1, y: 3, w: 6, h: 5 };
    (*this).spr_off = fi(0);
    (*this).was_on_ground = false;
    create_hair(this);
}
unsafe fn player_update(thisp: *mut Obj) {
    if PAUSE_PLAYER {
        return;
    }
    let mut this = thisp;
    let input = if p8btn(K_RIGHT) {
        1
    } else if p8btn(K_LEFT) {
        -1
    } else {
        0
    };

    let mut do_kill = false;
    if spikes_at(
        (*this).x + fi((*this).hitbox.x),
        (*this).y + fi((*this).hitbox.y),
        (*this).hitbox.w,
        (*this).hitbox.h,
        (*this).spd.x,
        (*this).spd.y,
    ) {
        do_kill = true;
    }
    if (*this).y > fi(128) {
        do_kill = true;
    }
    if do_kill {
        PLAYER_DUMMY = *this;
        kill_player(this);
        this = addr_of_mut!(PLAYER_DUMMY);
    }

    let on_ground = obj_is_solid(this, fi(0), fi(1));
    let on_ice = obj_is_ice(this, fi(0), fi(1));

    if on_ground && !(*this).was_on_ground {
        init_object(Smoke, (*this).x, (*this).y + fi(4));
    }

    let jump = p8btn(K_JUMP) && !(*this).p_jump;
    (*this).p_jump = p8btn(K_JUMP);
    if jump {
        (*this).jbuffer = 8;
    } else if (*this).jbuffer > 0 {
        (*this).jbuffer -= 1;
    }

    let dash = p8btn(K_DASH) && !(*this).p_dash;
    (*this).p_dash = p8btn(K_DASH);

    if on_ground {
        (*this).grace = 12;
        if (*this).djump < MAX_DJUMP {
            psfx(54);
            (*this).djump = MAX_DJUMP;
        }
    } else if (*this).grace > 0 {
        (*this).grace -= 1;
    }

    (*this).dash_effect_time -= 1;
    if (*this).dash_time > 0 {
        init_object(Smoke, (*this).x, (*this).y);
        (*this).dash_time -= 1;
        (*this).spd.x = appr((*this).spd.x, (*this).dash_target.x, (*this).dash_accel.x * fx(0.5));
        (*this).spd.y = appr((*this).spd.y, (*this).dash_target.y, (*this).dash_accel.y * fx(0.5));
    } else {
        let maxrun = fi(1);
        let mut accel = fx(0.6);
        let deccel = fx(0.15);

        if !on_ground {
            accel = fx(0.4);
        } else if on_ice {
            accel = fx(0.05);
            if input == (if (*this).flip_x { -1 } else { 1 }) {
                accel = fx(0.05);
            }
        }

        if (*this).spd.x.abs() > maxrun {
            (*this).spd.x = appr((*this).spd.x, sign((*this).spd.x) * maxrun, deccel * fx(0.5));
        } else {
            (*this).spd.x = appr((*this).spd.x, fi(input) * maxrun, accel * fx(0.5));
        }

        if (*this).spd.x != fi(0) {
            (*this).flip_x = (*this).spd.x < fi(0);
        }

        let mut maxfall = fi(2);
        let mut gravity = fx(0.21);

        if (*this).spd.y.abs() <= fx(0.15) {
            gravity = gravity * fx(0.5);
        }

        if input != 0 && obj_is_solid(this, fi(input), fi(0)) && !obj_is_ice(this, fi(input), fi(0)) {
            maxfall = fx(0.4);
            if rng::rnd(fi(20)) < fi(2) {
                init_object(Smoke, (*this).x + fi(input * 6), (*this).y);
            }
        }

        if !on_ground {
            (*this).spd.y = appr((*this).spd.y, maxfall, gravity * fx(0.5));
        }

        if (*this).jbuffer > 0 {
            if (*this).grace > 0 {
                psfx(1);
                (*this).jbuffer = 0;
                (*this).grace = 0;
                (*this).spd.y = fi(-2);
                init_object(Smoke, (*this).x, (*this).y + fi(4));
            } else {
                let wall_dir = if obj_is_solid(this, fi(-3), fi(0)) {
                    -1
                } else if obj_is_solid(this, fi(3), fi(0)) {
                    1
                } else {
                    0
                };
                if wall_dir != 0 {
                    psfx(2);
                    (*this).jbuffer = 0;
                    (*this).spd.y = fi(-2);
                    (*this).spd.x = fi(-wall_dir) * (maxrun + fi(1));
                    if !obj_is_ice(this, fi(wall_dir * 3), fi(0)) {
                        init_object(Smoke, (*this).x + fi(wall_dir * 6), (*this).y);
                    }
                }
            }
        }

        let d_full = fi(5);
        let d_half = d_full * fx(0.70710678118);

        if (*this).djump > 0 && dash {
            init_object(Smoke, (*this).x, (*this).y);
            (*this).djump -= 1;
            (*this).dash_time = 8;
            HAS_DASHED = true;
            (*this).dash_effect_time = 20;
            let v_input = if p8btn(K_UP) {
                -1
            } else if p8btn(K_DOWN) {
                1
            } else {
                0
            };
            if input != 0 {
                if v_input != 0 {
                    (*this).spd.x = fi(input) * d_half;
                    (*this).spd.y = fi(v_input) * d_half;
                } else {
                    (*this).spd.x = fi(input) * d_full;
                    (*this).spd.y = fi(0);
                }
            } else if v_input != 0 {
                (*this).spd.x = fi(0);
                (*this).spd.y = fi(v_input) * d_full;
            } else {
                (*this).spd.x = if (*this).flip_x { fi(-1) } else { fi(1) };
                (*this).spd.y = fi(0);
            }

            psfx(3);
            FREEZE = 4;
            SHAKE = 12;
            (*this).dash_target.x = fi(2) * sign((*this).spd.x);
            (*this).dash_target.y = fi(2) * sign((*this).spd.y);
            (*this).dash_accel.x = fx(1.5);
            (*this).dash_accel.y = fx(1.5);

            if (*this).spd.y < fi(0) {
                (*this).dash_target.y = (*this).dash_target.y * fx(0.75);
            }
            if (*this).spd.y != fi(0) {
                (*this).dash_accel.x = (*this).dash_accel.x * fx(0.70710678118);
            }
            if (*this).spd.x != fi(0) {
                (*this).dash_accel.y = (*this).dash_accel.y * fx(0.70710678118);
            }
        } else if dash && (*this).djump <= 0 {
            psfx(9);
            init_object(Smoke, (*this).x, (*this).y);
        }
    }

    (*this).spr_off += fx(0.125);
    if !on_ground {
        if obj_is_solid(this, fi(input), fi(0)) {
            (*this).spr = fi(5);
        } else {
            (*this).spr = fi(3);
        }
    } else if p8btn(K_DOWN) {
        (*this).spr = fi(6);
    } else if p8btn(K_UP) {
        (*this).spr = fi(7);
    } else if (*this).spd.x == fi(0) || (!p8btn(K_LEFT) && !p8btn(K_RIGHT)) {
        (*this).spr = fi(1);
    } else {
        (*this).spr = fi(1 + ((*this).spr_off.to_int() % 4));
    }

    if (*this).y < fi(-4) && level_index() < 30 {
        next_room();
    }

    (*this).was_on_ground = on_ground;
}
unsafe fn player_draw(this: *mut Obj) {
    if (*this).x < fi(-1) || (*this).x > fi(121) {
        (*this).x = clamp((*this).x, fi(-1), fi(121));
        (*this).spd.x = fi(0);
    }
    set_hair_color((*this).djump);
    draw_hair(this, if (*this).flip_x { -1 } else { 1 });
    p8spr((*this).spr.to_int(), (*this).x.to_int(), (*this).y.to_int(), (*this).flip_x, (*this).flip_y);
    backend::flush(); // draw the player with the hair palette before resetting it
    unset_hair_color();
}

// ---- player_spawn ----
unsafe fn player_spawn_init(this: *mut Obj) {
    sfx::play(4);
    (*this).spr = fi(3);
    (*this).target.x = (*this).x;
    (*this).target.y = (*this).y;
    (*this).y = fi(128);
    (*this).spd.y = fi(-4);
    (*this).state = 0;
    (*this).delay = 0;
    (*this).solids = false;
    create_hair(this);
}
unsafe fn player_spawn_update(this: *mut Obj) {
    if (*this).state == 0 {
        if (*this).y < (*this).target.y + fi(16) {
            (*this).state = 1;
            (*this).delay = 6;
        }
    } else if (*this).state == 1 {
        (*this).spd.y += fx(0.25);
        if (*this).spd.y > fi(0) && (*this).delay > 0 {
            (*this).spd.y = fi(0);
            (*this).delay -= 1;
        }
        if (*this).spd.y > fi(0) && (*this).y > (*this).target.y {
            (*this).y = (*this).target.y;
            (*this).spd.x = fi(0);
            (*this).spd.y = fi(0);
            (*this).state = 2;
            (*this).delay = 10;
            SHAKE = 10;
            init_object(Smoke, (*this).x, (*this).y + fi(4));
            sfx::play(5);
        }
    } else if (*this).state == 2 {
        (*this).delay -= 1;
        (*this).spr = fi(6);
        if (*this).delay < 0 {
            let x = (*this).x;
            let y = (*this).y;
            destroy_object(this);
            init_object(Player, x, y);
        }
    }
}
unsafe fn player_spawn_draw(this: *mut Obj) {
    set_hair_color(MAX_DJUMP);
    draw_hair(this, 1);
    p8spr((*this).spr.to_int(), (*this).x.to_int(), (*this).y.to_int(), (*this).flip_x, (*this).flip_y);
    unset_hair_color();
}

// ---- spring ----
unsafe fn spring_init(this: *mut Obj) {
    (*this).hide_in = 0;
    (*this).hide_for = 0;
}
unsafe fn spring_update(this: *mut Obj) {
    if (*this).hide_for > 0 {
        (*this).hide_for -= 1;
        if (*this).hide_for <= 0 {
            (*this).spr = fi(18);
            (*this).delay = 0;
        }
    } else if (*this).spr == fi(18) {
        let hit = obj_collide(this, Player, fi(0), fi(0));
        if !hit.is_null() && (*hit).spd.y >= fi(0) {
            (*this).spr = fi(19);
            (*hit).y = (*this).y - fi(4);
            (*hit).spd.x = (*hit).spd.x * fx(0.2);
            (*hit).spd.y = fi(-3);
            (*hit).djump = MAX_DJUMP;
            (*this).delay = 20;
            init_object(Smoke, (*this).x, (*this).y);
            let below = obj_collide(this, FallFloor, fi(0), fi(1));
            if !below.is_null() {
                break_fall_floor(below);
            }
            psfx(8);
        }
    } else if (*this).delay > 0 {
        (*this).delay -= 1;
        if (*this).delay <= 0 {
            (*this).spr = fi(18);
        }
    }
    if (*this).hide_in > 0 {
        (*this).hide_in -= 1;
        if (*this).hide_in <= 0 {
            (*this).hide_for = 120;
            (*this).spr = fi(0);
        }
    }
}
unsafe fn break_spring(o: *mut Obj) {
    (*o).hide_in = 30;
}

// ---- balloon ----
unsafe fn balloon_init(this: *mut Obj) {
    (*this).offset = rng::rnd(fi(1));
    (*this).start = (*this).y;
    (*this).timer = 0;
    (*this).hitbox = Hitbox { x: -1, y: -1, w: 10, h: 10 };
}
unsafe fn balloon_update(this: *mut Obj) {
    if (*this).spr == fi(22) {
        (*this).offset += fx(0.005);
        (*this).y = (*this).start + (*this).offset.sin() * fi(2);
        let hit = obj_collide(this, Player, fi(0), fi(0));
        if !hit.is_null() && (*hit).djump < MAX_DJUMP {
            psfx(6);
            init_object(Smoke, (*this).x, (*this).y);
            (*hit).djump = MAX_DJUMP;
            (*this).spr = fi(0);
            (*this).timer = 120;
        }
    } else if (*this).timer > 0 {
        (*this).timer -= 1;
    } else {
        psfx(7);
        init_object(Smoke, (*this).x, (*this).y);
        (*this).spr = fi(22);
    }
}
unsafe fn balloon_draw(this: *mut Obj) {
    if (*this).spr == fi(22) {
        p8spr(13 + ((*this).offset * fi(8)).to_int() % 3, (*this).x.to_int(), (*this).y.to_int() + 6, false, false);
        p8spr((*this).spr.to_int(), (*this).x.to_int(), (*this).y.to_int(), false, false);
    }
}

// ---- fall_floor ----
unsafe fn fall_floor_init(this: *mut Obj) {
    (*this).state = 0;
}
unsafe fn fall_floor_update(this: *mut Obj) {
    if (*this).state == 0 {
        if obj_check(this, Player, fi(0), fi(-1)) || obj_check(this, Player, fi(-1), fi(0)) || obj_check(this, Player, fi(1), fi(0)) {
            break_fall_floor(this);
        }
    } else if (*this).state == 1 {
        (*this).delay -= 1;
        if (*this).delay <= 0 {
            (*this).state = 2;
            (*this).delay = 120;
            (*this).collideable = false;
        }
    } else if (*this).state == 2 {
        (*this).delay -= 1;
        if (*this).delay <= 0 && !obj_check(this, Player, fi(0), fi(0)) {
            psfx(7);
            (*this).state = 0;
            (*this).collideable = true;
            init_object(Smoke, (*this).x, (*this).y);
        }
    }
}
unsafe fn fall_floor_draw(this: *mut Obj) {
    if (*this).state != 2 {
        if (*this).state != 1 {
            p8spr(23, (*this).x.to_int(), (*this).y.to_int(), false, false);
        } else {
            p8spr(23 + (30 - (*this).delay) / 10, (*this).x.to_int(), (*this).y.to_int(), false, false);
        }
    }
}
unsafe fn break_fall_floor(o: *mut Obj) {
    if (*o).state == 0 {
        psfx(15);
        (*o).state = 1;
        (*o).delay = 30;
        init_object(Smoke, (*o).x, (*o).y);
        let hit = obj_collide(o, Spring, fi(0), fi(-1));
        if !hit.is_null() {
            break_spring(hit);
        }
    }
}

// ---- smoke ----
unsafe fn smoke_init(this: *mut Obj) {
    (*this).spr = fi(29);
    (*this).spd.y = fx(-0.1);
    (*this).spd.x = fx(0.3) + rng::rnd(fx(0.2));
    (*this).x += fi(-1) + rng::rnd(fi(2));
    (*this).y += fi(-1) + rng::rnd(fi(2));
    (*this).flip_x = maybe();
    (*this).flip_y = maybe();
    (*this).solids = false;
}
unsafe fn smoke_update(this: *mut Obj) {
    (*this).spr += fx(0.1);
    if (*this).spr >= fi(32) {
        destroy_object(this);
    }
}

// ---- fruit ----
unsafe fn fruit_init(this: *mut Obj) {
    (*this).start = (*this).y;
    (*this).off = fi(0);
}
unsafe fn fruit_update(this: *mut Obj) {
    let hit = obj_collide(this, Player, fi(0), fi(0));
    if !hit.is_null() {
        (*hit).djump = MAX_DJUMP;
        SFX_TIMER = 40;
        sfx::play(13);
        GOT_FRUIT[level_index() as usize] = true;
        init_object(Lifeup, (*this).x, (*this).y);
        destroy_object(this);
        return;
    }
    (*this).off += fx(0.5);
    (*this).y = (*this).start + ((*this).off / fi(40)).sin() * fx(2.5);
}

// ---- fly_fruit ----
unsafe fn fly_fruit_init(this: *mut Obj) {
    (*this).start = (*this).y;
    (*this).fly = false;
    (*this).step = fx(0.5);
    (*this).solids = false;
    (*this).sfx_delay = 16;
}
unsafe fn fly_fruit_update(this: *mut Obj) {
    let mut do_destroy = false;
    if (*this).fly {
        if (*this).sfx_delay > 0 {
            (*this).sfx_delay -= 1;
            if (*this).sfx_delay <= 0 {
                SFX_TIMER = 40;
                sfx::play(14);
            }
        }
        (*this).spd.y = appr((*this).spd.y, fx(-3.5), fx(0.125));
        if (*this).y < fi(-16) {
            do_destroy = true;
        }
    } else {
        if HAS_DASHED {
            (*this).fly = true;
        }
        (*this).step += fx(0.025);
        (*this).spd.y = (*this).step.sin() * fx(0.5);
    }
    let hit = obj_collide(this, Player, fi(0), fi(0));
    if !hit.is_null() {
        (*hit).djump = MAX_DJUMP;
        SFX_TIMER = 40;
        sfx::play(13);
        GOT_FRUIT[level_index() as usize] = true;
        init_object(Lifeup, (*this).x, (*this).y);
        do_destroy = true;
    }
    if do_destroy {
        destroy_object(this);
    }
}
unsafe fn fly_fruit_draw(this: *mut Obj) {
    let mut off = fi(0);
    if !(*this).fly {
        let dir = (*this).step.sin();
        if dir < fi(0) {
            off = fi(1) + fi(0).max(sign((*this).y - (*this).start));
        }
    } else {
        off = (off + fx(0.25)).rem_floor(fi(3));
    }
    p8spr(45 + off.to_int(), (*this).x.to_int() - 6, (*this).y.to_int() - 2, true, false);
    p8spr((*this).spr.to_int(), (*this).x.to_int(), (*this).y.to_int(), false, false);
    p8spr(45 + off.to_int(), (*this).x.to_int() + 6, (*this).y.to_int() - 2, false, false);
}

// ---- lifeup ----
unsafe fn lifeup_init(this: *mut Obj) {
    (*this).spd.y = fx(-0.25);
    (*this).duration = 60;
    (*this).x -= fi(2);
    (*this).y -= fi(4);
    (*this).flash = fi(0);
    (*this).solids = false;
}
unsafe fn lifeup_update(this: *mut Obj) {
    (*this).duration -= 1;
    if (*this).duration <= 0 {
        destroy_object(this);
    }
}
unsafe fn lifeup_draw(this: *mut Obj) {
    (*this).flash += fx(0.25);
    p8print(b"1000", (*this).x.to_int() - 2, (*this).y.to_int(), 7 + (*this).flash.to_int() % 2);
}

// ---- fake_wall ----
unsafe fn fake_wall_update(this: *mut Obj) {
    (*this).hitbox = Hitbox { x: -1, y: -1, w: 18, h: 18 };
    let hit = obj_collide(this, Player, fi(0), fi(0));
    if !hit.is_null() && (*hit).dash_effect_time > 0 {
        (*hit).spd.x = -sign((*hit).spd.x) * fx(1.5);
        (*hit).spd.y = fx(-1.5);
        (*hit).dash_time = -1;
        SFX_TIMER = 40;
        sfx::play(16);
        init_object(Smoke, (*this).x, (*this).y);
        init_object(Smoke, (*this).x + fi(8), (*this).y);
        init_object(Smoke, (*this).x, (*this).y + fi(8));
        init_object(Smoke, (*this).x + fi(8), (*this).y + fi(8));
        init_object(Fruit, (*this).x + fi(4), (*this).y + fi(4));
        destroy_object(this);
        return;
    }
    (*this).hitbox = Hitbox { x: 0, y: 0, w: 16, h: 16 };
}
unsafe fn fake_wall_draw(this: *mut Obj) {
    let x = (*this).x.to_int();
    let y = (*this).y.to_int();
    p8spr(64, x, y, false, false);
    p8spr(65, x + 8, y, false, false);
    p8spr(80, x, y + 8, false, false);
    p8spr(81, x + 8, y + 8, false, false);
}

// ---- key ----
unsafe fn key_update(this: *mut Obj) {
    let was = (*this).spr.floor().to_int();
    (*this).spr = fi(9) + ((fi(FRAMES) / fi(60)).sin() + fx(0.5)) * fi(1);
    let is = (*this).spr.floor().to_int();
    if is == 10 && is != was {
        (*this).flip_x = !(*this).flip_x;
    }
    if obj_check(this, Player, fi(0), fi(0)) {
        sfx::play(23);
        SFX_TIMER = 20;
        destroy_object(this);
        HAS_KEY = true;
    }
}

// ---- chest ----
unsafe fn chest_init(this: *mut Obj) {
    (*this).x -= fi(4);
    (*this).start = (*this).x;
    (*this).timer = 40;
}
unsafe fn chest_update(this: *mut Obj) {
    if HAS_KEY {
        (*this).timer -= 1;
        (*this).x = (*this).start - fi(1) + rng::rnd(fi(3));
        if (*this).timer <= 0 {
            SFX_TIMER = 20;
            sfx::play(16);
            init_object(Fruit, (*this).x, (*this).y - fi(4));
            destroy_object(this);
        }
    }
}

// ---- platform ----
unsafe fn platform_init(this: *mut Obj) {
    (*this).x -= fi(4);
    (*this).solids = false;
    (*this).hitbox.w = 16;
    (*this).last = (*this).x;
}
unsafe fn platform_update(this: *mut Obj) {
    (*this).spd.x = (*this).dir * fx(0.65);
    if (*this).x < fi(-16) {
        (*this).x = fi(128);
    } else if (*this).x > fi(128) {
        (*this).x = fi(-16);
    }
    if !obj_check(this, Player, fi(0), fi(0)) {
        let hit = obj_collide(this, Player, fi(0), fi(-1));
        if !hit.is_null() {
            obj_move_x(hit, (*this).x - (*this).last, fi(1));
        }
    }
    (*this).last = (*this).x;
}
unsafe fn platform_draw(this: *mut Obj) {
    p8spr(11, (*this).x.to_int(), (*this).y.to_int() - 1, false, false);
    p8spr(12, (*this).x.to_int() + 8, (*this).y.to_int() - 1, false, false);
}

// ---- message ----
unsafe fn message_draw(this: *mut Obj) {
    const TEXT: &[u8] = b"-- celeste mountain --#this memorial to those# perished on the climb";
    if obj_check(this, Player, fi(4), fi(0)) {
        if (*this).index < fi(TEXT.len() as i32) {
            (*this).index += fx(0.25);
            if (*this).index >= (*this).last + fi(1) {
                (*this).last += fi(1);
                sfx::play(35);
            }
        }
        (*this).off2.x = 8;
        (*this).off2.y = 96;
        let count = (*this).index.to_int();
        let mut i = 0;
        while i < count && (i as usize) < TEXT.len() {
            let ch = TEXT[i as usize];
            if ch != b'#' {
                p8rectfill((*this).off2.x - 2, (*this).off2.y - 2, (*this).off2.x + 7, (*this).off2.y + 6, 7);
                p8print(&[ch], (*this).off2.x, (*this).off2.y, 0);
                (*this).off2.x += 5;
            } else {
                (*this).off2.x = 8;
                (*this).off2.y += 7;
            }
            i += 1;
        }
    } else {
        (*this).index = fi(0);
        (*this).last = fi(0);
    }
}

// ---- big_chest ----
unsafe fn big_chest_init(this: *mut Obj) {
    (*this).state = 0;
    (*this).hitbox.w = 16;
}
unsafe fn big_chest_draw(this: *mut Obj) {
    if (*this).state == 0 {
        let hit = obj_collide(this, Player, fi(0), fi(8));
        if !hit.is_null() && obj_is_solid(hit, fi(0), fi(1)) {
            sfx::music(-1, 500, 7);
            sfx::play(37);
            PAUSE_PLAYER = true;
            (*hit).spd.x = fi(0);
            (*hit).spd.y = fi(0);
            (*this).state = 1;
            init_object(Smoke, (*this).x, (*this).y);
            init_object(Smoke, (*this).x + fi(8), (*this).y);
            (*this).timer = 120;
            (*this).particle_count = 0;
        }
        p8spr(96, (*this).x.to_int(), (*this).y.to_int(), false, false);
        p8spr(97, (*this).x.to_int() + 8, (*this).y.to_int(), false, false);
    } else if (*this).state == 1 {
        (*this).timer -= 1;
        SHAKE = 10;
        FLASH_BG = true;
        if (*this).timer <= 90 && (*this).particle_count < 50 {
            let pc = (*this).particle_count as usize;
            (*this).particles[pc] = Particle {
                x: fi(1) + rng::rnd(fi(14)),
                y: fi(0),
                spd: fi(8) + rng::rnd(fi(8)),
                h: fi(32) + rng::rnd(fi(32)),
                ..PARTICLE0
            };
            (*this).particle_count += 1;
        }
        if (*this).timer < 0 {
            (*this).state = 2;
            (*this).particle_count = 0;
            FLASH_BG = false;
            NEW_BG = true;
            init_object(Orb, (*this).x + fi(4), (*this).y + fi(4));
            PAUSE_PLAYER = false;
        }
        for i in 0..(*this).particle_count as usize {
            let p = addr_of_mut!((*this).particles[i]);
            (*p).y += (*p).spd * fx(0.5);
            p8line(
                ((*this).x + (*p).x).to_int(),
                ((*this).y + fi(8) - (*p).y).to_int(),
                ((*this).x + (*p).x).to_int(),
                ((*this).y + fi(8) - (*p).y + (*p).h).min((*this).y + fi(8)).to_int(),
                7,
            );
        }
    }
    p8spr(112, (*this).x.to_int(), (*this).y.to_int() + 8, false, false);
    p8spr(113, (*this).x.to_int() + 8, (*this).y.to_int() + 8, false, false);
}

// ---- orb ----
unsafe fn orb_init(this: *mut Obj) {
    (*this).spd.y = fi(-4);
    (*this).solids = false;
    (*this).particle_count = 0;
}
unsafe fn orb_draw(this: *mut Obj) {
    (*this).spd.y = appr((*this).spd.y, fi(0), fx(0.25));
    let hit = obj_collide(this, Player, fi(0), fi(0));
    let mut destroy_self = false;
    if (*this).spd.y == fi(0) && !hit.is_null() {
        MUSIC_TIMER = 90;
        sfx::play(51);
        FREEZE = 20;
        SHAKE = 20;
        destroy_self = true;
        MAX_DJUMP = 2;
        (*hit).djump = 2;
    }
    p8spr(102, (*this).x.to_int(), (*this).y.to_int(), false, false);
    let off = fi(FRAMES) / fi(60);
    let mut i = fi(0);
    while i <= fi(7) {
        p8circfill(
            ((*this).x + fi(4) + (off + i / fi(8)).cos() * fi(8)).to_int(),
            ((*this).y + fi(4) + (off + i / fi(8)).sin() * fi(8)).to_int(),
            1,
            7,
        );
        i += fi(1);
    }
    if destroy_self {
        destroy_object(this);
    }
}

// ---- flag ----
unsafe fn flag_init(this: *mut Obj) {
    (*this).x += fi(5);
    (*this).score = 0;
    (*this).show = false;
    for i in 0..FRUIT_COUNT {
        if GOT_FRUIT[i] {
            (*this).score += 1;
        }
    }
}
unsafe fn flag_draw(this: *mut Obj) {
    (*this).spr = fi(118) + (fi(FRAMES) / fi(10)).rem_floor(fi(3));
    p8spr((*this).spr.to_int(), (*this).x.to_int(), (*this).y.to_int(), false, false);
    if (*this).show {
        p8rectfill(32, 2, 96, 31, 0);
        p8spr(26, 55, 6, false, false);
        let mut f = Fmt::new();
        f.byte(b'x');
        f.int((*this).score);
        p8print(f.as_slice(), 64, 9, 7);
        draw_time(49, 16);
        let mut f = Fmt::new();
        f.str(b"deaths:");
        f.int(DEATHS);
        p8print(f.as_slice(), 48, 24, 7);
    } else if obj_check(this, Player, fi(0), fi(0)) {
        sfx::play(55);
        SFX_TIMER = 60;
        (*this).show = true;
    }
}

// ---- room_title ----
unsafe fn room_title_init(this: *mut Obj) {
    (*this).delay = 10;
}
unsafe fn room_title_draw(this: *mut Obj) {
    (*this).delay -= 1;
    if (*this).delay < -60 {
        destroy_object(this);
    } else if (*this).delay < 0 {
        p8rectfill(24, 58, 104, 70, 0);
        if ROOM.x == 3 && ROOM.y == 1 {
            p8print(b"old site", 48, 62, 7);
        } else if level_index() == 30 {
            p8print(b"summit", 52, 62, 7);
        } else {
            let level = (1 + level_index()) * 100;
            let mut f = Fmt::new();
            f.int(level);
            f.str(b" m");
            p8print(f.as_slice(), 52 + if level < 1000 { 2 } else { 0 }, 62, 7);
        }
        draw_time(4, 4);
    }
}

// ---- kill / draw_object / draw_time ----
unsafe fn kill_player(o: *mut Obj) {
    SFX_TIMER = 24;
    sfx::play(0);
    DEATHS += 1;
    SHAKE = 20;
    let mut dpc = 0;
    let mut dir = fi(0);
    while dir <= fi(7) {
        let angle = dir / fi(8);
        DEAD_PARTICLES[dpc] = Particle {
            active: true,
            x: (*o).x + fi(4),
            y: (*o).y + fi(4),
            t: fi(20),
            spd2: Vec2 { x: angle.sin() * fi(3), y: angle.cos() * fi(3) },
            ..PARTICLE0
        };
        dpc += 1;
        restart_room();
        dir += fi(1);
    }
    destroy_object(o);
}

unsafe fn draw_object(o: *mut Obj) {
    if !dispatch_draw(o) && (*o).spr > fi(0) {
        p8spr((*o).spr.to_int(), (*o).x.to_int(), (*o).y.to_int(), (*o).flip_x, (*o).flip_y);
    }
}

fn draw_time(x: i32, y: i32) {
    let (s, m, h) = unsafe { (SECONDS, (MINUTES % 60) as i32, (MINUTES / 60) as i32) };
    p8rectfill(x, y, x + 32, y + 6, 0);
    let mut f = Fmt::new();
    f.int2(h);
    f.byte(b':');
    f.int2(m);
    f.byte(b':');
    f.int2(s);
    p8print(f.as_slice(), x + 1, y + 1, 7);
}

// ---- type dispatch ----
unsafe fn dispatch_init(o: *mut Obj) {
    match (*o).type_ {
        Player => player_init(o),
        PlayerSpawn => player_spawn_init(o),
        Spring => spring_init(o),
        Balloon => balloon_init(o),
        Smoke => smoke_init(o),
        Platform => platform_init(o),
        FallFloor => fall_floor_init(o),
        Fruit => fruit_init(o),
        FlyFruit => fly_fruit_init(o),
        Chest => chest_init(o),
        Lifeup => lifeup_init(o),
        BigChest => big_chest_init(o),
        Orb => orb_init(o),
        Flag => flag_init(o),
        RoomTitle => room_title_init(o),
        FakeWall | Key | Message => {}
    }
}
unsafe fn dispatch_update(o: *mut Obj) {
    match (*o).type_ {
        Player => player_update(o),
        PlayerSpawn => player_spawn_update(o),
        Spring => spring_update(o),
        Balloon => balloon_update(o),
        Smoke => smoke_update(o),
        Platform => platform_update(o),
        FallFloor => fall_floor_update(o),
        Fruit => fruit_update(o),
        FlyFruit => fly_fruit_update(o),
        FakeWall => fake_wall_update(o),
        Key => key_update(o),
        Chest => chest_update(o),
        Lifeup => lifeup_update(o),
        Message | BigChest | Orb | Flag | RoomTitle => {}
    }
}
/// Returns true if a custom draw ran.
unsafe fn dispatch_draw(o: *mut Obj) -> bool {
    match (*o).type_ {
        Player => player_draw(o),
        PlayerSpawn => player_spawn_draw(o),
        Balloon => balloon_draw(o),
        Platform => platform_draw(o),
        FallFloor => fall_floor_draw(o),
        FlyFruit => fly_fruit_draw(o),
        FakeWall => fake_wall_draw(o),
        Lifeup => lifeup_draw(o),
        Message => message_draw(o),
        BigChest => big_chest_draw(o),
        Orb => orb_draw(o),
        Flag => flag_draw(o),
        RoomTitle => room_title_draw(o),
        Spring | Smoke | Fruit | Key | Chest => return false,
    }
    true
}

// ---- effects prelude ----
unsafe fn prelude_init_clouds() {
    for i in 0..=16 {
        CLOUDS[i] = Cloud {
            x: rng::rnd(fi(128)),
            y: rng::rnd(fi(128)),
            spd: fi(1) + rng::rnd(fi(4)),
            w: fi(32) + rng::rnd(fi(32)),
        };
    }
}
unsafe fn prelude_init_particles() {
    for i in 0..=24 {
        PARTICLES[i] = Particle {
            x: rng::rnd(fi(128)),
            y: rng::rnd(fi(128)),
            s: fi(0) + (rng::rnd(fi(5)) / fi(4)).floor(),
            spd: fx(0.25) + rng::rnd(fi(5)),
            off: rng::rnd(fi(1)),
            c: fi(6) + (fx(0.5) + rng::rnd(fi(1))).floor(),
            ..PARTICLE0
        };
    }
}

// ---- rooms ----
fn level_index() -> i32 {
    unsafe { ROOM.x % 8 + ROOM.y * 8 }
}
fn is_title() -> bool {
    level_index() == 31
}
unsafe fn title_screen() {
    for i in 0..=29 {
        GOT_FRUIT[i] = false;
    }
    FRAMES = 0;
    DEATHS = 0;
    MAX_DJUMP = 1;
    START_GAME = false;
    START_GAME_FLASH = 0;
    sfx::music(40, 0, 7);
    load_room(7, 3);
}
unsafe fn begin_game() {
    FRAMES = 0;
    SECONDS = 0;
    MINUTES = 0;
    MUSIC_TIMER = 0;
    START_GAME = false;
    sfx::music(0, 0, 7);
    load_room(0, 0);
}
unsafe fn restart_room() {
    WILL_RESTART = true;
    DELAY_RESTART = 30;
}
unsafe fn next_room() {
    if ROOM.x == 2 && ROOM.y == 1 {
        sfx::music(30, 500, 7);
    } else if ROOM.x == 3 && ROOM.y == 1 {
        sfx::music(20, 500, 7);
    } else if ROOM.x == 4 && ROOM.y == 2 {
        sfx::music(30, 500, 7);
    } else if ROOM.x == 5 && ROOM.y == 3 {
        sfx::music(30, 500, 7);
    }
    if ROOM.x == 7 {
        load_room(0, ROOM.y + 1);
    } else {
        load_room(ROOM.x + 1, ROOM.y);
    }
}
unsafe fn load_room(x: i32, y: i32) {
    HAS_DASHED = false;
    HAS_KEY = false;
    for i in 0..MAX_OBJECTS {
        OBJECTS[i].active = false;
    }
    ROOM.x = x;
    ROOM.y = y;
    for tx in 0..=15 {
        for ty in 0..=15 {
            let tile = p8mget(ROOM.x * 16 + tx, ROOM.y * 16 + ty);
            if tile == 11 {
                let p = init_object(Platform, fi(tx * 8), fi(ty * 8));
                if !p.is_null() {
                    (*p).dir = fi(-1);
                }
            } else if tile == 12 {
                let p = init_object(Platform, fi(tx * 8), fi(ty * 8));
                if !p.is_null() {
                    (*p).dir = fi(1);
                }
            } else {
                for &ty2 in OBJ_ORDER.iter() {
                    if tile == type_tile(ty2) {
                        init_object(ty2, fi(tx * 8), fi(ty * 8));
                    }
                }
            }
        }
    }
    if !is_title() {
        init_object(RoomTitle, fi(0), fi(0));
    }
}

// ---- public lifecycle ----
pub fn init() {
    unsafe {
        prelude_init_clouds();
        prelude_init_particles();
        title_screen();
    }
}

pub fn update() {
    unsafe {
        FRAMES = (FRAMES + 1) % 60;
        if FRAMES == 0 && level_index() < 30 {
            SECONDS = (SECONDS + 1) % 60;
            if SECONDS == 0 {
                MINUTES += 1;
            }
        }
        if MUSIC_TIMER > 0 {
            MUSIC_TIMER -= 1;
            if MUSIC_TIMER <= 0 {
                sfx::music(10, 0, 7);
            }
        }
        if SFX_TIMER > 0 {
            SFX_TIMER -= 1;
        }
        if FREEZE > 0 {
            FREEZE -= 1;
            return;
        }
        if SHAKE > 0 {
            SHAKE -= 1;
            p8camera(0, 0);
            if SHAKE > 0 {
                p8camera((fi(-2) + rng::rnd(fi(5))).to_int(), (fi(-2) + rng::rnd(fi(5))).to_int());
            }
        }
        if WILL_RESTART && DELAY_RESTART > 0 {
            DELAY_RESTART -= 1;
            if DELAY_RESTART <= 0 {
                WILL_RESTART = false;
                load_room(ROOM.x, ROOM.y);
            }
        }

        for i in 0..MAX_OBJECTS {
            let o = objp(i);
            loop {
                if !(*o).active {
                    break;
                }
                obj_move(o, (*o).spd.x, (*o).spd.y);
                let this_id = (*o).id;
                dispatch_update(o);
                if this_id != (*o).id {
                    continue; // slot replaced -> redo
                }
                break;
            }
        }

        if is_title() {
            // btnp (not btn): a fresh press. The launcher's still-held Cross is
            // suppressed by input::prime until released, so we don't auto-start.
            if !START_GAME && (p8btnp(K_JUMP) || p8btnp(K_DASH)) {
                sfx::music(-1, 0, 0);
                START_GAME_FLASH = 100;
                START_GAME = true;
                sfx::play(38);
            }
            if START_GAME {
                START_GAME_FLASH -= 1;
                if START_GAME_FLASH <= -60 {
                    begin_game();
                }
            }
        }
    }
}

pub fn draw() {
    unsafe {
        if FREEZE > 0 {
            return;
        }
        p8pal_reset();

        if START_GAME {
            let mut c = 10;
            if START_GAME_FLASH > 20 {
                if FRAMES % 20 < 10 {
                    c = 7;
                }
            } else if START_GAME_FLASH > 10 {
                c = 2;
            } else if START_GAME_FLASH > 0 {
                c = 1;
            } else {
                c = 0;
            }
            if c < 10 {
                p8pal(6, c);
                p8pal(12, c);
                p8pal(13, c);
                p8pal(5, c);
                p8pal(1, c);
                p8pal(7, c);
            }
        }

        let mut bg_col = 0;
        if FLASH_BG {
            bg_col = FRAMES / 10;
        } else if NEW_BG {
            bg_col = 2;
        }
        p8rectfill(0, 0, 128, 128, bg_col);

        if !is_title() {
            for i in 0..=16 {
                let c = addr_of_mut!(CLOUDS[i]);
                (*c).x += (*c).spd * fx(0.5);
                p8rectfill(
                    (*c).x.to_int(),
                    (*c).y.to_int(),
                    ((*c).x + (*c).w).to_int(),
                    ((*c).y + fi(4) + (fi(1) - (*c).w / fi(64)) * fi(12)).to_int(),
                    if NEW_BG { 14 } else { 1 },
                );
                if (*c).x > fi(128) {
                    (*c).x = -(*c).w;
                    (*c).y = rng::rnd(fi(128 - 8));
                }
            }
        }

        p8map(ROOM.x * 16, ROOM.y * 16, 0, 0, 16, 16, 4);

        for i in 0..MAX_OBJECTS {
            let o = objp(i);
            if (*o).active && ((*o).type_ == Platform || (*o).type_ == BigChest) {
                draw_object(o);
            }
        }

        let off = if is_title() { -4 } else { 0 };
        p8map(ROOM.x * 16, ROOM.y * 16, off, 0, 16, 16, 2);

        for i in 0..MAX_OBJECTS {
            let o = objp(i);
            loop {
                let this_id = (*o).id;
                if (*o).active && (*o).type_ != Platform && (*o).type_ != BigChest {
                    draw_object(o);
                }
                if this_id != (*o).id {
                    continue;
                }
                break;
            }
        }

        p8map(ROOM.x * 16, ROOM.y * 16, 0, 0, 16, 16, 8);

        for i in 0..=24 {
            let p = addr_of_mut!(PARTICLES[i]);
            (*p).x += (*p).spd * fx(0.5);
            (*p).y += (*p).off.sin() * fx(0.5);
            (*p).off += fx(0.025).min((*p).spd / fi(64));
            p8rectfill(
                (*p).x.to_int(),
                (*p).y.to_int(),
                ((*p).x + (*p).s).to_int(),
                ((*p).y + (*p).s).to_int(),
                (*p).c.to_int(),
            );
            if (*p).x > fi(128 + 4) {
                (*p).x = fi(-4);
                (*p).y = rng::rnd(fi(128));
            }
        }

        for i in 0..=7 {
            let p = addr_of_mut!(DEAD_PARTICLES[i]);
            if (*p).active {
                (*p).x += (*p).spd2.x * fx(0.5);
                (*p).y += (*p).spd2.y * fx(0.5);
                (*p).t -= fi(1);
                if (*p).t <= fi(0) {
                    (*p).active = false;
                }
                p8rectfill(
                    ((*p).x - (*p).t / fi(5)).to_int(),
                    ((*p).y - (*p).t / fi(5)).to_int(),
                    ((*p).x + (*p).t / fi(5)).to_int(),
                    ((*p).y + (*p).t / fi(5)).to_int(),
                    (14 + (*p).t.rem_floor(fi(2)).to_int()),
                );
            }
        }

        // screenshake border
        p8rectfill(-5, -5, -1, 133, 0);
        p8rectfill(-5, -5, 133, -1, 0);
        p8rectfill(-5, 128, 133, 133, 0);
        p8rectfill(128, -5, 133, 133, 0);

        // The 128x128 PICO-8 image is drawn 2x (256 wide) and centred in 320, so
        // there's a 32px black margin each side; clouds/particles spill into it.
        // Cover the full side margins with black (a couple px into the play area
        // so screenshake can't reveal a gap).
        p8rectfill(-20, -20, 1, 148, 0);
        p8rectfill(127, -20, 148, 148, 0);

        if is_title() {
            p8print(b"x+c", 58, 80, 5);
            p8print(b"matt thorson", 42, 96, 5);
            p8print(b"noel berry", 46, 102, 5);
        }

        if level_index() == 30 {
            let mut player = core::ptr::null_mut();
            for i in 0..MAX_OBJECTS {
                let o = objp(i);
                if (*o).active && (*o).type_ == Player {
                    player = o;
                    break;
                }
            }
            if !player.is_null() {
                let diff = fi(24).min(fi(40) - ((*player).x + fi(4) - fi(64)).abs());
                p8rectfill(0, 0, diff.to_int(), 128, 0);
                p8rectfill(128 - diff.to_int(), 0, 128, 128, 0);
            }
        }
    }
}

// ---- tiny no_std integer formatter ----
struct Fmt {
    buf: [u8; 24],
    n: usize,
}
impl Fmt {
    fn new() -> Self {
        Fmt { buf: [0; 24], n: 0 }
    }
    fn byte(&mut self, b: u8) {
        if self.n < self.buf.len() {
            self.buf[self.n] = b;
            self.n += 1;
        }
    }
    fn str(&mut self, s: &[u8]) {
        for &b in s {
            self.byte(b);
        }
    }
    fn int(&mut self, mut v: i32) {
        if v < 0 {
            self.byte(b'-');
            v = -v;
        }
        let mut tmp = [0u8; 12];
        let mut k = 0;
        loop {
            tmp[k] = b'0' + (v % 10) as u8;
            k += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            self.byte(tmp[k]);
        }
    }
    /// Zero-padded to 2 digits (for the clock).
    fn int2(&mut self, v: i32) {
        if v < 10 {
            self.byte(b'0');
        }
        self.int(v);
    }
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.n]
    }
}
