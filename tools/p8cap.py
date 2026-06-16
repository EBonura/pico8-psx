#!/usr/bin/env python3
"""Cart-vs-port visual comparison harness.

CART side: inject a minimal _init that jumps to a level (the cart sits at PICO-8's
8192-token ceiling, so only goto_level fits -- no markers/screenshot code), run
PICO-8 (must be VISIBLE on a screen; it renders black when hidden), screencapture
the display, crop the centred viewport. PORT side: frametest at the same level.
Outputs a 256x256-per-side comparison PNG.

Constraints learned the hard way: extcmd("screen") is unreliable on the full cart;
Accessibility/Quartz window queries are blocked here; PICO-8 must be the visible
front window (dual-screen). CROP is measured for the centred default window at the
current resolution -- re-measure if the window/display changes.
"""
import sys, os, subprocess, time
from PIL import Image
PICO8 = "/Users/ebonura/Desktop/pico-8/PICO-8.app/Contents/MacOS/pico8"
OUT = "/tmp/p8out"; os.makedirs(OUT, exist_ok=True)
DISPLAY = "1"
CROP = (958, 468, 1982, 1492)  # centred PICO-8 viewport within the display capture

def cart_shot(cart_p8, level, out):
    data = open(cart_p8, encoding="latin-1").read()
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

def side_by_side(cart_png, port_png, out):
    a = Image.open(cart_png).convert("RGB").resize((256, 256), Image.NEAREST)
    b = Image.open(port_png).convert("RGB").resize((256, 256), Image.NEAREST)
    c = Image.new("RGB", (520, 256), (40, 40, 40))
    c.paste(a, (0, 0)); c.paste(b, (264, 0)); c.save(out)
    return out

if __name__ == "__main__":
    cart, level, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    print(cart_shot(cart, level, out))
