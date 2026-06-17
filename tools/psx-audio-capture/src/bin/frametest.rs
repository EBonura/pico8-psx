//! Boot a disc (fast-boot HLE, so unlicensed homebrew discs run), drive the
//! pad, step a number of video frames, then dump the display to a PPM. Used to
//! verify input behaviour (e.g. the title screen waiting for a fresh press).
//!
//! Usage:
//!   frametest --disc dist/celeste.cue --out /tmp/f.ppm --frames 200 \
//!       [--hold 0x4000] [--press-at 120 --press-mask 0x4000]
//!
//! --hold MASK: hold that 16-bit pad mask the whole run (e.g. 0x4000 = Cross).
//! --press-at N --press-mask M: nothing held until frame N, then hold M (a fresh
//!   press, since M was not held before).

use std::path::Path;

use emulator_core::{
    fast_boot_disc_with_hle, warm_bios_for_disc_fast_boot, Bus, ButtonState, Cpu,
    DISC_FAST_BOOT_WARMUP_STEPS,
};

const DEFAULT_BIOS: &str = "/Users/ebonura/Downloads/ps1 bios/SCPH1001.BIN";
const CYCLES_PER_FRAME: u64 = 564_480; // ~33.8688 MHz / 60
const STEP_CAP: u64 = 6_000_000_000;

fn arg(flag: &str) -> Option<String> {
    let mut it = std::env::args();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next();
        }
    }
    None
}
fn mask_arg(flag: &str) -> Option<u16> {
    arg(flag).and_then(|s| {
        let s = s.trim();
        if let Some(h) = s.strip_prefix("0x") {
            u16::from_str_radix(h, 16).ok()
        } else {
            s.parse().ok()
        }
    })
}

fn load_disc(path: &Path) -> Result<psx_iso::Disc, String> {
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("cue")) {
        psoxide_settings::library::load_disc_from_cue(path)
    } else {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(psx_iso::Disc::from_bin(bytes))
    }
}

