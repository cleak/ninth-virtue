use anyhow::Result;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualQueryEx,
};

use super::access::{MemoryAccess, Win32ProcessMemory};

pub struct ScanResult {
    pub dos_base: usize,
    pub game_confirmed: bool,
}

/// Known DOSBox memory region sizes (region + guard page).
const KNOWN_SIZES: &[usize] = &[
    0x0401000, // 4 MB
    0x0801000, // 8 MB
    0x1001000, // 16 MB (default)
    0x2001000, // 32 MB
];

/// Also accept sizes without a guard page.
const KNOWN_SIZES_NO_GUARD: &[usize] = &[
    0x0400000, // 4 MB
    0x0800000, // 8 MB
    0x1000000, // 16 MB
    0x2000000, // 32 MB
];

const IVT_F000_THRESHOLD: usize = 15;

/// Maximum offset from region base to search for the DOS IVT.
/// DOSBox may place a small header before the emulated DOS memory.
const MAX_IVT_SEARCH_OFFSET: usize = 0x200;

/// Scan the target process for the DOS memory base address.
pub fn find_dos_base(mem: &Win32ProcessMemory) -> Result<ScanResult> {
    let handle = mem.handle();

    // Collect all committed RW regions
    let mut sized_candidates = Vec::new();
    let mut large_candidates = Vec::new();
    let mut addr: usize = 0;
    loop {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let ret = unsafe {
            VirtualQueryEx(
                handle,
                Some(addr as *const std::ffi::c_void),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if ret == 0 {
            break;
        }

        let base = info.BaseAddress as usize;
        let size = info.RegionSize;

        if info.State == MEM_COMMIT
            && (info.Protect == PAGE_READWRITE || info.Protect == PAGE_EXECUTE_READWRITE)
        {
            if KNOWN_SIZES.contains(&size) || KNOWN_SIZES_NO_GUARD.contains(&size) {
                sized_candidates.push(base);
            } else if size >= 0x100000 {
                large_candidates.push(base);
            }
        }

        addr = base.checked_add(size).unwrap_or(0);
        if addr == 0 {
            break;
        }
    }

    // Try sized candidates first, then fall back to large candidates
    let candidates = if sized_candidates.is_empty() {
        &large_candidates
    } else {
        &sized_candidates
    };

    for &region_base in candidates {
        if let Some(result) = probe_region(mem, region_base) {
            return Ok(result);
        }
    }

    // If sized candidates failed, also try large ones
    if !sized_candidates.is_empty() {
        for &region_base in &large_candidates {
            if let Some(result) = probe_region(mem, region_base) {
                return Ok(result);
            }
        }
    }

    anyhow::bail!("could not find DOS memory base in DOSBox process")
}

/// Probe a single memory region for the DOS IVT at various offsets.
/// DOSBox may place internal headers before the actual DOS address space.
fn probe_region(mem: &dyn MemoryAccess, region_base: usize) -> Option<ScanResult> {
    // Read enough to scan for IVT within the search range
    let read_size = MAX_IVT_SEARCH_OFFSET + 0x500;
    let mut buf = vec![0u8; read_size];
    mem.read_bytes(region_base, &mut buf).ok()?;

    // Collect all offsets where IVT looks valid, sorted by F000 count descending
    let mut ivt_hits: Vec<(usize, usize)> = Vec::new(); // (offset, f000_count)

    for offset in (0..=MAX_IVT_SEARCH_OFFSET).step_by(0x10) {
        if offset + 0x400 > buf.len() {
            break;
        }
        let ivt = &buf[offset..offset + 0x400];
        let f000_count = count_f000_segments(ivt);
        if f000_count >= IVT_F000_THRESHOLD {
            ivt_hits.push((offset, f000_count));
        }
    }

    ivt_hits.sort_by(|a, b| b.1.cmp(&a.1));

    // Try each candidate offset — prefer the one where game data validates
    let mut best_unconfirmed: Option<(usize, usize)> = None;

    for &(offset, f000_count) in &ivt_hits {
        let dos_base = region_base + offset;
        if validate_game_data_from_mem(mem, dos_base) {
            return Some(ScanResult {
                dos_base,
                game_confirmed: true,
            });
        }
        if best_unconfirmed.is_none() || f000_count > best_unconfirmed.unwrap().1 {
            best_unconfirmed = Some((offset, f000_count));
        }
    }

    // No game data found — return best IVT match as unconfirmed
    let (offset, _) = best_unconfirmed?;
    Some(ScanResult {
        dos_base: region_base + offset,
        game_confirmed: false,
    })
}

/// Count IVT entries with segment == 0xF000 (ROM BIOS).
fn count_f000_segments(ivt: &[u8]) -> usize {
    let entries = ivt.len() / 4;
    let mut count = 0;
    for i in 0..entries {
        let offset = i * 4;
        let segment = u16::from_le_bytes([ivt[offset + 2], ivt[offset + 3]]);
        if segment == 0xF000 {
            count += 1;
        }
    }
    count
}

/// Validate the Interrupt Vector Table by counting entries with segment == 0xF000 (ROM BIOS).
#[cfg(test)]
fn validate_ivt(ivt: &[u8]) -> bool {
    if ivt.len() < 0x400 {
        return false;
    }
    count_f000_segments(&ivt[..0x400]) >= IVT_F000_THRESHOLD
}

/// Validate that Ultima V game data is present at the expected save-data offsets.
/// `data` should start at the SAVE_BASE address; `game_data_offset` is typically 0.
pub(crate) fn validate_game_data(data: &[u8], game_data_offset: usize) -> bool {
    let valid_status = |b: u8| matches!(b, b'G' | b'P' | b'S' | b'D');

    // Check status bytes for first 4 character slots
    let char_base = game_data_offset + 0x02;
    let status_offsets = [
        char_base + 0x0B,
        char_base + 0x20 + 0x0B,
        char_base + 0x40 + 0x0B,
        char_base + 0x60 + 0x0B,
    ];

    for &off in &status_offsets {
        if off >= data.len() {
            return false;
        }
        if !valid_status(data[off]) {
            return false;
        }
    }

    // Check first character name is printable ASCII
    let name_start = game_data_offset + 0x02;
    if name_start + 9 > data.len() {
        return false;
    }
    let name = &data[name_start..name_start + 9];
    let null_pos = name.iter().position(|&b| b == 0).unwrap_or(9);
    if null_pos == 0 {
        return false;
    }
    name[..null_pos].iter().all(|&b| (0x20..=0x7E).contains(&b))
}

fn validate_game_data_from_mem(mem: &dyn MemoryAccess, dos_base: usize) -> bool {
    use crate::game::offsets::SAVE_BASE;

    let start = dos_base + SAVE_BASE;
    let size = 0x02 + 4 * 0x20; // chars offset + 4 records
    let mut buf = vec![0u8; size];
    if mem.read_bytes(start, &mut buf).is_err() {
        return false;
    }
    validate_game_data(&buf, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_valid_ivt() -> Vec<u8> {
        let mut buf = vec![0u8; 0x500];
        // Plant 20 entries with segment 0xF000
        for i in 0..20 {
            let offset = i * 4;
            buf[offset + 2] = 0x00;
            buf[offset + 3] = 0xF0;
        }
        // BDA: some non-zero data
        buf[0x400] = 0x78;
        buf[0x401] = 0x03;
        buf
    }

    fn plant_game_data(buf: &mut [u8], offset: usize) {
        let name = b"Avatar\0\0\0";
        buf[offset + 0x02..offset + 0x02 + 9].copy_from_slice(name);
        buf[offset + 0x02 + 0x0B] = b'G';

        for i in 1..4 {
            let base = offset + 0x02 + i * 0x20;
            buf[base] = b'A';
            buf[base + 1] = 0;
            buf[base + 0x0B] = b'G';
        }
    }

    #[test]
    fn valid_ivt_passes() {
        let buf = make_valid_ivt();
        assert!(validate_ivt(&buf[..0x400]));
    }

    #[test]
    fn ivt_with_14_entries_fails() {
        let mut buf = vec![0u8; 0x400];
        for i in 0..14 {
            let offset = i * 4;
            buf[offset + 2] = 0x00;
            buf[offset + 3] = 0xF0;
        }
        assert!(!validate_ivt(&buf));
    }

    #[test]
    fn ivt_threshold_boundary() {
        let mut buf = vec![0u8; 0x400];
        for i in 0..15 {
            let offset = i * 4;
            buf[offset + 2] = 0x00;
            buf[offset + 3] = 0xF0;
        }
        assert!(validate_ivt(&buf));
    }

    #[test]
    fn zero_ivt_fails() {
        let buf = vec![0u8; 0x400];
        assert!(!validate_ivt(&buf));
    }

    #[test]
    fn random_pattern_ivt_fails() {
        let mut buf = vec![0u8; 0x400];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((i * 7 + 13) % 256) as u8;
        }
        assert!(!validate_ivt(&buf));
    }

    #[test]
    fn valid_game_data_passes() {
        let mut buf = vec![0u8; 0x100];
        plant_game_data(&mut buf, 0);
        assert!(validate_game_data(&buf, 0));
    }

    #[test]
    fn invalid_status_byte_fails() {
        let mut buf = vec![0u8; 0x100];
        plant_game_data(&mut buf, 0);
        buf[0x02 + 0x0B] = b'X';
        assert!(!validate_game_data(&buf, 0));
    }

    #[test]
    fn empty_name_fails() {
        let mut buf = vec![0u8; 0x100];
        plant_game_data(&mut buf, 0);
        buf[0x02] = 0;
        assert!(!validate_game_data(&buf, 0));
    }

    #[test]
    fn valid_ivt_but_no_game_data() {
        let buf = make_valid_ivt();
        assert!(validate_ivt(&buf[..0x400]));
        let game_buf = vec![0u8; 0x100];
        assert!(!validate_game_data(&game_buf, 0));
    }
}
