//! Inject PSoXide's PSX linker script into the final link, by absolute
//! path derived from this crate's location. This keeps the game
//! buildable from anywhere (no brittle `-T../../..` relative paths in
//! RUSTFLAGS) while the script itself lives in the pinned submodule.

use std::path::PathBuf;

fn main() {
    // .../pico8-psx/games/celeste  ->  repo root is two levels up.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("game crate must live at <repo>/games/<name>");
    let ld = repo_root.join("third_party/PSoXide/sdk/psoxide.ld");
    let ld = ld.canonicalize().unwrap_or(ld);

    // `-T` selects the linker script; `--oformat=binary` dumps a flat
    // PSX-EXE image (the script lays out the executable header) instead
    // of an ELF.
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rustc-link-arg=--oformat=binary");
    println!("cargo:rerun-if-changed={}", ld.display());
}
