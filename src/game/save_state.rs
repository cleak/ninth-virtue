//! Save-state system: deterministic snapshot/restore of DOS memory.
//!
//! Uses the code-cave trap mechanism to pause emulated execution at a
//! known quiescent point (`get_command`), then reads or writes the full
//! 1 MB DOS address space while the game is deterministically frozen.
//!
//! See the plan in `.claude/plans/goofy-jumping-valiant.md` for the
//! full design rationale.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

use crate::game::injection::PatchState;
use crate::game::map::LocationType;
use crate::game::offsets::{
    CHAR_NAME, CHAR_NAME_LEN, CHAR_RECORDS_OFFSET, MAP_LOCATION, MAP_X, MAP_Y, MAP_Z, SAVE_BASE,
};
use crate::memory::access::MemoryAccess;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Size of the DOS conventional address space (1 MB).
const DOS_MEM_SIZE: usize = 0x10_0000;

/// Magic bytes identifying a ninth-virtue save file.
const MAGIC: [u8; 4] = *b"NV5S";

/// Current file format version.
const VERSION: u8 = 1;

/// Fixed header size before the raw memory dump.
const HEADER_SIZE: usize = 4 + 1 + 8 + 1 + 1 + 1 + 1 + 16 + 4; // = 37

/// Subdirectory (inside the game directory) for save files.
const SAVE_DIR: &str = "ninth-virtue-saves";

/// Number of save slots.
pub const NUM_SLOTS: usize = 5;

// ---------------------------------------------------------------------------
// Slot metadata
// ---------------------------------------------------------------------------

/// Lightweight metadata for displaying a save slot in the UI.
#[derive(Debug, Clone)]
#[allow(dead_code)] // location_id reserved for future UI mode-mismatch warnings
pub struct SlotInfo {
    pub timestamp: i64,
    pub location_id: u8,
    pub location: String,
    pub leader_name: String,
}

// ---------------------------------------------------------------------------
// File I/O helpers
// ---------------------------------------------------------------------------

fn save_dir(game_dir: &Path) -> PathBuf {
    game_dir.join(SAVE_DIR)
}

fn slot_path(game_dir: &Path, slot: usize) -> PathBuf {
    save_dir(game_dir).join(format!("slot_{slot}.nv5"))
}

// ---------------------------------------------------------------------------
// Header encode/decode
// ---------------------------------------------------------------------------

fn encode_header(
    timestamp: i64,
    location_id: u8,
    x: u8,
    y: u8,
    z: u8,
    leader_name: &[u8],
) -> [u8; HEADER_SIZE] {
    let mut hdr = [0u8; HEADER_SIZE];
    hdr[0..4].copy_from_slice(&MAGIC);
    hdr[4] = VERSION;
    hdr[5..13].copy_from_slice(&timestamp.to_le_bytes());
    hdr[13] = location_id;
    hdr[14] = x;
    hdr[15] = y;
    hdr[16] = z;
    // Leader name: up to 16 bytes, null-padded (rest is already 0).
    let name_len = leader_name.len().min(16);
    hdr[17..17 + name_len].copy_from_slice(&leader_name[..name_len]);
    // Bytes 33..37 are reserved (zero).
    hdr
}

fn decode_header(data: &[u8]) -> Result<(i64, u8, u8, u8, u8, String)> {
    ensure!(
        data.len() >= HEADER_SIZE,
        "save file too small ({} bytes, need at least {HEADER_SIZE})",
        data.len()
    );
    ensure!(data[0..4] == MAGIC, "bad magic — not a ninth-virtue save");
    ensure!(
        data[4] == VERSION,
        "unsupported save version {} (expected {VERSION})",
        data[4]
    );

    let timestamp = i64::from_le_bytes(data[5..13].try_into().unwrap());
    let location_id = data[13];
    let x = data[14];
    let y = data[15];
    let z = data[16];

    let name_bytes = &data[17..33];
    let name_end = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
    let leader_name = String::from_utf8_lossy(&name_bytes[..name_end]).to_string();

    Ok((timestamp, location_id, x, y, z, leader_name))
}

// ---------------------------------------------------------------------------
// Extract metadata from a raw 1 MB snapshot
// ---------------------------------------------------------------------------

fn extract_metadata(snapshot: &[u8]) -> (u8, u8, u8, u8, [u8; 16]) {
    let location_id = snapshot[SAVE_BASE + MAP_LOCATION];
    let x = snapshot[SAVE_BASE + MAP_X];
    let y = snapshot[SAVE_BASE + MAP_Y];
    let z = snapshot[SAVE_BASE + MAP_Z];

    let mut leader_name = [0u8; 16];
    let name_start = SAVE_BASE + CHAR_RECORDS_OFFSET + CHAR_NAME;
    let name_end = name_start + CHAR_NAME_LEN;
    let src = &snapshot[name_start..name_end];
    let len = src.iter().position(|&b| b == 0).unwrap_or(CHAR_NAME_LEN);
    leader_name[..len].copy_from_slice(&src[..len]);

    (location_id, x, y, z, leader_name)
}

// ---------------------------------------------------------------------------
// Public API — called by the GameController
// ---------------------------------------------------------------------------

