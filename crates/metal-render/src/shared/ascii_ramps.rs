//! Glyph ramps for the ascii post effect (moved here when the ascii gen model was dropped).


// --------------------------------------------------------------------- font

/// One 8x8 glyph, top row first, most significant bit leftmost.
type Glyph = [u8; 8];

const BLANK: Glyph = [0; 8];

#[rustfmt::skip]
const DOT: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b00000000,
    0b00000000, 0b00011000, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const COLON: Glyph = [
    0b00000000, 0b00011000, 0b00011000, 0b00000000,
    0b00000000, 0b00011000, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const DASH: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b01111110,
    0b01111110, 0b00000000, 0b00000000, 0b00000000,
];
#[rustfmt::skip]
const EQUALS: Glyph = [
    0b00000000, 0b00000000, 0b01111110, 0b00000000,
    0b01111110, 0b00000000, 0b00000000, 0b00000000,
];
#[rustfmt::skip]
const PLUS: Glyph = [
    0b00000000, 0b00011000, 0b00011000, 0b01111110,
    0b01111110, 0b00011000, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const STAR: Glyph = [
    0b00000000, 0b00011000, 0b01011010, 0b00111100,
    0b00111100, 0b01011010, 0b00011000, 0b00000000,
];
#[rustfmt::skip]
const HASH: Glyph = [
    0b00100100, 0b00100100, 0b01111110, 0b00100100,
    0b00100100, 0b01111110, 0b00100100, 0b00100100,
];
#[rustfmt::skip]
const PERCENT: Glyph = [
    0b01100010, 0b01100100, 0b00001000, 0b00010000,
    0b00100000, 0b01000110, 0b10000110, 0b00000000,
];
#[rustfmt::skip]
const AT: Glyph = [
    0b00111100, 0b01000010, 0b10011010, 0b10101010,
    0b10101110, 0b10000000, 0b01111100, 0b00000000,
];
#[rustfmt::skip]
const SLASH: Glyph = [
    0b00000010, 0b00000100, 0b00001000, 0b00010000,
    0b00100000, 0b01000000, 0b10000000, 0b00000000,
];
#[rustfmt::skip]
const BACKSLASH: Glyph = [
    0b10000000, 0b01000000, 0b00100000, 0b00010000,
    0b00001000, 0b00000100, 0b00000010, 0b00000000,
];
#[rustfmt::skip]
const PIPE: Glyph = [
    0b00011000, 0b00011000, 0b00011000, 0b00011000,
    0b00011000, 0b00011000, 0b00011000, 0b00011000,
];
#[rustfmt::skip]
const UNDERSCORE: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b00000000,
    0b00000000, 0b00000000, 0b00000000, 0b11111111,
];
#[rustfmt::skip]
const MIDDOT: Glyph = [
    0b00000000, 0b00000000, 0b00000000, 0b00011000,
    0b00011000, 0b00000000, 0b00000000, 0b00000000,
];
#[rustfmt::skip]
const SMALL_O: Glyph = [
    0b00000000, 0b00000000, 0b00111100, 0b01000010,
    0b01000010, 0b01000010, 0b00111100, 0b00000000,
];
#[rustfmt::skip]
const BIG_O: Glyph = [
    0b00111100, 0b01000010, 0b01000010, 0b01000010,
    0b01000010, 0b01000010, 0b00111100, 0b00000000,
];
#[rustfmt::skip]
const ZERO: Glyph = [
    0b00111100, 0b01000110, 0b01001010, 0b01010010,
    0b01100010, 0b01000010, 0b00111100, 0b00000000,
];
#[rustfmt::skip]
const DISC: Glyph = [
    0b00111100, 0b01111110, 0b11111111, 0b11111111,
    0b11111111, 0b11111111, 0b01111110, 0b00111100,
];

/// The block-drawing ramp, `▁▂▃▄▅▆▇█`, generated rather than typed: a glyph
/// filled from the bottom by `k` of its 8 rows is arithmetic, and eight
/// hand-written literals would be eight chances to fill one from the top.
const fn bar(k: usize) -> Glyph {
    let mut rows = [0u8; 8];
    let mut r = 8 - k;
    while r < 8 {
        rows[r] = 0xFF;
        r += 1;
    }
    rows
}

/// The four picked sets. Every one starts at blank and ends at its solid, so
/// the shader can index them all the same way — density 0 is the first entry
/// by construction rather than by convention.
const RAMPS: [&[Glyph]; 4] = [
    // density — the classic ASCII gradient
    &[BLANK, DOT, COLON, DASH, EQUALS, PLUS, STAR, HASH, PERCENT, AT],
    // bars — the block elements
    &[BLANK, bar(1), bar(2), bar(3), bar(4), bar(5), bar(6), bar(7), bar(8)],
    // strokes — which read as a plotted line rather than as a texture
    &[BLANK, UNDERSCORE, DASH, SLASH, PIPE, BACKSLASH, PLUS, HASH],
    // circles
    &[BLANK, MIDDOT, DOT, SMALL_O, BIG_O, ZERO, DISC],
];

/// How many glyph slots the uniform carries. Comfortably above the longest
/// ramp; the shader indexes it by the ramp's own length.
#[allow(dead_code)]
const SLOTS: usize = 32;

/// One glyph as the two `u32` the shader unpacks: four rows in each, low byte
/// first.
fn pack(g: &Glyph) -> [u32; 2] {
    let word = |a: u8, b: u8, c: u8, d: u8| {
        a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
    };
    [word(g[0], g[1], g[2], g[3]), word(g[4], g[5], g[6], g[7])]
}

/// Every ramp's glyphs back to back, packed as the shader unpacks them. The
/// `fx.ascii` post effect embeds this table in `post.wgsl` (`ASCII_GLYPHS`);
/// a test there keeps the two equal.
pub fn packed_ramps() -> Vec<[u32; 2]> {
    RAMPS.iter().flat_map(|r| r.iter().map(pack)).collect()
}

/// The length of each ramp, in `RAMPS` order.
pub fn ramp_lengths() -> Vec<usize> {
    RAMPS.iter().map(|r| r.len()).collect()
}

// -------------------------------------------------------------------- model
