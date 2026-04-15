use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;

use crate::game::injection::{
    VISIBILITY_SNAPSHOT_BODY_LEN, VISIBILITY_SNAPSHOT_LIGHT_IDX, VISIBILITY_SNAPSHOT_LOCATION_IDX,
    VISIBILITY_SNAPSHOT_READY_MARKER, VISIBILITY_SNAPSHOT_READY_OFFSET,
    VISIBILITY_SNAPSHOT_SCROLL_X_IDX, VISIBILITY_SNAPSHOT_SCROLL_Y_IDX,
    VISIBILITY_SNAPSHOT_TILES_OFFSET, VISIBILITY_SNAPSHOT_TOTAL_LEN, VISIBILITY_SNAPSHOT_X_IDX,
    VISIBILITY_SNAPSHOT_Y_IDX, VISIBILITY_SNAPSHOT_Z_IDX,
};
use crate::game::offsets::{
    COMBAT_TERRAIN_GRID, COMBAT_TERRAIN_LEN, DUNGEON_FLOORS, DUNGEON_LEVEL_LEN,
    DUNGEON_ORIENTATION, DUNGEON_TILES_DS_OFFSET, DUNGEON_TILES_LEN, DUNGEON_TILES_SAVE_OFFSET,
    LIGHT_INTENSITY, MAP_LOCATION, MAP_SCROLL_X, MAP_SCROLL_Y, MAP_TILES, MAP_TILES_LEN,
    MAP_TRANSPORT, MAP_X, MAP_Y, MAP_Z, OBJ_FLOOR, OBJ_TILE1, OBJ_X, OBJ_Y, OBJECT_ENTRY_SIZE,
    OBJECT_TABLE, OBJECT_TABLE_SLOTS, VIEWPORT_VISIBILITY_GRID, VIEWPORT_VISIBILITY_HEIGHT,
    VIEWPORT_VISIBILITY_LEN, VIEWPORT_VISIBILITY_STRIDE, VIEWPORT_VISIBILITY_WIDTH, ds_addr,
    inv_addr,
};
use crate::memory::access::MemoryAccess;

/// Cardinal directions used by first-person dungeon navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardinalDirection {
    North,
    East,
    South,
    West,
}

impl CardinalDirection {
    /// Convert Ultima V's dungeon-facing byte into a cardinal direction.
    pub fn from_dungeon_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::North),
            1 => Some(Self::East),
            2 => Some(Self::South),
            3 => Some(Self::West),
            _ => None,
        }
    }
}

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
    /// Dungeon scenes expose one floor-local 8x8 semantic-cell grid as 64 row-major bytes.
    Dungeon8x8,
    /// Combat-only 11x11 active grid with a 32-byte row stride.
    Combat11x11Stride32,
}

/// Outdoor scenes with Z above this boundary use the Underworld map data.
pub const UNDERWORLD_Z_THRESHOLD: u8 = 0x7F;

/// Whether the shared outdoor scene is currently showing the Underworld.
pub fn is_underworld_z(z: u8) -> bool {
    z > UNDERWORLD_Z_THRESHOLD
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

    /// Whether the current location id refers to Ultima V's shared outdoor
    /// scene code. MAP_Z still distinguishes Britannia from the Underworld.
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
            Self::Dungeon(_) => TileGridEncoding::Dungeon8x8,
            Self::Combat(_) => TileGridEncoding::Combat11x11Stride32,
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
    /// First-person facing in dungeons: 0=north, 1=east, 2=south, 3=west.
    pub dungeon_facing: Option<CardinalDirection>,
    #[allow(dead_code)] // future: player sprite rendering
    pub transport: u8,
    /// Upper-left coordinates of the loaded 32x32 map window.
    ///
    /// Combat and dungeon scenes can leave these bytes holding the prior
    /// overworld chunk origin, so treat them as meaningful only for overworld
    /// and surface-interior local scenes.
    pub scroll_x: u8,
    pub scroll_y: u8,
    /// Primary terrain buffer for the current scene.
    ///
    /// Overworld and settlement scenes use the full 32x32 byte grid. Dungeon
    /// scenes copy only the current 8x8 floor into the first 64 bytes.
    pub tiles: [u8; MAP_TILES_LEN],
    /// Raw combat terrain scratch buffer, when the current scene is combat.
    pub combat_tiles: Option<[u8; COMBAT_TERRAIN_LEN]>,
    /// Current 2D visibility window flattened to an 11x11 active region.
    ///
    /// The reader prefers a synchronized post-render snapshot captured inside
    /// the game loop. When that synchronized path is unavailable, it falls
    /// back to repeated `DS:0xAB02` samples; when the synchronized snapshot is
    /// present but stale, visibility is withheld until a matching frame arrives.
    /// Hidden cells read back as 0xFF. Present only for overworld and 2D local scenes.
    pub visibility_tiles: Option<[u8; VIEWPORT_VISIBILITY_LEN]>,
    /// Active objects on the map (NPCs, monsters, vehicles).
    /// Each entry has a tile byte (add 0x100 for the full tile index) and position.
    pub objects: Vec<ObjectEntry>,
}

