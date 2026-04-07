//! In-memory code patch for Ultima V's `get_command` function.
//!
//! Injects a small code cave (trampoline) at the entry of `get_command`
//! that checks a dirty-flag byte.  When the flag is set (by
//! [`trigger_redraw`]), the cave calls the game's `redraw_full_stats`
//! function and clears the flag before proceeding to the real function.
//!
//! This fires on every input poll — regardless of which overlay is
//! loaded — because `get_command` is in the resident code segment.
//!
//! See `docs/redraw-mechanism.md` for the full design.

use anyhow::{Context, Result, bail, ensure};

use crate::game::offsets::SAVE_BASE;
use crate::memory::access::MemoryAccess;

// ---------------------------------------------------------------------------
// Signatures and offsets (all CS-relative unless noted)
// ---------------------------------------------------------------------------

/// First 7 bytes of `redraw_full_stats` at CS:0x2900.
const REDRAW_SIGNATURE: [u8; 7] = [0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x02, 0x56];
const REDRAW_OFFSET: usize = 0x2900;

/// The overlay enters `get_command` 32 bytes past the prologue (the
/// overlay segment is 2 paragraphs above CS).  The first instruction
/// at that entry point is a 5-byte CMP which we replace with a 3-byte
/// JMP + 2 NOPs.
const HOOK_BYTES: [u8; 5] = [0x80, 0x3E, 0x93, 0x58, 0x21]; // CMP byte [0x5893], 0x21
const HOOK_OFFSET: usize = 0x268C;
/// Where to resume after the displaced CMP instruction.
const HOOK_RESUME: usize = 0x2691; // JB 0x269A (the instruction after the CMP)

/// First 7 bytes at the main loop top CS:0x00B8 (cross-validation).
const LOOP_TOP_SIGNATURE: [u8; 7] = [0xC7, 0x46, 0xFE, 0x00, 0x00, 0x80, 0x3E];
const LOOP_TOP_OFFSET: usize = 0x00B8;

/// CS:0x0174 must contain a near JMP opcode (0xE9) for cross-validation.
const LOOP_JMP_OFFSET: usize = 0x0174;

/// Save-relative offset for our dirty flag byte.  Past the end of the
/// on-disk save file, in the runtime-only RAM region.
const FLAG_SAVE_OFFSET: usize = 0x3C0;

/// Conversion factor: `DS_offset = save_offset + DS_SAVE_DELTA`.
const DS_SAVE_DELTA: u16 = 0x55A6;

/// Size of the injected code cave in bytes.
/// Layout: flag check (15) + displaced CMP (5) + JMP back (3) = 23
const CAVE_SIZE: usize = 23;

/// Minimum contiguous zero-byte run to accept as a code cave.
const MIN_CAVE_RUN: usize = CAVE_SIZE + 2; // +2 padding

/// Range within the code segment to search for a code cave.
const CAVE_SCAN_START: usize = 0x4000;
const CAVE_SCAN_END: usize = 0x8000;

/// How much DOS memory to scan for the redraw signature.
const SIG_SCAN_SIZE: usize = 0x10_0000; // 1 MB

// ---------------------------------------------------------------------------
// Patch state
// ---------------------------------------------------------------------------

/// Everything needed to undo the patch and to trigger redraws.
#[derive(Debug)]
pub struct PatchState {
    /// Absolute host address of CS:0x0000 in DOSBox memory.
    cs_base: usize,
    /// CS-relative offset where the code cave was placed.
    cave_cs_offset: usize,
    /// Original 5 bytes from the hook site.
    original_hook: [u8; 5],
    /// Original bytes from the code cave location.
    original_cave: [u8; CAVE_SIZE],
    /// Absolute host address of the dirty flag byte.
    flag_addr: usize,
}

// ---------------------------------------------------------------------------
// CS-base discovery
// ---------------------------------------------------------------------------

/// Scan emulated DOS memory for the `redraw_full_stats` signature and
/// cross-validate with the loop-top signature and JMP opcode.
fn find_cs_base(mem: &dyn MemoryAccess, dos_base: usize) -> Result<usize> {
    let mut buf = vec![0u8; SIG_SCAN_SIZE];
    mem.read_bytes(dos_base, &mut buf)
        .context("reading DOS memory for CS scan")?;

    let sig_len = REDRAW_SIGNATURE.len();
    for i in 0..buf.len().saturating_sub(sig_len) {
        if buf[i..i + sig_len] != REDRAW_SIGNATURE {
            continue;
        }
        if i < REDRAW_OFFSET {
            continue;
        }
        let candidate = dos_base + i - REDRAW_OFFSET;

        // Cross-validate: near JMP opcode at CS:0x0174.
        let jmp_opcode = match mem.read_u8(candidate + LOOP_JMP_OFFSET) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if jmp_opcode != 0xE9 {
            continue;
        }

        // Cross-validate: loop top bytes at CS:0x00B8.
        let mut top_buf = [0u8; 7];
        if mem
            .read_bytes(candidate + LOOP_TOP_OFFSET, &mut top_buf)
            .is_err()
        {
            continue;
        }
        if top_buf != LOOP_TOP_SIGNATURE {
            continue;
        }

        return Ok(candidate);
    }

    bail!(
        "code segment signature not found — is Ultima V loaded? \
         (scanned {SIG_SCAN_SIZE} bytes from dos_base {dos_base:#x})"
    );
}

