use anyhow::Result;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualQueryEx,
};

use crate::game::offsets::SAVE_BASE;

use super::access::{MemoryAccess, Win32ProcessMemory};

pub struct ScanResult {
    pub dos_base: usize,
    pub game_confirmed: bool,
}

#[derive(Clone, Copy)]
struct RegionCandidate {
    base: usize,
    size: usize,
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
/// DOSBox variants can place sizable headers before the emulated DOS memory.
const MAX_IVT_SEARCH_OFFSET: usize = 0x10000;
/// Brute-force save-data scans only need to cover the largest supported DOS region.
const MAX_GAME_DATA_SCAN_SIZE: usize = 0x2001000;
const GAME_DATA_VALIDATION_SIZE: usize = crate::game::offsets::INV_PARTY_SIZE + 1;

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
                sized_candidates.push(RegionCandidate { base, size });
            } else if size >= 0x100000 {
                large_candidates.push(RegionCandidate { base, size });
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

    let mut first_unconfirmed = find_best_region_result(mem, candidates);
    if let Some(result) = first_unconfirmed.take_if(|result| result.game_confirmed) {
        return Ok(result);
    }

    // If sized candidates failed to confirm the game, also try large ones.
    if !sized_candidates.is_empty()
        && let Some(result) = find_best_region_result(mem, &large_candidates)
    {
        if result.game_confirmed {
            return Ok(result);
        }
        if first_unconfirmed.is_none() {
            first_unconfirmed = Some(result);
        }
    }

    if let Some(result) = first_unconfirmed {
        return Ok(result);
    }

    anyhow::bail!("could not find DOS memory base in DOSBox process")
}

fn find_best_region_result(
    mem: &dyn MemoryAccess,
    region_candidates: &[RegionCandidate],
) -> Option<ScanResult> {
    let mut first_unconfirmed = None;

    for candidate in region_candidates {
        let Some(result) = probe_region(mem, candidate.base, candidate.size) else {
            continue;
        };
        if result.game_confirmed {
            return Some(result);
        }
        if first_unconfirmed.is_none() {
            first_unconfirmed = Some(result);
        }
    }

    first_unconfirmed
}

/// Probe a single memory region for the DOS IVT at various offsets.
/// DOSBox may place internal headers before the actual DOS address space.
fn probe_region(
    mem: &dyn MemoryAccess,
    region_base: usize,
    region_size: usize,
) -> Option<ScanResult> {
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
    // DOSBox-X can place the live DOS base well past the initial IVT search
    // window, so fall back to scanning the region for a confirmed save layout.
    if let Some(dos_base) = find_game_data_base_in_region(mem, region_base, region_size) {
        return Some(ScanResult {
            dos_base,
            game_confirmed: true,
        });
    }

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
    let valid_gender = |b: u8| matches!(b, 0x0B | 0x0C);
    let valid_class = |b: u8| matches!(b, b'A' | b'B' | b'F' | b'M');
    let valid_status = |b: u8| matches!(b, b'G' | b'P' | b'S' | b'D');

    let char_base = game_data_offset + 0x02;
    let mut has_named_party_member = false;

    for index in 0..4 {
        let base = char_base + index * 0x20;
        if base + 0x0B >= data.len() {
            return false;
        }
        if !valid_gender(data[base + 0x09])
            || !valid_class(data[base + 0x0A])
            || !valid_status(data[base + 0x0B])
        {
            return false;
        }

        let name = &data[base..base + 9];
        let null_pos = name.iter().position(|&b| b == 0).unwrap_or(9);
        if null_pos > 0 {
            if !name[..null_pos].iter().all(|&b| (0x20..=0x7E).contains(&b)) {
                return false;
            }
            has_named_party_member = true;
        }
    }

    if !has_named_party_member {
        return false;
    }

    let party_size_offset = game_data_offset + crate::game::offsets::INV_PARTY_SIZE;
    if party_size_offset >= data.len() {
        return false;
    }

    (1..=6).contains(&data[party_size_offset])
}

fn validate_game_data_from_mem(mem: &dyn MemoryAccess, dos_base: usize) -> bool {
    let start = dos_base + SAVE_BASE;
    let mut buf = vec![0u8; GAME_DATA_VALIDATION_SIZE];
    if mem.read_bytes(start, &mut buf).is_err() {
        return false;
    }
    validate_game_data(&buf, 0)
}

