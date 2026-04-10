use std::path::Path;

use anyhow::{Context, Result};

use crate::game::map::LocationType;
use crate::game::quest::Virtue;

/// Offset in DATA.OVL where the 256-byte chunk flag table starts.
/// Each byte is either 0xFF (water-only chunk) or the chunk's index into BRIT.DAT.
const DATA_OVL_CHUNK_FLAGS: usize = 0x3886;
const DATA_OVL_LOCATION_X: usize = 0x1E9A;
const DATA_OVL_LOCATION_Y: usize = 0x1EC2;
const DATA_OVL_LOCATION_COUNT: usize = 0x28;
const DATA_OVL_SHRINE_X: usize = 0x1F7E;
const DATA_OVL_SHRINE_Y: usize = 0x1F86;

/// Tile ID used for water-only chunks.
const WATER_TILE: u8 = 0x01;

/// Filter categories exposed by the overworld minimap label controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldLabelCategory {
    Town,
    Dwelling,
    Castle,
    Keep,
    Dungeon,
    Shrine,
}

/// Semantic type of a parsed overworld label entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldLabelKind {
    Location(LocationType),
    Shrine(Virtue),
}

/// A named overworld entrance read from DATA.OVL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldLocation {
    /// The source kind used to derive the label name and category.
    pub kind: WorldLabelKind,
    /// Britannia overworld X coordinate.
    pub x: u8,
    /// Britannia overworld Y coordinate.
    pub y: u8,
}

impl WorldLocation {
    /// Display name shown in the minimap overlay.
    pub fn name(self) -> &'static str {
        match self.kind {
            WorldLabelKind::Location(location) => location.name(),
            WorldLabelKind::Shrine(virtue) => virtue.name(),
        }
    }

    /// Filter bucket used by the minimap label controls.
    pub fn category(self) -> WorldLabelCategory {
        match self.kind {
            WorldLabelKind::Location(location) => match location {
                LocationType::Town(_) => WorldLabelCategory::Town,
                LocationType::Dwelling(_) => WorldLabelCategory::Dwelling,
                LocationType::Castle(_) => WorldLabelCategory::Castle,
                LocationType::Keep(_) => WorldLabelCategory::Keep,
                LocationType::Dungeon(_) => WorldLabelCategory::Dungeon,
                LocationType::Overworld => WorldLabelCategory::Town,
            },
            WorldLabelKind::Shrine(_) => WorldLabelCategory::Shrine,
        }
    }
}

/// The full 256x256 overworld tile grid, loaded from BRIT.DAT.
pub struct WorldMap {
    tiles: Box<[u8; 256 * 256]>,
    locations: Vec<WorldLocation>,
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
        let locations = parse_locations(&ovl)?;
        let shrines = parse_shrines(&ovl)?;

        let brit_path = find_file(game_dir, "brit.dat")?;
        let brit = std::fs::read(&brit_path)
            .with_context(|| format!("failed to read {}", brit_path.display()))?;

        let mut map = Self::from_raw(chunk_flags, &brit)?;
        map.locations = locations;
        map.locations.extend(shrines);
        Ok(map)
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

        Ok(Self {
            tiles,
            locations: Vec::new(),
        })
    }

    /// Get the tile ID at world coordinates (x, y). Always valid for u8 inputs.
    pub fn get_tile(&self, x: u8, y: u8) -> u8 {
        self.tiles[y as usize * 256 + x as usize]
    }

    /// Return all parsed overworld label points from DATA.OVL.
    pub fn locations(&self) -> &[WorldLocation] {
        &self.locations
    }
}

fn parse_locations(ovl: &[u8]) -> Result<Vec<WorldLocation>> {
    anyhow::ensure!(
        ovl.len() >= DATA_OVL_LOCATION_Y + DATA_OVL_LOCATION_COUNT,
        "DATA.OVL too small for location coordinates ({} bytes)",
        ovl.len()
    );

    let xs = &ovl[DATA_OVL_LOCATION_X..DATA_OVL_LOCATION_X + DATA_OVL_LOCATION_COUNT];
    let ys = &ovl[DATA_OVL_LOCATION_Y..DATA_OVL_LOCATION_Y + DATA_OVL_LOCATION_COUNT];

    let mut locations = Vec::with_capacity(DATA_OVL_LOCATION_COUNT);
    for idx in 0..DATA_OVL_LOCATION_COUNT {
        let location_id = idx as u8 + 1;
        let location = LocationType::named_location(location_id)
            .with_context(|| format!("missing location id {location_id} for overworld labels"))?;
        locations.push(WorldLocation {
            kind: WorldLabelKind::Location(location),
            x: xs[idx],
            y: ys[idx],
        });
    }

    Ok(locations)
}

