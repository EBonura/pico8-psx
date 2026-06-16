#!/usr/bin/env python3
"""Capture a PICO-8 screenshot of celeste2.p8 at a determinate level + camera.
Injects a debug _init/_update (goto_level + pinned camera + extcmd screen/shutdown)
into a copy of the cart, runs PICO-8 headless-ish, returns the screenshot path.

Requires desktop_path -> /tmp/p8out in pico-8 config (set once by the agent)."""
import sys, os, subprocess, time, shutil
PICO8 = "/Users/ebonura/Desktop/pico-8/PICO-8.app/Contents/MacOS/pico8"
SRC = "/tmp/carts/celeste2.p8"
OUT = "/tmp/p8out"

def shot(level, cx, cy, name):
    os.makedirs(OUT, exist_ok=True)
    lines = open(SRC).read().split("\n")
    li = lines.index("__lua__")
    end = li + 1
    while end < len(lines) and not (lines[end].startswith("__") and lines[end].rstrip().endswith("__")):
        end += 1
    dbg = [
        f"function _init() game_start() goto_level({level}) level_intro=0 end",
        "__ou=_update __dt=0",
        f'function _update() __dt+=1 __ou() camera_x={cx} camera_y={cy} camera({cx},{cy}) if __dt==30 then extcmd("screen") end if __dt==45 then extcmd("shutdown") end end',
    ]
    out_lines = lines[:end] + dbg + lines[end:]
    cart = os.path.join(OUT, "dbg.p8")
    open(cart, "w").write("\n".join(out_lines))
    png = os.path.join(OUT, "dbg_0.png")
    if os.path.exists(png): os.remove(png)
    p = subprocess.Popen([PICO8, "-run", cart, "-windowed", "1"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(50):
        if os.path.exists(png): break
        time.sleep(0.3)
    time.sleep(0.4)
    p.terminate()
    try: p.wait(timeout=4)
    except Exception: p.kill()
    if os.path.exists(png):
        dst = os.path.join(OUT, name); shutil.copy(png, dst); return dst
    return None

if __name__ == "__main__":
    lvl, cx, cy, name = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    r = shot(lvl, cx, cy, name)
    print("OK", r) if r else print("FAILED (no screenshot)")
