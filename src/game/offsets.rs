/// Base address of the SAVED.GAM image within DOS memory.
pub const SAVE_BASE: usize = 0x24826;

/// Character records start at save offset 0x02.
pub const CHAR_RECORDS_OFFSET: usize = 0x02;

/// Each character record is 32 bytes.
pub const CHAR_RECORD_SIZE: usize = 0x20;

// Character field offsets within a record
pub const CHAR_NAME: usize = 0x00;
pub const CHAR_NAME_LEN: usize = 9;
pub const CHAR_GENDER: usize = 0x09;
pub const CHAR_CLASS: usize = 0x0A;
pub const CHAR_STATUS: usize = 0x0B;
pub const CHAR_STR: usize = 0x0C;
pub const CHAR_DEX: usize = 0x0D;
pub const CHAR_INT: usize = 0x0E;
pub const CHAR_MP: usize = 0x0F;
pub const CHAR_HP: usize = 0x10;
pub const CHAR_MAX_HP: usize = 0x12;
pub const CHAR_XP: usize = 0x14;
pub const CHAR_LEVEL: usize = 0x16;
pub const CHAR_EQUIPMENT: usize = 0x19;
pub const CHAR_EQUIPMENT_LEN: usize = 6;

// Inventory save offsets (add SAVE_BASE for DOS address)
pub const INV_FOOD: usize = 0x202;
pub const INV_GOLD: usize = 0x204;
pub const INV_KEYS: usize = 0x206;
pub const INV_GEMS: usize = 0x207;
pub const INV_TORCHES: usize = 0x208;
pub const INV_ARROWS: usize = 0x235;
pub const INV_REAGENTS: usize = 0x2AA;
pub const INV_PARTY_SIZE: usize = 0x2B5;
pub const INV_KARMA: usize = 0x2E2;

/// Compute the absolute address of a character field.
pub const fn char_addr(dos_base: usize, char_index: usize, field_offset: usize) -> usize {
    dos_base + SAVE_BASE + CHAR_RECORDS_OFFSET + (char_index * CHAR_RECORD_SIZE) + field_offset
}

/// Compute the absolute address of an inventory field.
pub const fn inv_addr(dos_base: usize, save_offset: usize) -> usize {
    dos_base + SAVE_BASE + save_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char0_status_matches_ct() {
        // CT file delta: 0x24A28 (Food DOS addr) - 0x202 (Food save offset) = 0x24826
        assert_eq!(SAVE_BASE, 0x24826);
        // Character 0 status at DOS offset 0x24833
        assert_eq!(char_addr(0, 0, CHAR_STATUS), 0x24833);
    }

    #[test]
    fn food_offset_matches_ct() {
        assert_eq!(inv_addr(0, INV_FOOD), 0x24A28);
    }

    #[test]
    fn gold_offset_matches_ct() {
        assert_eq!(inv_addr(0, INV_GOLD), 0x24A2A);
    }

    #[test]
    fn char_record_stride() {
        let c0 = char_addr(0, 0, 0);
        let c1 = char_addr(0, 1, 0);
        assert_eq!(c1 - c0, CHAR_RECORD_SIZE);
        assert_eq!(CHAR_RECORD_SIZE, 0x20);
    }
}
