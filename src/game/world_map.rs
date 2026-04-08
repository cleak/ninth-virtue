use std::path::Path;

use anyhow::{Context, Result};

/// Offset in DATA.OVL where the 256-byte chunk flag table starts.
/// Each byte is either 0xFF (water-only chunk) or the chunk's index into BRIT.DAT.
const DATA_OVL_CHUNK_FLAGS: usize = 0x3886;

/// Tile ID used for water-only chunks.
const WATER_TILE: u8 = 0x01;

/// The full 256x256 overworld tile grid, loaded from BRIT.DAT.
pub struct WorldMap {
    tiles: Box<[u8; 256 * 256]>,
}

impl WorldMap {
    /// Load the overworld from BRIT.DAT and DATA.OVL in the game directory.
    pub fn load(game_dir: &Path) -> Result<Self> {
        let ovl_path = find_file(game_dir, "data.ovl")?;
        let ovl = std::fs::read(&ovl_path)
            .with_context(|| format!("failed to read {}", ovl_path.display()))?;
        anyhow::ensure!(
            ovl.len() >= DATA_OVL_CHUNK_FLAGS + 256,
            "DATA.OVL too small ({} bytes)",
            ovl.len()
        );
        let chunk_flags: &[u8; 256] = ovl[DATA_OVL_CHUNK_FLAGS..DATA_OVL_CHUNK_FLAGS + 256]
            .try_into()
            .unwrap();

        let brit_path = find_file(game_dir, "brit.dat")?;
        let brit = std::fs::read(&brit_path)
            .with_context(|| format!("failed to read {}", brit_path.display()))?;

        Self::from_raw(chunk_flags, &brit)
    }

    /// Build a WorldMap from raw chunk flags and BRIT.DAT data.
    ///
    /// Each of the 256 chunk flag bytes is either 0xFF (water-only chunk,
    /// filled with tile 0x01) or the chunk's sequential index into `brit_data`.
    /// Each chunk is 256 bytes (16x16 tiles, row-major).
    fn from_raw(chunk_flags: &[u8; 256], brit_data: &[u8]) -> Result<Self> {
        let mut tiles = Box::new([0u8; 256 * 256]);

        for (chunk_idx, &flag) in chunk_flags.iter().enumerate() {
            let chunk_x = chunk_idx % 16;
            let chunk_y = chunk_idx / 16;

            if flag == 0xFF {
                for ly in 0..16 {
                    for lx in 0..16 {
                        tiles[(chunk_y * 16 + ly) * 256 + chunk_x * 16 + lx] = WATER_TILE;
                    }
                }
            } else {
                let file_offset = flag as usize * 256;
                anyhow::ensure!(
                    file_offset + 256 <= brit_data.len(),
                    "BRIT.DAT too small: chunk {} wants offset {}, file is {} bytes",
                    chunk_idx,
                    file_offset + 256,
                    brit_data.len()
                );
                let chunk_data = &brit_data[file_offset..file_offset + 256];
                for ly in 0..16 {
                    for lx in 0..16 {
                        tiles[(chunk_y * 16 + ly) * 256 + chunk_x * 16 + lx] =
                            chunk_data[ly * 16 + lx];
                    }
                }
            }
        }

        Ok(Self { tiles })
    }

    /// Get the tile ID at world coordinates (x, y). Always valid for u8 inputs.
    pub fn get_tile(&self, x: u8, y: u8) -> u8 {
        self.tiles[y as usize * 256 + x as usize]
    }
}

/// Case-insensitive file search in a directory.
fn find_file(dir: &Path, name: &str) -> Result<std::path::PathBuf> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            return Ok(entry.path());
        }
    }
    anyhow::bail!("{name} not found in {}", dir.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_water_chunks() {
        let chunk_flags = [0xFFu8; 256];
        let map = WorldMap::from_raw(&chunk_flags, &[]).unwrap();
        for y in 0..=255u8 {
            for x in 0..=255u8 {
                assert_eq!(map.get_tile(x, y), 0x01);
            }
        }
    }

    #[test]
    fn single_non_water_chunk() {
        let mut chunk_flags = [0xFFu8; 256];
        chunk_flags[0] = 0x00;
        let brit_data = vec![0x05u8; 256]; // grass

        let map = WorldMap::from_raw(&chunk_flags, &brit_data).unwrap();
        assert_eq!(map.get_tile(0, 0), 0x05);
        assert_eq!(map.get_tile(15, 15), 0x05);
        assert_eq!(map.get_tile(16, 0), 0x01); // adjacent chunk is water
    }

    #[test]
    fn chunk_ordering() {
        let mut chunk_flags = [0xFFu8; 256];
        chunk_flags[0] = 0x00;
        chunk_flags[1] = 0x01;

        let mut brit_data = vec![0x05u8; 256]; // chunk 0: grass
        brit_data.extend(vec![0x07u8; 256]); // chunk 1: desert

        let map = WorldMap::from_raw(&chunk_flags, &brit_data).unwrap();
        assert_eq!(map.get_tile(0, 0), 0x05); // chunk 0
        assert_eq!(map.get_tile(16, 0), 0x07); // chunk 1
        assert_eq!(map.get_tile(0, 16), 0x01); // chunk 16: water
    }
}
