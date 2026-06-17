//! Headless SPU audio capture for rsx-redux. Sideloads a PS-EXE, runs N seconds,
//! and drains the SPU sample buffer to a WAV -- the rsx-redux counterpart to
//! `tools/psx-audio-capture` (PSoXide), so the two can be diffed for SPU accuracy.
//!
//! Usage: rsx-audio-capture <game.exe> <out.wav> [seconds]
//! BIOS: $RSX_BIOS, else ./SCPH1001.bin.
use rsx_redux::cpu::CPU;
use std::{env, fs};

fn main() {
    let a: Vec<String> = env::args().collect();
    let (exe, out) = (&a[1], &a[2]);
    let seconds: f64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(12.0);
    let bios_path = env::var("RSX_BIOS").unwrap_or_else(|_| "SCPH1001.bin".into());

    let mut cpu = CPU::new(Some(fs::read(exe).expect("read exe")), String::new());
    cpu.bus.load_bios(fs::read(&bios_path).expect("read BIOS ($RSX_BIOS or ./SCPH1001.bin)"));

    let target = (seconds * 44100.0 * 2.0) as usize; // stereo i16
    let mut samples: Vec<i16> = Vec::with_capacity(target);
    let mut frames = 0u64;
    while samples.len() < target && frames < 60 * 90 {
        cpu.step_frame();
        samples.append(&mut cpu.bus.spu.audio_buffer);
        frames += 1;
    }

    let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(out, spec).unwrap();
    for s in samples.iter().take(target) { w.write_sample(*s).unwrap(); }
    w.finalize().unwrap();
    eprintln!("[rsx-audio-capture] {frames} frames, {} samples -> {out}", samples.len());
}
