#!/usr/bin/env python3
"""Generate LONG (224-sample) wavetables for the 7 tonal instruments, used for LOW
notes only to kill short-wavetable imaging.

The 56-sample tables (gen_waveforms_from_recording.py) replay at ~f*56 Hz; for a
60Hz bass that's ~3.4kHz, and the SPU's 4-point interpolation leaves audible images
in 2.5-11kHz (measured ~2x PICO-8 on the celeste2 tilt bass). A 224-sample table
replays 4x faster (~13kHz) so the images move out where the interpolation kills
them. 224 samples also carries h1..h84 (vs h28), restoring the real upper harmonics
the short table couldn't represent at low pitch -- high notes don't need them (above
Nyquist), so the crossover to the short table is seamless. The 4x table's SPU pitch
is exactly the short pitch << 2 (same waveform, 4x samples = 4x rate), so no new
pitch table is needed; the engine just left-shifts for low notes.

Output: WAVEFORM_ADPCM_LONG + WAVEFORM_OFFSET_LONG (paste into each audio_data.rs).
Uses the SAME global gain as the short tables so levels match at the crossover.
"""
import numpy as np
from scipy.io import wavfile
import adpcm

WAV = "/tmp/carts/music_out/pico8_instruments.wav"
SHORT = 56
LONG = 448               # 8x the short table: pushes the SPU replay images above the
                         # audible band (first image at f*(LONG-HMAX); for a 60Hz bass
                         # = ~21kHz, vs 4x which lands them at ~8kHz, still audible)
HMAX_LONG = 84            # harmonics available in the pitch-24 recording (Nyquist)
FUND = 262.5
NAMES = ["triangle", "tiltsaw", "saw", "square", "pulse", "organ", "noise", "phaser"]
CYCLES = 20
NCYC_LEN = CYCLES * 84    # harmonic k lands exactly on FFT bin 20k

sr, x = wavfile.read(WAV)
if x.ndim > 1:
    x = x.mean(axis=1)
x = x.astype(np.float64) / 32768.0
e = np.convolve(np.abs(x), np.ones(512) / 512, mode="same")
onset = int(np.argmax(e > 0.01))
win = int(0.8 * sr)


def period_at(seg, nsamp, hmax):
    """One nsamp-sample cycle from harmonics 1..hmax (same recovery as the short
    generator, just more harmonics / finer grid)."""
    seg = seg[:NCYC_LEN] - seg[:NCYC_LEN].mean()
    S = np.fft.rfft(seg)
    B = np.zeros(nsamp // 2 + 1, dtype=complex)
    for k in range(1, min(hmax, nsamp // 2) + 1):
        bin_k = CYCLES * k
        if bin_k < len(S):
            B[k] = S[bin_k] * (nsamp / NCYC_LEN)
    return np.fft.irfft(B, n=nsamp)


# Extract short + long periods for every tonal instrument (skip noise = idx 6).
short, long = {}, {}
for i, nm in enumerate(NAMES):
    if i == 6:
        continue
    s = onset + i * win
    seg = x[s + int(0.18 * sr): s + int(0.55 * sr)]
    short[nm] = period_at(seg, SHORT, SHORT // 2)
    long[nm] = period_at(seg, LONG, HMAX_LONG)

# Reproduce the short generator's single global gain (peak of the SHORT periods ->
# 0.92) so the long tables sit at the exact same level as the shipping short ones.
g = 0.92 / max(np.abs(p).max() for p in short.values())
for nm in long:
    long[nm] = long[nm] * g


def encode_waveform(samples):
    """SPU ADPCM with PREDICTION (filters 1-4): the oversampled long table is
    smooth, so 2nd-order prediction makes the residual tiny -> ~30dB less 4-bit
    quantization noise than filter-0 (validated 46-60dB vs 22-29dB SNR), which is
    what makes the long table beat the short one instead of being buried in noise.
    Loop-seam is held to ~1 sample step by iterating the predictor start-state."""
    s16 = np.clip(np.round(np.asarray(samples) * 32767), -32768, 32767).astype(int)
    return adpcm.encode_looped(s16.tolist())


# Long tables only exist for tonal instruments; noise (idx 6) gets a dummy offset
# (the long path never fires for noise). Order the bytes by instrument index so the
# offset for instr i is directly indexable.
data, offsets = [], [0] * 8
for i, nm in enumerate(NAMES):
    if i == 6:
        offsets[i] = 0  # dummy; never used
        continue
    offsets[i] = len(data)
    data += encode_waveform(long[nm])
adpcm = data

rows = []
for i in range(0, len(adpcm), 16):
    rows.append("    " + ", ".join(str(b & 0xFF) for b in adpcm[i:i + 16]) + ",")
out = (f"pub static WAVEFORM_ADPCM_LONG: [u8; {len(adpcm)}] = [\n" + "\n".join(rows) + "\n];\n\n"
       f"pub static WAVEFORM_OFFSET_LONG: [u16; 8] = [\n    "
       + ", ".join(f"0x{o:04X}" for o in offsets) + ",\n];\n")
open("/tmp/waveform_long.txt", "w").write(out)
print(f"{len(adpcm)} bytes long ADPCM; offsets {[hex(o) for o in offsets]}")
print("per-instrument long RMS vs short RMS (should be ~1.0 = level match):")
for nm in NAMES:
    if nm == "noise":
        continue
    print(f"  {nm:9} long {np.sqrt(np.mean(long[nm]**2)):.4f}  short {np.sqrt(np.mean((short[nm]*g)**2)):.4f}")
print("wrote /tmp/waveform_long.txt")