impl MapState {
    /// Whether the active scene is one of Ultima V's shared outdoor worlds.
    pub fn is_outdoor(&self) -> bool {
        self.location.is_overworld()
    }

    /// Ultima V reuses MAP_LOCATION=0 for both Britannia and the Underworld.
    /// MAP_Z selects the active outdoor world, with high values meaning the
    /// Underworld.
    pub fn is_underworld(&self) -> bool {
        self.is_outdoor() && is_underworld_z(self.z)
    }

    /// Human-readable scene name for the current runtime state.
    pub fn display_location_name(&self) -> &'static str {
        if self.is_underworld() {
            "Underworld"
        } else {
            self.location.name()
        }
    }
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

#[derive(Debug, Clone, Copy)]
struct VisibilityWindowKey {
    location_id: u8,
    z: u8,
    x: u8,
    y: u8,
    scroll_x: u8,
    scroll_y: u8,
}

/// Read the current map state from DOSBox memory.
#[allow(dead_code)] // used by debug tools and tests, while the main app prefers the snapshot-aware path
pub fn read_map_state(mem: &dyn MemoryAccess, dos_base: usize) -> Result<MapState> {
    read_map_state_with_visibility_snapshot(mem, dos_base, None)
}

/// Read the current map state, optionally preferring a stabilized compact
/// visibility snapshot that was captured inside the game's render loop.
pub fn read_map_state_with_visibility_snapshot(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    visibility_snapshot_addr: Option<usize>,
) -> Result<MapState> {
    let location_id = mem.read_u8(inv_addr(dos_base, MAP_LOCATION))?;
    let location = LocationType::from_id(location_id);
    let z = mem.read_u8(inv_addr(dos_base, MAP_Z))?;
    let x = mem.read_u8(inv_addr(dos_base, MAP_X))?;
    let y = mem.read_u8(inv_addr(dos_base, MAP_Y))?;
    let dungeon_facing = matches!(location, LocationType::Dungeon(_))
        .then(|| mem.read_u8(inv_addr(dos_base, DUNGEON_ORIENTATION)).ok())
        .flatten()
        .and_then(CardinalDirection::from_dungeon_byte);
    let transport = mem.read_u8(inv_addr(dos_base, MAP_TRANSPORT))?;
    let scroll_x = mem.read_u8(inv_addr(dos_base, MAP_SCROLL_X))?;
    let scroll_y = mem.read_u8(inv_addr(dos_base, MAP_SCROLL_Y))?;

    let mut tiles = [0u8; MAP_TILES_LEN];
    match location {
        LocationType::Dungeon(_) => {
            let mut dungeon = [0u8; DUNGEON_TILES_LEN];
            debug_assert_eq!(
                ds_addr(dos_base, DUNGEON_TILES_DS_OFFSET),
                inv_addr(dos_base, DUNGEON_TILES_SAVE_OFFSET),
                "dungeon terrain buffer DS/save aliases drifted"
            );
            mem.read_bytes(inv_addr(dos_base, DUNGEON_TILES_SAVE_OFFSET), &mut dungeon)?;
            // Clamp the floor byte read from live game memory before slicing the packed 8x8x8
            // dungeon buffer so unexpected values degrade to the nearest valid floor instead of
            // indexing past the current dungeon data.
            let level = usize::from(z).min(DUNGEON_FLOORS - 1);
            let src = level * DUNGEON_LEVEL_LEN;
            tiles[..DUNGEON_LEVEL_LEN].copy_from_slice(&dungeon[src..src + DUNGEON_LEVEL_LEN]);
        }
        _ => mem.read_bytes(inv_addr(dos_base, MAP_TILES), &mut tiles)?,
    }

    let combat_tiles = if matches!(location, LocationType::Combat(_)) {
        let mut tiles = [0u8; COMBAT_TERRAIN_LEN];
        mem.read_bytes(ds_addr(dos_base, COMBAT_TERRAIN_GRID), &mut tiles)?;
        Some(tiles)
    } else {
        None
    };
    let visibility_key = VisibilityWindowKey {
        location_id,
        z,
        x,
        y,
        scroll_x,
        scroll_y,
    };

    let visibility_tiles = match location {
        LocationType::Overworld
        | LocationType::Town(_)
        | LocationType::Dwelling(_)
        | LocationType::Castle(_)
        | LocationType::Keep(_) => {
            read_visibility_window(mem, dos_base, visibility_snapshot_addr, visibility_key)
                .ok()
                .flatten()
        }
        LocationType::Dungeon(_) | LocationType::Combat(_) => None,
    };

    let objects = read_objects(mem, dos_base).unwrap_or_default();

    Ok(MapState {
        location,
        z,
        x,
        y,
        dungeon_facing,
        transport,
        scroll_x,
        scroll_y,
        tiles,
        combat_tiles,
        visibility_tiles,
        objects,
    })
}

