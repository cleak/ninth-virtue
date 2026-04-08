use anyhow::Result;

use crate::game::offsets::{
    ENTITY_CURRENT_TILE, ENTITY_MAX_COUNT, ENTITY_RECORD_SIZE, ENTITY_TABLE, ENTITY_TYPE, ENTITY_X,
    ENTITY_Y, inv_addr,
};
use crate::memory::access::MemoryAccess;

/// A map entity read from the game's entity table.
///
/// The entity table at DS:0x5C5A holds 32 slots of 8 bytes each.
/// See `docs/memory-map.md` for the full field layout and disassembly evidence.
#[derive(Debug, Clone)]
pub struct Entity {
    /// Index of this entity in the entity table (0-31).
    #[allow(dead_code)] // useful for debugging; asserted in tests
    pub slot: u8,
    /// Entity type at record field +0 (0 = empty slot).
    /// For vehicles/player: matches the transport type (e.g. 0x1C = on foot).
    /// For NPCs/creatures: the base type masked with 0xFC for behavior lookup.
    pub entity_type: u8,
    /// Display tile at record field +1. For the minimap, the actual atlas
    /// sprite is at `tile_id + 256` (the animated page of the tile atlas).
    pub tile_id: u8,
    /// X position within the current map.
    pub x: u8,
    /// Y position within the current map.
    pub y: u8,
}

impl Entity {
    /// Returns true if this entity slot is occupied and displayable.
    ///
    /// Empty slots have entity_type == 0. Tile values 0x1D and 0x1E are
    /// special "dead/gone" markers that the game skips during rendering.
    pub fn is_active(&self) -> bool {
        self.entity_type != 0 && self.tile_id != 0 && self.tile_id != 0x1D && self.tile_id != 0x1E
    }
}

/// Read all active entities from the game's entity table.
///
/// Returns up to 32 entities with their type, display tile, and position.
/// Empty and dead slots are filtered out.
pub fn read_entities(mem: &dyn MemoryAccess, dos_base: usize) -> Result<Vec<Entity>> {
    let mut entities = Vec::new();

    for i in 0..ENTITY_MAX_COUNT {
        let base = inv_addr(dos_base, ENTITY_TABLE + i * ENTITY_RECORD_SIZE);
        let entity_type = mem.read_u8(base + ENTITY_TYPE)?;
        let tile_id = mem.read_u8(base + ENTITY_CURRENT_TILE)?;
        let x = mem.read_u8(base + ENTITY_X)?;
        let y = mem.read_u8(base + ENTITY_Y)?;
        let entity = Entity {
            slot: i as u8,
            entity_type,
            tile_id,
            x,
            y,
        };
        if entity.is_active() {
            entities.push(entity);
        }
    }

    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::offsets::SAVE_BASE;
    use crate::memory::access::MockMemory;

    #[test]
    fn entity_table_layout() {
        // Verify the entity table starts at the expected DS offset
        assert_eq!(ENTITY_TABLE + 0x55A6, 0x5C5A);
        // Verify table fits: 32 records * 8 bytes = 256 bytes
        assert_eq!(ENTITY_MAX_COUNT * ENTITY_RECORD_SIZE, 256);
        // Verify table ends before NPC table B at save 0x7B8
        assert!(ENTITY_TABLE + ENTITY_MAX_COUNT * ENTITY_RECORD_SIZE <= 0x7B8);
    }

    #[test]
    fn entity_active_check() {
        let active = Entity {
            slot: 5,
            entity_type: 0x84,
            tile_id: 0x85,
            x: 10,
            y: 20,
        };
        assert!(active.is_active());

        let inactive = Entity {
            slot: 0,
            entity_type: 0,
            tile_id: 0,
            x: 0,
            y: 0,
        };
        assert!(!inactive.is_active());

        // Field +1 = 0x1D is a "dead/gone" marker
        let dead = Entity {
            slot: 1,
            entity_type: 0x84,
            tile_id: 0x1D,
            x: 5,
            y: 5,
        };
        assert!(!dead.is_active());
    }

    #[test]
    fn read_entities_from_mock() {
        let mock = MockMemory::new(0x30000);

        // Write entity slot 1: an NPC at (15, 22)
        // Field +0 = entity type 0x84, field +1 = display tile 0x85
        let slot1 = SAVE_BASE + ENTITY_TABLE + 1 * ENTITY_RECORD_SIZE;
        mock.write_u8(slot1 + ENTITY_TYPE, 0x84).unwrap();
        mock.write_u8(slot1 + ENTITY_CURRENT_TILE, 0x85).unwrap();
        mock.write_u8(slot1 + ENTITY_X, 15).unwrap();
        mock.write_u8(slot1 + ENTITY_Y, 22).unwrap();

        // Write entity slot 5: a monster at (8, 3)
        let slot5 = SAVE_BASE + ENTITY_TABLE + 5 * ENTITY_RECORD_SIZE;
        mock.write_u8(slot5 + ENTITY_TYPE, 0xA0).unwrap();
        mock.write_u8(slot5 + ENTITY_CURRENT_TILE, 0xA1).unwrap();
        mock.write_u8(slot5 + ENTITY_X, 8).unwrap();
        mock.write_u8(slot5 + ENTITY_Y, 3).unwrap();

        // Slot 0 is empty (all zeros from MockMemory init)

        let entities = read_entities(&mock, 0).unwrap();

        // Should have 2 active entities
        assert_eq!(entities.len(), 2);

        assert_eq!(entities[0].slot, 1);
        assert_eq!(entities[0].entity_type, 0x84);
        assert_eq!(entities[0].tile_id, 0x85);
        assert_eq!(entities[0].x, 15);
        assert_eq!(entities[0].y, 22);

        assert_eq!(entities[1].slot, 5);
        assert_eq!(entities[1].entity_type, 0xA0);
        assert_eq!(entities[1].tile_id, 0xA1);
        assert_eq!(entities[1].x, 8);
        assert_eq!(entities[1].y, 3);
    }

    #[test]
    fn empty_entity_table() {
        let mock = MockMemory::new(0x30000);
        let entities = read_entities(&mock, 0).unwrap();
        assert!(entities.is_empty());
    }
}
