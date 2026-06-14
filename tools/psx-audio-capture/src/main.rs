//! Capture the PSoXide SPU output of a booting PS1 disc to a WAV, so the
//! PSX synthesis can be diffed against the PICO-8 reference recordings.
//!
//! Fast-boots the disc (HLE warmup, skipping the BIOS intro), then steps the
//! CPU while draining the SPU's 44.1 kHz stereo output until `--seconds` of
//! audio is captured.
//!
//! Usage:
//!   psx-audio-capture --disc dist/celeste.cue --out /tmp/psx_celeste.wav \
//!       --seconds 20 [--skip 2] [--bios /path/SCPH1001.BIN]
//!
//! --skip discards that many seconds of audio up front (e.g. boot blip), so
//! the WAV starts at steady state.

use std::path::Path;

use emulator_core::{
    fast_boot_disc_with_hle, spu, warm_bios_for_disc_fast_boot, Bus, Cpu,
    DISC_FAST_BOOT_WARMUP_STEPS,
};

const SAMPLE_RATE: u32 = 44_100;
const DEFAULT_BIOS: &str = "/Users/ebonura/Downloads/ps1 bios/SCPH1001.BIN";
const STEP_CAP: u64 = 6_000_000_000; // backstop so a stuck cart can't run forever

fn arg(flag: &str) -> Option<String> {
    let mut it = std::env::args();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next();
        }
    }
    None
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
    let out = arg("--out").unwrap_or_else(|| "/tmp/psx_audio.wav".into());
    let seconds: f32 = arg("--seconds").and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let skip: f32 = arg("--skip").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let bios_path = arg("--bios").unwrap_or_else(|| DEFAULT_BIOS.into());

    let bios = std::fs::read(&bios_path).expect("BIOS readable");
    let disc = load_disc(Path::new(&disc_path)).expect("disc readable");
    let mut bus = Bus::new(bios).expect("bus");
    let mut cpu = Cpu::new();

    warm_bios_for_disc_fast_boot(&mut bus, &mut cpu, DISC_FAST_BOOT_WARMUP_STEPS)
        .expect("BIOS warmup");
    let info = fast_boot_disc_with_hle(&mut bus, &mut cpu, &disc, false).expect("fast boot");
    eprintln!("[capture] fast-boot entry=0x{:08x} payload={}B", info.initial_pc, info.payload_len);
    bus.cdrom.insert_disc(Some(disc));
    bus.attach_digital_pad_port1();

    // Optional: press Cross (0x4000) for ~0.4s starting at this second, to drive
    // a title screen into gameplay before/while capturing.
    let press_at: f32 = arg("--press-at").and_then(|s| s.parse().ok()).unwrap_or(-1.0);

    let skip_samples = (skip * SAMPLE_RATE as f32) as usize;
    let target = skip_samples + (seconds * SAMPLE_RATE as f32) as usize;
    let mut samples: Vec<(i16, i16)> = Vec::with_capacity(target);
    let mut accum = 0u64;
    let mut steps = 0u64;
    let mut pressed_done = false;

    while samples.len() < target && steps < STEP_CAP {
        if press_at >= 0.0 {
            let sec = samples.len() as f32 / SAMPLE_RATE as f32;
            let down = sec >= press_at && sec < press_at + 0.4;
            if down {
                bus.set_port1_buttons(emulator_core::ButtonState::from_bits(0x4000));
            } else if !pressed_done && sec >= press_at + 0.4 {
                bus.set_port1_buttons(emulator_core::ButtonState::from_bits(0));
                pressed_done = true;
            }
        }
        let before = bus.cycles();
        if let Err(e) = cpu.step(&mut bus) {
            eprintln!("[capture] CPU stopped at step {steps}: {e:?}");
            break;
        }
        steps += 1;
        accum = accum.saturating_add(bus.cycles().saturating_sub(before));
        let n = (accum / spu::SAMPLE_CYCLES) as usize;
        if n == 0 {
            continue;
        }
        accum %= spu::SAMPLE_CYCLES;
        bus.run_spu_samples(n);
        samples.extend(bus.spu.drain_audio());
    }

    let out_samples = if samples.len() > skip_samples { &samples[skip_samples..] } else { &[] };
    write_wav(Path::new(&out), out_samples).expect("write WAV");

    let peak = out_samples.iter().map(|&(l, r)| l.abs().max(r.abs())).max().unwrap_or(0);
    eprintln!(
        "[capture] steps={steps} captured={:.2}s peak={peak} ({:.1}%) -> {out}",
        out_samples.len() as f32 / SAMPLE_RATE as f32,
        peak as f32 / 32768.0 * 100.0,
    );
}

fn write_wav(path: &Path, samples: &[(i16, i16)]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_size = (samples.len() * 4) as u32;
    let byte_rate = SAMPLE_RATE * 2 * 16 / 8;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_size).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&2u16.to_le_bytes())?; // stereo
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?; // block align
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_size.to_le_bytes())?;
    for &(l, r) in samples {
        f.write_all(&l.to_le_bytes())?;
        f.write_all(&r.to_le_bytes())?;
    }
    Ok(())
}
