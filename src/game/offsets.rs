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

// Map and position save offsets (add SAVE_BASE for DOS address)
pub const MAP_TRANSPORT: usize = 0x2D6;
pub const MAP_LOCATION: usize = 0x2ED;
pub const MAP_Z: usize = 0x2EF;
pub const MAP_X: usize = 0x2F0;
pub const MAP_Y: usize = 0x2F1;
pub const MAP_SCROLL_X: usize = 0x2F5;
pub const MAP_SCROLL_Y: usize = 0x2F6;
pub const LIGHT_INTENSITY: usize = 0x2FF;
pub const LIGHT_SPELL_DUR: usize = 0x300;
pub const TORCH_DUR: usize = 0x301;
/// Dungeon-facing orientation: 0=north, 1=east, 2=south, 3=west.
pub const DUNGEON_ORIENTATION: usize = 0x105D;
/// Save-relative alias for the active dungeon terrain buffer.
///
/// The live DATA.OVL dungeon buffer is mirrored into SAVED.GAM at this offset,
/// so code that reads via [`inv_addr`] should use this constant.
pub const DUNGEON_TILES_SAVE_OFFSET: usize = 0x3B4;
/// DATA.OVL-relative offset for the active dungeon terrain buffer (`DS:0x595A`).
///
/// Code that reads the live buffer via [`ds_addr`] should use this constant.
pub const DUNGEON_TILES_DS_OFFSET: usize = 0x595A;
pub const DUNGEON_FLOORS: usize = 8;
pub const DUNGEON_LEVEL_WIDTH: usize = 8;
pub const DUNGEON_LEVEL_HEIGHT: usize = 8;
pub const DUNGEON_LEVEL_LEN: usize = DUNGEON_LEVEL_WIDTH * DUNGEON_LEVEL_HEIGHT;
pub const DUNGEON_TILES_LEN: usize = DUNGEON_FLOORS * DUNGEON_LEVEL_LEN;
pub const MAP_TILES: usize = 0x1062;
pub const MAP_TILES_LEN: usize = 1024;

/// DATA.OVL's live data segment base in DOS memory.
///
/// The original code addresses `MAP_TILES` at DS:0x6608, while the same buffer
/// sits at save offset `MAP_TILES` within `SAVED.GAM`. That gives us the
/// runtime DOS base for other DS-relative combat buffers.
pub const DATA_SEG_MAP_TILES: usize = 0x6608;
pub const DATA_SEG_BASE: usize = SAVE_BASE + MAP_TILES - DATA_SEG_MAP_TILES;

/// Combat terrain scratch grid (DS:0xAD14): 11 active columns per row with a
/// 32-byte stride.
pub const COMBAT_TERRAIN_GRID: usize = 0xAD14;
pub const COMBAT_TERRAIN_WIDTH: usize = 11;
pub const COMBAT_TERRAIN_HEIGHT: usize = 11;
pub const COMBAT_TERRAIN_STRIDE: usize = 32;
pub const COMBAT_TERRAIN_LEN: usize = COMBAT_TERRAIN_HEIGHT * COMBAT_TERRAIN_STRIDE;
/// Current 2D visibility scratch grid (DS:0xAB02): 11 active columns per row
/// with a 32-byte stride. Hidden cells read back as 0xFF.
pub const VIEWPORT_VISIBILITY_GRID: usize = 0xAB02;
pub const VIEWPORT_VISIBILITY_WIDTH: usize = 11;
pub const VIEWPORT_VISIBILITY_HEIGHT: usize = 11;
pub const VIEWPORT_VISIBILITY_STRIDE: usize = 32;
pub const VIEWPORT_VISIBILITY_LEN: usize = VIEWPORT_VISIBILITY_WIDTH * VIEWPORT_VISIBILITY_HEIGHT;

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

// Vehicle/object table (save offset 0x6B4): 32 entries x 8 bytes each.
// Contains monsters, NPCs, and vehicles (frigates, skiffs, etc.).
pub const OBJECT_TABLE: usize = 0x6B4;
pub const OBJECT_TABLE_SLOTS: usize = 32;
pub const OBJECT_ENTRY_SIZE: usize = 8;

// Object entry field offsets within each 8-byte entry
pub const OBJ_TILE1: usize = 0; // sprite tile (add 0x100 for full index)
pub const OBJ_X: usize = 2;
pub const OBJ_Y: usize = 3;
pub const OBJ_FLOOR: usize = 4;
pub const OBJ_DEPENDS1: usize = 5; // frigate: hull HP
pub const OBJ_DEPENDS3: usize = 7; // frigate: skiffs aboard

// Frigate tile byte ranges (sprite index minus 0x100):
//   32..=39 = regular ships (with/without sails, 4 directions)
//   44..=47 = pirate ships (4 directions)
pub const SHIP_TILE_MIN: u8 = 32;
pub const SHIP_TILE_MAX: u8 = 39;
pub const PIRATE_TILE_MIN: u8 = 44;
pub const PIRATE_TILE_MAX: u8 = 47;

