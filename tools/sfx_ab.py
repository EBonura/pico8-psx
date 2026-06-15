#!/usr/bin/env python3
"""Build an A/B listening sample: for each SFX play the PICO-8 original, then the
PS1 (PSoXide SPU) recording of the same SFX, so the two can be compared by ear.

Reuses sfx_bench's aligner to split the PSX soundtest capture into per-SFX
segments, pairs each with its PICO-8 reference recording, peak-normalises both to
the same level, resamples to a common rate, and concatenates:

    [pico8 sfx0] (gap) [psx sfx0] (longer gap) [pico8 sfx1] (gap) [psx sfx1] ...

A short stereo hint makes the source obvious without narration: the PICO-8
original is panned slightly LEFT, the PS1 recording slightly RIGHT.

Usage:
  python3 tools/sfx_ab.py <capture.wav> <celeste|celeste2> <out.wav>
          [--ids 0,1,5,10] [--max N] [--sr 44100]
"""
import argparse
import numpy as np
from scipy.signal import resample_poly

import sfx_bench as sb  # align_segments, load, ref_paths, frames_from_refs, trim_silence


def to_sr(x, sr, tsr):
    if sr == tsr:
        return x
    from math import gcd
    g = gcd(int(sr), tsr)
    return resample_poly(x, tsr // g, int(sr) // g)


def norm(x, peak=0.9):
    m = np.abs(x).max()
    return x * (peak / m) if m > 1e-9 else x


def pan(x, left):
    """Stereo-place a mono clip: 1.0/0.55 on the near side, 0.55/1.0 on the far."""
    near, far = 1.0, 0.55
    l, r = (near, far) if left else (far, near)
    return np.stack([x * l, x * r], axis=1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("capture")
    ap.add_argument("game", choices=["celeste", "celeste2"])
    ap.add_argument("out")
    ap.add_argument("--ids", help="comma-separated SFX ids (default: a spread)")
    ap.add_argument("--max", type=int, default=20, help="how many SFX if --ids omitted")
    ap.add_argument("--maxlen", type=float, default=1.8,
                    help="cap each clip (s) so looping SFX stay a fair A/B length")
    ap.add_argument("--sr", type=int, default=44100)
    ap.add_argument("--win", type=int, default=96)
    a = ap.parse_args()

    psx, psr = sb.load(a.capture)
    refs = sb.ref_paths(a.game)
    frames = sb.frames_from_refs(refs, a.win)
    segs, spf = sb.align_segments(psx, psr, refs, frames)
    n = min(len(segs), len(refs))

    if a.ids:
        ids = [int(i) for i in a.ids.split(",") if i.strip() != ""]
        ids = [i for i in ids if 0 <= i < n]
    else:
        # an even spread across the SFX bank, capped at --max
        if a.max >= n:
            ids = list(range(n))
        else:
            ids = sorted({int(round(k * (n - 1) / (a.max - 1))) for k in range(a.max)})

    tsr = a.sr
    pair_gap = np.zeros((int(0.30 * tsr), 2), np.float32)   # within an A/B pair
    item_gap = np.zeros((int(0.70 * tsr), 2), np.float32)   # between SFX
    tick = norm(np.sin(2 * np.pi * 1000 * np.arange(int(0.04 * tsr)) / tsr)
                * np.hanning(int(0.04 * tsr)), 0.25)          # soft marker before each pair
    tick = np.stack([tick, tick], axis=1)

    out = []
    print(f"{a.game}: {len(ids)} SFX (spf={spf:.1f}) -> {a.out}")
    for i in ids:
        s, e = segs[i]
        ps = sb.trim_silence(psx[s:e])
        rx, rsr = sb.load(refs[i])
        rx = sb.trim_silence(rx)
        if len(ps) < 256 or len(rx) < 256:
            print(f"  sfx{i:<3} skipped (too short)")
            continue
        cap = int(a.maxlen * tsr)
        a8 = norm(to_sr(rx, rsr, tsr))[:cap]      # PICO-8 original
        b8 = norm(to_sr(ps, psr, tsr))[:cap]      # PS1 recording
        out += [tick, pan(a8, left=True), pair_gap, pan(b8, left=False), item_gap]
        print(f"  sfx{i:<3} pico8 {len(a8)/tsr:.2f}s  |  psx {len(b8)/tsr:.2f}s")

    audio = np.concatenate(out, axis=0)
    audio = np.clip(audio, -1, 1)
    pcm = (audio * 32767).astype("<i2")
    import wave
    w = wave.open(a.out, "wb")
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(tsr)
    w.writeframes(pcm.tobytes())
    w.close()
    print(f"wrote {a.out}  ({len(audio)/tsr:.1f}s, stereo {tsr}Hz; "
          f"PICO-8 left, PS1 right)")


if __name__ == "__main__":
    main()