fn parse_shrines(ovl: &[u8]) -> Result<Vec<WorldLocation>> {
    let count = Virtue::ALL.len();
    anyhow::ensure!(
        ovl.len() >= DATA_OVL_SHRINE_Y + count,
        "DATA.OVL too small for shrine coordinates ({} bytes)",
        ovl.len()
    );

    let xs = &ovl[DATA_OVL_SHRINE_X..DATA_OVL_SHRINE_X + count];
    let ys = &ovl[DATA_OVL_SHRINE_Y..DATA_OVL_SHRINE_Y + count];

    let mut shrines = Vec::with_capacity(count);
    for (idx, virtue) in Virtue::ALL.into_iter().enumerate() {
        let x = xs[idx];
        let y = ys[idx];

        // DATA.OVL encodes the non-overworld Shrine of Spirituality as (0, 0).
        if virtue == Virtue::Spirituality && x == 0 && y == 0 {
            continue;
        }

        shrines.push(WorldLocation {
            kind: WorldLabelKind::Shrine(virtue),
            x,
            y,
        });
    }

    Ok(shrines)
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

    #[test]
    fn parse_location_coordinates() {
        let mut ovl = vec![0u8; DATA_OVL_LOCATION_Y + DATA_OVL_LOCATION_COUNT];
        ovl[DATA_OVL_LOCATION_X] = 81;
        ovl[DATA_OVL_LOCATION_Y] = 106;
        ovl[DATA_OVL_LOCATION_X + DATA_OVL_LOCATION_COUNT - 1] = 128;
        ovl[DATA_OVL_LOCATION_Y + DATA_OVL_LOCATION_COUNT - 1] = 128;

        let locations = parse_locations(&ovl).unwrap();
        assert_eq!(locations.len(), DATA_OVL_LOCATION_COUNT);
        assert_eq!(
            locations[0],
            WorldLocation {
                kind: WorldLabelKind::Location(LocationType::Town(1)),
                x: 81,
                y: 106,
            }
        );
        assert_eq!(
            locations[DATA_OVL_LOCATION_COUNT - 1],
            WorldLocation {
                kind: WorldLabelKind::Location(LocationType::Dungeon(40)),
                x: 128,
                y: 128,
            }
        );
    }

    #[test]
    fn parse_shrine_coordinates_only_skips_spirituality_sentinel() {
        let mut ovl = vec![0u8; DATA_OVL_SHRINE_Y + Virtue::ALL.len()];
        for idx in 0..Virtue::ALL.len() {
            ovl[DATA_OVL_SHRINE_X + idx] = idx as u8 + 10;
            ovl[DATA_OVL_SHRINE_Y + idx] = idx as u8 + 20;
        }

        ovl[DATA_OVL_SHRINE_X] = 233;
        ovl[DATA_OVL_SHRINE_Y] = 66;
        ovl[DATA_OVL_SHRINE_X + 1] = 128;
        ovl[DATA_OVL_SHRINE_Y + 1] = 92;
        ovl[DATA_OVL_SHRINE_X + 2] = 0;
        ovl[DATA_OVL_SHRINE_Y + 2] = 0;
        ovl[DATA_OVL_SHRINE_X + 6] = 0;
        ovl[DATA_OVL_SHRINE_Y + 6] = 0;
        ovl[DATA_OVL_SHRINE_X + 7] = 231;
        ovl[DATA_OVL_SHRINE_Y + 7] = 216;

        let shrines = parse_shrines(&ovl).unwrap();
        assert_eq!(shrines.len(), 7);
        assert_eq!(
            shrines[0],
            WorldLocation {
                kind: WorldLabelKind::Shrine(Virtue::Honesty),
                x: 233,
                y: 66,
            }
        );
        assert_eq!(
            shrines[2],
            WorldLocation {
                kind: WorldLabelKind::Shrine(Virtue::Valor),
                x: 0,
                y: 0,
            }
        );
        assert_eq!(
            shrines[6],
            WorldLocation {
                kind: WorldLabelKind::Shrine(Virtue::Humility),
                x: 231,
                y: 216,
            }
        );
    }
}
