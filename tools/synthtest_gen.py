#!/usr/bin/env python3
"""Emit MATCHED PICO-8 + PSX test songs so we can replicate PICO-8 music on the
PSX one isolated case at a time (per-instrument, percussion, then a trio) and
find why the music sequencer sounds wrong.

Same note data drives both sides:
  - a PICO-8 .p8 recorder cart (real __sfx__/__music__ sections + a recorder)
  - a Rust file (games/celeste/src/assets/synthtest_data.rs) for the PSX engine

Each song plays for SONG_FRAMES then silence GAP_FRAMES, in order, looping; the
host capture splits on the gaps. PICO-8 records the same sequence with the same
frame timing.
"""
SONG_FRAMES = 180   # 3s per song at 60fps
GAP_FRAMES  = 30

# ---- test material -----------------------------------------------------------
# note = (pitch, instr, vol, effect);  None = empty/off note (vol 0)
def scale(instr, base=24, vol=5):
    steps = [0,2,4,5,7,9,11,12,14,16,17,19,21,23,24, -1]  # C major up; -1=rest
    return [None if s < 0 else (base+s, instr, vol, 0) for s in steps] + [None]*16

def drums():
    # kick (low noise) / snare (high noise) rhythm with rests
    K=(12,6,6,0); S=(34,6,5,0); n=None
    pat=[K,n,n,n, S,n,n,n, K,n,K,n, S,n,n,n]
    return pat + pat

def bass():
    notes=[12,12,12,12, 7,7,7,7, 9,9,9,9, 5,5,5,5]
    return [(p,0,6,0) for p in notes] + [(p,0,6,0) for p in notes]

def melody():  # triangle lead
    s=[24,24,28,31, 28,26,24,26, 29,29,33,36, 33,31,29,31]
    return [(p,0,5,0) for p in s]*2

def slide_test():   # effect 1: each note slides from the previous note's pitch
    s=[24,36,29,41, 24,36,29,41]
    return [(p,0,5,1) for p in s]*2

def vibrato_test(): # effect 2: sustained note with vibrato
    return [(24,0,5,2) for _ in range(16)]

def arp_test():     # effect 6: group [24,28,31,36] arpeggiated fast
    g=[24,28,31,36]
    return [(p,0,5,6) for p in g]*8

def drop_test():    # effect 3: pitch drops to nothing (percussive)
    return [(36,0,6,3),None,(36,0,6,3),None]*4

# SFX table: (speed, loop_start, loop_end, notes[32])
def mksfx(speed, notes):
    notes = (notes + [None]*32)[:32]
    return (speed, 0, 0, notes)

SFX = [
    mksfx(16, scale(0)),     # 0: triangle scale
    mksfx(12, drums()),      # 1: noise percussion
    mksfx(14, bass()),       # 2: triangle bass
    mksfx(16, melody()),     # 3: triangle melody (for trio)
    mksfx(16, slide_test()), # 4: slide (effect 1)
    mksfx(16, vibrato_test()),# 5: vibrato (effect 2)
    mksfx(16, arp_test()),   # 6: arpeggio fast (effect 6)
    mksfx(16, drop_test()),  # 7: drop (effect 3)
]
# music patterns: (flags, [sfx_or_None x4])  -- one pattern per song here
SONGS = [
    (0, [0, None, None, None]),   # song0: scale only
    (0, [1, None, None, None]),   # song1: drums only
    (0, [3, 2, 1, None]),         # song2: melody + bass + drums
    (0, [4, None, None, None]),   # song3: slide
    (0, [5, None, None, None]),   # song4: vibrato
    (0, [6, None, None, None]),   # song5: arpeggio
    (0, [7, None, None, None]),   # song6: drop
]
SONG_NAMES = ["scale", "drums", "trio", "slide", "vibrato", "arp", "drop"]

# ---- encode helpers ----------------------------------------------------------
def note_word(n):
    if n is None: return 0
    pitch, instr, vol, eff = n
    return (pitch & 0x3F) | ((instr & 7) << 6) | ((vol & 7) << 9) | ((eff & 7) << 12)

