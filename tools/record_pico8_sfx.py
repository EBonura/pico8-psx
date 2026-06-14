#!/usr/bin/env python3
"""Record per-SFX reference audio from a PICO-8 cart, as the ground truth to
compare the PSX SPU synthesis against.

PICO-8's headless WAV export (`-export foo%d.wav`) writes correctly-sized but
SILENT files -- the synth doesn't run without a real audio device. So instead
we drive a *windowed* PICO-8 instance with a generated recorder cart that, for
each non-empty SFX, calls extcmd("audio_rec"), plays the SFX, waits its exact
duration, then extcmd("audio_end") to save a WAV. Durations come from the
(silent but correctly-timed) headless export, so each clip is tight.

Output: one `<prefix>_sfxNN.wav` per non-empty SFX in --out (22050 Hz mono 16-bit,
PICO-8's native recording format).

Usage:
    python3 tools/record_pico8_sfx.py CART.p8.png --prefix c1 --out /tmp/ref/celeste
Requires PICO-8 installed; override the binary with PICO8=/path/to/pico8.
"""
import argparse, os, subprocess, sys, time, wave, math, glob, tempfile

PICO8 = os.environ.get(
    "PICO8", "/Users/ebonura/Desktop/pico-8/PICO-8.app/Contents/MacOS/pico8"
)
CAP_SECONDS = 10.0  # looping SFX would play forever; cap the recording window
TAIL_FRAMES = 12    # ~0.2s extra to catch the note release


def run_pico8(args, wait=None, cwd=None):
    """Launch pico8; if wait is set, run windowed for `wait`s then terminate.
    `cwd` matters for -export, whose output paths are relative to it."""
    p = subprocess.Popen([PICO8, *args], cwd=cwd,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if wait is None:
        p.wait()
    else:
        time.sleep(wait)
        p.terminate()
        try:
            p.wait(timeout=5)
        except subprocess.TimeoutExpired:
            p.kill()


def cart_to_p8(cart, workdir):
    """Convert any cart form to a readable .p8; return its path."""
    if cart.endswith(".p8") and not cart.endswith(".p8.png"):
        return cart
    run_pico8([os.path.abspath(cart), "-export", "cart.p8"], cwd=workdir)
    return os.path.join(workdir, "cart.p8")


def sfx_section(p8):
    lines = open(p8).read().splitlines()
    i = lines.index("__sfx__")
    rows = []
    for l in lines[i + 1:]:
        if l.startswith("__"):
            break
        rows.append(l.strip())
    return lines, rows


def silent_export_durations(cart, workdir):
    """Headless-export all SFX (silent) just to read each one's exact length."""
    d = os.path.join(workdir, "silent")
    os.makedirs(d, exist_ok=True)
    run_pico8([os.path.abspath(cart), "-export", "s%d.wav"], cwd=d)
    durs = {}
    for f in glob.glob(os.path.join(d, "s*.wav")):
        n = int(os.path.basename(f)[1:-4])
        w = wave.open(f, "rb")
        durs[n] = w.getnframes() / w.getframerate()
        w.close()
    return durs


def build_recorder(p8_lines, sfx_rows, used, durs, prefix, out_path):
    """Write a recorder cart: the source __sfx__/__music__ plus a driver that
    records each used SFX to its own wav."""
    entries = []
    for n in used:
        frames = min(durs.get(n, 1.0), CAP_SECONDS) * 60 + TAIL_FRAMES
        entries.append(f"{{{n},{int(math.ceil(frames))}}}")
    lua_table = "{" + ",".join(entries) + "}"
    driver = f"""list={lua_table}
ci=1 phase=0 t=0
function _update60()
 if ci>#list then extcmd("audio_end") return end
 local e=list[ci]
 if phase==0 then
  for c=0,3 do sfx(-1,c) end
  extcmd("set_filename","{prefix}_sfx"..e[1])
  extcmd("audio_rec")
  sfx(e[1],0)
  t=0 phase=1
 else
  t+=1
  if t>=e[2] then extcmd("audio_end") ci+=1 phase=0 end
 end
end
function _draw()
 cls()
 local lbl=ci<=#list and list[ci][1] or "done"
 print("rec sfx "..lbl.."  "..min(ci,#list).."/"..#list,4,60,7)
end
"""
    li = p8_lines.index("__lua__")
    gi = p8_lines.index("__gfx__")
    with open(out_path, "w") as f:
        f.write("\n".join(p8_lines[: li + 1]) + "\n")
        f.write(driver)
        f.write("\n".join(p8_lines[gi:]) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cart")
    ap.add_argument("--prefix", required=True, help="output filename prefix, e.g. c1")
    ap.add_argument("--out", required=True, help="output directory for the wavs")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    with tempfile.TemporaryDirectory() as wd:
        p8 = cart_to_p8(args.cart, wd)
        lines, rows = sfx_section(p8)
        durs = silent_export_durations(args.cart, wd)
        used = [n for n, r in enumerate(rows) if len(r) >= 8 and any(c != "0" for c in r[8:])]
        total = sum(min(durs.get(n, 1.0), CAP_SECONDS) + TAIL_FRAMES / 60 + 1 / 60 for n in used)
        print(f"{len(used)} non-empty SFX; ~{total:.0f}s of real-time recording")
        rec = os.path.join(wd, "recorder.p8")
        build_recorder(lines, rows, used, durs, args.prefix, rec)
        run_pico8(["-run", rec, "-desktop", os.path.abspath(args.out), "-volume", "128"],
                  wait=total + 8)

    wavs = sorted(glob.glob(os.path.join(args.out, f"{args.prefix}_sfx*.wav")))
    print(f"recorded {len(wavs)} wavs -> {args.out}")


if __name__ == "__main__":
    main()