fn main() {
    let disc_path = arg("--disc").expect("--disc <cue|bin> required");
    let out = arg("--out").unwrap_or_else(|| "/tmp/frame.ppm".into());
    let frames: u32 = arg("--frames").and_then(|s| s.parse().ok()).unwrap_or(200);
    let hold = mask_arg("--hold").unwrap_or(0);
    let press_at: i64 = arg("--press-at").and_then(|s| s.parse().ok()).unwrap_or(-1);
    let press_mask = mask_arg("--press-mask").unwrap_or(0x4000);
    let bios_path = arg("--bios").unwrap_or_else(|| DEFAULT_BIOS.into());

    let bios = std::fs::read(&bios_path).expect("BIOS readable");
    let disc = load_disc(Path::new(&disc_path)).expect("disc readable");
    let mut bus = Bus::new(bios).expect("bus");
    let mut cpu = Cpu::new();
    warm_bios_for_disc_fast_boot(&mut bus, &mut cpu, DISC_FAST_BOOT_WARMUP_STEPS).expect("warmup");
    fast_boot_disc_with_hle(&mut bus, &mut cpu, &disc, false).expect("fast boot");
    bus.cdrom.insert_disc(Some(disc));
    bus.attach_digital_pad_port1();

    // --fps: count actual framebuffer swaps (display-area Y flips between the two
    // stacked buffers) over the run to measure the game's real render rate.
    let fps_mode = arg("--fps").is_some();
    let mut last_y = bus.gpu.display_area().y;
    let mut swaps = 0u64;
    let mut fps_window_frames = 0u64;
    let press_started_at = if press_at >= 0 { press_at as u32 } else { 0 };

    // --hold-from MASK@FRAME: hold MASK from FRAME to the end (e.g. dash = 0x2020).
    let (hold2_mask, hold2_from) = arg("--hold-from")
        .and_then(|s| {
            let (m, f) = s.split_once('@')?;
            let m = u16::from_str_radix(m.trim().trim_start_matches("0x"), 16).ok()?;
            Some((m, f.trim().parse::<i64>().ok()?))
        })
        .unwrap_or((0, -1));

    // --script "F:MASK,F:MASK,...": a held-button timeline. The active mask is the
    // last entry whose frame <= the current frame (a 0 mask releases). Lets a run
    // drive multi-step menu paths (e.g. open settings, toggle a row, launch, pause).
    let script: Vec<(u32, u16)> = arg("--script")
        .map(|s| {
            let mut v: Vec<(u32, u16)> = s
                .split(',')
                .filter_map(|e| {
                    let (f, m) = e.split_once(':')?;
                    let f = f.trim().parse::<u32>().ok()?;
                    let m = u16::from_str_radix(m.trim().trim_start_matches("0x"), 16).ok()?;
                    Some((f, m))
                })
                .collect();
            v.sort_by_key(|e| e.0);
            v
        })
        .unwrap_or_default();

    // --profile [--profile-from FRAME]: sample the CPU PC every instruction into
    // a flat histogram (RAM base 0x80010000), dump the hottest addresses at the
    // end. Map those to functions with the ELF symbol table.
    let profile = arg("--profile").is_some();
    let profile_from: i64 = arg("--profile-from")
        .and_then(|s| s.parse().ok())
        .unwrap_or(if press_at >= 0 { press_at + 120 } else { 120 });
    const PROF_BASE: u32 = 0x8001_0000;
    let mut pc_hist: Vec<u64> = if profile { vec![0u64; 0x80_000] } else { Vec::new() };
    let mut prof_samples: u64 = 0;

    let mut steps = 0u64;
    for frame in 0..frames {
        // Decide this frame's held buttons.
        let mut mask = hold;
        if let Some(&(_, m)) = script.iter().rev().find(|&&(f, _)| frame as u32 >= f) {
            mask |= m;
        }
        if press_at >= 0 && frame as i64 >= press_at && frame as i64 <= press_at + 24 {
            mask |= press_mask; // hold the start press ~0.4s then release
        }
        if hold2_from >= 0 && frame as i64 >= hold2_from {
            mask |= hold2_mask;
        }
        bus.set_port1_buttons(ButtonState::from_bits(mask));

        let profiling = profile && frame as i64 >= profile_from;
        let mut accum = 0u64;
        while accum < CYCLES_PER_FRAME && steps < STEP_CAP {
            if profiling {
                let pc = cpu.pc();
                if pc >= PROF_BASE {
                    let idx = ((pc - PROF_BASE) >> 2) as usize;
                    if idx < pc_hist.len() {
                        pc_hist[idx] += 1;
                        prof_samples += 1;
                    }
                }
            }
            let before = bus.cycles();
            if let Err(e) = cpu.step(&mut bus) {
                eprintln!("[frametest] CPU stopped at step {steps}: {e:?}");
                break;
            }
            steps += 1;
            accum = accum.saturating_add(bus.cycles().saturating_sub(before));
        }
        // After this emulated 1/60s slice, did the displayed buffer flip?
        let y = bus.gpu.display_area().y;
        if frame as u32 > press_started_at + 60 {
            if y != last_y {
                swaps += 1;
            }
            fps_window_frames += 1;
        }
        last_y = y;
    }
    if fps_mode {
        let secs = fps_window_frames as f64 / 60.0;
        eprintln!(
            "[frametest] real render rate: {swaps} swaps over {secs:.2}s = {:.1} fps (target 60)",
            swaps as f64 / secs
        );
    }

    if profile {
        // Dump every nonzero PC bucket to a file (pc<TAB>count) for external
        // per-function aggregation, plus the top 60 to stderr.
        let mut buckets: Vec<(u32, u64)> = pc_hist
            .iter()
            .enumerate()
            .filter(|(_, &c)| c > 0)
            .map(|(i, &c)| (PROF_BASE + (i as u32) * 4, c))
            .collect();
        buckets.sort_by(|a, b| b.1.cmp(&a.1));
        let prof_out = arg("--profile-out").unwrap_or_else(|| "/tmp/pcprof.txt".into());
        let mut body = String::new();
        for (pc, c) in &buckets {
            body.push_str(&format!("{pc:08x}\t{c}\n"));
        }
        std::fs::write(&prof_out, body).expect("write profile");
        eprintln!("[frametest] {prof_samples} PC samples; full histogram -> {prof_out}");
        eprintln!("[frametest] top 60 hottest instructions:");
        for (pc, c) in buckets.iter().take(60) {
            let pct = *c as f64 / prof_samples.max(1) as f64 * 100.0;
            eprintln!("  0x{pc:08x}  {c:>9}  {pct:5.2}%");
        }
    }

    let (rgba, w, h) = bus.gpu.display_rgba8();
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in rgba.chunks_exact(4) {
        ppm.extend_from_slice(&px[..3]);
    }
    std::fs::write(&out, &ppm).expect("write PPM");
    eprintln!("[frametest] {frames} frames, hold=0x{hold:04x} press_at={press_at} -> {out} ({w}x{h})");
}
