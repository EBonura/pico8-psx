# pico8-psx

[PICO-8](https://www.lexaloffle.com/pico-8.php) games demade for the **PlayStation 1**,
built on the [PSoXide](https://github.com/EBonura/PSoXide) Rust SDK. Real hardware, real discs.

Two full, playable ports, **Celeste** (the original) and **Celeste 2: Lani's Trek**, on a
single bootable disc with a cover-art launcher: animated menu, dissolve transitions, UI sounds,
a follow camera, and an in-game pause menu (volume, screen mode, borders).

## Build & run

```sh
git clone --recursive https://github.com/EBonura/pico8-psx

make demo-disc        # the collection      -> dist/demo.{bin,cue}
make celeste-disc     # standalone Celeste  -> dist/celeste.{bin,cue}
make celeste2-disc    # standalone Celeste 2 -> dist/celeste2.{bin,cue}
```

Rust **nightly** is pinned by `rust-toolchain.toml` (rustup auto-installs it). Boot the `.cue`
in an emulator (DuckStation, PCSX-Redux) or burn it for real hardware. In the launcher: D-pad
to choose, X to play; hold Select+Start in-game to return.

## Layout

Each game is a standalone Cargo workspace exposing `run()`, shipped on its own or linked into
the `demo` launcher, both games in one combined EXE, packed into a single `.bin`/`.cue` disc
image. Shared runtime, rendering, SPU audio, fonts, pause menu, lives in `shared/`. The PSoXide SDK is pinned as a git submodule under `third_party/`;
`tools/` holds the PICO-8 -> Rust asset/audio converters.

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
