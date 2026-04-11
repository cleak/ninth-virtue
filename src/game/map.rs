use anyhow::Result;

use crate::game::offsets::{
    COMBAT_TERRAIN_GRID, COMBAT_TERRAIN_LEN, MAP_LOCATION, MAP_SCROLL_X, MAP_SCROLL_Y, MAP_TILES,
    MAP_TILES_LEN, MAP_TRANSPORT, MAP_X, MAP_Y, MAP_Z, OBJ_FLOOR, OBJ_TILE1, OBJ_X, OBJ_Y,
    OBJECT_ENTRY_SIZE, OBJECT_TABLE, OBJECT_TABLE_SLOTS, ds_addr, inv_addr,
};
use crate::memory::access::MemoryAccess;

/// Which type of location the party is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationType {
    Overworld,
    Town(u8),
    Dwelling(u8),
    Castle(u8),
    Keep(u8),
    Dungeon(u8),
    Combat(u8),
}

/// How the live terrain buffer is arranged in DOS memory for a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileGridEncoding {
    /// Plain 32x32 row-major bytes.
    RowMajor32,
    /// Four 16x16 chunks packed sequentially.
    Chunked16x16,
    /// Combat-only 11x11 active grid with a 32-byte row stride.
    Combat11x11Stride32,
}

impl LocationType {
    fn from_id(id: u8) -> Self {
        match id {
            0 => Self::Overworld,
            1..=8 => Self::Town(id),
            9..=16 => Self::Dwelling(id),
            17..=24 => Self::Castle(id),
            25..=32 => Self::Keep(id),
            33..=0x7F => Self::Dungeon(id),
            0x80..=u8::MAX => Self::Combat(id),
        }
    }

    /// Return a named non-overworld location by its DATA.OVL table index.
    pub(crate) fn named_location(id: u8) -> Option<Self> {
        match id {
            1..=40 => Some(Self::from_id(id)),
            _ => None,
        }
    }

    /// Human-readable name for the location.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Overworld => "Overworld",
            Self::Town(id) => match id {
                1 => "Moonglow",
                2 => "Britain",
                3 => "Jhelom",
                4 => "Yew",
                5 => "Minoc",
                6 => "Trinsic",
                7 => "Skara Brae",
                8 => "New Magincia",
                _ => "Town",
            },
            Self::Dwelling(id) => match id {
                9 => "Fogsbane",
                10 => "Stormcrow",
                11 => "Greyhaven",
                12 => "Waveguide",
                13 => "Iolo's Hut",
                14 => "Sutek's Hut",
                15 => "Sin Vraal's Hut",
                16 => "Grendel's Hut",
                _ => "Dwelling",
            },
            Self::Castle(id) => match id {
                17 => "Castle British",
                18 => "Blackthorn's",
                19 => "West Britanny",
                20 => "North Britanny",
                21 => "East Britanny",
                22 => "Paws",
                23 => "Cove",
                24 => "Buccaneer's Den",
                _ => "Castle",
            },
            Self::Keep(id) => match id {
                25 => "Ararat",
                26 => "Bordermarch",
                27 => "Farthing",
                28 => "Windemere",
                29 => "Stonegate",
                30 => "Lycaeum",
                31 => "Empath Abbey",
                32 => "Serpent's Hold",
                _ => "Keep",
            },
            Self::Dungeon(id) => match id {
                33 => "Deceit",
                34 => "Despise",
                35 => "Destard",
                36 => "Wrong",
                37 => "Covetous",
                38 => "Shame",
                39 => "Hythloth",
                40 => "Doom",
                _ => "Dungeon",
            },
            Self::Combat(_) => "Combat",
        }
    }

    /// Whether the current scene uses Britannia overworld coordinates.
    pub fn is_overworld(self) -> bool {
        matches!(self, Self::Overworld)
    }

    /// Return the live tile-window encoding used by this scene type.
    pub fn tile_grid_encoding(self) -> TileGridEncoding {
        match self {
            Self::Overworld => TileGridEncoding::Chunked16x16,
            Self::Town(_) | Self::Dwelling(_) | Self::Castle(_) | Self::Keep(_) => {
                TileGridEncoding::RowMajor32
            }
            Self::Combat(_) => TileGridEncoding::Combat11x11Stride32,
            // Dungeons likely need their own dedicated buffer (0x3B4 / DS:0x595A),
            // but keep the current path unchanged until that is reverse engineered.
            Self::Dungeon(_) => TileGridEncoding::RowMajor32,
        }
    }
}

