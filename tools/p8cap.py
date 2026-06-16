#!/usr/bin/env python3
"""Cart-vs-port visual comparison harness (celeste2).

CART side: copy the cart, free ~50 tokens by stripping the dead title-screen draw
guts (we jump straight to a level, never showing the title), inject a minimal
_init -> goto_level(N) plus an optional per-frame player pin at (px,py) so the
camera settles on an exact spot, run PICO-8 (must be VISIBLE -- it renders black
when hidden), screencapture the display, crop the centred viewport.

The cart sits at PICO-8's 8192-token ceiling, hence the title strip to make room.
extcmd("screen"), Accessibility and Quartz are blocked/flaky from a CLI; a
screencapture of a visible window (dual-screen) is reliable. CROP is measured for
the centred default window at this resolution.

Usage: p8cap.py CART.p8 LEVEL out.png [px py]
"""
import sys, os, subprocess, time
from PIL import Image

PICO8 = "/Users/ebonura/Desktop/pico-8/PICO-8.app/Contents/MacOS/pico8"
OUT = "/tmp/p8out"
os.makedirs(OUT, exist_ok=True)
DISPLAY = "1"
CROP = (958, 468, 1982, 1492)  # centred PICO-8 viewport within the display capture


def cart_shot(cart_p8, level, out, px=None, py=None):
    data = open(cart_p8, encoding="latin-1").read()
    i = data.find("sspr(64, 32")
    j = data.find("draw_snow()", i)
    if i != -1 and j != -1:
        data = data[:i] + data[j + len("draw_snow()"):]
    if px is not None:
        inj = ("\nfunction _init() game_start() goto_level(%d) end"
               "\n__ou=_update function _update() __ou() for o in all(objects) do"
               " if o.base==player then o.x=%d o.y=%d end end end\n" % (level, px, py))
    else:
        inj = "\nfunction _init() game_start() goto_level(%d) end\n" % level
    cart = os.path.join(OUT, "cap.p8")
    open(cart, "w", encoding="latin-1").write(data.replace("\n__gfx__", inj + "__gfx__", 1))
    subprocess.run(["pkill", "-f", "MacOS/pico8"]); time.sleep(1)
    p = subprocess.Popen([PICO8, "-run", cart], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(6)
    full = os.path.join(OUT, "full.png")
    subprocess.run(["screencapture", "-x", "-D", DISPLAY, full])
    p.terminate()
    Image.open(full).crop(CROP).resize((256, 256), Image.NEAREST).save(out)
    return out


if __name__ == "__main__":
    cart, level, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    px = int(sys.argv[4]) if len(sys.argv) > 5 else None
    py = int(sys.argv[5]) if len(sys.argv) > 5 else None
    print(cart_shot(cart, level, out, px, py))
