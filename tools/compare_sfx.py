#!/usr/bin/env python3
"""Diff the PSX SPU soundtest capture against the PICO-8 SFX references.

The soundtest disc (games/celeste/src/bin/soundtest.rs) plays each SFX in
isolation separated by a ~0.3s silence gap. This splits the capture on those
gaps (merging across short internal quiet notes so one SFX stays one segment),
pairs segment N with reference c1_sfxN.wav, and reports per-SFX:
  - duration (PSX vs reference)
  - dominant pitch (PSX vs reference) and their ratio
  - spectral-shape correlation (timbre similarity, 0..1)

Usage:
  python3 tools/compare_sfx.py /tmp/psx_soundtest_full.wav audio-ref/celeste/sfx --prefix c1
"""
import sys, glob, wave, argparse
import numpy as np


def load(path):
    w = wave.open(path, "rb")
    sr, ch, n = w.getframerate(), w.getnchannels(), w.getnframes()
    a = np.frombuffer(w.readframes(n), dtype=np.int16).astype(np.float32)
    w.close()
    if ch == 2:
        a = a.reshape(-1, 2).mean(1)
    return a / 32768.0, sr


def envelope(x, sr, win_s=0.01):
    win = max(1, int(win_s * sr))
    return np.sqrt(np.convolve(x ** 2, np.ones(win) / win, "same"))


# Soundtest layout (keep in sync with games/celeste/src/bin/soundtest.rs):
# sequence is [gap][sfx0][gap][sfx1]...; each block = GAP + FRAMES[n] frames.
GAP_FRAMES = 18
FRAMES = [
    38, 38, 54, 38, 70, 54, 54, 38, 54, 54, 261, 516,
    261, 70, 70, 54, 261, 261, 261, 516, 134, 516, 261, 102,
    389, 389, 198, 198, 389, 389, 198, 198, 198, 198, 198, 70,
    198, 261, 198, 261, 261, 134, 516, 261, 134, 261, 261, 261,
    516, 134, 134, 86, 516, 516, 54, 182, 261, 261, 261, 261,
    261, 261, 600,
]


def onset_frames():
    """Frame index where each SFX's audio window begins (after its gap)."""
    out, cur = [], 0
    for n in range(len(FRAMES)):
        cur += GAP_FRAMES            # the gap before sfx n
        out.append(cur)
        cur += FRAMES[n]             # the play window
    return out


def deterministic_segments(x, sr):
    """Split by the known frame layout. The emulator's frames/sample rate
    isn't exactly 735, so calibrate samples-per-frame by maximising audio
    energy landing inside the predicted windows (vs the silent gaps)."""
    onsets = onset_frames()
    best_spf, best_score = None, -1.0
    for spf in np.arange(734.0, 737.5, 0.05):
        score = 0.0
        for n, of in enumerate(onsets):
            a = int(of * spf)
            b = int((of + FRAMES[n]) * spf)
            if b <= len(x):
                score += float(np.sum(x[a:b] ** 2))
        if score > best_score:
            best_score, best_spf = score, spf
    segs = []
    for n, of in enumerate(onsets):
        a = int(of * best_spf)
        b = min(int((of + FRAMES[n]) * best_spf), len(x))
        segs.append((a, b))
    return segs, best_spf


from scipy.signal import resample_poly, stft