fn read_visibility_window(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    visibility_snapshot_addr: Option<usize>,
    key: VisibilityWindowKey,
) -> Result<Option<[u8; VIEWPORT_VISIBILITY_LEN]>> {
    const VISIBILITY_SAMPLE_COUNT: usize = 5;
    const VISIBILITY_HIDDEN_TILE: u8 = 0xFF;
    const SNAPSHOT_RETRY_COUNT: usize = 24;
    const SNAPSHOT_RETRY_DELAY_MS: u64 = 5;

    if let Some(snapshot_addr) = visibility_snapshot_addr {
        let light = mem.read_u8(inv_addr(dos_base, LIGHT_INTENSITY))?;
        for attempt in 0..SNAPSHOT_RETRY_COUNT {
            if let Some(snapshot) = read_stable_visibility_window(mem, snapshot_addr, key, light)? {
                return Ok(Some(snapshot));
            }

            if attempt + 1 < SNAPSHOT_RETRY_COUNT {
                std::thread::sleep(Duration::from_millis(SNAPSHOT_RETRY_DELAY_MS));
            }
        }

        // When the synchronized hook is installed, prefer a temporarily missing
        // visibility window over trusting the asynchronously mutating scratch
        // buffer. The caller can keep displaying the last stable mask while we
        // wait for the next matching render-pass snapshot.
        return Ok(None);
    }

    let active = read_async_visibility_window(
        mem,
        ds_addr(dos_base, VIEWPORT_VISIBILITY_GRID),
        VISIBILITY_SAMPLE_COUNT,
        VISIBILITY_HIDDEN_TILE,
    )?;
    Ok(Some(active))
}