/// Snapshot of the current map state read from game memory.
#[derive(Debug, Clone)]
pub struct MapState {
    pub location: LocationType,
    pub z: u8,
    pub x: u8,
    pub y: u8,
    #[allow(dead_code)] // future: player sprite rendering
    pub transport: u8,
    /// Upper-left coordinates of the loaded 32x32 map window.
    ///
    /// Combat can leave these bytes holding the prior overworld chunk origin,
    /// so treat them as meaningful only for non-combat local scenes.
    pub scroll_x: u8,
    pub scroll_y: u8,
    /// Raw 32x32 tile buffer read from game memory.
    pub tiles: [u8; MAP_TILES_LEN],
    /// Raw combat terrain scratch buffer, when the current scene is combat.
    pub combat_tiles: Option<[u8; COMBAT_TERRAIN_LEN]>,
    /// Active objects on the map (NPCs, monsters, vehicles).
    /// Each entry has a tile byte (add 0x100 for the full tile index) and position.
    pub objects: Vec<ObjectEntry>,
}

/// An object from the 32-slot object table (save offset 0x6B4).
///
/// Represents anything rendered on the map that isn't terrain:
/// the party avatar (slot 0), vehicles, NPCs, and monsters.
#[derive(Debug, Clone)]
pub struct ObjectEntry {
    /// Tile byte from field +0 (add 0x100 for the full tile atlas index).
    pub tile: u8,
    pub x: u8,
    pub y: u8,
    pub floor: u8,
}

/// Read the current map state from DOSBox memory.
pub fn read_map_state(mem: &dyn MemoryAccess, dos_base: usize) -> Result<MapState> {
    let location_id = mem.read_u8(inv_addr(dos_base, MAP_LOCATION))?;
    let location = LocationType::from_id(location_id);
    let z = mem.read_u8(inv_addr(dos_base, MAP_Z))?;
    let x = mem.read_u8(inv_addr(dos_base, MAP_X))?;
    let y = mem.read_u8(inv_addr(dos_base, MAP_Y))?;
    let transport = mem.read_u8(inv_addr(dos_base, MAP_TRANSPORT))?;
    let scroll_x = mem.read_u8(inv_addr(dos_base, MAP_SCROLL_X))?;
    let scroll_y = mem.read_u8(inv_addr(dos_base, MAP_SCROLL_Y))?;

    let mut tiles = [0u8; MAP_TILES_LEN];
    mem.read_bytes(inv_addr(dos_base, MAP_TILES), &mut tiles)?;

    let combat_tiles = if matches!(location, LocationType::Combat(_)) {
        let mut tiles = [0u8; COMBAT_TERRAIN_LEN];
        mem.read_bytes(ds_addr(dos_base, COMBAT_TERRAIN_GRID), &mut tiles)?;
        Some(tiles)
    } else {
        None
    };

    let objects = read_objects(mem, dos_base).unwrap_or_default();

    Ok(MapState {
        location,
        z,
        x,
        y,
        transport,
        scroll_x,
        scroll_y,
        tiles,
        combat_tiles,
        objects,
    })
}

