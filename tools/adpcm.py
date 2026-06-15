"""PSX SPU ADPCM encode/decode with prediction filters, for LOOPED wavetables.

The long (oversampled) wavetables are smooth, so 2nd-order ADPCM prediction
(filters 2-4) makes the per-sample residual tiny -> very low 4-bit quantization
noise (the thing that killed the filter-0 long tables). This module picks the best
(filter, shift) per 28-sample block and, for a looped periodic waveform, iterates
the predictor start-state to the loop's fixed point so the loop is seamless (the
SPU carries prev1/prev2 from the last block into the loop-start block).
"""

# SPU ADPCM filter coefficients (x64): sample = resid + (p1*POS + p2*NEG) >> 6
POS = [0, 60, 115, 98, 122]
NEG = [0, 0, -52, -55, -60]


def _clamp(v, lo, hi):
    return lo if v < lo else hi if v > hi else v


def _decode_block(header, data, p1, p2):
    """Decode one 16-byte block (header byte0, then 14 data bytes) from state."""
    shift = header & 0x0F
    filt = (header >> 4) & 0x07
    step = 4096 >> shift if shift <= 12 else 0
    f0, f1 = POS[filt], NEG[filt]
    out = []
    for byte in data:
        for nib in (byte & 0x0F, (byte >> 4) & 0x0F):
            t = nib - 16 if nib >= 8 else nib  # sign-extend 4-bit
            pred = (p1 * f0 + p2 * f1) >> 6
            s = _clamp(t * step + pred, -32768, 32767)
            p2, p1 = p1, s
            out.append(s)
    return out, p1, p2


def _try_block(samples, p1, p2, filt, shift):
    """Encode 28 samples with a fixed (filter, shift); return (nibbles, err, p1, p2)."""
    step = 4096 >> shift
    f0, f1 = POS[filt], NEG[filt]
    nibs, err = [], 0
    for s in samples:
        pred = (p1 * f0 + p2 * f1) >> 6
        resid = s - pred
        nib = _clamp(int(round(resid / step)), -8, 7)
        dec = _clamp(nib * step + pred, -32768, 32767)
        err += (dec - s) ** 2
        p2, p1 = p1, dec
        nibs.append(nib & 0x0F)
    return nibs, err, p1, p2


def _encode_block(samples, p1, p2, flags):
    """Pick the best (filter, shift) for one block; return (16 bytes, p1, p2)."""
    best = None
    for filt in range(5):
        for shift in range(13):
            nibs, err, np1, np2 = _try_block(samples, p1, p2, filt, shift)
            if best is None or err < best[0]:
                best = (err, filt, shift, nibs, np1, np2)
    _, filt, shift, nibs, np1, np2 = best
    blk = [(filt << 4) | shift, flags]
    for k in range(0, 28, 2):
        blk.append((nibs[k] & 0x0F) | ((nibs[k + 1] & 0x0F) << 4))
    return blk, np1, np2


def encode_looped(s16, iters=8):
    """Encode an int16 array (one loop period, multiple of 28 samples) as looped
    SPU ADPCM. Iterates the start-state to the loop fixed point so the seam is
    clean. Returns the byte list."""
    n = len(s16) // 28
    p1 = p2 = 0
    blocks = None
    for _ in range(iters):
        sp1, sp2 = p1, p2  # start-state for this pass = last pass's end-state
        out, t1, t2 = [], sp1, sp2
        for b in range(n):
            flags = (0x04 if b == 0 else 0) | (0x03 if b == n - 1 else 0)
            blk, t1, t2 = _encode_block(s16[b * 28:(b + 1) * 28], t1, t2, flags)
            out.append(blk)
        blocks = out
        p1, p2 = t1, t2  # feed end-state into the next pass
    return [b for blk in blocks for b in blk]


def decode(adpcm, loops=1, p1=0, p2=0):
    """Decode a byte list (n blocks); play `loops` times from the loop-start block
    to inspect the steady-state and the loop seam."""
    nblk = len(adpcm) // 16
    loop_start = 0
    for b in range(nblk):
        if adpcm[b * 16 + 1] & 0x04:
            loop_start = b
    out = []
    for lp in range(loops):
        first = 0 if lp == 0 else loop_start
        for b in range(first, nblk):
            hdr = adpcm[b * 16]
            data = adpcm[b * 16 + 2:b * 16 + 16]
            dec, p1, p2 = _decode_block(hdr, data, p1, p2)
            out.extend(dec)
    return out, p1, p2
