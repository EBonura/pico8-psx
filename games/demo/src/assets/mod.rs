//! Static assets for the demo-disc launcher menu.
//!
//! `palette` is the universal PICO-8 16-colour CLUT (shared verbatim with
//! the games). `cover_celeste*` are the real PICO-8 cart labels (128x128
//! 4bpp, PICO8_CLUT palette), extracted from the published cart PNGs by
//! `tools/png_to_cover.py`.

pub mod cover_bonnie;
pub mod cover_celeste;
pub mod cover_celeste2;
pub mod palette;