/// Read 1 MB and write to a slot file.  Caller must have already trapped
/// the game (and will release it afterward).
pub fn save_trapped(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    slot: usize,
    game_dir: &Path,
) -> Result<SlotInfo> {
    ensure!(
        slot < NUM_SLOTS,
        "slot {slot} out of range (0..{NUM_SLOTS})"
    );

    let mut snapshot = vec![0u8; DOS_MEM_SIZE];
    mem.read_bytes(dos_base, &mut snapshot)
        .context("reading DOS memory for snapshot")?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let (location_id, x, y, z, leader_name) = extract_metadata(&snapshot);
    let header = encode_header(timestamp, location_id, x, y, z, &leader_name);

    let dir = save_dir(game_dir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating save directory {}", dir.display()))?;

    let path = slot_path(game_dir, slot);
    let mut file_data = Vec::with_capacity(HEADER_SIZE + DOS_MEM_SIZE);
    file_data.extend_from_slice(&header);
    file_data.extend_from_slice(&snapshot);
    std::fs::write(&path, &file_data)
        .with_context(|| format!("writing save file {}", path.display()))?;

    let location = LocationType::from_id(location_id).name().to_string();
    let name_end = leader_name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(leader_name.len());
    let leader = String::from_utf8_lossy(&leader_name[..name_end]).to_string();

    log::info!("Saved slot {slot} to {}", path.display());

    Ok(SlotInfo {
        timestamp,
        location_id,
        location,
        leader_name: leader,
    })
}

/// Read a slot file and write it to DOS memory, skipping the cave region.
/// Caller must have already trapped the game (and will release it afterward).
pub fn load_trapped(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    patch: &PatchState,
    slot: usize,
    game_dir: &Path,
) -> Result<()> {
    ensure!(
        slot < NUM_SLOTS,
        "slot {slot} out of range (0..{NUM_SLOTS})"
    );

    let path = slot_path(game_dir, slot);
    let file_data =
        std::fs::read(&path).with_context(|| format!("reading save file {}", path.display()))?;

    let expected_size = HEADER_SIZE + DOS_MEM_SIZE;
    ensure!(
        file_data.len() == expected_size,
        "save file is {} bytes, expected {expected_size}",
        file_data.len()
    );

    let _ = decode_header(&file_data)?;

    let snapshot = &file_data[HEADER_SIZE..];

    // Validate cave range before writing — a bad range would slice-panic
    // or corrupt memory.
    let (cave_off, cave_len) = patch.cave_range(dos_base);
    let after = cave_off + cave_len;
    ensure!(
        after <= DOS_MEM_SIZE,
        "cave range {cave_off:#x}+{cave_len} exceeds 1 MB"
    );

    // Write around the cave — never overwrite the running code.
    // If the first write succeeds but the second fails, the game has
    // partial state.  This is unavoidable without a "undo" mechanism,
    // but the second write failing is extremely unlikely (same process,
    // contiguous address space, same permissions as the first write).
    mem.write_bytes(dos_base, &snapshot[..cave_off])
        .and_then(|()| mem.write_bytes(dos_base + after, &snapshot[after..]))
        .context("writing DOS memory for restore")?;

    log::info!("Loaded slot {slot} from {}", path.display());
    Ok(())
}

/// Read slot metadata from disk without loading the full snapshot.
pub fn read_slot_info(game_dir: &Path, slot: usize) -> Option<SlotInfo> {
    use std::io::Read as _;
    let path = slot_path(game_dir, slot);
    let mut f = std::fs::File::open(&path).ok()?;
    let mut header_buf = [0u8; HEADER_SIZE];
    f.read_exact(&mut header_buf).ok()?;
    let (timestamp, location_id, _x, _y, _z, leader_name) = decode_header(&header_buf).ok()?;
    let location = LocationType::from_id(location_id).name().to_string();
    Some(SlotInfo {
        timestamp,
        location_id,
        location,
        leader_name,
    })
}

/// Scan all slots and return their metadata (None for empty slots).
pub fn list_slots(game_dir: &Path) -> Vec<Option<SlotInfo>> {
    (0..NUM_SLOTS)
        .map(|slot| read_slot_info(game_dir, slot))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let ts: i64 = 1_700_000_000;
        let name = b"Shamino";
        let hdr = encode_header(ts, 2, 100, 50, 0, name);
        let (ts2, loc, x, y, z, name2) = decode_header(&hdr).unwrap();
        assert_eq!(ts2, ts);
        assert_eq!(loc, 2);
        assert_eq!(x, 100);
        assert_eq!(y, 50);
        assert_eq!(z, 0);
        assert_eq!(name2, "Shamino");
    }

    #[test]
    fn header_bad_magic() {
        let mut hdr = [0u8; HEADER_SIZE];
        hdr[0..4].copy_from_slice(b"XXXX");
        assert!(decode_header(&hdr).is_err());
    }

    #[test]
    fn header_bad_version() {
        let mut hdr = encode_header(0, 0, 0, 0, 0, b"");
        hdr[4] = 99;
        assert!(decode_header(&hdr).is_err());
    }

    #[test]
    fn extract_metadata_reads_correct_offsets() {
        let mut snapshot = vec![0u8; DOS_MEM_SIZE];
        snapshot[SAVE_BASE + MAP_LOCATION] = 5; // Minoc
        snapshot[SAVE_BASE + MAP_X] = 42;
        snapshot[SAVE_BASE + MAP_Y] = 99;
        snapshot[SAVE_BASE + MAP_Z] = 1;

        let name_start = SAVE_BASE + CHAR_RECORDS_OFFSET + CHAR_NAME;
        snapshot[name_start..name_start + 4].copy_from_slice(b"Iolo");

        let (loc, x, y, z, name) = extract_metadata(&snapshot);
        assert_eq!(loc, 5);
        assert_eq!(x, 42);
        assert_eq!(y, 99);
        assert_eq!(z, 1);
        assert_eq!(&name[..4], b"Iolo");
        assert_eq!(name[4], 0);
    }
}
