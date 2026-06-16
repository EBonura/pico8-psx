//! Animated menu atmosphere.
//!
//! Two pieces, both driven by the menu's frame clock:
//! - The selected game's REAL backdrop -- Celeste 1's drifting cloud bars +
//!   floating particles, or Celeste 2's parallax `circfill` clouds + snow --
//!   ported from the games and drawn through the shared `pico8::backend` in
//!   PICO-8 128-space (so it sits in the centred 256-px playfield, like the
//!   games' own bordered field).
//! - The selection "tracer": a bright comet (×2, opposite) that runs around the
//!   chosen cover's border with a fading tail, drawn in screen space.

use pico8::backend;
use pico8::fixed::{fx, Fix32};
use pico8::rng;
use psx_gpu as gpu;

const HALF: Fix32 = fx(0.5);

#[inline]
fn fi(n: i32) -> Fix32 {
    Fix32::from_int(n)
}

// ---- Celeste 1 backdrop: cloud bars + floating particles ----
#[derive(Clone, Copy)]
struct C1Cloud {
    x: Fix32,
    y: Fix32,
    spd: Fix32,
    w: Fix32,
}
#[derive(Clone, Copy)]
struct C1Part {
    x: Fix32,
    y: Fix32,
    s: Fix32,
    spd: Fix32,
    off: Fix32,
    c: i32,
}
const C1C0: C1Cloud = C1Cloud { x: Fix32::ZERO, y: Fix32::ZERO, spd: Fix32::ZERO, w: Fix32::ZERO };
const C1P0: C1Part =
    C1Part { x: Fix32::ZERO, y: Fix32::ZERO, s: Fix32::ZERO, spd: Fix32::ZERO, off: Fix32::ZERO, c: 6 };

// ---- Celeste 2 backdrop: parallax clouds + snow ----
#[derive(Clone, Copy)]
struct C2Cloud {
    x: Fix32,
    y: Fix32,
    s: Fix32,
}
const C2C0: C2Cloud = C2Cloud { x: Fix32::ZERO, y: Fix32::ZERO, s: Fix32::ZERO };

static mut C1CLOUDS: [C1Cloud; 17] = [C1C0; 17];
static mut C1PARTS: [C1Part; 25] = [C1P0; 25];
static mut C2CLOUDS: [C2Cloud; 26] = [C2C0; 26];
static mut SNOW: [(Fix32, Fix32); 26] = [(Fix32::ZERO, Fix32::ZERO); 26];

/// Seed both backdrops' particle fields (matches the games' init routines).
pub fn init() {
    unsafe {
        rng::srand(42);
        for i in 0..17 {
            C1CLOUDS[i] = C1Cloud {
                x: rng::rnd(fi(128)),
                y: rng::rnd(fi(128)),
                spd: fi(1) + rng::rnd(fi(4)),
                w: fi(32) + rng::rnd(fi(32)),
            };
        }
        for i in 0..25 {
            C1PARTS[i] = C1Part {
                x: rng::rnd(fi(128)),
                y: rng::rnd(fi(128)),
                s: (rng::rnd(fi(5)) / fi(4)).floor(),
                spd: fx(0.25) + rng::rnd(fi(5)),
                off: rng::rnd(fi(1)),
                c: 6 + (fx(0.5) + rng::rnd(fi(1))).floor().to_int(),
            };
        }
        for i in 0..26 {
            C2CLOUDS[i] = C2Cloud {
                x: rng::rnd(fi(132)),
                y: rng::rnd(fi(132)),
                s: fi(16) + rng::rnd(fi(32)),
            };
            SNOW[i] = (rng::rnd(fi(132)), rng::rnd(fi(132)));
        }
    }
}

/// Draw the selected game's backdrop (0 = Celeste, else Celeste 2). Resets the
/// shared backend's camera/palette first (a game may have left them dirty).
pub fn draw(sel: usize, frame: i32) {
    unsafe {
        backend::camera(0, 0);
        backend::pal_reset();
        backend::rectfill(0, 0, 127, 127, 0); // cls(0) -- both games' bg is black here
        if sel == 0 {
            draw_celeste1();
        } else {
            draw_celeste2(frame);
        }
    }
}