/// Maximum hull HP for a frigate (per Ultima V game logic).
pub const FRIGATE_MAX_HULL: u8 = 99;

// Game state flags (save offsets) — used for labeling in debug tools.
pub const ACTIVE_PLAYER: usize = 0x2D5;
pub const ANIM_NEXT_FRAME: usize = 0x2EB;
pub const UPDATE_MAP: usize = 0x2FE;
pub const NEW_PROMPT: usize = 0x3B0;

// Shrine quest progress (save offsets)
pub const SHRINE_ORDAINED: usize = 0x326;
pub const SHRINE_CODEX_VISITED: usize = 0x328;

/// Compute the absolute address of a character field.
pub const fn char_addr(dos_base: usize, char_index: usize, field_offset: usize) -> usize {
    dos_base + SAVE_BASE + CHAR_RECORDS_OFFSET + (char_index * CHAR_RECORD_SIZE) + field_offset
}

/// Compute the absolute address of an inventory field.
pub const fn inv_addr(dos_base: usize, save_offset: usize) -> usize {
    dos_base + SAVE_BASE + save_offset
}

/// Compute the absolute DOS address of a DS-relative live buffer.
pub const fn ds_addr(dos_base: usize, ds_offset: usize) -> usize {
    dos_base + DATA_SEG_BASE + ds_offset
}

/// Compute the absolute address of an object table entry field.
pub const fn obj_addr(dos_base: usize, slot: usize, field_offset: usize) -> usize {
    dos_base + SAVE_BASE + OBJECT_TABLE + (slot * OBJECT_ENTRY_SIZE) + field_offset
}

/// Return a human-readable label for a save-relative offset, if known.
pub fn label_for_save_offset(offset: usize) -> Option<&'static str> {
    match offset {
        o if (CHAR_RECORDS_OFFSET..CHAR_RECORDS_OFFSET + 16 * CHAR_RECORD_SIZE).contains(&o) => {
            let rel = (o - CHAR_RECORDS_OFFSET) % CHAR_RECORD_SIZE;
            match rel {
                0x00..=0x08 => Some("name"),
                0x09 => Some("gender"),
                0x0A => Some("class"),
                0x0B => Some("status"),
                0x0C => Some("str"),
                0x0D => Some("dex"),
                0x0E => Some("int"),
                0x0F => Some("mp"),
                0x10..=0x11 => Some("hp"),
                0x12..=0x13 => Some("max_hp"),
                0x14..=0x15 => Some("xp"),
                0x16 => Some("level"),
                0x19..=0x1E => Some("equipment"),
                _ => None,
            }
        }
        0x202..=0x203 => Some("food"),
        0x204..=0x205 => Some("gold"),
        0x206 => Some("keys"),
        0x207 => Some("gems"),
        0x208 => Some("torches"),
        0x235 => Some("arrows"),
        0x2AA..=0x2B1 => Some("reagents"),
        0x2B5 => Some("party_size"),
        ACTIVE_PLAYER => Some("ACTIVE_PLAYER"),
        0x2E2 => Some("karma"),
        ANIM_NEXT_FRAME => Some("ANIM_NEXT_FRAME"),
        0x2EC => Some("wind_dir"),
        0x2ED => Some("location"),
        0x2EF => Some("z_coord"),
        0x2F0 => Some("x_coord"),
        0x2F1 => Some("y_coord"),
        0x2F2 => Some("crosshair_vis"),
        0x2F5 => Some("chunk_x"),
        0x2F6 => Some("chunk_y"),
        UPDATE_MAP => Some("UPDATE_MAP"),
        LIGHT_INTENSITY => Some("light_intensity"),
        LIGHT_SPELL_DUR => Some("light_spell_dur"),
        TORCH_DUR => Some("torch_dur"),
        NEW_PROMPT => Some("NEW_PROMPT"),
        DUNGEON_ORIENTATION => Some("dungeon_orientation"),
        SHRINE_ORDAINED => Some("shrine_ordained"),
        SHRINE_CODEX_VISITED => Some("shrine_codex_visited"),
        _ => None,
    }
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

    #[test]
    fn ds_offsets_match_known_live_buffers() {
        assert_eq!(DATA_SEG_BASE, 0x1F280);
        assert_eq!(ds_addr(0, DATA_SEG_MAP_TILES), inv_addr(0, MAP_TILES));
        assert_eq!(ds_addr(0, 0x5896), inv_addr(0, MAP_X));
        assert_eq!(ds_addr(0, 0x5897), inv_addr(0, MAP_Y));
        assert_eq!(
            ds_addr(0, DUNGEON_TILES_DS_OFFSET),
            inv_addr(0, DUNGEON_TILES_SAVE_OFFSET)
        );
    }
}
