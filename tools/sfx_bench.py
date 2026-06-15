#!/usr/bin/env python3
"""SFX similarity testbench: split a PSX SPU soundtest capture and score every
SFX against its PICO-8 reference recording.

Pipeline (see tools/sfx_bench.sh for the one-command runner):
  1. games/<game>/src/bin/soundtest.rs plays SFX 0..N, each for a FIXED
     window (regular layout -> robust split), separated by 18-frame silence gaps.
  2. tools/psx-audio-capture records the SPU output to a WAV.
  3. This script aligns the regular layout to the capture (2D contrast search
     over samples-per-frame + boot offset) and scores each SFX:
       - sim  : time-aligned log-STFT correlation in [0,1] (timbre similarity).
       - pitch: dominant spectral peak of PSX vs reference (Hz), to catch any
                wrong-octave / wrong-note bug independent of timbre.

Usage:
  python3 tools/sfx_bench.py <capture.wav> <celeste|celeste2> [--win 96] [--dump DIR]
The reference recordings (audio-ref/<game>/sfx) are the PICO-8 ground truth;
regenerate them with tools/record_pico8_sfx.py if the cart SFX change.
"""
import os, glob, math, wave, argparse
import numpy as np
from scipy.signal import resample_poly, stft

GAP_FRAMES = 18  # keep in sync with run_sfx_soundtest's SOUNDTEST_GAP_FRAMES
PREFIX = {"celeste": "c1", "celeste2": "c2"}


def load(path):
    w = wave.open(path, "rb")
    sr, ch, n = w.getframerate(), w.getnchannels(), w.getnframes()
    a = np.frombuffer(w.readframes(n), dtype=np.int16).astype(np.float32)
    w.close()
    if ch == 2:
        a = a.reshape(-1, 2).mean(1)
    return a / 32768.0, sr


def ref_paths(game):
    pre = PREFIX[game]
    fs = glob.glob(f"audio-ref/{game}/sfx/{pre}_sfx*.wav")
    return sorted(fs, key=lambda p: int(os.path.basename(p).split("_sfx")[1].split(".")[0]))


def frames_from_refs(refs, win):
    """Fixed soundtest window per SFX (must match SOUNDTEST_FRAMES in
    games/<game>/src/bin/soundtest.rs) -- a regular layout makes the split
    robust: every SFX is `win` frames after an 18-frame gap, no drift."""
    return [win] * len(refs)


def onset_frames(frames):
    out, cur = [], 0
    for f in frames:
        cur += GAP_FRAMES
        out.append(cur)
        cur += f
    return out




