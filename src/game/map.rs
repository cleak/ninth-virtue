use anyhow::Result;

use crate::game::offsets::{
    MAP_LOCATION, MAP_SCROLL_X, MAP_SCROLL_Y, MAP_TILES, MAP_TILES_LEN, MAP_TRANSPORT, MAP_X,
    MAP_Y, MAP_Z, OBJ_TILE1, OBJ_X, OBJ_Y, OBJECT_ENTRY_SIZE, OBJECT_TABLE, OBJECT_TABLE_SLOTS,
    inv_addr,
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
}

impl LocationType {
    fn from_id(id: u8) -> Self {
        match id {
            0 => Self::Overworld,
            1..=8 => Self::Town(id),
            9..=16 => Self::Dwelling(id),
            17..=24 => Self::Castle(id),
            25..=32 => Self::Keep(id),
            33..=40 => Self::Dungeon(id),
            _ => Self::Overworld,
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
    /// Upper-left chunk coordinates of the loaded 2x2 chunk area.
    pub scroll_x: u8,
    pub scroll_y: u8,
    /// 32x32 tile grid stored as 4 chunks of 16x16.
    pub tiles: [u8; MAP_TILES_LEN],
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
}

/// Read the current map state from DOSBox memory.
pub fn read_map_state(mem: &dyn MemoryAccess, dos_base: usize) -> Result<MapState> {
    let location_id = mem.read_u8(inv_addr(dos_base, MAP_LOCATION))?;
    let z = mem.read_u8(inv_addr(dos_base, MAP_Z))?;
    let x = mem.read_u8(inv_addr(dos_base, MAP_X))?;
    let y = mem.read_u8(inv_addr(dos_base, MAP_Y))?;
    let transport = mem.read_u8(inv_addr(dos_base, MAP_TRANSPORT))?;
    let scroll_x = mem.read_u8(inv_addr(dos_base, MAP_SCROLL_X))?;
    let scroll_y = mem.read_u8(inv_addr(dos_base, MAP_SCROLL_Y))?;

    let mut tiles = [0u8; MAP_TILES_LEN];
    mem.read_bytes(inv_addr(dos_base, MAP_TILES), &mut tiles)?;

    let objects = read_objects(mem, dos_base).unwrap_or_default();

    Ok(MapState {
        location: LocationType::from_id(location_id),
        z,
        x,
        y,
        transport,
        scroll_x,
        scroll_y,
        tiles,
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
        assert_eq!(LocationType::from_id(255), LocationType::Overworld);
    }

    #[test]
    fn location_names() {
        assert_eq!(LocationType::Town(2).name(), "Britain");
        assert_eq!(LocationType::Dungeon(33).name(), "Deceit");
        assert_eq!(LocationType::Overworld.name(), "Overworld");
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
        assert!(state.objects.is_empty()); // no objects written
    }
}
