/// Standard EGA 16-color palette as RGBA bytes.
///
/// Index 6 (brown) uses 0xAA5500 instead of 0xAAAA00 due to CGA-era
/// hardware that adjusted dark yellow to look more natural.
pub const EGA_PALETTE: [[u8; 4]; 16] = [
    [0x00, 0x00, 0x00, 0xFF], // 0  Black
    [0x00, 0x00, 0xAA, 0xFF], // 1  Blue
    [0x00, 0xAA, 0x00, 0xFF], // 2  Green
    [0x00, 0xAA, 0xAA, 0xFF], // 3  Cyan
    [0xAA, 0x00, 0x00, 0xFF], // 4  Red
    [0xAA, 0x00, 0xAA, 0xFF], // 5  Magenta
    [0xAA, 0x55, 0x00, 0xFF], // 6  Brown
    [0xAA, 0xAA, 0xAA, 0xFF], // 7  Light gray
    [0x55, 0x55, 0x55, 0xFF], // 8  Dark gray
    [0x55, 0x55, 0xFF, 0xFF], // 9  Light blue
    [0x55, 0xFF, 0x55, 0xFF], // 10 Light green
    [0x55, 0xFF, 0xFF, 0xFF], // 11 Light cyan
    [0xFF, 0x55, 0x55, 0xFF], // 12 Light red
    [0xFF, 0x55, 0xFF, 0xFF], // 13 Light magenta
    [0xFF, 0xFF, 0x55, 0xFF], // 14 Yellow
    [0xFF, 0xFF, 0xFF, 0xFF], // 15 White
];

/// Decode a 4-bit-per-pixel EGA tile row byte into two RGBA pixels.
///
/// Each byte contains two pixels: high nibble first, low nibble second.
/// Returns `(pixel0_rgba, pixel1_rgba)`.
#[inline]
pub fn decode_ega_byte(byte: u8) -> ([u8; 4], [u8; 4]) {
    let hi = (byte >> 4) as usize;
    let lo = (byte & 0x0F) as usize;
    (EGA_PALETTE[hi], EGA_PALETTE[lo])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_has_16_entries() {
        assert_eq!(EGA_PALETTE.len(), 16);
    }

    #[test]
    fn all_palette_entries_fully_opaque() {
        for (i, color) in EGA_PALETTE.iter().enumerate() {
            assert_eq!(color[3], 0xFF, "palette entry {i} should be fully opaque");
        }
    }

    #[test]
    fn decode_byte_black_white() {
        let (hi, lo) = decode_ega_byte(0x0F);
        assert_eq!(hi, EGA_PALETTE[0]); // black
        assert_eq!(lo, EGA_PALETTE[15]); // white
    }

    #[test]
    fn decode_byte_same_color() {
        let (hi, lo) = decode_ega_byte(0x66);
        assert_eq!(hi, EGA_PALETTE[6]); // brown
        assert_eq!(lo, EGA_PALETTE[6]); // brown
    }

    #[test]
    fn brown_is_correct() {
        assert_eq!(EGA_PALETTE[6], [0xAA, 0x55, 0x00, 0xFF]);
    }
}
