#!/usr/bin/env python3
"""Generate WAVEFORM_ADPCM from a direct recording of PICO-8's 8 instruments,
so the PSX wavetables carry PICO-8's *exact* timbre AND true relative loudness.

Input: tools/record made by /tmp/carts/inst_rec.p8 -- 8 isolated tones, pitch 24
(262.5 Hz), vol 5, 0.8s each, in order triangle..phaser. We extract one clean,
phase-averaged period per instrument, resample to 56 samples, and scale ALL of
them by a single global factor (preserving the recorded loudness ratios -- saw
and organ really are quieter than triangle). Noise is random at its measured
relative level. Encoded filter-0 / shift-0 (direct 4-bit, seamless loop),
byte-compatible offsets so the pitch table stays calibrated.
"""
import numpy as np
from scipy.io import wavfile
from scipy.signal import resample

WAV = "/tmp/carts/music_out/pico8_instruments.wav"
TONAL_SAMPLES = 56
NOISE_SAMPLES = 224
FUND = 262.5          # measured fundamental of pitch 24
NAMES = ["triangle","tiltsaw","saw","square","pulse","organ","noise","phaser"]

sr, x = wavfile.read(WAV)
if x.ndim > 1: x = x.mean(axis=1)
x = x.astype(np.float64) / 32768.0
e = np.convolve(np.abs(x), np.ones(512)/512, mode="same")
onset = int(np.argmax(e > 0.01))
win = int(0.8 * sr)
period = sr / FUND     # ~84.0 samples

CYCLES = 20                 # 20 cycles * 84 = 1680 samples; period is exactly 84.0
NCYC_LEN = CYCLES * 84      # so harmonic k lands exactly on FFT bin CYCLES*k (no leakage)

def extract_period(seg):
    """One 56-sample cycle reconstructed from the measured harmonics h1..h28,
    preserving each harmonic's true amplitude+phase (hence timbre AND loudness)."""
    seg = seg[:NCYC_LEN] - seg[:NCYC_LEN].mean()
    S = np.fft.rfft(seg)
    B = np.zeros(TONAL_SAMPLES//2 + 1, dtype=complex)
    for k in range(1, TONAL_SAMPLES//2 + 1):       # harmonics 1..28
        bin_k = CYCLES * k
        if bin_k < len(S):
            B[k] = S[bin_k] * (TONAL_SAMPLES / NCYC_LEN)
    return np.fft.irfft(B, n=TONAL_SAMPLES)

# extract all tonal periods at NATURAL amplitude (keeps loudness ratios)
periods = {}
for i, nm in enumerate(NAMES):
    if i == 6:   # noise handled separately
        continue
    s = onset + i*win
    seg = x[s+int(0.18*sr): s+int(0.55*sr)]
    periods[nm] = extract_period(seg)

# noise: measure its RMS, synthesize random at that level
ns = onset + 6*win
noise_seg = x[ns+int(0.18*sr): ns+int(0.55*sr)]
noise_rms = np.sqrt(np.mean((noise_seg-noise_seg.mean())**2))

# single global gain: scale so the loudest peak hits ~0.92 (headroom for the
# 4-bit grid), applied to every instrument equally -> ratios preserved
peak = max(np.abs(p).max() for p in periods.values())
g = 0.92 / peak
for nm in periods: periods[nm] = periods[nm] * g

rng = np.random.default_rng(1)
noise = rng.uniform(-1, 1, NOISE_SAMPLES)
noise *= (noise_rms * g) / np.sqrt(np.mean(noise**2))   # match measured loudness
noise = np.clip(noise, -1, 1)

def nib(v):
    return max(-8, min(7, int(round(v * 8))))

def encode_waveform(samples):
    n = len(samples) // 28
    out = []
    for b in range(n):
        chunk = samples[b*28:(b+1)*28]
        flags = (0x04 if b == 0 else 0) | (0x03 if b == n-1 else 0)
        blk = [0x00, flags]
        for k in range(0, 28, 2):
            lo, hi = nib(chunk[k]), nib(chunk[k+1])
            blk.append((lo & 0xF) | ((hi & 0xF) << 4))
        out += blk
    return out

adpcm, offsets = [], []
report = []
for i, nm in enumerate(NAMES):
    offsets.append(len(adpcm))
    samples = noise if i == 6 else periods[nm]
    report.append((nm, float(np.sqrt(np.mean(samples**2))), float(np.abs(samples).max())))
    adpcm += encode_waveform(samples)

rows = []
for i in range(0, len(adpcm), 16):
    rows.append("    " + ", ".join(str(b & 0xFF) for b in adpcm[i:i+16]) + ",")
open("/tmp/waveform_adpcm.txt", "w").write(
    f"pub static WAVEFORM_ADPCM: [u8; {len(adpcm)}] = [\n" + "\n".join(rows) + "\n];\n")

print(f"onset={onset/sr*1000:.0f}ms period={period:.1f} global_gain={g:.3f}")
print(f"{'inst':9} {'wt_rms':>7} {'wt_peak':>7}  (relative loudness preserved)")
base = report[0][1]
for nm, r, pk in report:
    print(f"{nm:9} {r:7.4f} {pk:7.3f}   rel={r/base:.2f}")
print(f"\n{len(adpcm)} bytes; offsets {[hex(o) for o in offsets]}")
print("wrote /tmp/waveform_adpcm.txt")