fn find_game_data_base_in_region(
    mem: &dyn MemoryAccess,
    region_base: usize,
    region_size: usize,
) -> Option<usize> {
    let search_size = region_size.min(MAX_GAME_DATA_SCAN_SIZE);
    if search_size <= SAVE_BASE + GAME_DATA_VALIDATION_SIZE {
        return None;
    }

    let mut buf = vec![0u8; search_size];
    mem.read_bytes(region_base, &mut buf).ok()?;

    let max_offset = search_size - SAVE_BASE - GAME_DATA_VALIDATION_SIZE;
    for dos_offset in (0..=max_offset).step_by(0x10) {
        if validate_game_data(&buf[dos_offset + SAVE_BASE..], 0) {
            return Some(region_base + dos_offset);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::offsets::SAVE_BASE;
    use crate::memory::access::MockMemory;

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

    fn plant_character(
        buf: &mut [u8],
        record_base: usize,
        name: &[u8],
        gender: u8,
        class: u8,
        status: u8,
    ) {
        let mut name_buf = [0u8; 9];
        name_buf[..name.len()].copy_from_slice(name);
        buf[record_base..record_base + 9].copy_from_slice(&name_buf);
        buf[record_base + 0x09] = gender;
        buf[record_base + 0x0A] = class;
        buf[record_base + 0x0B] = status;
    }

    fn plant_game_data(buf: &mut [u8], offset: usize) {
        plant_character(buf, offset + 0x02, b"Avatar", 0x0B, b'A', b'G');
        plant_character(buf, offset + 0x22, b"Shamino", 0x0B, b'F', b'G');
        plant_character(buf, offset + 0x42, b"Iolo", 0x0B, b'B', b'G');
        plant_character(buf, offset + 0x62, b"Jaana", 0x0C, b'M', b'G');
        buf[offset + crate::game::offsets::INV_PARTY_SIZE] = 4;
    }

    fn seed_probe_region(mem: &MockMemory, region_base: usize, confirmed: bool) {
        let mut region = vec![0u8; MAX_IVT_SEARCH_OFFSET + 0x500];
        for i in 0..20 {
            let offset = i * 4;
            region[offset + 2] = 0x00;
            region[offset + 3] = 0xF0;
        }
        region[0x400] = 0x78;
        region[0x401] = 0x03;
        mem.set_bytes(region_base, &region);

        if confirmed {
            let mut game_data = vec![0u8; 0x300];
            plant_game_data(&mut game_data, 0);
            mem.set_bytes(region_base + SAVE_BASE, &game_data);
        }
    }

    fn seed_probe_region_at_offset(
        mem: &MockMemory,
        region_base: usize,
        ivt_offset: usize,
        confirmed: bool,
    ) {
        let mut region = vec![0u8; MAX_IVT_SEARCH_OFFSET + 0x500];
        for i in 0..20 {
            let offset = ivt_offset + i * 4;
            region[offset + 2] = 0x00;
            region[offset + 3] = 0xF0;
        }
        region[ivt_offset + 0x400] = 0x78;
        region[ivt_offset + 0x401] = 0x03;
        mem.set_bytes(region_base, &region);

        if confirmed {
            let mut game_data = vec![0u8; 0x300];
            plant_game_data(&mut game_data, 0);
            mem.set_bytes(region_base + ivt_offset + SAVE_BASE, &game_data);
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
        let mut buf = vec![0u8; 0x300];
        plant_game_data(&mut buf, 0);
        assert!(validate_game_data(&buf, 0));
    }

    #[test]
    fn invalid_status_byte_fails() {
        let mut buf = vec![0u8; 0x300];
        plant_game_data(&mut buf, 0);
        buf[0x02 + 0x0B] = b'X';
        assert!(!validate_game_data(&buf, 0));
    }

    #[test]
    fn all_names_empty_fails() {
        let mut buf = vec![0u8; 0x300];
        plant_game_data(&mut buf, 0);
        for index in 0..4 {
            let base = 0x02 + index * 0x20;
            buf[base..base + 9].fill(0);
        }
        assert!(!validate_game_data(&buf, 0));
    }

    #[test]
    fn valid_ivt_but_no_game_data() {
        let buf = make_valid_ivt();
        assert!(validate_ivt(&buf[..0x400]));
        let game_buf = vec![0u8; 0x300];
        assert!(!validate_game_data(&game_buf, 0));
    }

    #[test]
    fn prefers_confirmed_region_over_earlier_unconfirmed_region() {
        let region_a = 0x1000;
        let region_b = 0x60000;
        let mem = MockMemory::new(region_b + SAVE_BASE + 0x1000);
        seed_probe_region(&mem, region_a, false);
        seed_probe_region(&mem, region_b, true);

        let result = find_best_region_result(
            &mem,
            &[
                RegionCandidate {
                    base: region_a,
                    size: 0x1001000,
                },
                RegionCandidate {
                    base: region_b,
                    size: 0x1001000,
                },
            ],
        )
        .unwrap();

        assert_eq!(result.dos_base, region_b);
        assert!(result.game_confirmed);
    }

    #[test]
    fn returns_unconfirmed_region_when_no_confirmed_region_exists() {
        let region_a = 0x1000;
        let region_b = 0x60000;
        let mem = MockMemory::new(region_b + SAVE_BASE + 0x1000);
        seed_probe_region(&mem, region_a, false);
        seed_probe_region(&mem, region_b, false);

        let result = find_best_region_result(
            &mem,
            &[
                RegionCandidate {
                    base: region_a,
                    size: 0x1001000,
                },
                RegionCandidate {
                    base: region_b,
                    size: 0x1001000,
                },
            ],
        )
        .unwrap();

        assert_eq!(result.dos_base, region_a);
        assert!(!result.game_confirmed);
    }

    #[test]
    fn probe_region_finds_confirmed_game_after_large_header_offset() {
        let region_base = 0x1000;
        let ivt_offset = 0x6c60;
        let mem = MockMemory::new(region_base + ivt_offset + SAVE_BASE + 0x1000);
        seed_probe_region_at_offset(&mem, region_base, ivt_offset, true);

        let result = probe_region(&mem, region_base, 0x1001000).unwrap();

        assert_eq!(result.dos_base, region_base + ivt_offset);
        assert!(result.game_confirmed);
    }

    #[test]
    fn find_game_data_base_in_region_finds_confirmed_save_without_ivt_match() {
        let region_base = 0x1000;
        let game_data_offset = 0x6c60;
        let region_size = game_data_offset + SAVE_BASE + 0x300;
        let mem = MockMemory::new(region_base + region_size);
        let mut game_data = vec![0u8; 0x300];
        plant_game_data(&mut game_data, 0);
        mem.set_bytes(region_base + game_data_offset + SAVE_BASE, &game_data);

        let dos_base = find_game_data_base_in_region(&mem, region_base, region_size).unwrap();

        assert_eq!(dos_base, region_base + game_data_offset);
    }
}