def logspec(x, sr, target_sr=22050):
    """Log-magnitude STFT at a common 22.05 kHz rate (PICO-8's output rate),
    as a time x freq fingerprint for aligned PSX-vs-reference comparison."""
    if sr != target_sr:
        from math import gcd
        g = gcd(int(sr), target_sr)
        x = resample_poly(x, target_sr // g, int(sr) // g)
    nperseg = 1024
    if len(x) < nperseg:  # keep freq bins identical across clips
        x = np.pad(x, (0, nperseg - len(x)))
    _, _, z = stft(x, fs=target_sr, nperseg=nperseg, noverlap=768)
    return np.log1p(np.abs(z)), target_sr


def trim_silence(x, thr_rel=0.02):
    """Drop leading/trailing near-silence so two clips align on their first
    audible sample (the reference recordings carry a little lead-in)."""
    e = np.abs(x)
    loud = np.where(e > thr_rel * (e.max() + 1e-9))[0]
    return x[loud[0]: loud[-1] + 1] if len(loud) else x


def spec_sim(a, sa, b, sb):
    """Time-aligned spectral similarity in [0,1]: correlation of the two
    log-STFTs over their common length (both trimmed to their first note)."""
    A, _ = logspec(trim_silence(a), sa)
    B, _ = logspec(trim_silence(b), sb)
    t = min(A.shape[1], B.shape[1])
    if t < 2:
        return float("nan")
    A, B = A[:, :t].ravel(), B[:, :t].ravel()
    if A.std() < 1e-6 or B.std() < 1e-6:
        return float("nan")
    return float(np.corrcoef(A, B)[0, 1])


def spectral_shape(x, sr, nbins=48):
    """Log-spaced magnitude spectrum of the loudest 0.4s, normalised -- a
    rough timbre fingerprint comparable across the two synths."""
    win = int(0.4 * sr)
    if len(x) > win:
        e = np.convolve(x ** 2, np.ones(win), "valid")
        x = x[int(np.argmax(e)):][:win]
    if len(x) < 256:
        return np.zeros(nbins)
    sp = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    fr = np.fft.rfftfreq(len(x), 1 / sr)
    edges = np.logspace(np.log10(50), np.log10(sr / 2), nbins + 1)
    out = np.array([sp[(fr >= edges[i]) & (fr < edges[i + 1])].sum() for i in range(nbins)])
    return out / (out.sum() + 1e-9)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("capture")
    ap.add_argument("refdir")
    ap.add_argument("--prefix", default="c1")
    ap.add_argument("--dump", help="dir to write aligned A/B wav pairs for listening")
    args = ap.parse_args()

    psx, sr = load(args.capture)
    segs, spf = deterministic_segments(psx, sr)
    refs = sorted(glob.glob(f"{args.refdir}/{args.prefix}_sfx*.wav"),
                  key=lambda p: int(p.rsplit("_sfx", 1)[1].split(".")[0]))
    print(f"PSX capture {len(psx)/sr:.0f}s -> {len(segs)} windows (spf={spf:.2f}); "
          f"{len(refs)} references")

    import os
    dump = getattr(args, "dump", None)
    if dump:
        os.makedirs(dump, exist_ok=True)

    print(f"{'sfx':>4} {'psxDur':>7} {'refDur':>7} {'dDur':>6} {'spec_sim':>9}")
    rows = []
    for i, (a, b) in enumerate(segs):
        if i >= len(refs):
            break
        seg = psx[a:b]
        rx, rsr = load(refs[i])
        sim = spec_sim(seg, sr, rx, rsr)
        ddur = len(seg) / sr - len(rx) / rsr
        rows.append((i, sim, ddur))
        flag = "" if (not np.isnan(sim) and sim >= 0.6) else "  <--"
        print(f"{i:>4} {len(seg)/sr:>6.2f}s {len(rx)/rsr:>6.2f}s {ddur:>+6.2f} {sim:>9.2f}{flag}")
        if dump:
            write_mono(f"{dump}/sfx{i:02d}_psx.wav", seg, sr)
            write_mono(f"{dump}/sfx{i:02d}_pico8.wav", rx, rsr)

    sims = np.array([s for _, s, _ in rows if not np.isnan(s)])
    if len(sims):
        print(f"\nsummary: spec_sim mean {sims.mean():.2f}, median {np.median(sims):.2f}; "
              f">=0.6 for {int((sims >= 0.6).sum())}/{len(sims)}; "
              f"max |dur drift| {max(abs(d) for _, _, d in rows):.2f}s")
        worst = sorted((r for r in rows if not np.isnan(r[1])), key=lambda r: r[1])[:10]
        print("lowest-similarity SFX:", ", ".join(f"sfx{i}({s:.2f})" for i, s, _ in worst))
        if dump:
            print(f"A/B pairs -> {dump}/ (sfxNN_psx.wav vs sfxNN_pico8.wav)")


def write_mono(path, x, sr):
    import wave as _w
    y = np.clip(x * 32768, -32768, 32767).astype("<i2")
    w = _w.open(path, "wb")
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(int(sr))
    w.writeframes(y.tobytes())
    w.close()


if __name__ == "__main__":
    main()
