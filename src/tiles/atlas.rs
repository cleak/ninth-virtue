use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::ega;
use super::lzw;

/// Number of tiles in the Ultima V tileset.
pub const TILE_COUNT: usize = 512;

/// Tile dimensions in pixels.
pub const TILE_SIZE: usize = 16;

/// Bytes per tile in the raw EGA 4bpp format (16 rows x 8 bytes/row).
const BYTES_PER_TILE: usize = 128;

/// Expected raw file size: 512 tiles x 128 bytes.
const RAW_FILE_SIZE: usize = TILE_COUNT * BYTES_PER_TILE;

/// RGBA bytes per tile (16x16 pixels x 4 bytes/pixel).
const RGBA_PER_TILE: usize = TILE_SIZE * TILE_SIZE * 4;

/// A decoded tile atlas holding all 512 Ultima V tiles as RGBA pixel data.
pub struct TileAtlas {
    /// Flat RGBA data for all tiles. Tile N starts at `N * RGBA_PER_TILE`.
    data: Vec<u8>,
}

impl TileAtlas {
    /// Load and decode the tile atlas from the game directory.
    ///
    /// Looks for `tiles.16` (case-insensitive). The file may be either:
    /// - Raw: exactly 65,536 bytes (512 tiles x 128 bytes each)
    /// - LZW compressed: 4-byte LE uncompressed length + compressed data
    pub fn load(game_dir: &Path) -> Result<Self> {
        let tiles_path = find_tiles_file(game_dir)?;
        let file_data = std::fs::read(&tiles_path)
            .with_context(|| format!("failed to read {}", tiles_path.display()))?;

        let tile_data = if file_data.len() == RAW_FILE_SIZE {
            file_data
        } else {
            // Try LZW decompression
            lzw::decompress(&file_data)
                .with_context(|| format!("LZW decompression of {} failed", tiles_path.display()))?
        };

        anyhow::ensure!(
            tile_data.len() == RAW_FILE_SIZE,
            "decompressed tiles.16 is {} bytes (expected {RAW_FILE_SIZE})",
            tile_data.len()
        );

        let data = decode_raw_tiles(&tile_data)?;
        Ok(Self { data })
    }

    /// Get the RGBA pixel data for a tile by ID (0-511).
    /// Returns a 16x16x4 = 1024 byte slice.
    pub fn tile_rgba(&self, tile_id: u16) -> &[u8] {
        let idx = (tile_id as usize) % TILE_COUNT;
        let start = idx * RGBA_PER_TILE;
        &self.data[start..start + RGBA_PER_TILE]
    }
}

/// Decode raw EGA 4bpp tile data into RGBA.
fn decode_raw_tiles(data: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        data.len() == RAW_FILE_SIZE,
        "expected {RAW_FILE_SIZE} bytes, got {}",
        data.len()
    );

    let mut rgba = vec![0u8; TILE_COUNT * RGBA_PER_TILE];

    for tile in 0..TILE_COUNT {
        let src_offset = tile * BYTES_PER_TILE;
        let dst_offset = tile * RGBA_PER_TILE;

        for row in 0..TILE_SIZE {
            let row_src = src_offset + row * 8; // 8 bytes per row
            let row_dst = dst_offset + row * TILE_SIZE * 4; // 16 pixels x 4 bytes

            for col_byte in 0..8 {
                let byte = data[row_src + col_byte];
                let (hi, lo) = ega::decode_ega_byte(byte);

                let px = row_dst + col_byte * 2 * 4;
                rgba[px..px + 4].copy_from_slice(&hi);
                rgba[px + 4..px + 8].copy_from_slice(&lo);
            }
        }
    }

    Ok(rgba)
}

/// Find the tiles.16 file in the game directory (case-insensitive).
fn find_tiles_file(game_dir: &Path) -> Result<PathBuf> {
    let entries = std::fs::read_dir(game_dir)
        .with_context(|| format!("failed to read directory: {}", game_dir.display()))?;

    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("tiles.16")
        {
            return Ok(entry.path());
        }
    }

    anyhow::bail!("tiles.16 not found in {}", game_dir.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::ega::EGA_PALETTE;

    /// Build a minimal synthetic tile (128 bytes) with a known pattern.
    fn make_test_tile(color_index: u8) -> [u8; BYTES_PER_TILE] {
        // Fill entire tile with one color: both nibbles set to the same index
        let byte = (color_index << 4) | color_index;
        [byte; BYTES_PER_TILE]
    }

    #[test]
    fn decode_single_color_tile() {
        // Build a full 512-tile raw buffer, all filled with color index 2 (green)
        let tile = make_test_tile(2);
        let mut raw = vec![0u8; RAW_FILE_SIZE];
        for t in 0..TILE_COUNT {
            raw[t * BYTES_PER_TILE..(t + 1) * BYTES_PER_TILE].copy_from_slice(&tile);
        }

        let rgba = decode_raw_tiles(&raw).unwrap();

        // Check first pixel of tile 0
        let expected = EGA_PALETTE[2]; // green
        assert_eq!(&rgba[0..4], &expected);

        // Check last pixel of tile 0
        let last_px = RGBA_PER_TILE - 4;
        assert_eq!(&rgba[last_px..last_px + 4], &expected);
    }

    #[test]
    fn decode_alternating_colors() {
        // Tile with alternating black (0) and white (F): byte = 0x0F
        let mut raw = vec![0u8; RAW_FILE_SIZE];
        // Fill tile 0 with 0x0F pattern
        for i in 0..BYTES_PER_TILE {
            raw[i] = 0x0F;
        }

        let rgba = decode_raw_tiles(&raw).unwrap();

        // First pixel should be black (index 0)
        assert_eq!(&rgba[0..4], &EGA_PALETTE[0]);
        // Second pixel should be white (index 15)
        assert_eq!(&rgba[4..8], &EGA_PALETTE[15]);
    }

    #[test]
    fn tile_rgba_returns_correct_slice() {
        let tile = make_test_tile(5); // magenta
        let mut raw = vec![0u8; RAW_FILE_SIZE];
        // Put magenta in tile 100
        raw[100 * BYTES_PER_TILE..101 * BYTES_PER_TILE].copy_from_slice(&tile);

        let rgba = decode_raw_tiles(&raw).unwrap();
        let atlas = TileAtlas { data: rgba };

        let slice = atlas.tile_rgba(100);
        assert_eq!(slice.len(), RGBA_PER_TILE);
        assert_eq!(&slice[0..4], &EGA_PALETTE[5]);
    }

    #[test]
    fn reject_wrong_size() {
        let raw = vec![0u8; 1000]; // way too small
        assert!(decode_raw_tiles(&raw).is_err());
    }
}