def align_segments(psx, sr, refs, frames):
    """Split by the (regular, fixed-window) frame layout. 2D-calibrate the
    samples-per-frame AND a small boot offset (the delay before the soundtest's
    first audio) by maximising the energy landing inside the predicted per-SFX
    windows. Prefix-sum energy makes each trial O(1) per window. With every window
    the same length there is no cumulative drift, so one (spf, offset) aligns the
    whole capture; spec_sim trims the residual leading/trailing silence."""
    cum = np.concatenate([[0.0], np.cumsum(psx ** 2)])
    def wsum(a, b):
        a, b = max(0, int(a)), min(len(psx), int(b))
        return cum[b] - cum[a] if b > a else 0.0
    onsets = onset_frames(frames)
    fr = frames
    N = len(frames)
    # Score = window energy DENSITY minus gap energy density. Maximising raw energy
    # is biased toward large spf (wider windows capture more); the contrast picks
    # the spf+offset where the windows are loud AND the 18-frame gaps are silent.
    def score(spf, off):
        we = sum(wsum(off + onsets[n] * spf, off + (onsets[n] + fr[n]) * spf) for n in range(N))
        ge = sum(wsum(off + (onsets[n] - GAP_FRAMES) * spf, off + onsets[n] * spf) for n in range(N))
        ww, gw = sum(fr) * spf, N * GAP_FRAMES * spf
        return we / (ww + 1e-9) - ge / (gw + 1e-9)
    def search(spfs, offs):
        b = (-1e18, spfs[len(spfs) // 2], 0.0)
        for spf in spfs:
            for off in offs:
                s = score(spf, off)
                if s > b[0]:
                    b = (s, spf, float(off))
        return b
    _, spf, off = search(np.arange(600.0, 1600.0, 5.0), range(0, int(1.2 * sr), int(0.03 * sr)))
    _, spf, off = search(np.arange(spf - 6, spf + 6, 0.25),
                         range(int(off - 0.06 * sr), int(off + 0.06 * sr), int(0.004 * sr)))
    segs = [(int(off + onsets[n] * spf), int(min(off + (onsets[n] + fr[n]) * spf, len(psx))))
            for n in range(len(frames))]
    return segs, spf


def trim_silence(x, thr=0.02):
    if len(x) == 0:
        return x
    e = np.abs(x)
    m = e.max()
    if m < 1e-9:
        return x
    loud = np.where(e > thr * m)[0]
    return x[loud[0]: loud[-1] + 1] if len(loud) else x


def logspec(x, sr, tsr=22050):
    if sr != tsr:
        from math import gcd
        g = gcd(int(sr), tsr)
        x = resample_poly(x, tsr // g, int(sr) // g)
    nper = 1024
    if len(x) < nper:
        x = np.pad(x, (0, nper - len(x)))
    _, _, z = stft(x, fs=tsr, nperseg=nper, noverlap=768)
    return np.log1p(np.abs(z))


def spec_sim(a, sa, b, sb):
    if len(a) < 256 or len(b) < 256:
        return float("nan")
    A = logspec(trim_silence(a), sa)
    B = logspec(trim_silence(b), sb)
    t = min(A.shape[1], B.shape[1])
    if t < 2:
        return float("nan")
    A, B = A[:, :t].ravel(), B[:, :t].ravel()
    if A.std() < 1e-6 or B.std() < 1e-6:
        return float("nan")
    return float(np.corrcoef(A, B)[0, 1])


def dom_pitch(x, sr):
    x = trim_silence(x)
    if len(x) < 512:
        return 0.0
    sp = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    fr = np.fft.rfftfreq(len(x), 1 / sr)
    m = (fr > 50) & (fr < 5000)
    return float(fr[m][np.argmax(sp[m])]) if m.any() else 0.0


def write_mono(path, x, sr):
    w = wave.open(path, "wb")
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(int(sr))
    w.writeframes(np.clip(x * 32768, -32768, 32767).astype("<i2").tobytes())
    w.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("capture")
    ap.add_argument("game", choices=["celeste", "celeste2"])
    ap.add_argument("--win", type=int, default=96)
    ap.add_argument("--dump", help="dir for A/B wav pairs (sfxNN_psx.wav vs _pico8.wav)")
    a = ap.parse_args()

    psx, sr = load(a.capture)
    refs = ref_paths(a.game)
    frames = frames_from_refs(refs, a.win)
    segs, spf = align_segments(psx, sr, refs, frames)
    print(f"{a.game}: PSX {len(psx)/sr:.0f}s -> {len(segs)} windows (spf={spf:.2f}); {len(refs)} refs\n")
    if a.dump:
        os.makedirs(a.dump, exist_ok=True)

    print(f"{'sfx':>4} {'psxDur':>7} {'refDur':>7} {'pitchPSX':>9} {'pitchREF':>9} {'sim':>6}")
    rows = []
    refine = int(0.18 * sr)  # search +/- for each SFX's best alignment (the global
    step = max(1, int(0.012 * sr))  # spf leaves residual per-SFX drift on late SFX)
    for i, (s, e) in enumerate(segs):
        if i >= len(refs):
            break
        rx, rsr = load(refs[i])
        # measure each SFX at its best local alignment, so the score reflects the
        # synthesis (not the harness's residual timing drift). The search is narrow
        # and mismatched audio doesn't correlate, so this can't manufacture a score.
        best, bs = float("-inf"), s
        for d in range(-refine, refine + 1, step):
            if s + d < 0 or e + d > len(psx):
                continue
            sm = spec_sim(psx[s + d:e + d], sr, rx, rsr)
            if not np.isnan(sm) and sm > best:
                best, bs = sm, s + d
        s, e = bs, bs + (e - s)
        seg = psx[s:e]
        sim = best if best > float("-inf") else float("nan")
        pp, pr = dom_pitch(seg, sr), dom_pitch(rx, rsr)
        rows.append((i, sim, pp, pr))
        flag = "" if (not np.isnan(sim) and sim >= 0.6) else "  <--"
        print(f"{i:>4} {len(seg)/sr:>6.2f}s {len(rx)/rsr:>6.2f}s {pp:>8.0f}Hz {pr:>8.0f}Hz {sim:>6.2f}{flag}")
        if a.dump:
            write_mono(f"{a.dump}/sfx{i:02d}_psx.wav", seg, sr)
            write_mono(f"{a.dump}/sfx{i:02d}_pico8.wav", rx, rsr)

    sims = np.array([s for _, s, _, _ in rows if not np.isnan(s)])
    if len(sims):
        print(f"\nSUMMARY {a.game}: mean sim {sims.mean():.3f}, median {np.median(sims):.3f}; "
              f">=0.6: {int((sims >= 0.6).sum())}/{len(sims)}; >=0.8: {int((sims >= 0.8).sum())}/{len(sims)}")
        worst = sorted((r for r in rows if not np.isnan(r[1])), key=lambda r: r[1])[:12]
        print("worst:", ", ".join(f"sfx{i}({s:.2f})" for i, s, _, _ in worst))
    if a.dump:
        print(f"A/B pairs -> {a.dump}/")


if __name__ == "__main__":
    main()