/// Read active objects from the 32-slot object table as a single snapshot.
fn read_objects(mem: &dyn MemoryAccess, dos_base: usize) -> Result<Vec<ObjectEntry>> {
    let mut raw = [0u8; OBJECT_TABLE_SLOTS * OBJECT_ENTRY_SIZE];
    mem.read_bytes(inv_addr(dos_base, OBJECT_TABLE), &mut raw)?;

    let mut objects = Vec::new();
    for rec in raw.chunks_exact(OBJECT_ENTRY_SIZE) {
        let tile = rec[OBJ_TILE1];
        // 0x00 = empty slot, 0x1D/0x1E = dead/gone sentinel markers
        if matches!(tile, 0 | 0x1D | 0x1E) {
            continue;
        }
        objects.push(ObjectEntry {
            tile,
            x: rec[OBJ_X],
            y: rec[OBJ_Y],
            floor: rec[OBJ_FLOOR],
        });
    }
    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::offsets::SAVE_BASE;
    use crate::memory::access::MockMemory;

    #[test]
    fn parse_location_types() {
        assert_eq!(LocationType::from_id(0), LocationType::Overworld);
        assert_eq!(LocationType::from_id(2), LocationType::Town(2));
        assert_eq!(LocationType::from_id(9), LocationType::Dwelling(9));
        assert_eq!(LocationType::from_id(17), LocationType::Castle(17));
        assert_eq!(LocationType::from_id(25), LocationType::Keep(25));
        assert_eq!(LocationType::from_id(33), LocationType::Dungeon(33));
        assert_eq!(LocationType::from_id(0x7F), LocationType::Dungeon(0x7F));
        assert_eq!(LocationType::from_id(0x80), LocationType::Combat(0x80));
        assert_eq!(LocationType::from_id(255), LocationType::Combat(255));
    }

    #[test]
    fn location_names() {
        assert_eq!(LocationType::Town(2).name(), "Britain");
        assert_eq!(LocationType::Dungeon(33).name(), "Deceit");
        assert_eq!(LocationType::Combat(0x80).name(), "Combat");
        assert_eq!(LocationType::Overworld.name(), "Overworld");
    }

    #[test]
    fn tile_grid_encodings_match_scene_types() {
        assert_eq!(
            LocationType::Overworld.tile_grid_encoding(),
            TileGridEncoding::Chunked16x16
        );
        assert_eq!(
            LocationType::Combat(0x80).tile_grid_encoding(),
            TileGridEncoding::Combat11x11Stride32
        );
        assert_eq!(
            LocationType::Town(2).tile_grid_encoding(),
            TileGridEncoding::RowMajor32
        );
    }

    #[test]
    fn read_map_state_from_mock() {
        let mock = MockMemory::new(0x30000);

        // Write position data
        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 2).unwrap(); // Britain
        mock.write_u8(base + MAP_Z, 0xFF).unwrap(); // surface
        mock.write_u8(base + MAP_X, 100).unwrap();
        mock.write_u8(base + MAP_Y, 50).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap(); // on foot

        // Write some tile data
        let tile_addr = base + MAP_TILES;
        for i in 0..MAP_TILES_LEN {
            mock.write_u8(tile_addr + i, 0x05).unwrap(); // all grass
        }
        // Place water at top-left
        mock.write_u8(tile_addr, 0x01).unwrap();

        let state = read_map_state(&mock, 0).unwrap();
        assert_eq!(state.location, LocationType::Town(2));
        assert_eq!(state.z, 0xFF);
        assert_eq!(state.x, 100);
        assert_eq!(state.y, 50);
        assert_eq!(state.tiles[0], 0x01); // water
        assert_eq!(state.tiles[1], 0x05); // grass
        assert!(state.combat_tiles.is_none());
        assert!(state.objects.is_empty()); // no objects written
    }

    #[test]
    fn read_combat_state_reads_dedicated_combat_grid() {
        let mock = MockMemory::new(0x40000);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 0xFF).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 6).unwrap();
        mock.write_u8(base + MAP_Y, 8).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();

        for i in 0..MAP_TILES_LEN {
            mock.write_u8(base + MAP_TILES + i, 0x07).unwrap();
        }
        for i in 0..COMBAT_TERRAIN_LEN {
            mock.write_u8(ds_addr(0, COMBAT_TERRAIN_GRID) + i, (i % 251) as u8)
                .unwrap();
        }

        let state = read_map_state(&mock, 0).unwrap();
        let combat_tiles = state
            .combat_tiles
            .expect("combat scenes should read the scratch grid");
        assert_eq!(state.location, LocationType::Combat(0xFF));
        assert_eq!(combat_tiles[0], 0);
        assert_eq!(combat_tiles[1], 1);
    }
}