// ---------------------------------------------------------------------------
// Code cave discovery
// ---------------------------------------------------------------------------

/// Find a contiguous run of zero bytes suitable for our stub.
fn find_code_cave(mem: &dyn MemoryAccess, cs_base: usize) -> Result<usize> {
    let scan_len = CAVE_SCAN_END - CAVE_SCAN_START;
    let mut buf = vec![0u8; scan_len];
    mem.read_bytes(cs_base + CAVE_SCAN_START, &mut buf)
        .context("reading code segment for cave scan")?;

    let mut run_start = 0usize;
    let mut run_len = 0usize;

    for (i, &byte) in buf.iter().enumerate() {
        if byte == 0x00 {
            if run_len == 0 {
                run_start = i;
            }
            run_len += 1;
            if run_len >= MIN_CAVE_RUN {
                return Ok(CAVE_SCAN_START + run_start + 1);
            }
        } else {
            run_len = 0;
        }
    }

    bail!(
        "no suitable code cave found in CS:{CAVE_SCAN_START:#06x}..{CAVE_SCAN_END:#06x} \
         (need {MIN_CAVE_RUN} contiguous zero bytes)"
    );
}

// ---------------------------------------------------------------------------
// Cave encoding
// ---------------------------------------------------------------------------

/// Build the 23-byte code cave stub.  Pure function — no I/O.
///
/// ```text
///  0: 80 3E xx xx 00       CMP byte [DS:flag], 0
///  5: 74 08                 JE  skip  (→ byte 15)
///  7: C6 06 xx xx 00       MOV byte [DS:flag], 0
/// 12: E8 xx xx              CALL 0x2900
/// 15: 80 3E 93 58 21       CMP byte [0x5893], 0x21  (displaced)
/// 20: E9 xx xx              JMP  0x2691
/// ```
fn encode_cave(cave_cs_offset: usize, flag_ds_offset: u16) -> [u8; CAVE_SIZE] {
    let flag_lo = (flag_ds_offset & 0xFF) as u8;
    let flag_hi = (flag_ds_offset >> 8) as u8;

    let call_next = cave_cs_offset + 15;
    let call_disp = (REDRAW_OFFSET as i32 - call_next as i32) as i16;

    let jmp_next = cave_cs_offset + 23;
    let jmp_disp = (HOOK_RESUME as i32 - jmp_next as i32) as i16;

    let cd = call_disp.to_le_bytes();
    let jd = jmp_disp.to_le_bytes();

    [
        0x80, 0x3E, flag_lo, flag_hi, 0x00, // CMP byte [DS:flag], 0
        0x74, 0x08, // JE skip (+8 → byte 15)
        0xC6, 0x06, flag_lo, flag_hi, 0x00, // MOV byte [DS:flag], 0
        0xE8, cd[0], cd[1], // CALL redraw_full_stats
        0x80, 0x3E, 0x93, 0x58, 0x21, // CMP byte [0x5893], 0x21 (displaced)
        0xE9, jd[0], jd[1], // JMP hook_resume
    ]
}

// ---------------------------------------------------------------------------
// Patch application
// ---------------------------------------------------------------------------

