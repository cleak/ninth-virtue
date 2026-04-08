use anyhow::Result;

use crate::game::offsets::*;
use crate::memory::access::MemoryAccess;

/// A frigate (ship) found in the object table.
#[derive(Debug, Clone, PartialEq)]
pub struct Frigate {
    /// Slot index in the object table (0..31).
    pub slot: usize,
    /// Tile byte (sprite index minus 0x100).
    pub tile: u8,
    pub x: u8,
    pub y: u8,
    /// Hull hit points (0 = sunk, max 99).
    pub hull: u8,
    /// Number of skiffs aboard.
    pub skiffs: u8,
}

impl Frigate {
    /// Whether this is a pirate ship (vs. a regular frigate).
    pub fn is_pirate(&self) -> bool {
        (PIRATE_TILE_MIN..=PIRATE_TILE_MAX).contains(&self.tile)
    }

    /// Human-readable label for display.
    pub fn label(&self) -> &'static str {
        if self.is_pirate() {
            "Pirate Ship"
        } else {
            "Frigate"
        }
    }
}

/// Returns true if a tile byte represents a frigate (regular or pirate).
///
/// Works for both object-table tile bytes and the transport byte at
/// save offset 0x2D6, since the transport byte uses the same encoding.
pub fn is_frigate_tile(tile: u8) -> bool {
    (SHIP_TILE_MIN..=SHIP_TILE_MAX).contains(&tile)
        || (PIRATE_TILE_MIN..=PIRATE_TILE_MAX).contains(&tile)
}

/// Read all frigates from the object table.
pub fn read_frigates(mem: &dyn MemoryAccess, dos_base: usize) -> Result<Vec<Frigate>> {
    let mut frigates = Vec::new();

    for slot in 0..OBJECT_TABLE_SLOTS {
        let tile = mem.read_u8(obj_addr(dos_base, slot, OBJ_TILE1))?;
        if !is_frigate_tile(tile) {
            continue;
        }

        let x = mem.read_u8(obj_addr(dos_base, slot, OBJ_X))?;
        let y = mem.read_u8(obj_addr(dos_base, slot, OBJ_Y))?;
        let hull = mem.read_u8(obj_addr(dos_base, slot, OBJ_DEPENDS1))?;
        let skiffs = mem.read_u8(obj_addr(dos_base, slot, OBJ_DEPENDS3))?;

        frigates.push(Frigate {
            slot,
            tile,
            x,
            y,
            hull,
            skiffs,
        });
    }

    Ok(frigates)
}

/// Write a frigate's hull HP back to game memory.
pub fn write_frigate_hull(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    frigate: &Frigate,
) -> Result<()> {
    mem.write_u8(obj_addr(dos_base, frigate.slot, OBJ_DEPENDS1), frigate.hull)
}

/// Write a frigate's skiffs-aboard count back to game memory.
pub fn write_frigate_skiffs(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    frigate: &Frigate,
) -> Result<()> {
    mem.write_u8(
        obj_addr(dos_base, frigate.slot, OBJ_DEPENDS3),
        frigate.skiffs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::access::MockMemory;

    fn mock_with_frigates() -> MockMemory {
        // Need enough space for the object table (0x6B4 + 32*8 = 0x7B4)
        let mem = MockMemory::new(SAVE_BASE + 0x800);

        // Slot 0: party avatar (not a ship, tile=28)
        mem.set_bytes(obj_addr(0, 0, OBJ_TILE1), &[28]);

        // Slot 1: a frigate (tile=36 = ShipNoSailsUp)
        mem.set_bytes(obj_addr(0, 1, OBJ_TILE1), &[36]);
        mem.set_bytes(obj_addr(0, 1, OBJ_X), &[100]);
        mem.set_bytes(obj_addr(0, 1, OBJ_Y), &[50]);
        mem.set_bytes(obj_addr(0, 1, OBJ_DEPENDS1), &[80]); // hull=80
        mem.set_bytes(obj_addr(0, 1, OBJ_DEPENDS3), &[2]); // 2 skiffs

        // Slot 5: a pirate ship (tile=44 = PirateShipUp)
        mem.set_bytes(obj_addr(0, 5, OBJ_TILE1), &[44]);
        mem.set_bytes(obj_addr(0, 5, OBJ_X), &[200]);
        mem.set_bytes(obj_addr(0, 5, OBJ_Y), &[150]);
        mem.set_bytes(obj_addr(0, 5, OBJ_DEPENDS1), &[30]); // hull=30
        mem.set_bytes(obj_addr(0, 5, OBJ_DEPENDS3), &[0]); // no skiffs

        // Slot 10: a skiff (tile=40, NOT a frigate)
        mem.set_bytes(obj_addr(0, 10, OBJ_TILE1), &[40]);

        mem
    }

    #[test]
    fn reads_only_frigates() {
        let mem = mock_with_frigates();
        let frigates = read_frigates(&mem, 0).unwrap();
        assert_eq!(frigates.len(), 2);
    }

    #[test]
    fn frigate_fields_correct() {
        let mem = mock_with_frigates();
        let frigates = read_frigates(&mem, 0).unwrap();

        let f = &frigates[0];
        assert_eq!(f.slot, 1);
        assert_eq!(f.tile, 36);
        assert_eq!(f.x, 100);
        assert_eq!(f.y, 50);
        assert_eq!(f.hull, 80);
        assert_eq!(f.skiffs, 2);
        assert!(!f.is_pirate());
        assert_eq!(f.label(), "Frigate");
    }

    #[test]
    fn pirate_ship_detected() {
        let mem = mock_with_frigates();
        let frigates = read_frigates(&mem, 0).unwrap();

        let p = &frigates[1];
        assert_eq!(p.slot, 5);
        assert!(p.is_pirate());
        assert_eq!(p.label(), "Pirate Ship");
        assert_eq!(p.hull, 30);
    }

    #[test]
    fn write_hull_roundtrip() {
        let mem = mock_with_frigates();
        let mut frigates = read_frigates(&mem, 0).unwrap();

        frigates[0].hull = FRIGATE_MAX_HULL;
        write_frigate_hull(&mem, 0, &frigates[0]).unwrap();

        let reread = read_frigates(&mem, 0).unwrap();
        assert_eq!(reread[0].hull, FRIGATE_MAX_HULL);
    }

    #[test]
    fn write_skiffs_roundtrip() {
        let mem = mock_with_frigates();
        let mut frigates = read_frigates(&mem, 0).unwrap();

        frigates[0].skiffs = 5;
        write_frigate_skiffs(&mem, 0, &frigates[0]).unwrap();

        let reread = read_frigates(&mem, 0).unwrap();
        assert_eq!(reread[0].skiffs, 5);
        // hull unchanged
        assert_eq!(reread[0].hull, 80);
    }

    #[test]
    fn no_frigates_returns_empty() {
        let mem = MockMemory::new(SAVE_BASE + 0x800);
        let frigates = read_frigates(&mem, 0).unwrap();
        assert!(frigates.is_empty());
    }
}
