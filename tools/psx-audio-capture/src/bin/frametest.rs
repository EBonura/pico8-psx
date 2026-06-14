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

    let mut steps = 0u64;
    for frame in 0..frames {
        // Decide this frame's held buttons.
        let mut mask = hold;
        if press_at >= 0 && frame as i64 >= press_at {
            mask |= press_mask;
        }
        bus.set_port1_buttons(ButtonState::from_bits(mask));

        let mut accum = 0u64;
        while accum < CYCLES_PER_FRAME && steps < STEP_CAP {
            let before = bus.cycles();
            if let Err(e) = cpu.step(&mut bus) {
                eprintln!("[frametest] CPU stopped at step {steps}: {e:?}");
                break;
            }
            steps += 1;
            accum = accum.saturating_add(bus.cycles().saturating_sub(before));
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
