//! In-memory code patch for Ultima V's `get_command` and `putchar` functions.
//!
//! Injects a code cave into the game's dead initialization region
//! (CS:0x0000-0x00B7, which runs once at startup and is never
//! re-entered).  The cave has two entry points:
//!
//! - **Entry A** via `get_command` (CS:0x268C) — fires on every input
//!   poll for redraw + save-state trapping.
//! - **Entry B** via `putchar` (CS:0x16BA) — fires on every character
//!   output for save-state trapping during animations.
//!
//! # Serialization contract
//!
//! **All writes to DOSBox memory must be serialized through a single
//! thread.**  The cave, hooks, flags, and game data form an
//! interdependent system — concurrent writes from multiple threads
//! could leave them in an inconsistent state.  Currently all callers
//! run on the egui main thread; if save/load ever moves to a
//! background thread, access must be gated by a mutex or channel.
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
pub(crate) const HOOK_OFFSET: usize = 0x268C;
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

/// Save-relative offset for the trap flag byte (one past the dirty flag).
/// Used by the save-state system to deterministically pause execution.
///
/// Values: 0 = normal, 1 = trap requested, 2 = trapped (in spin loop).
const TRAP_FLAG_SAVE_OFFSET: usize = 0x3C1;

/// Save-relative offsets for the saved SP and BP registers (16-bit words).
/// The cave writes SP/BP here before entering the spin loop and restores
/// them after exiting.  On a normal save+release these round-trip to the
/// same values.  On a load the host overwrites the 1 MB (including these
/// words) with the saved state, so the cave restores the *saved* SP/BP,
/// making the call chain consistent with the restored memory — regardless
/// of which game mode was active at load time.
const SAVED_SP_SAVE_OFFSET: usize = 0x3C2; // u16 at 0x3C2..0x3C3
const SAVED_BP_SAVE_OFFSET: usize = 0x3C4; // u16 at 0x3C4..0x3C5

/// Save-relative offset for the source byte that records which entry
/// point entered the trap (1 = get_command, 2 = putchar).  Used after
/// the spin loop to route to the correct displaced instruction + resume.
const SOURCE_SAVE_OFFSET: usize = 0x3C6;

/// `putchar` function at CS:0x16BA.  Secondary hook for trapping during
/// animations (camping, combat) when `get_command` isn't being called.
pub(crate) const PUTCHAR_OFFSET: usize = 0x16BA;
/// Resume point after the displaced 3-byte prologue.
const PUTCHAR_RESUME: usize = 0x16BD;
/// Expected first 3 bytes: PUSH BP; MOV BP, SP (standard Borland prologue).
const PUTCHAR_PROLOGUE: [u8; 3] = [0x55, 0x8B, 0xEC];

/// Conversion factor: `DS_offset = save_offset + DS_SAVE_DELTA`.
const DS_SAVE_DELTA: u16 = 0x55A6;

/// Size of the injected code cave in bytes.
/// Combined cave with two entry points (get_command + putchar), shared
/// trap logic, MIDPAK music restart after trap, source-based exit
/// routing, and two displaced-instruction exit paths.
const CAVE_SIZE: usize = 102;

/// Byte offset of Entry B (putchar) within the cave.
const CAVE_ENTRY_B_OFFSET: usize = 29;

/// Minimum contiguous zero-byte run to accept as a code cave.
const MIN_CAVE_RUN: usize = CAVE_SIZE + 2; // +2 padding

/// Range within the code segment to search for a code cave.
/// Upper bound constrained by 16-bit JMP displacement: the cave must
/// be reachable from both CS:0x268C (get_command) and CS:0x16BA
/// (putchar).  Max forward from 0x16BD: 0x16BD + 0x7FFF = 0x96BC.
const CAVE_SCAN_START: usize = 0x4000;
const CAVE_SCAN_END: usize = 0x9600;

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
    /// Original 5 bytes from the get_command hook site.
    original_hook: [u8; 5],
    /// Original bytes from the code cave location.
    original_cave: [u8; CAVE_SIZE],
    /// Absolute host address of the dirty flag byte.
    flag_addr: usize,
    /// Absolute host address of the trap flag byte (for save states).
    trap_flag_addr: usize,
    /// Whether the secondary putchar hook is installed.
    has_putchar_hook: bool,
    /// Original 3 bytes from the putchar hook site.
    original_putchar: [u8; 3],
}