/// Apply the code-cave patch to the running game.
pub fn apply_patch(mem: &dyn MemoryAccess, dos_base: usize) -> Result<PatchState> {
    // 1. Find the code segment.
    let cs_base = find_cs_base(mem, dos_base)?;
    log::debug!("CS base found at {cs_base:#x} (dos_base={dos_base:#x})");

    // 2. Read current hook bytes (the 5-byte CMP at the overlay entry).
    let hook_addr = cs_base + HOOK_OFFSET;
    let mut current_hook = [0u8; 5];
    mem.read_bytes(hook_addr, &mut current_hook)
        .context("reading hook bytes")?;
    log::debug!("Hook site at CS:{HOOK_OFFSET:#06x} = {current_hook:02X?}");

    // 3. Find a code cave.
    let cave_cs_offset = find_code_cave(mem, cs_base)?;
    let cave_addr = cs_base + cave_cs_offset;
    log::debug!("Code cave at CS:{cave_cs_offset:#06x} (abs {cave_addr:#x})");

    // 4. Idempotency: if hook site already starts with JMP, adopt.
    if current_hook[0] == 0xE9 {
        let disp = i16::from_le_bytes([current_hook[1], current_hook[2]]);
        let target = (HOOK_OFFSET as i32 + 3 + disp as i32) as usize;
        if (CAVE_SCAN_START..CAVE_SCAN_END).contains(&target) {
            let mut probe = [0u8; 5];
            if mem.read_bytes(cs_base + target, &mut probe).is_ok()
                && probe[0] == 0x80
                && probe[1] == 0x3E
                && probe[4] == 0x00
            {
                let flag_ds = u16::from_le_bytes([probe[2], probe[3]]);
                let flag_save = flag_ds.wrapping_sub(DS_SAVE_DELTA) as usize;
                let flag_addr = dos_base + SAVE_BASE + flag_save;
                log::info!(
                    "Adopting existing patch at CS:{target:#06x} \
                     (flag DS:{flag_ds:#06x} = save+{flag_save:#x})"
                );
                return Ok(PatchState {
                    cs_base,
                    cave_cs_offset: target,
                    original_hook: current_hook, // already a JMP
                    original_cave: [0; CAVE_SIZE],
                    flag_addr,
                });
            }
        }
    }

    // Not yet patched — verify hook bytes match expected.
    ensure!(
        current_hook == HOOK_BYTES,
        "hook site at CS:{HOOK_OFFSET:#06x} is {current_hook:02X?}, expected {HOOK_BYTES:02X?} — \
         already patched or unsupported game version"
    );

    // 5. Compute flag address.
    let flag_addr = dos_base + SAVE_BASE + FLAG_SAVE_OFFSET;
    let flag_ds_offset = FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    log::debug!("Flag: save+{FLAG_SAVE_OFFSET:#x} = DS:{flag_ds_offset:#06x} (abs {flag_addr:#x})");

    // 6. Verify flag byte is clean.
    let flag_current = mem.read_u8(flag_addr).context("reading flag byte")?;
    ensure!(
        flag_current == 0,
        "flag byte at save+{FLAG_SAVE_OFFSET:#x} is {flag_current:#x}, not zero"
    );

    // 7. Save original cave bytes.
    let mut original_cave = [0u8; CAVE_SIZE];
    mem.read_bytes(cave_addr, &mut original_cave)
        .context("reading original cave bytes")?;

    // 8. Write the code cave.
    let cave_bytes = encode_cave(cave_cs_offset, flag_ds_offset);
    log::debug!("Cave bytes: {cave_bytes:02X?}");
    mem.write_bytes(cave_addr, &cave_bytes)
        .context("writing code cave")?;

    // 9. Verify cave write.
    let mut readback = [0u8; CAVE_SIZE];
    mem.read_bytes(cave_addr, &mut readback)
        .context("verifying cave write")?;
    if readback != cave_bytes {
        let _ = mem.write_bytes(cave_addr, &original_cave);
        bail!("cave verification failed");
    }

    // 10. Patch hook site → JMP cave + 2 NOPs (replacing 5-byte CMP).
    let hook_disp = (cave_cs_offset as i32 - (HOOK_OFFSET as i32 + 3)) as i16;
    let hd = hook_disp.to_le_bytes();
    let patched_hook: [u8; 5] = [0xE9, hd[0], hd[1], 0x90, 0x90];
    log::debug!("Hooking CS:{HOOK_OFFSET:#06x}: {HOOK_BYTES:02X?} -> {patched_hook:02X?}");
    mem.write_bytes(hook_addr, &patched_hook)
        .context("writing hook")?;

    // 11. Verify hook write.
    let mut hook_rb = [0u8; 5];
    mem.read_bytes(hook_addr, &mut hook_rb)
        .context("verifying hook write")?;
    if hook_rb != patched_hook {
        let _ = mem.write_bytes(hook_addr, &HOOK_BYTES);
        let _ = mem.write_bytes(cave_addr, &original_cave);
        bail!("hook verification failed");
    }
    log::debug!("Patch is live");

    Ok(PatchState {
        cs_base,
        cave_cs_offset,
        original_hook: HOOK_BYTES,
        original_cave,
        flag_addr,
    })
}

// ---------------------------------------------------------------------------
// Patch removal
// ---------------------------------------------------------------------------

/// Restore the original bytes.  Errors swallowed (process may be dead).
pub fn remove_patch(mem: &dyn MemoryAccess, state: &PatchState) {
    log::debug!("Removing patch");
    let hook_addr = state.cs_base + HOOK_OFFSET;
    let _ = mem.write_bytes(hook_addr, &state.original_hook);
    let cave_addr = state.cs_base + state.cave_cs_offset;
    let _ = mem.write_bytes(cave_addr, &state.original_cave);
    let _ = mem.write_u8(state.flag_addr, 0);
}

