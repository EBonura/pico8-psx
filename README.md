# pico8-psx

[PICO-8](https://www.lexaloffle.com/pico-8.php) games demade for the **PlayStation 1**,
built on the [PSoXide](https://github.com/EBonura/PSoXide) Rust SDK. Real hardware, real discs.

The **Celeste Classic Collection**: two full, playable ports, **Celeste** (the original) and
**Celeste 2: Lani's Trek**, on a single bootable disc with a cover-art launcher (animated menu,
dissolve transitions, UI sounds, a follow camera, side-gradient borders, and an in-game pause
menu for volume, screen mode, and borders). Verified end to end on a modchipped console.

## Build & run

```sh
git clone --recursive https://github.com/EBonura/pico8-psx

make collection-disc  # the collection       -> dist/celeste-collection.{bin,cue}
make celeste-disc     # standalone Celeste   -> dist/celeste.{bin,cue}
make celeste2-disc    # standalone Celeste 2 -> dist/celeste2.{bin,cue}
```

Rust **nightly** is pinned by `rust-toolchain.toml` (rustup auto-installs it). Boot the `.cue`
in [PSoXide](https://github.com/EBonura/PSoXide) (or another PS1 emulator), or burn it to a CD-R
for real hardware. In the launcher: D-pad to choose, X to play; hold Select+Start in-game to
return.

## Running on real hardware

These ports are tuned for actual PS1 silicon, not just an emulator. The interesting bugs only
showed up on a console, so each was first reproduced by making the emulator more faithful, then
fixed against it:

- **Audio**: ADPCM samples are uploaded to SPU RAM by DMA. The original PIO path never armed the
  SPU transfer mode, so the writes were silently dropped on hardware (and on FIFO-accurate
  emulators), and the music came out as a drone.
- **Sprite recolour**: palette swaps (`pal()`, e.g. Madeline's hair on dash) ping-pong between two
  CLUT slots, because the GPU caches the CLUT and reloads it only when the CLUT word changes, not
  when VRAM is overwritten.
- **Input and visuals**: controller reads are drained and retried so a held button cannot re-fire,
  and the dithering and side gradients are drawn to read correctly on a CRT.

The hardware-accuracy work itself (SPU / GPU / CLUT fidelity, plus an on-disc conformance suite)
lives in [PSoXide](https://github.com/EBonura/PSoXide). Burn with e.g.
`cdrdao write --driver generic-mmc celeste-collection.cue` and boot on a modchipped console.

## Layout

Each game is a standalone Cargo workspace exposing `run()`, shipped on its own or linked into the
`celeste-collection` launcher (both games in one combined EXE, packed into a single `.bin`/`.cue`
disc image). Shared runtime (rendering, SPU audio, fonts, pause menu) lives in `shared/`. The
PSoXide SDK is pinned as a git submodule under `third_party/`; `tools/` holds the PICO-8 to Rust
asset/audio converters.

## Credits

- **Celeste** (2016) and **Celeste 2: Lani's Trek** (2021), the original PICO-8 carts, by
  Maddy Thorson and Noel Berry; Celeste 2 music by Lena Raine.
- **[PICO-8](https://www.lexaloffle.com/pico-8.php)** by Lexaloffle Games.
- Menu sounds from Kenney's [Interface Sounds](https://kenney.nl/assets/interface-sounds)
  (CC0 / public domain).
- Built on the [PSoXide](https://github.com/EBonura/PSoXide) PS1 SDK (GPL-2.0).

## License

The port code in this repository is **GPL-2.0**, matching the PSoXide SDK it links against
(see [`LICENSE`](LICENSE)). This is an unofficial, non-commercial fan port: Celeste, Celeste 2,
and PICO-8 belong to their respective creators, and all rights to the original games remain
theirs.
