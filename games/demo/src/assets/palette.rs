// PICO-8 16-colour sprite/map CLUT (RGB555, entry 0 = transparent).
//
// The launcher only needs this CLUT (to draw the cart-label covers, which
// are 4bpp in the PICO-8 palette); the games carry the full palette table
// (PICO8_RGB / TEXT_CLUTS) for their own flat-shape and text rendering.
pub static PICO8_CLUT: [u16; 16] = [
    0x0000, 0x28A3, 0x288F, 0x2A00, 0x1955, 0x254B, 0x6318, 0x77DF, 0x241F, 0x029F, 0x13BF, 0x1B80, 0x7EA5, 0x4DD0, 0x55DF, 0x573F,
];