impl PatchState {
    pub fn cs_base(&self) -> usize {
        self.cs_base
    }
    pub fn cave_cs_offset(&self) -> usize {
        self.cave_cs_offset
    }
    pub fn original_hook(&self) -> &[u8; 5] {
        &self.original_hook
    }
    pub fn original_cave(&self) -> &[u8; CAVE_SIZE] {
        &self.original_cave
    }
    pub fn flag_addr(&self) -> usize {
        self.flag_addr
    }
    pub fn trap_flag_addr(&self) -> usize {
        self.trap_flag_addr
    }
    pub fn has_putchar_hook(&self) -> bool {
        self.has_putchar_hook
    }
    pub fn original_putchar(&self) -> &[u8; 3] {
        &self.original_putchar
    }

    /// Offset and size of the cave within the 1 MB DOS address space.
    pub fn cave_range(&self, dos_base: usize) -> (usize, usize) {
        let offset = (self.cs_base - dos_base) + self.cave_cs_offset;
        (offset, CAVE_SIZE)
    }
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

/// Build the 90-byte combined code cave.  Pure function — no I/O.
///
/// The cave has two entry points:
/// - **Entry A** (from `get_command` hook at CS:0x268C): redraw check,
///   then trap check.  Marks source = 1.
/// - **Entry B** (from `putchar` hook at CS:0x16BA): trap check only.
///   Marks source = 2.
///
/// Both entries share common trap logic: save SP/BP, ack, spin, restore
/// SP/BP.  After the spin loop, the source byte routes to the correct
/// exit path — each with its own displaced instruction and resume JMP.
///
/// ```text
/// === Entry A: get_command ===
///  0: CMP dirty_flag, 0             ; redraw check
///  5: JE (→ 15)
///  7: MOV dirty_flag, 0
/// 12: CALL redraw_full_stats
/// 15: CMP trap_flag, 1              ; trap check
/// 20: JNE get_cmd_exit (→ 76)       ; fast path
/// 22: MOV byte [source], 1          ; mark: get_command
/// 27: JMP common (→ 41)
///
/// === Entry B: putchar ===
/// 29: CMP trap_flag, 1              ; trap check
/// 34: JNE putchar_exit (→ 84)       ; fast path
/// 36: MOV byte [source], 2          ; mark: putchar
///
/// === Common trap logic ===
/// 41: MOV [saved_sp], SP
/// 45: MOV [saved_bp], BP
/// 49: MOV byte [trap_flag], 2       ; ack
/// 54: CMP byte [trap_flag], 0       ; spin
/// 59: JNE spin (→ 54)
/// 61: MOV SP, [saved_sp]
/// 65: MOV BP, [saved_bp]
/// 69: CMP byte [source], 1          ; which entry?
/// 74: JNE putchar_exit (→ 84)
///
/// === get_command exit ===
/// 76: CMP byte [0x5893], 0x21       ; displaced
/// 81: JMP 0x2691                     ; resume get_command
///
/// === putchar exit ===
/// 84: PUSH BP                        ; displaced
/// 85: MOV BP, SP
/// 87: JMP 0x16BD                     ; resume putchar
/// ```
fn encode_cave(
    cave_cs_offset: usize,
    flag_ds: u16,
    trap_ds: u16,
    sp_ds: u16,
    bp_ds: u16,
    source_ds: u16,
) -> [u8; CAVE_SIZE] {
    let lo = |v: u16| (v & 0xFF) as u8;
    let hi = |v: u16| (v >> 8) as u8;

    let call_disp = (REDRAW_OFFSET as i32 - (cave_cs_offset as i32 + 15)) as i16;
    let gc_resume_disp = (HOOK_RESUME as i32 - (cave_cs_offset as i32 + 96)) as i16;
    let pc_resume_disp = (PUTCHAR_RESUME as i32 - (cave_cs_offset as i32 + 102)) as i16;

    let cd = call_disp.to_le_bytes();
    let gd = gc_resume_disp.to_le_bytes();
    let pd = pc_resume_disp.to_le_bytes();

    [
        // === Entry A: get_command (bytes 0-28) ===
        0x80,
        0x3E,
        lo(flag_ds),
        hi(flag_ds),
        0x00, //  0: CMP byte [dirty_flag], 0
        0x74,
        0x08, //  5: JE +8 → 15
        0xC6,
        0x06,
        lo(flag_ds),
        hi(flag_ds),
        0x00, //  7: MOV byte [dirty_flag], 0
        0xE8,
        cd[0],
        cd[1], // 12: CALL redraw_full_stats
        0x80,
        0x3E,
        lo(trap_ds),
        hi(trap_ds),
        0x01, // 15: CMP byte [trap_flag], 1
        0x75,
        0x42, // 20: JNE +66 → 88 (get_cmd_exit)
        0xC6,
        0x06,
        lo(source_ds),
        hi(source_ds),
        0x01, // 22: MOV byte [source], 1
        0xEB,
        0x0C, // 27: JMP +12 → 41 (common)
        // === Entry B: putchar (bytes 29-40) ===
        0x80,
        0x3E,
        lo(trap_ds),
        hi(trap_ds),
        0x01, // 29: CMP byte [trap_flag], 1
        0x75,
        0x3C, // 34: JNE +60 → 96 (putchar_exit)
        0xC6,
        0x06,
        lo(source_ds),
        hi(source_ds),
        0x02, // 36: MOV byte [source], 2
        // === Common trap logic (bytes 41-76) ===
        0x89,
        0x26,
        lo(sp_ds),
        hi(sp_ds), // 41: MOV [saved_sp], SP
        0x89,
        0x2E,
        lo(bp_ds),
        hi(bp_ds), // 45: MOV [saved_bp], BP
        0xC6,
        0x06,
        lo(trap_ds),
        hi(trap_ds),
        0x02, // 49: MOV byte [trap_flag], 2
        0x80,
        0x3E,
        lo(trap_ds),
        hi(trap_ds),
        0x00, // 54: CMP byte [trap_flag], 0
        0x75,
        0xF9, // 59: JNE -7 → 54 (spin)
        0x8B,
        0x26,
        lo(sp_ds),
        hi(sp_ds), // 61: MOV SP, [saved_sp]
        0x8B,
        0x2E,
        lo(bp_ds),
        hi(bp_ds), // 65: MOV BP, [saved_bp]
        // Restart MIDPAK music: flush pending MIDI events (serve),
        // then start playback from the restored sequence state.
        // On save this restarts the current song cleanly.
        // On load this resyncs the driver with the restored state.
        0xB8,
        0x05,
        0x07, // 69: MOV AX, 0x0705 (MIDPAK: serve/flush)
        0xCD,
        0x66, // 72: INT 66h
        0xB8,
        0x02,
        0x07, // 74: MOV AX, 0x0702 (MIDPAK: start playback)
        0x33,
        0xDB, // 77: XOR BX, BX (sequence 0)
        0xCD,
        0x66, // 79: INT 66h
        // === Source routing (bytes 81-87) ===
        0x80,
        0x3E,
        lo(source_ds),
        hi(source_ds),
        0x01, // 81: CMP byte [source], 1
        0x75,
        0x08, // 86: JNE +8 → 96 (putchar_exit)
        // === get_command exit (bytes 88-95) ===
        0x80,
        0x3E,
        0x93,
        0x58,
        0x21, // 88: CMP byte [0x5893], 0x21
        0xE9,
        gd[0],
        gd[1], // 93: JMP get_command resume
        // === putchar exit (bytes 96-101) ===
        0x55, // 96: PUSH BP
        0x8B,
        0xEC, // 97: MOV BP, SP
        0xE9,
        pd[0],
        pd[1], // 99: JMP putchar resume
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
                let trap_flag_addr = dos_base + SAVE_BASE + flag_save + 1;

                // Clear any stale trap flag from a previous crashed session.
                let _ = mem.write_u8(trap_flag_addr, 0);
                let _ = mem.write_u8(flag_addr, 0);

                log::info!(
                    "Adopting existing patch at CS:{target:#06x} \
                     (flag DS:{flag_ds:#06x} = save+{flag_save:#x})"
                );

                // Re-write the cave to the current layout.  The
                // adopted cave may be from an older build with a
                // smaller layout.  Overwriting is safe because the
                // cave sits in a zero-byte region and the new layout
                // only extends into trailing zeros.
                let trap_ds = (flag_save as u16 + 1) + DS_SAVE_DELTA;
                let sp_ds = SAVED_SP_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
                let bp_ds = SAVED_BP_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
                let src_ds = SOURCE_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
                let cave_bytes = encode_cave(target, flag_ds, trap_ds, sp_ds, bp_ds, src_ds);
                let _ = mem.write_bytes(cs_base + target, &cave_bytes);
                log::info!("Re-wrote cave to current {CAVE_SIZE}-byte layout");

                // Try to install the putchar hook (non-fatal).
                let (has_putchar_hook, original_putchar) =
                    try_install_putchar_hook(mem, cs_base, target);

                return Ok(PatchState {
                    cs_base,
                    cave_cs_offset: target,
                    original_hook: current_hook,
                    original_cave: [0; CAVE_SIZE],
                    flag_addr,
                    trap_flag_addr,
                    has_putchar_hook,
                    original_putchar,
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

    // 5. Compute flag addresses.
    let flag_addr = dos_base + SAVE_BASE + FLAG_SAVE_OFFSET;
    let flag_ds_offset = FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    let trap_flag_addr = dos_base + SAVE_BASE + TRAP_FLAG_SAVE_OFFSET;
    let trap_ds_offset = TRAP_FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    let sp_ds_offset = SAVED_SP_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    let bp_ds_offset = SAVED_BP_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    let source_ds_offset = SOURCE_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    log::debug!("Flag: save+{FLAG_SAVE_OFFSET:#x} = DS:{flag_ds_offset:#06x} (abs {flag_addr:#x})");
    log::debug!(
        "Trap: save+{TRAP_FLAG_SAVE_OFFSET:#x} = DS:{trap_ds_offset:#06x} (abs {trap_flag_addr:#x})"
    );

    // 6. Verify flag bytes are clean.
    let flag_current = mem.read_u8(flag_addr).context("reading flag byte")?;
    ensure!(
        flag_current == 0,
        "flag byte at save+{FLAG_SAVE_OFFSET:#x} is {flag_current:#x}, not zero"
    );
    let trap_current = mem
        .read_u8(trap_flag_addr)
        .context("reading trap flag byte")?;
    ensure!(
        trap_current == 0,
        "trap flag byte at save+{TRAP_FLAG_SAVE_OFFSET:#x} is {trap_current:#x}, not zero"
    );

    // 7. Save original cave bytes.
    let mut original_cave = [0u8; CAVE_SIZE];
    mem.read_bytes(cave_addr, &mut original_cave)
        .context("reading original cave bytes")?;

    // 8. Write the code cave.
    let cave_bytes = encode_cave(
        cave_cs_offset,
        flag_ds_offset,
        trap_ds_offset,
        sp_ds_offset,
        bp_ds_offset,
        source_ds_offset,
    );
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

    // 12. Try to install the putchar hook (non-fatal).
    let (has_putchar_hook, original_putchar) =
        try_install_putchar_hook(mem, cs_base, cave_cs_offset);

    Ok(PatchState {
        cs_base,
        cave_cs_offset,
        original_hook: HOOK_BYTES,
        original_cave,
        flag_addr,
        trap_flag_addr,
        has_putchar_hook,
        original_putchar,
    })
}

/// Try to install the secondary putchar hook.  Returns (success, original_bytes).
/// Non-fatal: if putchar's prologue doesn't match, we skip the hook.
fn try_install_putchar_hook(
    mem: &dyn MemoryAccess,
    cs_base: usize,
    cave_cs_offset: usize,
) -> (bool, [u8; 3]) {
    let putchar_addr = cs_base + PUTCHAR_OFFSET;
    let mut original = [0u8; 3];

    // Read current putchar bytes.
    if mem.read_bytes(putchar_addr, &mut original).is_err() {
        log::warn!("Could not read putchar at CS:{PUTCHAR_OFFSET:#06x}");
        return (false, original);
    }

    // If already hooked (starts with JMP), adopt it.
    // Store the known prologue for restoration, not the JMP bytes.
    if original[0] == 0xE9 {
        log::info!("Putchar hook already installed");
        return (true, PUTCHAR_PROLOGUE);
    }

    // Verify expected prologue.
    if original != PUTCHAR_PROLOGUE {
        log::warn!(
            "Putchar at CS:{PUTCHAR_OFFSET:#06x} is {original:02X?}, \
             expected {PUTCHAR_PROLOGUE:02X?} — skipping putchar hook"
        );
        return (false, original);
    }

    // Entry B is at cave byte 29.
    let entry_b_cs = cave_cs_offset + CAVE_ENTRY_B_OFFSET;
    let disp = (entry_b_cs as i32 - (PUTCHAR_OFFSET as i32 + 3)) as i16;
    let pd = disp.to_le_bytes();
    let patched: [u8; 3] = [0xE9, pd[0], pd[1]];

    if mem.write_bytes(putchar_addr, &patched).is_err() {
        log::warn!("Failed to write putchar hook");
        return (false, original);
    }

    // Verify.
    let mut readback = [0u8; 3];
    if mem.read_bytes(putchar_addr, &mut readback).is_err() || readback != patched {
        let _ = mem.write_bytes(putchar_addr, &original);
        log::warn!("Putchar hook verification failed");
        return (false, original);
    }

    log::info!("Putchar hook installed at CS:{PUTCHAR_OFFSET:#06x} → CS:{entry_b_cs:#06x}");
    (true, PUTCHAR_PROLOGUE)
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
        mem.set_bytes(cs_base + PUTCHAR_OFFSET, &PUTCHAR_PROLOGUE);

        (mem, dos_base)
    }

    fn ds_offsets() -> (u16, u16, u16, u16, u16) {
        (
            FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA,
            TRAP_FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA,
            SAVED_SP_SAVE_OFFSET as u16 + DS_SAVE_DELTA,
            SAVED_BP_SAVE_OFFSET as u16 + DS_SAVE_DELTA,
            SOURCE_SAVE_OFFSET as u16 + DS_SAVE_DELTA,
        )
    }

    #[test]
    fn encode_cave_correct() {
        let cave = 0x5001usize;
        let (flag_ds, trap_ds, sp_ds, bp_ds, src_ds) = ds_offsets();
        let bytes = encode_cave(cave, flag_ds, trap_ds, sp_ds, bp_ds, src_ds);
        assert_eq!(bytes.len(), CAVE_SIZE);

        // Entry A: redraw check
        assert_eq!(bytes[0], 0x80); // CMP
        assert_eq!(bytes[5], 0x74); // JE
        assert_eq!(bytes[6], 0x08); // +8 -> 15
        // Trap check → get_cmd_exit at 88
        assert_eq!(bytes[20], 0x75); // JNE
        assert_eq!(bytes[21], 0x42); // +66 → 88
        assert_eq!(bytes[26], 0x01); // source = 1
        assert_eq!(bytes[27], 0xEB);
        assert_eq!(bytes[28], 0x0C); // +12 → 41

        // Entry B: putchar → putchar_exit at 96
        assert_eq!(bytes[34], 0x75); // JNE
        assert_eq!(bytes[35], 0x3C); // +60 → 96
        assert_eq!(bytes[40], 0x02); // source = 2

        // Save SP/BP at 41-48
        assert_eq!(bytes[41], 0x89);
        assert_eq!(bytes[42], 0x26); // MOV [sp], SP
        assert_eq!(bytes[45], 0x89);
        assert_eq!(bytes[46], 0x2E); // MOV [bp], BP
        // Ack at 49, spin at 54
        assert_eq!(bytes[53], 0x02); // trap_flag = 2
        assert_eq!(bytes[59], 0x75);
        assert_eq!(bytes[60], 0xF9); // JNE -7 → 54
        // Restore SP/BP at 61-68
        assert_eq!(bytes[61], 0x8B);
        assert_eq!(bytes[62], 0x26);
        assert_eq!(bytes[65], 0x8B);
        assert_eq!(bytes[66], 0x2E);
        // MIDPAK serve+start at 69-80
        assert_eq!(bytes[69], 0xB8);
        assert_eq!(bytes[70], 0x05);
        assert_eq!(bytes[71], 0x07);
        assert_eq!(bytes[72], 0xCD);
        assert_eq!(bytes[73], 0x66); // serve
        assert_eq!(bytes[74], 0xB8);
        assert_eq!(bytes[75], 0x02);
        assert_eq!(bytes[76], 0x07);
        assert_eq!(bytes[77], 0x33);
        assert_eq!(bytes[78], 0xDB); // XOR BX, BX
        assert_eq!(bytes[79], 0xCD);
        assert_eq!(bytes[80], 0x66); // start
        // Source routing at 81
        assert_eq!(bytes[81], 0x80); // CMP source, 1
        assert_eq!(bytes[86], 0x75);
        assert_eq!(bytes[87], 0x08); // JNE +8 → 96
        // get_command exit at 88
        assert_eq!(&bytes[88..93], &HOOK_BYTES);
        // putchar exit at 96
        assert_eq!(bytes[96], 0x55); // PUSH BP
        assert_eq!(bytes[97], 0x8B);
        assert_eq!(bytes[98], 0xEC);

        // Displacement targets
        let cd = i16::from_le_bytes([bytes[13], bytes[14]]);
        assert_eq!((cave as i32 + 15 + cd as i32) as usize, REDRAW_OFFSET);
        let gd = i16::from_le_bytes([bytes[94], bytes[95]]);
        assert_eq!((cave as i32 + 96 + gd as i32) as usize, HOOK_RESUME);
        let pd = i16::from_le_bytes([bytes[100], bytes[101]]);
        assert_eq!((cave as i32 + 102 + pd as i32) as usize, PUTCHAR_RESUME);
    }

    #[test]
    fn encode_cave_various_offsets() {
        for cave in [0x4001, 0x5000, 0x76C1, 0x9500] {
            let (flag_ds, trap_ds, sp_ds, bp_ds, src_ds) = ds_offsets();
            let bytes = encode_cave(cave, flag_ds, trap_ds, sp_ds, bp_ds, src_ds);

            let cd = i16::from_le_bytes([bytes[13], bytes[14]]);
            assert_eq!((cave as i32 + 15 + cd as i32) as usize, REDRAW_OFFSET);
            let gd = i16::from_le_bytes([bytes[94], bytes[95]]);
            assert_eq!((cave as i32 + 96 + gd as i32) as usize, HOOK_RESUME);
            let pd = i16::from_le_bytes([bytes[100], bytes[101]]);
            assert_eq!((cave as i32 + 102 + pd as i32) as usize, PUTCHAR_RESUME);
        }
    }

    #[test]
    fn apply_succeeds() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).expect("should succeed");
        assert_eq!(state.flag_addr, dos_base + SAVE_BASE + FLAG_SAVE_OFFSET);
        assert_eq!(
            state.trap_flag_addr,
            dos_base + SAVE_BASE + TRAP_FLAG_SAVE_OFFSET
        );

        // get_command hook site should now start with JMP.
        let mut hook = [0u8; 5];
        mem.read_bytes(state.cs_base + HOOK_OFFSET, &mut hook)
            .unwrap();
        assert_eq!(hook[0], 0xE9);
        assert_eq!(hook[3], 0x90); // NOP
        assert_eq!(hook[4], 0x90); // NOP

        // putchar hook should also be installed.
        assert!(state.has_putchar_hook);
        let mut pc_hook = [0u8; 3];
        mem.read_bytes(state.cs_base + PUTCHAR_OFFSET, &mut pc_hook)
            .unwrap();
        assert_eq!(pc_hook[0], 0xE9); // JMP
    }

    #[test]
    fn apply_idempotent() {
        let (mem, dos_base) = setup_mock();
        let s1 = apply_patch(&mem, dos_base).expect("first");
        let s2 = apply_patch(&mem, dos_base).expect("second (adopt)");
        assert_eq!(s1.flag_addr, s2.flag_addr);
        assert_eq!(s1.cave_cs_offset, s2.cave_cs_offset);
    }

    /// Helper: manually restore hooks and cave (mirrors controller logic).
    fn manual_remove(mem: &MockMemory, state: &PatchState) {
        let hook_addr = state.cs_base() + HOOK_OFFSET;
        let _ = mem.write_bytes(hook_addr, state.original_hook());
        if state.has_putchar_hook() {
            let putchar_addr = state.cs_base() + PUTCHAR_OFFSET;
            let _ = mem.write_bytes(putchar_addr, state.original_putchar());
        }
        let cave_addr = state.cs_base() + state.cave_cs_offset();
        let _ = mem.write_bytes(cave_addr, state.original_cave());
        let _ = mem.write_u8(state.flag_addr(), 0);
        let _ = mem.write_u8(state.trap_flag_addr(), 0);
    }

    #[test]
    fn remove_restores() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).unwrap();
        manual_remove(&mem, &state);

        let mut hook = [0u8; 5];
        mem.read_bytes(state.cs_base() + HOOK_OFFSET, &mut hook)
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
        manual_remove(&mem, &state);
        apply_patch(&mem, dos_base).expect("re-apply should work");
    }
}
