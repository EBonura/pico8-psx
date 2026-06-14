#!/usr/bin/env python3
"""Generate the 8 PICO-8 instrument waveforms as clean, seamless-looping SPU
ADPCM and emit the `WAVEFORM_ADPCM` Rust array.

The original waveform table (transcoded from an old C++ port) encoded the
oscillators with ADPCM *prediction filters* (1-4). On sharp-edged shapes
(square/saw/pulse) those filters add ripple/overshoot and don't loop cleanly,
so every note's timbre is wrong in both games. Re-encoding with **filter 0**
(direct 4-bit, no prediction) reproduces the shape exactly and loops seamlessly.

Layout is kept byte-compatible with the existing table so the pitch table stays
calibrated: 7 tonal waveforms of 56 samples (2 ADPCM blocks / 32 bytes) +
noise of 224 samples (8 blocks / 128 bytes), at the same offsets.
"""
import math, random

TONAL_SAMPLES = 56  # one cycle, 2 ADPCM blocks
NOISE_SAMPLES = 224  # 8 blocks

random.seed(1)


def tri(t):  # symmetric triangle, -1..1..-1
    return -1 + 4 * t if t < 0.5 else 3 - 4 * t


def waveform(kind, n):
    """One cycle of PICO-8 oscillator `kind` as n samples in [-1, 1]."""
    out = []
    for i in range(n):
        t = i / n
        if kind == 0:  # triangle
            v = tri(t)
        elif kind == 1:  # tilted saw (rises slow, falls fast)
            a = 0.85
            v = (t / a) * 2 - 1 if t < a else 1 - ((t - a) / (1 - a)) * 2
        elif kind == 2:  # sawtooth
            v = t * 2 - 1
        elif kind == 3:  # square (50%)
            v = 1.0 if t < 0.5 else -1.0
        elif kind == 4:  # pulse (~30% duty)
            v = 1.0 if t < 0.3125 else -1.0
        elif kind == 5:  # organ: fundamental + octave (rounded)
            v = (math.sin(2 * math.pi * t) + 0.5 * math.sin(4 * math.pi * t)) / 1.5
        elif kind == 7:  # phaser: two slightly detuned saws (stepped feel)
            v = ((t * 2 - 1) + (((t * 1.5) % 1.0) * 2 - 1)) / 2
        else:
            v = 0.0
        out.append(max(-1.0, min(1.0, v)))
    return out


def encode_block(samples28, loop_start, loop_end):
    """One 16-byte ADPCM block, filter 0, shift 0 (4-bit direct)."""
    flags = (0x04 if loop_start else 0) | (0x03 if loop_end else 0)
    blk = [0x00, flags]  # header: shift=0 filter=0 ; flags
    for k in range(0, 28, 2):
        lo = sample_to_nibble(samples28[k])
        hi = sample_to_nibble(samples28[k + 1])
        blk.append((lo & 0xF) | ((hi & 0xF) << 4))
    return blk


def sample_to_nibble(v):  # v in [-1,1] -> 4-bit two's complement, decoded as n<<12
    n = round(v * 8)
    return max(-8, min(7, n))


def encode_waveform(samples):
    """Encode a looping waveform (block 0 = loop-start, last = loop-end)."""
    nblocks = len(samples) // 28
    data = []
    for b in range(nblocks):
        chunk = samples[b * 28:(b + 1) * 28]
        data += encode_block(chunk, loop_start=(b == 0), loop_end=(b == nblocks - 1))
    return data


def main():
    adpcm = []
    offsets = []
    for kind in range(8):
        offsets.append(len(adpcm))
        if kind == 6:  # noise: random samples, still filter-0 / seamless loop
            samples = [random.uniform(-1, 1) for _ in range(NOISE_SAMPLES)]
        else:
            samples = waveform(kind, TONAL_SAMPLES)
        adpcm += encode_waveform(samples)

    # emit Rust
    print(f"// {len(adpcm)} bytes; offsets {[hex(o) for o in offsets]}")
    rows = []
    for i in range(0, len(adpcm), 16):
        rows.append("    " + ", ".join(str(b & 0xFF) for b in adpcm[i:i + 16]) + ",")
    body = "\n".join(rows)
    open("/tmp/waveform_adpcm.txt", "w").write(
        f"pub static WAVEFORM_ADPCM: [u8; {len(adpcm)}] = [\n{body}\n];\n"
    )
    print(f"offsets = {offsets}")
    print("wrote /tmp/waveform_adpcm.txt")


if __name__ == "__main__":
    main()
