# pico8-psx

A collection of [PICO-8](https://www.lexaloffle.com/pico-8.php) games demade
for the **PlayStation 1**, built on the [PSoXide](https://github.com/EBonura/PSoXide)
Rust SDK. Real hardware, real discs, no emul- only nostalgia.

So far: **Celeste** (the original PICO-8 Celeste) and **Celeste 2: Lani's
Trek** (the PICO-8 sequel by EXOK).

The headline artifact is the **demo disc**: one bootable PS1 image that opens
to a cover menu showing each game's real PICO-8 cart label. Pick one with the
D-pad, press X to play; hold Select+Start in-game to drop back to the menu.

## How it's wired

The PSoXide SDK is pinned as a **git submodule** under `third_party/PSoXide`,
not vendored or copied. Each game is its own standalone Cargo workspace that
path-depends into the submodule's SDK + engine crates. The submodule pins an
exact SDK commit, so a clone always builds against a known-good SDK.

```
pico8-psx/
├── rust-toolchain.toml          # nightly + rust-src (matches PSoXide)
├── Makefile                     # build + disc-pack the demo / each game
├── third_party/PSoXide/         # git submodule, pinned SDK commit
├── tools/
│   ├── p8_to_rust.py            # PICO-8 .p8 -> gfx.rs/tilemap.rs converter
│   └── png_to_cover.py          # cart-label PNG -> 4bpp cover array
└── games/
    ├── celeste/
    │   ├── .cargo/config.toml   # target = mipsel-sony-psx, build-std
    │   ├── build.rs             # injects PSoXide's psoxide.ld linker script
    │   ├── Cargo.toml           # standalone workspace; path-deps the submodule
    │   └── src/
    │       ├── lib.rs           # pub fn run() -- the game; returns on Select+Start
    │       └── main.rs          # thin bin shim: loop { celeste::run() }
    ├── celeste2/                # same lib+bin layout; assets-src/ holds the cart
    └── demo/                    # the launcher: links both games as libraries,
        └── src/                 # shows the cover menu, dispatches to run()
```

Each game is a **library** exposing `run()` (which boots the game and returns
when the player holds Select+Start) plus a thin binary that just calls it in a
loop -- so a game can ship standalone *or* be linked into the launcher. The
`demo` crate links both game libs, draws the cover menu, and calls the chosen
`run()`; on return it re-uploads its menu VRAM (the game clobbered it) and
shows the menu again. Because `mkisopsx` packs exactly one boot EXE, the whole
thing is a single combined EXE -> a single disc.

The "linking" trick: `build.rs` resolves the submodule's `sdk/psoxide.ld` by
absolute path and passes `-T <script> --oformat=binary` to the final link, so
the build works from anywhere without brittle relative RUSTFLAGS.

> Note: the launcher's release profile uses `lto = "thin"`, not `lto = true`.
> Fat LTO garbage-collects the cross-crate `celeste::run` / `celeste2::run`
> call chains and silently drops both games from the binary.

## Prerequisites

- Rust **nightly** (pinned by `rust-toolchain.toml`; `rustup` auto-installs it)
- The `mipsel-sony-psx` target ships with nightly; `rust-src` is needed for
  `-Zbuild-std` and is listed in the toolchain components.

## Build & run

```sh
git clone --recursive https://github.com/EBonura/pico8-psx
# or, if already cloned:  make submodule

make demo-disc        # the headline artifact -> dist/demo.bin / dist/demo.cue

make celeste-disc     # standalone Celeste  -> dist/celeste.{bin,cue}
make celeste2-disc    # standalone Celeste 2 -> dist/celeste2.{bin,cue}
```

Boot the `.cue` in an emulator (DuckStation, PCSX-Redux) or burn it to a disc /
load via an ODE for real hardware.

### Testing in the PSoXide emulator

The submodule ships a host-side emulator. Boot a disc and dump VRAM:

```sh
cd third_party/PSoXide/emu/crates/emulator-core
PSOXIDE_BIOS=/path/to/SCPH1001.BIN \
PSOXIDE_DISC=$PWD/../../../../../dist/demo.cue \
PSOXIDE_VRAM_DUMP=/tmp/demo.ppm \
cargo run --example boot_disc --release -- 500000000
```

`probe_disc_pad_trace` adds scripted input via `PSOXIDE_PAD1_PULSES`
(`<mask>@<start_vblank>+<frames>`, comma-separated; masks are psx-pad's
button bits, e.g. RIGHT `0x20`, X `0x4000`, Select+Start `0x09`) and dumps the
final frame to `PSOXIDE_VISIBLE_DUMP`.

## Status

The **demo disc boots to the cover menu**, and both games launch from it and
render, verified end-to-end in the PSoXide emulator (boot -> select -> launch
-> return-to-menu):

- **Celeste** — full ccleste port (object engine, dash, 16.16 fixed-point
  physics, room transitions, SPU audio). Renders gameplay from room 1.
- **Celeste 2** — uploads the real Lani's Trek spritesheet/CLUT/map and renders
  level 1's opening trailhead screen from the actual tilemap (render bring-up;
  the grapple/physics port comes next).

## Assets

PICO-8 graphics are converted to the PS1 4bpp arrays each game uploads to VRAM
by `tools/p8_to_rust.py`, which parses a cart's `__gfx__`/`__gff__`/`__map__`
sections into `gfx.rs` (256x256 spritesheet, pre-doubled from 128x128) and
`tilemap.rs` (map + per-sprite flags). The universal PICO-8 `font.rs` and
`palette.rs` are shared verbatim between games. Celeste 2's assets come from the
standalone level-1 cart in [ExOK/Celeste2](https://github.com/ExOK/Celeste2),
kept under `games/celeste2/assets-src/` for reproducibility:

```sh
python3 tools/p8_to_rust.py games/celeste2/assets-src/celeste2-level1.p8 \
    games/celeste2/src/assets
```

## Adding another PICO-8 game

Copy `games/celeste/` to `games/<name>/`, rename the crate, drop the source
cart under `assets-src/`, regenerate `gfx.rs`/`tilemap.rs` with
`tools/p8_to_rust.py`, and add `<name>` / `<name>-disc` targets to the Makefile.
Each game stays an independent workspace, so they build and version
independently while sharing one SDK pin.