# ---- PICO-8 .p8 cart ---------------------------------------------------------
def p8_sfx_line(sfx):
    speed, ls, le, notes = sfx
    s = f"00{speed:02x}{ls:02x}{le:02x}"
    for n in notes:
        if n is None:
            s += "00000"
        else:
            pitch, instr, vol, eff = n
            s += f"{pitch:02x}{instr:01x}{vol:01x}{eff:01x}"
    return s

def p8_music_line(song):
    flags, chans = song
    s = f"{flags:02x} "
    for c in chans:
        s += "40" if c is None else f"{c:02x}"
    return s

lua = f"""t=0
done=false
songs={{{','.join(str(i) for i in range(len(SONGS)))}}}
sf={SONG_FRAMES}
gf={GAP_FRAMES}
function _update60()
 if t==0 then extcmd("set_filename","synthtest"); extcmd("audio_rec") end
 local period=sf+gf
 local idx=t\\period
 local ph=t%period
 if ph==0 and idx<#songs then music(songs[idx+1],0,0)
 elseif ph==sf then music(-1) end
 t+=1
 if t==#songs*period+10 and not done then extcmd("audio_end"); done=true end
end
function _draw() cls() print("synthtest t="..t,4,40,7) if done then print("saved",4,50,11) end end
"""

sfx_lines = [p8_sfx_line(s) for s in SFX] + ["00010000" + "00000"*32]*(64-len(SFX))
music_lines = [p8_music_line(s) for s in SONGS]

# Build on a known-good full cart (celeste2.p8) so PICO-8 loads it cleanly;
# replace only the __lua__, __sfx__ and __music__ sections.
import re
base = open("/tmp/carts/celeste2.p8").read()
def replace_section(src, name, body):
    pat = re.compile(r'__'+name+r'__\n.*?(?=\n__[a-z]+__|\Z)', re.S)
    repl = f'__{name}__\n{body}'
    if pat.search(src):
        return pat.sub(lambda m: repl, src, count=1)
    return src.rstrip('\n') + '\n' + repl + '\n'
cart = replace_section(base, 'lua', lua)
cart = replace_section(cart, 'sfx', "\n".join(sfx_lines[:len(SFX)]))
cart = replace_section(cart, 'music', "\n".join(music_lines))
open("/tmp/carts/synthtest.p8", "w").write(cart)

# ---- Rust data ---------------------------------------------------------------
def rust():
    out = ["// generated by tools/synthtest_gen.py -- matched PICO-8 test songs",
           f"pub const SONG_FRAMES: u32 = {SONG_FRAMES};",
           f"pub const GAP_FRAMES: u32 = {GAP_FRAMES};",
           f"pub const NUM_SONGS: usize = {len(SONGS)};"]
    out.append(f"pub static TEST_SFX_META: [[u8; 4]; {len(SFX)}] = [")
    for speed, ls, le, _ in SFX:
        out.append(f"    [{speed}, {ls}, {le}, 0],")
    out.append("];")
    out.append(f"pub static TEST_SFX_NOTES: [[u16; 32]; {len(SFX)}] = [")
    for _, _, _, notes in SFX:
        words = ", ".join(str(note_word(n)) for n in notes)
        out.append(f"    [{words}],")
    out.append("];")
    out.append(f"pub static TEST_MUSIC: [[u8; 8]; {len(SONGS)}] = [")
    for flags, chans in SONGS:
        cb = [ (0x80 if c is None else c) for c in chans ]
        out.append(f"    [{flags}, {cb[0]}, {cb[1]}, {cb[2]}, {cb[3]}, 0, 0, 0],")
    out.append("];")
    return "\n".join(out) + "\n"

open("games/celeste/src/assets/synthtest_data.rs", "w").write(rust())
print("wrote /tmp/carts/synthtest.p8 and games/celeste/src/assets/synthtest_data.rs")
print("songs:", list(zip(range(len(SONGS)), SONG_NAMES)))
