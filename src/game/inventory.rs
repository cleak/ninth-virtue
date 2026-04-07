use anyhow::Result;

use crate::game::offsets::*;
use crate::memory::access::MemoryAccess;

pub const REAGENT_NAMES: &[&str] = &[
    "Sulph. Ash",
    "Ginseng",
    "Garlic",
    "Spider Silk",
    "Blood Moss",
    "Black Pearl",
    "Nightshade",
    "Mandrake",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    pub food: u16,
    pub gold: u16,
    pub keys: u8,
    pub gems: u8,
    pub torches: u8,
    pub arrows: u8,
    pub reagents: [u8; 8],
    pub karma: u8,
}

pub fn read_inventory(mem: &dyn MemoryAccess, dos_base: usize) -> Result<Inventory> {
    let food = mem.read_u16_le(inv_addr(dos_base, INV_FOOD))?;
    let gold = mem.read_u16_le(inv_addr(dos_base, INV_GOLD))?;
    let keys = mem.read_u8(inv_addr(dos_base, INV_KEYS))?;
    let gems = mem.read_u8(inv_addr(dos_base, INV_GEMS))?;
    let torches = mem.read_u8(inv_addr(dos_base, INV_TORCHES))?;
    let arrows = mem.read_u8(inv_addr(dos_base, INV_ARROWS))?;
    let karma = mem.read_u8(inv_addr(dos_base, INV_KARMA))?;

    let mut reagents = [0u8; 8];
    mem.read_bytes(inv_addr(dos_base, INV_REAGENTS), &mut reagents)?;

    Ok(Inventory {
        food,
        gold,
        keys,
        gems,
        torches,
        arrows,
        reagents,
        karma,
    })
}

pub fn write_inventory(mem: &dyn MemoryAccess, dos_base: usize, inv: &Inventory) -> Result<()> {
    mem.write_u16_le(inv_addr(dos_base, INV_FOOD), inv.food)?;
    mem.write_u16_le(inv_addr(dos_base, INV_GOLD), inv.gold)?;
    mem.write_u8(inv_addr(dos_base, INV_KEYS), inv.keys)?;
    mem.write_u8(inv_addr(dos_base, INV_GEMS), inv.gems)?;
    mem.write_u8(inv_addr(dos_base, INV_TORCHES), inv.torches)?;
    mem.write_u8(inv_addr(dos_base, INV_ARROWS), inv.arrows)?;
    mem.write_u8(inv_addr(dos_base, INV_KARMA), inv.karma)?;
    mem.write_bytes(inv_addr(dos_base, INV_REAGENTS), &inv.reagents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::access::MockMemory;

    fn setup_mock() -> MockMemory {
        let mem = MockMemory::new(SAVE_BASE + 0x300);

        mem.set_bytes(inv_addr(0, INV_FOOD), &500u16.to_le_bytes());
        mem.set_bytes(inv_addr(0, INV_GOLD), &1200u16.to_le_bytes());
        mem.set_bytes(inv_addr(0, INV_KEYS), &[3]);
        mem.set_bytes(inv_addr(0, INV_GEMS), &[5]);
        mem.set_bytes(inv_addr(0, INV_TORCHES), &[10]);
        mem.set_bytes(inv_addr(0, INV_ARROWS), &[99]);
        mem.set_bytes(inv_addr(0, INV_REAGENTS), &[10, 20, 30, 40, 5, 15, 25, 35]);
        mem.set_bytes(inv_addr(0, INV_KARMA), &[75]);

        mem
    }

    #[test]
    fn read_inventory_fields() {
        let mem = setup_mock();
        let inv = read_inventory(&mem, 0).unwrap();
        assert_eq!(inv.food, 500);
        assert_eq!(inv.gold, 1200);
        assert_eq!(inv.keys, 3);
        assert_eq!(inv.gems, 5);
        assert_eq!(inv.torches, 10);
        assert_eq!(inv.arrows, 99);
        assert_eq!(inv.reagents, [10, 20, 30, 40, 5, 15, 25, 35]);
        assert_eq!(inv.karma, 75);
    }

    #[test]
    fn write_then_read_roundtrip() {
        let mem = MockMemory::new(SAVE_BASE + 0x300);
        let inv = Inventory {
            food: 9999,
            gold: 9999,
            keys: 99,
            gems: 99,
            torches: 99,
            arrows: 99,
            reagents: [99; 8],
            karma: 99,
        };
        write_inventory(&mem, 0, &inv).unwrap();
        let inv2 = read_inventory(&mem, 0).unwrap();
        assert_eq!(inv, inv2);
    }

    #[test]
    fn boundary_values() {
        let mem = MockMemory::new(SAVE_BASE + 0x300);

        let inv_min = Inventory::default();
        write_inventory(&mem, 0, &inv_min).unwrap();
        assert_eq!(read_inventory(&mem, 0).unwrap(), inv_min);

        let inv_max = Inventory {
            food: 9999,
            gold: 9999,
            keys: 255,
            gems: 255,
            torches: 255,
            arrows: 255,
            reagents: [255; 8],
            karma: 255,
        };
        write_inventory(&mem, 0, &inv_max).unwrap();
        assert_eq!(read_inventory(&mem, 0).unwrap(), inv_max);
    }

    #[test]
    fn writing_one_field_doesnt_corrupt_adjacent() {
        let mem = setup_mock();
        let original = read_inventory(&mem, 0).unwrap();

        let mut modified = original.clone();
        modified.gold = 5000;
        write_inventory(&mem, 0, &modified).unwrap();

        let readback = read_inventory(&mem, 0).unwrap();
        assert_eq!(readback.food, original.food);
        assert_eq!(readback.gold, 5000);
        assert_eq!(readback.keys, original.keys);
        assert_eq!(readback.gems, original.gems);
        assert_eq!(readback.arrows, original.arrows);
        assert_eq!(readback.reagents, original.reagents);
        assert_eq!(readback.karma, original.karma);
    }
}
