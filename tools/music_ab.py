#!/usr/bin/env python3
"""Build a music A/B listening sample: play ~N seconds of the PICO-8 original
theme, then ~N seconds of the PS1 (PSoXide SPU) rendering of the same music(),
so the two can be compared by ear.

Unlike sfx_ab (discrete events), music is one continuous stream: trim each side's
leading silence to the musical downbeat, cross-correlate onset envelopes to refine
the alignment, normalise, then concatenate. The PICO-8 original is panned slightly
LEFT, the PS1 rendering slightly RIGHT (same convention as sfx_ab).

Usage:
  python3 tools/music_ab.py <pico8_ref.wav> <psx_capture.wav> <out.wav> [--secs 15]
"""
import argparse
import numpy as np
from scipy.signal import resample_poly

import sfx_bench as sb  # load(), trim_silence()


def to_sr(x, sr, tsr):
    if sr == tsr:
        return x
    from math import gcd
    g = gcd(int(sr), tsr)
    return resample_poly(x, tsr // g, int(sr) // g)


def norm(x, peak=0.9):
    m = np.abs(x).max()
    return x * (peak / m) if m > 1e-9 else x


def lead_trim(x, sr, thr=0.02):
    """Drop everything before the first sustained sound (musical downbeat)."""
    e = np.abs(x)
    m = e.max()
    if m < 1e-9:
        return x
    loud = np.where(e > thr * m)[0]
    return x[loud[0]:] if len(loud) else x


def env(x, sr, hop=0.01):
    h = int(hop * sr)
    return np.array([np.abs(x[i:i + h]).mean() for i in range(0, len(x) - h, h)])


def align(a, asr, b, bsr):
    """Refine b's start so its spectrum best matches a's over a 3s window. Returns
    a sample offset to drop from the front of b (>=0), or 0 if no shift helps.
    Validated by spec_sim (not onset cross-correlation, which locks spurious peaks
    on a quiet musical intro)."""
    A = to_sr(a, asr, 44100)
    B = to_sr(b, bsr, 44100)
    L = int(3 * 44100)
    base = sb.spec_sim(A[:L], 44100, B[:L], 44100)
    best, off = base, 0
    for ms in range(-300, 1500, 25):
        d = int(ms / 1000 * 44100)
        aa, bb = (A, B[d:]) if d >= 0 else (A[-d:], B)
        if len(aa) < L or len(bb) < L:
            continue
        s = sb.spec_sim(aa[:L], 44100, bb[:L], 44100)
        if not np.isnan(s) and s > best + 0.02:  # only shift if it clearly helps
            best, off = s, ms
    return int(max(0, off) / 1000 * bsr)


def pan(x, left):
    near, far = 1.0, 0.6
    l, r = (near, far) if left else (far, near)
    return np.stack([x * l, x * r], axis=1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pico8")
    ap.add_argument("psx")
    ap.add_argument("out")
    ap.add_argument("--secs", type=float, default=15.0)
    ap.add_argument("--sr", type=int, default=44100)
    a = ap.parse_args()

    pr, prsr = sb.load(a.pico8)
    px, pxsr = sb.load(a.psx)
    pr = lead_trim(pr, prsr)
    px = lead_trim(px, pxsr)
    # refine the PS1 start against the PICO-8 onset pattern
    d = align(pr, prsr, px, pxsr)
    if d > 0:
        px = px[d:]
    tsr = a.sr
    n = int(a.secs * tsr)
    o = norm(to_sr(pr, prsr, tsr))[:n]
    p = norm(to_sr(px, pxsr, tsr))[:n]
    gap = np.zeros((int(0.8 * tsr), 2), np.float32)
    tick = np.sin(2 * np.pi * 1000 * np.arange(int(0.05 * tsr)) / tsr) * np.hanning(int(0.05 * tsr)) * 0.25
    tick = np.stack([tick, tick], axis=1)
    out = np.concatenate([tick, pan(o, True), gap, tick, pan(p, False), gap], axis=0)
    out = np.clip(out, -1, 1)

    import wave
    w = wave.open(a.out, "wb")
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(tsr)
    w.writeframes((out * 32767).astype("<i2").tobytes())
    w.close()
    print(f"wrote {a.out}  ({len(out)/tsr:.1f}s; PICO-8 {len(o)/tsr:.1f}s left, "
          f"PS1 {len(p)/tsr:.1f}s right; align +{d/pxsr*1000:.0f}ms)")


if __name__ == "__main__":
    main()