fn read_async_visibility_window(
    mem: &dyn MemoryAccess,
    visibility_addr: usize,
    sample_count: usize,
    hidden_tile: u8,
) -> Result<[u8; VIEWPORT_VISIBILITY_LEN]> {
    let mut scratch = [0u8; VIEWPORT_VISIBILITY_STRIDE * VIEWPORT_VISIBILITY_HEIGHT];
    let mut samples = Vec::with_capacity(sample_count);

    for sample_idx in 0..sample_count {
        mem.read_bytes(visibility_addr, &mut scratch)?;
        let active = extract_visibility_window(&scratch);
        samples.push(active);

        if sample_idx + 1 < sample_count {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    let repeated_mask = dominant_visibility_mask(&samples, hidden_tile);
    if let Some((_, latest_idx)) = repeated_mask.filter(|(count, _)| *count > 1) {
        return Ok(samples[latest_idx]);
    }

    Ok(select_visibility_window_medoid(&samples, hidden_tile))
}

fn read_stable_visibility_window(
    mem: &dyn MemoryAccess,
    snapshot_addr: usize,
    key: VisibilityWindowKey,
    light: u8,
) -> Result<Option<[u8; VIEWPORT_VISIBILITY_LEN]>> {
    let mut snapshot = [0u8; VISIBILITY_SNAPSHOT_TOTAL_LEN];
    mem.read_bytes(snapshot_addr, &mut snapshot)?;

    if snapshot[VISIBILITY_SNAPSHOT_READY_OFFSET] != VISIBILITY_SNAPSHOT_READY_MARKER {
        return Ok(None);
    }

    if snapshot[VISIBILITY_SNAPSHOT_LOCATION_IDX] != key.location_id
        || snapshot[VISIBILITY_SNAPSHOT_Z_IDX] != key.z
        || snapshot[VISIBILITY_SNAPSHOT_X_IDX] != key.x
        || snapshot[VISIBILITY_SNAPSHOT_Y_IDX] != key.y
        || snapshot[VISIBILITY_SNAPSHOT_SCROLL_X_IDX] != key.scroll_x
        || snapshot[VISIBILITY_SNAPSHOT_SCROLL_Y_IDX] != key.scroll_y
        || snapshot[VISIBILITY_SNAPSHOT_LIGHT_IDX] != light
    {
        return Ok(None);
    }

    let mut active = [0u8; VIEWPORT_VISIBILITY_LEN];
    active
        .copy_from_slice(&snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET..VISIBILITY_SNAPSHOT_BODY_LEN]);
    Ok(Some(active))
}

fn extract_visibility_window(
    scratch: &[u8; VIEWPORT_VISIBILITY_STRIDE * VIEWPORT_VISIBILITY_HEIGHT],
) -> [u8; VIEWPORT_VISIBILITY_LEN] {
    let mut active = [0u8; VIEWPORT_VISIBILITY_LEN];
    for row in 0..VIEWPORT_VISIBILITY_HEIGHT {
        let src = row * VIEWPORT_VISIBILITY_STRIDE;
        let dst = row * VIEWPORT_VISIBILITY_WIDTH;
        active[dst..dst + VIEWPORT_VISIBILITY_WIDTH]
            .copy_from_slice(&scratch[src..src + VIEWPORT_VISIBILITY_WIDTH]);
    }
    active
}

fn dominant_visibility_mask(
    samples: &[[u8; VIEWPORT_VISIBILITY_LEN]],
    hidden_tile: u8,
) -> Option<(usize, usize)> {
    let mut counts = HashMap::<[u8; VIEWPORT_VISIBILITY_LEN], (usize, usize)>::new();
    for (idx, sample) in samples.iter().enumerate() {
        let mask = visibility_mask(sample, hidden_tile);
        let entry = counts.entry(mask).or_insert((0, idx));
        entry.0 += 1;
        entry.1 = idx;
    }

    counts
        .into_values()
        .max_by_key(|&(count, latest_idx)| (count, latest_idx))
}

fn select_visibility_window_medoid(
    samples: &[[u8; VIEWPORT_VISIBILITY_LEN]],
    hidden_tile: u8,
) -> [u8; VIEWPORT_VISIBILITY_LEN] {
    debug_assert!(
        !samples.is_empty(),
        "visibility medoid selection requires at least one sample"
    );

    let mut best_idx = 0usize;
    let mut best_score = (usize::MAX, usize::MAX, usize::MAX);

    for (idx, sample) in samples.iter().enumerate() {
        let total_distance = samples
            .iter()
            .map(|other| visibility_mask_distance(sample, other, hidden_tile))
            .sum::<usize>();
        let hidden_cells = sample.iter().filter(|&&tile| tile == hidden_tile).count();
        let score = (total_distance, hidden_cells, samples.len() - 1 - idx);
        if score < best_score {
            best_score = score;
            best_idx = idx;
        }
    }

    samples[best_idx]
}

fn visibility_mask(
    sample: &[u8; VIEWPORT_VISIBILITY_LEN],
    hidden_tile: u8,
) -> [u8; VIEWPORT_VISIBILITY_LEN] {
    let mut mask = [0u8; VIEWPORT_VISIBILITY_LEN];
    for (idx, &tile) in sample.iter().enumerate() {
        mask[idx] = u8::from(tile == hidden_tile);
    }
    mask
}

fn visibility_mask_distance(
    left: &[u8; VIEWPORT_VISIBILITY_LEN],
    right: &[u8; VIEWPORT_VISIBILITY_LEN],
    hidden_tile: u8,
) -> usize {
    left.iter()
        .zip(right.iter())
        .filter(|(lhs, rhs)| (**lhs == hidden_tile) != (**rhs == hidden_tile))
        .count()
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
    use crate::memory::access::{MemoryAccess, MockMemory};
    use std::cell::Cell;

    struct SnapshotRetryMemory {
        base: MockMemory,
        snapshot_addr: usize,
        switch_after_reads: usize,
        snapshot_reads: Cell<usize>,
        stale_snapshot: [u8; VISIBILITY_SNAPSHOT_TOTAL_LEN],
        fresh_snapshot: [u8; VISIBILITY_SNAPSHOT_TOTAL_LEN],
    }

    impl SnapshotRetryMemory {
        fn new(
            size: usize,
            snapshot_addr: usize,
            switch_after_reads: usize,
            stale_snapshot: [u8; VISIBILITY_SNAPSHOT_TOTAL_LEN],
            fresh_snapshot: [u8; VISIBILITY_SNAPSHOT_TOTAL_LEN],
        ) -> Self {
            Self {
                base: MockMemory::new(size),
                snapshot_addr,
                switch_after_reads,
                snapshot_reads: Cell::new(0),
                stale_snapshot,
                fresh_snapshot,
            }
        }
    }

    impl MemoryAccess for SnapshotRetryMemory {
        fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
            if addr == self.snapshot_addr && buf.len() == VISIBILITY_SNAPSHOT_TOTAL_LEN {
                let reads = self.snapshot_reads.get();
                self.snapshot_reads.set(reads + 1);
                let snapshot = if reads < self.switch_after_reads {
                    &self.stale_snapshot
                } else {
                    &self.fresh_snapshot
                };
                buf.copy_from_slice(snapshot);
                return Ok(());
            }

            self.base.read_bytes(addr, buf)
        }

        fn write_bytes(&self, addr: usize, data: &[u8]) -> Result<()> {
            self.base.write_bytes(addr, data)
        }
    }

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
            LocationType::Dungeon(34).tile_grid_encoding(),
            TileGridEncoding::Dungeon8x8
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
        for row in 0..VIEWPORT_VISIBILITY_HEIGHT {
            for col in 0..VIEWPORT_VISIBILITY_STRIDE {
                let value = if col < VIEWPORT_VISIBILITY_WIDTH {
                    ((row as u8) << 4) | col as u8
                } else {
                    0xEE
                };
                mock.write_u8(
                    ds_addr(0, VIEWPORT_VISIBILITY_GRID) + row * VIEWPORT_VISIBILITY_STRIDE + col,
                    value,
                )
                .unwrap();
            }
        }

        let state = read_map_state(&mock, 0).unwrap();
        assert_eq!(state.location, LocationType::Town(2));
        assert_eq!(state.z, 0xFF);
        assert_eq!(state.x, 100);
        assert_eq!(state.y, 50);
        assert_eq!(state.dungeon_facing, None);
        assert_eq!(state.tiles[0], 0x01); // water
        assert_eq!(state.tiles[1], 0x05); // grass
        assert!(state.combat_tiles.is_none());
        let visibility = state
            .visibility_tiles
            .expect("2D scenes should include the projected 11x11 visibility window");
        assert_eq!(visibility[0], 0x00);
        assert_eq!(visibility[VIEWPORT_VISIBILITY_WIDTH - 1], 0x0A);
        let middle_row = VIEWPORT_VISIBILITY_WIDTH * 5;
        assert_eq!(visibility[middle_row], 0x50);
        assert_eq!(
            visibility[VIEWPORT_VISIBILITY_LEN - 1],
            0xA0 | (VIEWPORT_VISIBILITY_WIDTH as u8 - 1)
        );
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
        assert!(state.visibility_tiles.is_none());
        assert_eq!(combat_tiles[0], 0);
        assert_eq!(combat_tiles[1], 1);
    }

    #[test]
    fn read_map_state_treats_visibility_window_as_best_effort() {
        let visibility_addr = ds_addr(0, VIEWPORT_VISIBILITY_GRID);
        let mock = MockMemory::new(visibility_addr + VIEWPORT_VISIBILITY_LEN - 1);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 2).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 10).unwrap();
        mock.write_u8(base + MAP_Y, 12).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();
        mock.write_u8(base + MAP_SCROLL_X, 8).unwrap();
        mock.write_u8(base + MAP_SCROLL_Y, 9).unwrap();

        for i in 0..MAP_TILES_LEN {
            mock.write_u8(base + MAP_TILES + i, 0x05).unwrap();
        }

        let state = read_map_state(&mock, 0)
            .expect("visibility scratch read failures should not abort the map snapshot");
        assert_eq!(state.location, LocationType::Town(2));
        assert_eq!(state.x, 10);
        assert_eq!(state.y, 12);
        assert_eq!(state.tiles[0], 0x05);
        assert!(state.visibility_tiles.is_none());
    }

    #[test]
    fn stable_visibility_snapshot_is_used_when_metadata_matches() {
        let snapshot_addr = 0x2F000;
        let mock = MockMemory::new(snapshot_addr + VISIBILITY_SNAPSHOT_TOTAL_LEN + 0x100);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 2).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 10).unwrap();
        mock.write_u8(base + MAP_Y, 12).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();
        mock.write_u8(base + MAP_SCROLL_X, 8).unwrap();
        mock.write_u8(base + MAP_SCROLL_Y, 9).unwrap();
        mock.write_u8(base + LIGHT_INTENSITY, 0x0A).unwrap();

        for i in 0..MAP_TILES_LEN {
            mock.write_u8(base + MAP_TILES + i, 0x05).unwrap();
        }
        for row in 0..VIEWPORT_VISIBILITY_HEIGHT {
            for col in 0..VIEWPORT_VISIBILITY_STRIDE {
                mock.write_u8(
                    ds_addr(0, VIEWPORT_VISIBILITY_GRID) + row * VIEWPORT_VISIBILITY_STRIDE + col,
                    0xFF,
                )
                .unwrap();
            }
        }

        let mut snapshot = [0u8; VISIBILITY_SNAPSHOT_TOTAL_LEN];
        snapshot[VISIBILITY_SNAPSHOT_LOCATION_IDX] = 2;
        snapshot[VISIBILITY_SNAPSHOT_Z_IDX] = 0;
        snapshot[VISIBILITY_SNAPSHOT_X_IDX] = 10;
        snapshot[VISIBILITY_SNAPSHOT_Y_IDX] = 12;
        snapshot[VISIBILITY_SNAPSHOT_SCROLL_X_IDX] = 8;
        snapshot[VISIBILITY_SNAPSHOT_SCROLL_Y_IDX] = 9;
        snapshot[VISIBILITY_SNAPSHOT_LIGHT_IDX] = 0x0A;
        snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET] = 0x11;
        let center = VISIBILITY_SNAPSHOT_TILES_OFFSET
            + (VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
            + VIEWPORT_VISIBILITY_WIDTH / 2;
        snapshot[center] = 0x22;
        snapshot[VISIBILITY_SNAPSHOT_READY_OFFSET] = VISIBILITY_SNAPSHOT_READY_MARKER;
        mock.set_bytes(snapshot_addr, &snapshot);

        let state = read_map_state_with_visibility_snapshot(&mock, 0, Some(snapshot_addr)).unwrap();
        let visibility = state.visibility_tiles.unwrap();
        assert_eq!(visibility[0], 0x11);
        assert_eq!(
            visibility[(VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
                + VIEWPORT_VISIBILITY_WIDTH / 2],
            0x22
        );
    }

    #[test]
    fn stale_visibility_snapshot_is_ignored_when_metadata_mismatches() {
        let snapshot_addr = 0x2F000;
        let mock = MockMemory::new(snapshot_addr + VISIBILITY_SNAPSHOT_TOTAL_LEN + 0x100);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 2).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 10).unwrap();
        mock.write_u8(base + MAP_Y, 12).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();
        mock.write_u8(base + MAP_SCROLL_X, 8).unwrap();
        mock.write_u8(base + MAP_SCROLL_Y, 9).unwrap();
        mock.write_u8(base + LIGHT_INTENSITY, 0x0A).unwrap();

        for i in 0..MAP_TILES_LEN {
            mock.write_u8(base + MAP_TILES + i, 0x05).unwrap();
        }
        for row in 0..VIEWPORT_VISIBILITY_HEIGHT {
            for col in 0..VIEWPORT_VISIBILITY_STRIDE {
                let value = if col < VIEWPORT_VISIBILITY_WIDTH {
                    ((row as u8) << 4) | col as u8
                } else {
                    0xEE
                };
                mock.write_u8(
                    ds_addr(0, VIEWPORT_VISIBILITY_GRID) + row * VIEWPORT_VISIBILITY_STRIDE + col,
                    value,
                )
                .unwrap();
            }
        }

        let mut snapshot = [0u8; VISIBILITY_SNAPSHOT_TOTAL_LEN];
        snapshot[VISIBILITY_SNAPSHOT_LOCATION_IDX] = 2;
        snapshot[VISIBILITY_SNAPSHOT_Z_IDX] = 0;
        snapshot[VISIBILITY_SNAPSHOT_X_IDX] = 99; // mismatch
        snapshot[VISIBILITY_SNAPSHOT_Y_IDX] = 12;
        snapshot[VISIBILITY_SNAPSHOT_SCROLL_X_IDX] = 8;
        snapshot[VISIBILITY_SNAPSHOT_SCROLL_Y_IDX] = 9;
        snapshot[VISIBILITY_SNAPSHOT_LIGHT_IDX] = 0x0A;
        snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET] = 0x77;
        snapshot[VISIBILITY_SNAPSHOT_READY_OFFSET] = VISIBILITY_SNAPSHOT_READY_MARKER;
        mock.set_bytes(snapshot_addr, &snapshot);

        let state = read_map_state_with_visibility_snapshot(&mock, 0, Some(snapshot_addr)).unwrap();
        assert!(
            state.visibility_tiles.is_none(),
            "stale synchronized snapshots should not fall back to the async scratch buffer"
        );
    }

    #[test]
    fn visibility_reader_waits_for_matching_snapshot_before_accepting_visibility() {
        let snapshot_addr = 0x2F000;
        let mock = SnapshotRetryMemory::new(
            snapshot_addr + VISIBILITY_SNAPSHOT_TOTAL_LEN + 0x100,
            snapshot_addr,
            2,
            {
                let mut snapshot = [0u8; VISIBILITY_SNAPSHOT_TOTAL_LEN];
                snapshot[VISIBILITY_SNAPSHOT_LOCATION_IDX] = 2;
                snapshot[VISIBILITY_SNAPSHOT_Z_IDX] = 0;
                snapshot[VISIBILITY_SNAPSHOT_X_IDX] = 99;
                snapshot[VISIBILITY_SNAPSHOT_Y_IDX] = 12;
                snapshot[VISIBILITY_SNAPSHOT_SCROLL_X_IDX] = 8;
                snapshot[VISIBILITY_SNAPSHOT_SCROLL_Y_IDX] = 9;
                snapshot[VISIBILITY_SNAPSHOT_LIGHT_IDX] = 0x0A;
                snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET] = 0x77;
                snapshot[VISIBILITY_SNAPSHOT_READY_OFFSET] = VISIBILITY_SNAPSHOT_READY_MARKER;
                snapshot
            },
            {
                let mut snapshot = [0u8; VISIBILITY_SNAPSHOT_TOTAL_LEN];
                snapshot[VISIBILITY_SNAPSHOT_LOCATION_IDX] = 2;
                snapshot[VISIBILITY_SNAPSHOT_Z_IDX] = 0;
                snapshot[VISIBILITY_SNAPSHOT_X_IDX] = 10;
                snapshot[VISIBILITY_SNAPSHOT_Y_IDX] = 12;
                snapshot[VISIBILITY_SNAPSHOT_SCROLL_X_IDX] = 8;
                snapshot[VISIBILITY_SNAPSHOT_SCROLL_Y_IDX] = 9;
                snapshot[VISIBILITY_SNAPSHOT_LIGHT_IDX] = 0x0A;
                snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET] = 0x11;
                snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET
                    + (VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
                    + VIEWPORT_VISIBILITY_WIDTH / 2] = 0x22;
                snapshot[VISIBILITY_SNAPSHOT_READY_OFFSET] = VISIBILITY_SNAPSHOT_READY_MARKER;
                snapshot
            },
        );

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 2).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 10).unwrap();
        mock.write_u8(base + MAP_Y, 12).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();
        mock.write_u8(base + MAP_SCROLL_X, 8).unwrap();
        mock.write_u8(base + MAP_SCROLL_Y, 9).unwrap();
        mock.write_u8(base + LIGHT_INTENSITY, 0x0A).unwrap();

        for i in 0..MAP_TILES_LEN {
            mock.write_u8(base + MAP_TILES + i, 0x05).unwrap();
        }
        for row in 0..VIEWPORT_VISIBILITY_HEIGHT {
            for col in 0..VIEWPORT_VISIBILITY_STRIDE {
                mock.write_u8(
                    ds_addr(0, VIEWPORT_VISIBILITY_GRID) + row * VIEWPORT_VISIBILITY_STRIDE + col,
                    0xFF,
                )
                .unwrap();
            }
        }

        let state = read_map_state_with_visibility_snapshot(&mock, 0, Some(snapshot_addr)).unwrap();
        let visibility = state.visibility_tiles.unwrap();
        assert_eq!(visibility[0], 0x11);
        assert_eq!(
            visibility[(VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
                + VIEWPORT_VISIBILITY_WIDTH / 2],
            0x22
        );
        assert_eq!(mock.snapshot_reads.get(), 3);
    }

    #[test]
    fn dominant_visibility_mask_prefers_latest_repeated_mask() {
        let stable = [0xFF; VIEWPORT_VISIBILITY_LEN];
        let mut transient = stable;
        transient[0] = 0x05;
        let mut updated = stable;
        updated[VIEWPORT_VISIBILITY_WIDTH + 1] = 0x31;

        let samples = [stable, transient, updated, transient];
        let dominant = dominant_visibility_mask(&samples, 0xFF);
        assert_eq!(dominant, Some((2, 3)));
    }

    #[test]
    fn visibility_window_medoid_chooses_central_mask_when_every_sample_is_unique() {
        let all_hidden = [0xFF; VIEWPORT_VISIBILITY_LEN];
        let mut center_only = all_hidden;
        center_only[(VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
            + VIEWPORT_VISIBILITY_WIDTH / 2] = 0x31;
        let mut center_and_east = center_only;
        center_and_east[(VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
            + VIEWPORT_VISIBILITY_WIDTH / 2
            + 1] = 0x31;

        let chosen =
            select_visibility_window_medoid(&[all_hidden, center_only, center_and_east], 0xFF);
        assert_eq!(chosen, center_only);
    }

    #[test]
    fn read_dungeon_state_reads_dedicated_dungeon_grid() {
        let mock = MockMemory::new(0x40000);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 34).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 1).unwrap();
        mock.write_u8(base + MAP_Y, 2).unwrap();
        mock.write_u8(base + DUNGEON_ORIENTATION, 2).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();

        for i in 0..MAP_TILES_LEN {
            mock.write_u8(base + MAP_TILES + i, 0x05).unwrap();
        }
        for i in 0..DUNGEON_TILES_LEN {
            mock.write_u8(base + DUNGEON_TILES_SAVE_OFFSET + i, (i % 251) as u8)
                .unwrap();
        }

        let state = read_map_state(&mock, 0).unwrap();
        assert_eq!(state.location, LocationType::Dungeon(34));
        assert_eq!(state.dungeon_facing, Some(CardinalDirection::South));
        assert!(state.visibility_tiles.is_none());
        assert_eq!(state.tiles[0], 0);
        assert_eq!(state.tiles[DUNGEON_LEVEL_LEN - 1], 63);
        assert_eq!(state.tiles[DUNGEON_LEVEL_LEN], 0);
    }

    #[test]
    fn read_dungeon_state_selects_current_floor() {
        let mock = MockMemory::new(0x40000);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 34).unwrap();
        mock.write_u8(base + MAP_Z, 3).unwrap();
        mock.write_u8(base + MAP_X, 1).unwrap();
        mock.write_u8(base + MAP_Y, 2).unwrap();
        mock.write_u8(base + DUNGEON_ORIENTATION, 1).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();

        for i in 0..DUNGEON_TILES_LEN {
            mock.write_u8(
                base + DUNGEON_TILES_SAVE_OFFSET + i,
                (i / DUNGEON_LEVEL_LEN) as u8,
            )
            .unwrap();
        }

        let state = read_map_state(&mock, 0).unwrap();
        assert_eq!(state.dungeon_facing, Some(CardinalDirection::East));
        assert_eq!(state.tiles[0], 3);
        assert_eq!(state.tiles[DUNGEON_LEVEL_LEN - 1], 3);
    }

    #[test]
    fn read_dungeon_state_clamps_out_of_range_floor_to_last_floor() {
        let mock = MockMemory::new(0x40000);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 34).unwrap();
        mock.write_u8(base + MAP_Z, 0xFF).unwrap();
        mock.write_u8(base + MAP_X, 1).unwrap();
        mock.write_u8(base + MAP_Y, 2).unwrap();
        mock.write_u8(base + DUNGEON_ORIENTATION, 0).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();

        for i in 0..DUNGEON_TILES_LEN {
            mock.write_u8(
                base + DUNGEON_TILES_SAVE_OFFSET + i,
                (i / DUNGEON_LEVEL_LEN) as u8,
            )
            .unwrap();
        }

        let state = read_map_state(&mock, 0).unwrap();
        let expected = (DUNGEON_FLOORS - 1) as u8;
        assert_eq!(state.dungeon_facing, Some(CardinalDirection::North));
        assert_eq!(state.tiles[0], expected);
        assert_eq!(state.tiles[DUNGEON_LEVEL_LEN - 1], expected);
    }

    #[test]
    fn dungeon_orientation_byte_maps_to_cardinal_direction() {
        assert_eq!(
            CardinalDirection::from_dungeon_byte(0),
            Some(CardinalDirection::North)
        );
        assert_eq!(
            CardinalDirection::from_dungeon_byte(1),
            Some(CardinalDirection::East)
        );
        assert_eq!(
            CardinalDirection::from_dungeon_byte(2),
            Some(CardinalDirection::South)
        );
        assert_eq!(
            CardinalDirection::from_dungeon_byte(3),
            Some(CardinalDirection::West)
        );
        assert_eq!(CardinalDirection::from_dungeon_byte(4), None);
    }

    #[test]
    fn read_objects_preserves_primary_tile_and_position() {
        let mock = MockMemory::new(0x40000);

        let base = SAVE_BASE;
        mock.write_u8(base + MAP_LOCATION, 34).unwrap();
        mock.write_u8(base + MAP_Z, 0).unwrap();
        mock.write_u8(base + MAP_X, 1).unwrap();
        mock.write_u8(base + MAP_Y, 2).unwrap();
        mock.write_u8(base + MAP_TRANSPORT, 0).unwrap();
        mock.write_u8(base + OBJECT_TABLE + OBJ_TILE1, 0x01)
            .unwrap();
        mock.write_u8(base + OBJECT_TABLE + OBJ_X, 4).unwrap();
        mock.write_u8(base + OBJECT_TABLE + OBJ_Y, 5).unwrap();
        mock.write_u8(base + OBJECT_TABLE + OBJ_FLOOR, 0).unwrap();

        let state = read_map_state(&mock, 0).unwrap();
        let obj = state.objects.first().unwrap();
        assert_eq!(obj.tile, 0x01);
        assert_eq!(obj.x, 4);
        assert_eq!(obj.y, 5);
        assert_eq!(obj.floor, 0);
    }

    #[test]
    fn underworld_detection_comes_from_outdoor_z() {
        assert!(!is_underworld_z(UNDERWORLD_Z_THRESHOLD));
        assert!(is_underworld_z(UNDERWORLD_Z_THRESHOLD + 1));

        let mut state = MapState {
            location: LocationType::Overworld,
            z: 0,
            x: 0,
            y: 0,
            dungeon_facing: None,
            transport: 0,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; MAP_TILES_LEN],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        };

        assert!(state.is_outdoor());
        assert!(!state.is_underworld());
        assert_eq!(state.display_location_name(), "Overworld");

        state.z = 0xFF;
        assert!(state.is_underworld());
        assert_eq!(state.display_location_name(), "Underworld");
    }
}