unsafe fn draw_celeste1() {
    // drifting cloud bars (colour 1), thicker when narrower
    for i in 0..17 {
        let c = &mut C1CLOUDS[i];
        c.x += c.spd * HALF;
        let h = fi(4) + (fi(1) - c.w / fi(64)) * fi(12);
        backend::rectfill(
            c.x.to_int() as i16,
            c.y.to_int() as i16,
            (c.x + c.w).to_int() as i16,
            (c.y + h).to_int() as i16,
            1,
        );
        if c.x > fi(128) {
            c.x = -c.w;
            c.y = rng::rnd(fi(120));
        }
    }
    // floating particles (colour 6/7), drifting + bobbing
    for i in 0..25 {
        let p = &mut C1PARTS[i];
        p.x += p.spd * HALF;
        p.y += p.off.sin() * HALF;
        p.off += fx(0.025).min(p.spd / fi(64));
        backend::rectfill(
            p.x.to_int() as i16,
            p.y.to_int() as i16,
            (p.x + p.s).to_int() as i16,
            (p.y + p.s).to_int() as i16,
            p.c,
        );
        if p.x > fi(132) {
            p.x = fi(-4);
            p.y = rng::rnd(fi(128));
        }
    }
}

unsafe fn draw_celeste2(frame: i32) {
    // parallax clouds (level 1 cloud colour 13), CAM at origin
    for i in 0..26 {
        let c = &mut C2CLOUDS[i];
        let s = c.s;
        let x = c.x.rem_floor(fi(128) + s) - s / fi(2);
        let y = c.y.rem_floor(fi(128) + s / fi(2));
        let (xi, yi) = (x.to_int() as i16, y.to_int() as i16);
        backend::circfill(xi, yi, (s / fi(3)).to_int() as i16, 13);
        if i % 2 == 0 {
            backend::circfill((x - s / fi(3)).to_int() as i16, yi, (s / fi(5)).to_int() as i16, 13);
            backend::circfill((x + s / fi(3)).to_int() as i16, yi, (s / fi(6)).to_int() as i16, 13);
        }
        c.x += fi(4 - (i as i32) % 4) * fx(0.25) * HALF;
    }
    // snow
    let t = fi(frame) / fi(60);
    for i in 0..26 {
        let s = &mut SNOW[i];
        let px = s.0.rem_floor(fi(132)) - fi(2);
        let py = s.1.rem_floor(fi(132));
        backend::circfill(px.to_int() as i16, py.to_int() as i16, (i as i32 % 2) as i16, 7);
        s.0 += fi(4 - (i as i32) % 4) * HALF;
        s.1 += (t * fx(0.25) + fi(i as i32) * fx(0.1)).sin() * HALF;
    }
}

// ---- selection tracer (screen space) ----

/// Map a perimeter distance `pos` (0..2*(w+h)) to a point on the rectangle
/// `(left,top)..(right,bot)`, travelling clockwise from the top-left.
fn perim_xy(pos: i32, left: i16, top: i16, right: i16, bot: i16, pw: i32, ph: i32) -> (i16, i16) {
    if pos < pw {
        (left + pos as i16, top)
    } else if pos < pw + ph {
        (right, top + (pos - pw) as i16)
    } else if pos < 2 * pw + ph {
        (right - (pos - pw - ph) as i16, bot)
    } else {
        (left, bot - (pos - 2 * pw - ph) as i16)
    }
}

/// Draw the selection tracer around a `size`-square cover at screen `(cx, cy)`:
/// a faint border track plus two bright comets (opposite each other) chasing
/// around it with a fading cyan tail.
pub fn draw_tracer(cx: i16, cy: i16, size: i16, frame: i32) {
    const PAD: i16 = 3;
    let (left, top) = (cx - PAD, cy - PAD);
    let (right, bot) = (cx + size + PAD, cy + size + PAD);
    let pw = (right - left) as i32;
    let ph = (bot - top) as i32;
    let perim = 2 * (pw + ph);

    // faint border track
    let track = |x0: i16, y0: i16, x1: i16, y1: i16| {
        gpu::draw_quad_flat([(x0, y0), (x1, y0), (x0, y1), (x1, y1)], 0x10, 0x18, 0x30);
    };
    track(left, top, right, top + 1);
    track(left, bot - 1, right, bot);
    track(left, top, left + 1, bot);
    track(right - 1, top, right, bot);

    const SPEED: i32 = 3;
    const TAIL: i32 = 9;
    const STEP: i32 = 3;
    let head = frame * SPEED;
    for comet in 0..2 {
        let base = head + comet * perim / 2;
        // tail first (dim), head last (bright) so the head draws on top
        let mut k = TAIL - 1;
        while k >= 0 {
            let pos = (((base - k * STEP) % perim) + perim) % perim;
            let (x, y) = perim_xy(pos, left, top, right, bot, pw, ph);
            let f = TAIL - k; // 1..TAIL, brightest at the head
            let r = (0x28 * f / TAIL) as u8;
            let g = (0x80 * f / TAIL) as u8;
            let b = (0xFF * f / TAIL) as u8;
            let sz = if k == 0 { 2 } else { 1 };
            gpu::draw_quad_flat(
                [(x - sz, y - sz), (x + sz, y - sz), (x - sz, y + sz), (x + sz, y + sz)],
                r,
                g,
                b,
            );
            k -= 1;
        }
    }
}