// ---------------------------------------------------------------------------
// Redraw trigger
// ---------------------------------------------------------------------------

/// Set the dirty flag so the next `get_command` call redraws stats.
pub fn trigger_redraw(mem: &dyn MemoryAccess, state: &PatchState) -> Result<()> {
    log::trace!("Setting redraw flag at {:#x}", state.flag_addr);
    mem.write_u8(state.flag_addr, 1)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::access::MockMemory;

    fn setup_mock() -> (MockMemory, usize) {
        let dos_base: usize = 0;
        let cs_base: usize = 0x20000;
        let total_size = SIG_SCAN_SIZE + 0x1000;
        let mem = MockMemory::new(total_size);

        mem.set_bytes(cs_base + REDRAW_OFFSET, &REDRAW_SIGNATURE);
        mem.set_bytes(cs_base + LOOP_JMP_OFFSET, &[0xE9, 0x41, 0xFF]);
        mem.set_bytes(cs_base + LOOP_TOP_OFFSET, &LOOP_TOP_SIGNATURE);
        mem.set_bytes(cs_base + HOOK_OFFSET, &HOOK_BYTES);

        (mem, dos_base)
    }

    #[test]
    fn encode_cave_correct() {
        let cave = 0x5001usize;
        let flag_ds = FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
        let bytes = encode_cave(cave, flag_ds);

        assert_eq!(bytes[0], 0x80);
        assert_eq!(bytes[5], 0x74);
        assert_eq!(bytes[6], 0x08);
        // displaced CMP byte [0x5893], 0x21
        assert_eq!(&bytes[15..20], &HOOK_BYTES);

        // CALL targets 0x2900
        let cd = i16::from_le_bytes([bytes[13], bytes[14]]);
        assert_eq!((cave as i32 + 15 + cd as i32) as usize, REDRAW_OFFSET);

        // JMP targets HOOK_RESUME (0x2691)
        let jd = i16::from_le_bytes([bytes[21], bytes[22]]);
        assert_eq!((cave as i32 + 23 + jd as i32) as usize, HOOK_RESUME);
    }

    #[test]
    fn encode_cave_various_offsets() {
        for cave in [0x4001, 0x5000, 0x76C1, 0x7F00] {
            let flag_ds = FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
            let bytes = encode_cave(cave, flag_ds);

            let cd = i16::from_le_bytes([bytes[13], bytes[14]]);
            assert_eq!((cave as i32 + 15 + cd as i32) as usize, REDRAW_OFFSET);

            let jd = i16::from_le_bytes([bytes[21], bytes[22]]);
            assert_eq!((cave as i32 + 23 + jd as i32) as usize, HOOK_RESUME);
        }
    }

    #[test]
    fn apply_succeeds() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).expect("should succeed");
        assert_eq!(state.flag_addr, dos_base + SAVE_BASE + FLAG_SAVE_OFFSET);

        // Hook site should now start with JMP.
        let mut hook = [0u8; 5];
        mem.read_bytes(state.cs_base + HOOK_OFFSET, &mut hook)
            .unwrap();
        assert_eq!(hook[0], 0xE9);
        assert_eq!(hook[3], 0x90); // NOP
        assert_eq!(hook[4], 0x90); // NOP
    }

    #[test]
    fn apply_idempotent() {
        let (mem, dos_base) = setup_mock();
        let s1 = apply_patch(&mem, dos_base).expect("first");
        let s2 = apply_patch(&mem, dos_base).expect("second (adopt)");
        assert_eq!(s1.flag_addr, s2.flag_addr);
        assert_eq!(s1.cave_cs_offset, s2.cave_cs_offset);
    }

    #[test]
    fn remove_restores() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).unwrap();
        remove_patch(&mem, &state);

        let mut hook = [0u8; 5];
        mem.read_bytes(state.cs_base + HOOK_OFFSET, &mut hook)
            .unwrap();
        assert_eq!(hook, HOOK_BYTES);
    }

    #[test]
    fn trigger_sets_flag() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).unwrap();
        assert_eq!(mem.read_u8(state.flag_addr).unwrap(), 0);
        trigger_redraw(&mem, &state).unwrap();
        assert_eq!(mem.read_u8(state.flag_addr).unwrap(), 1);
    }

    #[test]
    fn no_signature_fails() {
        let mem = MockMemory::new(SIG_SCAN_SIZE);
        assert!(find_cs_base(&mem, 0).is_err());
    }

    #[test]
    fn apply_remove_reapply() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).unwrap();
        remove_patch(&mem, &state);
        apply_patch(&mem, dos_base).expect("re-apply should work");
    }
}
