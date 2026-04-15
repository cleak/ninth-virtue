//! In-memory code patches for Ultima V's resident `get_command` loop.
//!
//! We maintain two small hooks in the resident code segment:
//! - a trampoline at the `get_command` overlay entry that checks our dirty flag
//!   and calls `redraw_full_stats` when the companion app has written game data
//! - a wrapper around the outdoor/local `call 0x5910` site that copies the
//!   final compact 11x11 visibility window into a runtime save-region buffer after
//!   the engine finishes its map render pass
//!
//! This keeps redraw triggering and visibility capture inside the game's own
//! main poll loop instead of racing it from the host process.
//!
//! See `docs/redraw-mechanism.md` for the original redraw-hook design.

use anyhow::{Context, Result, bail, ensure};

use crate::game::offsets::{SAVE_BASE, VIEWPORT_VISIBILITY_LEN};
use crate::memory::access::MemoryAccess;

// ---------------------------------------------------------------------------
// Signatures and offsets (all CS-relative unless noted)
// ---------------------------------------------------------------------------

/// First 7 bytes of `redraw_full_stats` at CS:0x2900.
const REDRAW_SIGNATURE: [u8; 7] = [0x55, 0x8B, 0xEC, 0x83, 0xEC, 0x02, 0x56];
const REDRAW_OFFSET: usize = 0x2900;

/// The overlay enters `get_command` 32 bytes past the prologue (the overlay
/// segment is 2 paragraphs above CS). The first instruction at that entry
/// point is a 5-byte CMP which we replace with a 3-byte JMP + 2 NOPs.
const HOOK_BYTES: [u8; 5] = [0x80, 0x3E, 0x93, 0x58, 0x21]; // CMP byte [0x5893], 0x21
const HOOK_OFFSET: usize = 0x268C;
/// Where to resume after the displaced CMP instruction.
const HOOK_RESUME: usize = 0x2691; // JB 0x269A (the instruction after the CMP)

/// `get_command` calls the resident 2D map renderer at CS:0x269A. Wrapping
/// this call lets us snapshot the post-render visibility window.
const VISIBILITY_CALL_BYTES: [u8; 3] = [0xE8, 0x73, 0x32]; // CALL 0x5910
const VISIBILITY_CALL_OFFSET: usize = 0x269A;
const MAP_RENDER_OFFSET: usize = 0x5910;

/// First 7 bytes at the main loop top CS:0x00B8 (cross-validation).
const LOOP_TOP_SIGNATURE: [u8; 7] = [0xC7, 0x46, 0xFE, 0x00, 0x00, 0x80, 0x3E];
const LOOP_TOP_OFFSET: usize = 0x00B8;

/// CS:0x0174 must contain a near JMP opcode (0xE9) for cross-validation.
const LOOP_JMP_OFFSET: usize = 0x0174;

/// Save-relative offset for our dirty flag byte.
const FLAG_SAVE_OFFSET: usize = 0x3C0;

/// Conversion factor: `DS_offset = save_offset + DS_SAVE_DELTA`.
const DS_SAVE_DELTA: u16 = 0x55A6;

/// DATA.OVL/runtime DS offsets used by the visibility snapshot metadata.
const LOCATION_DS_OFFSET: u16 = 0x5893;
const MAP_Z_DS_OFFSET: u16 = 0x5895;
const MAP_X_DS_OFFSET: u16 = 0x5896;
const MAP_Y_DS_OFFSET: u16 = 0x5897;
const MAP_SCROLL_X_DS_OFFSET: u16 = 0x589B;
const MAP_SCROLL_Y_DS_OFFSET: u16 = 0x589C;
const LIGHT_INTENSITY_DS_OFFSET: u16 = 0x58A5;
const VIEWPORT_VISIBILITY_GRID_DS_OFFSET: u16 = 0xAB02;
const VIEWPORT_VISIBILITY_ROW_ADVANCE: u16 = 0x0015; // 32-byte stride minus 11 active bytes
const VIEWPORT_ACTIVE_DIMENSION: u16 = 0x000B;

/// Size of the injected redraw trampoline in bytes.
const STATS_CAVE_SIZE: usize = 23;
/// Size of the injected visibility wrapper in bytes.
const VISIBILITY_WRAPPER_SIZE: usize = 84;

/// Runtime-only save offset used for the stabilized 11x11 visibility snapshot.
///
/// This sits in the quiet gap between the object table and the live 32x32
/// terrain window and avoids requiring a second large code cave in `ULTIMA.EXE`.
const VISIBILITY_SNAPSHOT_SAVE_OFFSET: usize = 0x0F00;
const VISIBILITY_SNAPSHOT_DS_OFFSET: u16 = VISIBILITY_SNAPSHOT_SAVE_OFFSET as u16 + DS_SAVE_DELTA;

/// Stable visibility snapshot layout written by the wrapper cave.
pub const VISIBILITY_SNAPSHOT_META_LEN: usize = 7;
pub const VISIBILITY_SNAPSHOT_TILES_OFFSET: usize = VISIBILITY_SNAPSHOT_META_LEN;
pub const VISIBILITY_SNAPSHOT_BODY_LEN: usize =
    VISIBILITY_SNAPSHOT_META_LEN + VIEWPORT_VISIBILITY_LEN;
pub const VISIBILITY_SNAPSHOT_READY_OFFSET: usize = VISIBILITY_SNAPSHOT_BODY_LEN;
pub const VISIBILITY_SNAPSHOT_TOTAL_LEN: usize = VISIBILITY_SNAPSHOT_BODY_LEN + 1;
pub const VISIBILITY_SNAPSHOT_READY_MARKER: u8 = 0xA5;
pub const VISIBILITY_SNAPSHOT_LOCATION_IDX: usize = 0;
pub const VISIBILITY_SNAPSHOT_Z_IDX: usize = 1;
pub const VISIBILITY_SNAPSHOT_X_IDX: usize = 2;
pub const VISIBILITY_SNAPSHOT_Y_IDX: usize = 3;
pub const VISIBILITY_SNAPSHOT_SCROLL_X_IDX: usize = 4;
pub const VISIBILITY_SNAPSHOT_SCROLL_Y_IDX: usize = 5;
pub const VISIBILITY_SNAPSHOT_LIGHT_IDX: usize = 6;

/// Total bytes reserved in the code cave: redraw stub + visibility wrapper.
const CAVE_BLOCK_SIZE: usize = STATS_CAVE_SIZE + VISIBILITY_WRAPPER_SIZE;

/// Minimum contiguous zero-byte run to accept as a code cave.
const MIN_CAVE_RUN: usize = CAVE_BLOCK_SIZE + 2; // +2 padding

/// Range within the code segment to search for a code cave.
const CAVE_SCAN_START: usize = 0x4000;
const CAVE_SCAN_END: usize = 0x8000;

/// How much DOS memory to scan for the redraw signature.
const SIG_SCAN_SIZE: usize = 0x10_0000; // 1 MB

// ---------------------------------------------------------------------------
// Patch state
// ---------------------------------------------------------------------------

/// Everything needed to undo the patches, trigger redraws, and read the
/// stabilized visibility snapshot.
#[derive(Debug)]
pub struct PatchState {
    /// Absolute host address of CS:0x0000 in DOSBox memory.
    cs_base: usize,
    /// CS-relative offset where the code-cave block was placed.
    cave_cs_offset: usize,
    /// Original 5 bytes from the hook site.
    original_hook: [u8; 5],
    /// Original 3 bytes from the wrapped visibility call site.
    original_visibility_call: [u8; 3],
    /// Original bytes from the code-cave block.
    original_cave: Vec<u8>,
    /// Whether this process installed the resident patch and therefore owns
    /// teardown.
    owns_installation: bool,
    /// Absolute host address of the dirty flag byte.
    flag_addr: usize,
    /// Absolute host address of the visibility snapshot buffer start.
    ///
    /// Callers should read `VISIBILITY_SNAPSHOT_TOTAL_LEN` bytes from this
    /// address and apply `VISIBILITY_SNAPSHOT_TILES_OFFSET` only once when
    /// they want the 11x11 tile body.
    visibility_snapshot_addr: usize,
}

impl PatchState {
    /// Absolute host address of the visibility snapshot buffer start written
    /// after each resident `0x5910` call.
    ///
    /// The buffer contains metadata, the 11x11 tiles, and the trailing ready
    /// marker. The ready byte lives at
    /// `visibility_snapshot_addr + VISIBILITY_SNAPSHOT_READY_OFFSET`.
    pub fn visibility_snapshot_addr(&self) -> usize {
        self.visibility_snapshot_addr
    }

    /// Whether this handle created the currently installed resident patch.
    pub fn owns_installation(&self) -> bool {
        self.owns_installation
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

        let jmp_opcode = match mem.read_u8(candidate + LOOP_JMP_OFFSET) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if jmp_opcode != 0xE9 {
            continue;
        }

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

/// Find a contiguous run of zero bytes suitable for the combined cave block.
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

/// Build the 23-byte redraw trampoline. Pure function — no I/O.
fn encode_stats_cave(cave_cs_offset: usize, flag_ds_offset: u16) -> [u8; STATS_CAVE_SIZE] {
    let flag_lo = (flag_ds_offset & 0xFF) as u8;
    let flag_hi = (flag_ds_offset >> 8) as u8;

    let call_next = cave_cs_offset + 15;
    let call_disp = (REDRAW_OFFSET as i32 - call_next as i32) as i16;

    let jmp_next = cave_cs_offset + STATS_CAVE_SIZE;
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

/// Build the wrapper that replaces `call 0x5910`.
fn encode_visibility_wrapper(
    wrapper_cs_offset: usize,
    snapshot_ds_offset: u16,
) -> [u8; VISIBILITY_WRAPPER_SIZE] {
    let ready_ds_offset = snapshot_ds_offset + VISIBILITY_SNAPSHOT_READY_OFFSET as u16;
    let render_call_next = wrapper_cs_offset + 3;
    let render_call_disp = (MAP_RENDER_OFFSET as i32 - render_call_next as i32) as i16;

    let mut bytes = Vec::with_capacity(VISIBILITY_WRAPPER_SIZE);
    let cd = render_call_disp.to_le_bytes();
    bytes.extend([0xE8, cd[0], cd[1]]); // CALL 0x5910
    bytes.extend([0x50, 0x53, 0x51, 0x52, 0x56, 0x57, 0x06]); // PUSH AX,BX,CX,DX,SI,DI,ES
    bytes.push(0x9C); // PUSHF
    bytes.push(0xFC); // CLD
    bytes.push(0x1E); // PUSH DS
    bytes.push(0x07); // POP ES
    bytes.extend(es_store_imm8(ready_ds_offset, 0x00)); // ready = 0 while copying
    bytes.extend([
        0xBF,
        (snapshot_ds_offset & 0xFF) as u8,
        (snapshot_ds_offset >> 8) as u8,
    ]);

    for ds_offset in [
        LOCATION_DS_OFFSET,
        MAP_Z_DS_OFFSET,
        MAP_X_DS_OFFSET,
        MAP_Y_DS_OFFSET,
        MAP_SCROLL_X_DS_OFFSET,
        MAP_SCROLL_Y_DS_OFFSET,
        LIGHT_INTENSITY_DS_OFFSET,
    ] {
        bytes.extend(mov_al_from_ds(ds_offset));
        bytes.push(0xAA); // STOSB
    }

    bytes.extend([
        0xBE,
        (VIEWPORT_VISIBILITY_GRID_DS_OFFSET & 0xFF) as u8,
        (VIEWPORT_VISIBILITY_GRID_DS_OFFSET >> 8) as u8,
    ]); // MOV SI, 0xAB02
    bytes.extend([
        0xBA,
        (VIEWPORT_ACTIVE_DIMENSION & 0xFF) as u8,
        (VIEWPORT_ACTIVE_DIMENSION >> 8) as u8,
    ]); // MOV DX, 11 rows

    let row_loop = wrapper_cs_offset + bytes.len();
    bytes.extend([
        0xB9,
        (VIEWPORT_ACTIVE_DIMENSION & 0xFF) as u8,
        (VIEWPORT_ACTIVE_DIMENSION >> 8) as u8,
    ]); // MOV CX, 11 cols
    bytes.extend([0xF3, 0xA4]); // REP MOVSB
    bytes.extend([
        0x81,
        0xC6,
        (VIEWPORT_VISIBILITY_ROW_ADVANCE & 0xFF) as u8,
        (VIEWPORT_VISIBILITY_ROW_ADVANCE >> 8) as u8,
    ]); // ADD SI, 21
    bytes.push(0x4A); // DEC DX

    let jne_next = wrapper_cs_offset + bytes.len() + 2;
    let jne_disp = (row_loop as i32 - jne_next as i32) as i8;
    bytes.extend([0x75, jne_disp as u8]); // JNE row_loop

    bytes.extend(es_store_imm8(
        ready_ds_offset,
        VISIBILITY_SNAPSHOT_READY_MARKER,
    ));
    bytes.push(0x9D); // POPF
    bytes.extend([0x07, 0x5F, 0x5E, 0x5A, 0x59, 0x5B, 0x58]); // POP ES,DI,SI,DX,CX,BX,AX
    bytes.push(0xC3); // RET

    let len = bytes.len();
    bytes
        .try_into()
        .unwrap_or_else(|_| panic!("visibility wrapper size drifted: {len}"))
}

fn mov_al_from_ds(ds_offset: u16) -> [u8; 3] {
    [0xA0, (ds_offset & 0xFF) as u8, (ds_offset >> 8) as u8]
}

fn es_store_imm8(es_offset: u16, value: u8) -> [u8; 6] {
    [
        0x26,
        0xC6,
        0x06,
        (es_offset & 0xFF) as u8,
        (es_offset >> 8) as u8,
        value,
    ]
}

fn encode_patched_hook(cave_cs_offset: usize) -> [u8; 5] {
    let hook_disp = (cave_cs_offset as i32 - (HOOK_OFFSET as i32 + 3)) as i16;
    let hd = hook_disp.to_le_bytes();
    [0xE9, hd[0], hd[1], 0x90, 0x90]
}

fn encode_visibility_call(wrapper_cs_offset: usize) -> [u8; 3] {
    let disp = (wrapper_cs_offset as i32 - (VISIBILITY_CALL_OFFSET as i32 + 3)) as i16;
    let bytes = disp.to_le_bytes();
    [0xE8, bytes[0], bytes[1]]
}

fn try_adopt_existing_patch(
    mem: &dyn MemoryAccess,
    dos_base: usize,
    cs_base: usize,
    current_hook: [u8; 5],
    current_visibility_call: [u8; 3],
) -> Result<Option<PatchState>> {
    if current_hook[0] != 0xE9 || current_visibility_call[0] != 0xE8 {
        return Ok(None);
    }

    let cave_disp = i16::from_le_bytes([current_hook[1], current_hook[2]]);
    let cave_cs_offset = (HOOK_OFFSET as i32 + 3 + cave_disp as i32) as usize;
    if !(CAVE_SCAN_START..CAVE_SCAN_END).contains(&cave_cs_offset) {
        return Ok(None);
    }

    let wrapper_disp = i16::from_le_bytes([current_visibility_call[1], current_visibility_call[2]]);
    let wrapper_cs_offset = (VISIBILITY_CALL_OFFSET as i32 + 3 + wrapper_disp as i32) as usize;
    if wrapper_cs_offset != cave_cs_offset + STATS_CAVE_SIZE {
        return Ok(None);
    }

    let mut probe = [0u8; 5];
    if mem
        .read_bytes(cs_base + cave_cs_offset, &mut probe)
        .is_err()
        || probe[0] != 0x80
        || probe[1] != 0x3E
        || probe[4] != 0x00
    {
        return Ok(None);
    }

    let mut wrapper_probe = [0u8; 3];
    if mem
        .read_bytes(cs_base + wrapper_cs_offset, &mut wrapper_probe)
        .is_err()
        || wrapper_probe[0] != 0xE8
    {
        return Ok(None);
    }

    let flag_ds = u16::from_le_bytes([probe[2], probe[3]]);
    let flag_save = flag_ds.wrapping_sub(DS_SAVE_DELTA) as usize;
    let flag_addr = dos_base + SAVE_BASE + flag_save;
    let visibility_snapshot_addr = dos_base + SAVE_BASE + VISIBILITY_SNAPSHOT_SAVE_OFFSET;
    let expected_hook = encode_patched_hook(cave_cs_offset);
    let expected_visibility_call = encode_visibility_call(wrapper_cs_offset);
    if current_hook != expected_hook || current_visibility_call != expected_visibility_call {
        return Ok(None);
    }

    let mut current_cave = vec![0u8; CAVE_BLOCK_SIZE];
    if mem
        .read_bytes(cs_base + cave_cs_offset, &mut current_cave)
        .is_err()
    {
        return Ok(None);
    }

    let mut expected_cave = vec![0u8; CAVE_BLOCK_SIZE];
    expected_cave[..STATS_CAVE_SIZE].copy_from_slice(&encode_stats_cave(cave_cs_offset, flag_ds));
    expected_cave[STATS_CAVE_SIZE..].copy_from_slice(&encode_visibility_wrapper(
        wrapper_cs_offset,
        VISIBILITY_SNAPSHOT_DS_OFFSET,
    ));
    if current_cave != expected_cave {
        return Ok(None);
    }

    log::info!(
        "Adopting existing patch at CS:{cave_cs_offset:#06x} \
         (flag DS:{flag_ds:#06x} = save+{flag_save:#x}, snapshot save+{VISIBILITY_SNAPSHOT_SAVE_OFFSET:#x})"
    );

    Ok(Some(PatchState {
        cs_base,
        cave_cs_offset,
        original_hook: HOOK_BYTES,
        original_visibility_call: VISIBILITY_CALL_BYTES,
        original_cave: current_cave,
        owns_installation: false,
        flag_addr,
        visibility_snapshot_addr,
    }))
}

fn rollback_partial_patch(
    mem: &dyn MemoryAccess,
    hook_addr: usize,
    visibility_call_addr: usize,
    cave_addr: usize,
    original_hook: &[u8; 5],
    original_visibility_call: &[u8; 3],
    original_cave: &[u8],
) {
    let _ = mem.write_bytes(hook_addr, original_hook);
    let _ = mem.write_bytes(visibility_call_addr, original_visibility_call);
    let _ = mem.write_bytes(cave_addr, original_cave);
}

// ---------------------------------------------------------------------------
// Patch application
// ---------------------------------------------------------------------------

/// Apply the combined redraw + visibility hooks to the running game.
pub fn apply_patch(mem: &dyn MemoryAccess, dos_base: usize) -> Result<PatchState> {
    let cs_base = find_cs_base(mem, dos_base)?;
    log::debug!("CS base found at {cs_base:#x} (dos_base={dos_base:#x})");

    let hook_addr = cs_base + HOOK_OFFSET;
    let mut current_hook = [0u8; 5];
    mem.read_bytes(hook_addr, &mut current_hook)
        .context("reading hook bytes")?;
    log::debug!("Hook site at CS:{HOOK_OFFSET:#06x} = {current_hook:02X?}");

    let visibility_call_addr = cs_base + VISIBILITY_CALL_OFFSET;
    let mut current_visibility_call = [0u8; 3];
    mem.read_bytes(visibility_call_addr, &mut current_visibility_call)
        .context("reading visibility call bytes")?;
    log::debug!(
        "Visibility call at CS:{VISIBILITY_CALL_OFFSET:#06x} = {current_visibility_call:02X?}"
    );

    if let Some(state) = try_adopt_existing_patch(
        mem,
        dos_base,
        cs_base,
        current_hook,
        current_visibility_call,
    )? {
        return Ok(state);
    }

    if current_hook != HOOK_BYTES || current_visibility_call != VISIBILITY_CALL_BYTES {
        bail!(
            "found an incompatible existing resident patch at CS:{HOOK_OFFSET:#06x}/CS:{VISIBILITY_CALL_OFFSET:#06x}; \
             restart DOSBox to clear stale injected code before reconnecting"
        );
    }

    let cave_cs_offset = find_code_cave(mem, cs_base)?;
    let cave_addr = cs_base + cave_cs_offset;
    let wrapper_cs_offset = cave_cs_offset + STATS_CAVE_SIZE;
    let visibility_snapshot_addr = dos_base + SAVE_BASE + VISIBILITY_SNAPSHOT_SAVE_OFFSET;
    log::debug!(
        "Code cave block at CS:{cave_cs_offset:#06x} (abs {cave_addr:#x}), \
         wrapper CS:{wrapper_cs_offset:#06x}, snapshot save+{VISIBILITY_SNAPSHOT_SAVE_OFFSET:#x}"
    );

    let flag_addr = dos_base + SAVE_BASE + FLAG_SAVE_OFFSET;
    let flag_ds_offset = FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
    let flag_current = mem.read_u8(flag_addr).context("reading flag byte")?;
    ensure!(
        flag_current == 0,
        "flag byte at save+{FLAG_SAVE_OFFSET:#x} is {flag_current:#x}, not zero"
    );
    mem.write_u8(
        visibility_snapshot_addr + VISIBILITY_SNAPSHOT_READY_OFFSET,
        0,
    )
    .context("clearing visibility snapshot ready marker")?;

    let mut original_cave = vec![0u8; CAVE_BLOCK_SIZE];
    mem.read_bytes(cave_addr, &mut original_cave)
        .context("reading original cave bytes")?;

    let mut cave_block = vec![0u8; CAVE_BLOCK_SIZE];
    cave_block[..STATS_CAVE_SIZE]
        .copy_from_slice(&encode_stats_cave(cave_cs_offset, flag_ds_offset));
    cave_block[STATS_CAVE_SIZE..STATS_CAVE_SIZE + VISIBILITY_WRAPPER_SIZE].copy_from_slice(
        &encode_visibility_wrapper(wrapper_cs_offset, VISIBILITY_SNAPSHOT_DS_OFFSET),
    );
    log::debug!(
        "Writing {} cave bytes at CS:{cave_cs_offset:#06x}",
        cave_block.len()
    );
    mem.write_bytes(cave_addr, &cave_block)
        .context("writing code cave block")?;

    let mut cave_readback = vec![0u8; CAVE_BLOCK_SIZE];
    mem.read_bytes(cave_addr, &mut cave_readback)
        .context("verifying code cave block")?;
    if cave_readback != cave_block {
        rollback_partial_patch(
            mem,
            hook_addr,
            visibility_call_addr,
            cave_addr,
            &current_hook,
            &current_visibility_call,
            &original_cave,
        );
        bail!("cave verification failed");
    }

    let patched_hook = encode_patched_hook(cave_cs_offset);
    let patched_visibility_call = encode_visibility_call(wrapper_cs_offset);
    let patch_result: Result<()> = (|| {
        mem.write_bytes(hook_addr, &patched_hook)
            .context("writing hook")?;
        mem.write_bytes(visibility_call_addr, &patched_visibility_call)
            .context("writing visibility call patch")?;

        let mut hook_readback = [0u8; 5];
        mem.read_bytes(hook_addr, &mut hook_readback)
            .context("verifying hook write")?;
        let mut call_readback = [0u8; 3];
        mem.read_bytes(visibility_call_addr, &mut call_readback)
            .context("verifying visibility call write")?;
        ensure!(
            hook_readback == patched_hook && call_readback == patched_visibility_call,
            "patch verification failed"
        );
        Ok(())
    })();
    if let Err(err) = patch_result {
        rollback_partial_patch(
            mem,
            hook_addr,
            visibility_call_addr,
            cave_addr,
            &current_hook,
            &current_visibility_call,
            &original_cave,
        );
        return Err(err);
    }

    log::debug!("Combined redraw/visibility patch is live");

    Ok(PatchState {
        cs_base,
        cave_cs_offset,
        original_hook: current_hook,
        original_visibility_call: current_visibility_call,
        original_cave,
        owns_installation: true,
        flag_addr,
        visibility_snapshot_addr,
    })
}

// ---------------------------------------------------------------------------
// Patch removal
// ---------------------------------------------------------------------------

/// Restore the original bytes. Errors are swallowed because the process may
/// already be dead.
pub fn remove_patch(mem: &dyn MemoryAccess, state: &PatchState) {
    if !state.owns_installation() {
        log::debug!("Skipping patch removal for adopted resident patch");
        return;
    }
    log::debug!("Removing patch");
    let hook_addr = state.cs_base + HOOK_OFFSET;
    let _ = mem.write_bytes(hook_addr, &state.original_hook);
    let visibility_call_addr = state.cs_base + VISIBILITY_CALL_OFFSET;
    let _ = mem.write_bytes(visibility_call_addr, &state.original_visibility_call);
    let cave_addr = state.cs_base + state.cave_cs_offset;
    let _ = mem.write_bytes(cave_addr, &state.original_cave);
    let _ = mem.write_u8(state.flag_addr, 0);
    let _ = mem.write_u8(
        state.visibility_snapshot_addr + VISIBILITY_SNAPSHOT_READY_OFFSET,
        0,
    );
}

// ---------------------------------------------------------------------------
// Redraw trigger
// ---------------------------------------------------------------------------

/// Set the dirty flag so the next `get_command` poll redraws stats.
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
    use crate::memory::access::{MemoryAccess, MockMemory};
    use std::cell::Cell;

    fn setup_mock() -> (MockMemory, usize) {
        let dos_base: usize = 0;
        let cs_base: usize = 0x20000;
        let total_size = SIG_SCAN_SIZE + 0x2000;
        let mem = MockMemory::new(total_size);

        mem.set_bytes(cs_base + REDRAW_OFFSET, &REDRAW_SIGNATURE);
        mem.set_bytes(cs_base + LOOP_JMP_OFFSET, &[0xE9, 0x41, 0xFF]);
        mem.set_bytes(cs_base + LOOP_TOP_OFFSET, &LOOP_TOP_SIGNATURE);
        mem.set_bytes(cs_base + HOOK_OFFSET, &HOOK_BYTES);
        mem.set_bytes(cs_base + VISIBILITY_CALL_OFFSET, &VISIBILITY_CALL_BYTES);

        (mem, dos_base)
    }

    struct FailingWriteMemory {
        base: MockMemory,
        fail_addr: usize,
        failed_once: Cell<bool>,
    }

    impl FailingWriteMemory {
        fn new(size: usize, fail_addr: usize) -> Self {
            Self {
                base: MockMemory::new(size),
                fail_addr,
                failed_once: Cell::new(false),
            }
        }

        fn set_bytes(&self, addr: usize, bytes: &[u8]) {
            self.base.set_bytes(addr, bytes);
        }
    }

    impl MemoryAccess for FailingWriteMemory {
        fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
            self.base.read_bytes(addr, buf)
        }

        fn write_bytes(&self, addr: usize, data: &[u8]) -> Result<()> {
            if addr == self.fail_addr && !self.failed_once.replace(true) {
                bail!("injected write failure at {addr:#x}");
            }
            self.base.write_bytes(addr, data)
        }
    }

    #[test]
    fn encode_stats_cave_targets_redraw_and_resume() {
        let cave = 0x5001usize;
        let flag_ds = FLAG_SAVE_OFFSET as u16 + DS_SAVE_DELTA;
        let bytes = encode_stats_cave(cave, flag_ds);

        assert_eq!(bytes[0], 0x80);
        assert_eq!(bytes[5], 0x74);
        assert_eq!(bytes[6], 0x08);
        assert_eq!(&bytes[15..20], &HOOK_BYTES);

        let cd = i16::from_le_bytes([bytes[13], bytes[14]]);
        assert_eq!((cave as i32 + 15 + cd as i32) as usize, REDRAW_OFFSET);

        let jd = i16::from_le_bytes([bytes[21], bytes[22]]);
        assert_eq!(
            (cave as i32 + STATS_CAVE_SIZE as i32 + jd as i32) as usize,
            HOOK_RESUME
        );
    }

    #[test]
    fn encode_visibility_wrapper_copies_to_snapshot_and_sets_ready_marker() {
        let wrapper = 0x5018usize;
        let snapshot = VISIBILITY_SNAPSHOT_DS_OFFSET;
        let bytes = encode_visibility_wrapper(wrapper, snapshot);

        let cd = i16::from_le_bytes([bytes[1], bytes[2]]);
        assert_eq!((wrapper as i32 + 3 + cd as i32) as usize, MAP_RENDER_OFFSET);
        assert!(
            bytes.windows(6).any(|window| {
                window == es_store_imm8(snapshot + VISIBILITY_SNAPSHOT_READY_OFFSET as u16, 0x00)
            }),
            "wrapper should clear the ready marker before copying"
        );
        assert!(
            bytes
                .windows(3)
                .any(|window| { window == [0xBF, (snapshot & 0xFF) as u8, (snapshot >> 8) as u8] }),
            "wrapper should point DI at the snapshot body"
        );
        assert!(
            bytes.windows(6).any(|window| {
                window
                    == es_store_imm8(
                        snapshot + VISIBILITY_SNAPSHOT_READY_OFFSET as u16,
                        VISIBILITY_SNAPSHOT_READY_MARKER,
                    )
            }),
            "wrapper should set the ready marker after copying"
        );
    }

    #[test]
    fn apply_succeeds() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).expect("should succeed");
        assert_eq!(state.flag_addr, dos_base + SAVE_BASE + FLAG_SAVE_OFFSET);
        assert_eq!(
            state.visibility_snapshot_addr(),
            dos_base + SAVE_BASE + VISIBILITY_SNAPSHOT_SAVE_OFFSET
        );

        let mut hook = [0u8; 5];
        mem.read_bytes(state.cs_base + HOOK_OFFSET, &mut hook)
            .unwrap();
        assert_eq!(hook, encode_patched_hook(state.cave_cs_offset));

        let mut call = [0u8; 3];
        mem.read_bytes(state.cs_base + VISIBILITY_CALL_OFFSET, &mut call)
            .unwrap();
        assert_eq!(
            call,
            encode_visibility_call(state.cave_cs_offset + STATS_CAVE_SIZE)
        );

        let ready_addr = state.visibility_snapshot_addr() + VISIBILITY_SNAPSHOT_READY_OFFSET;
        assert_eq!(
            mem.read_u8(ready_addr).unwrap(),
            0,
            "new snapshot buffer should start invalid"
        );
    }

    #[test]
    fn apply_idempotent() {
        let (mem, dos_base) = setup_mock();
        let s1 = apply_patch(&mem, dos_base).expect("first");
        let s2 = apply_patch(&mem, dos_base).expect("second (adopt)");
        let mut installed_cave = vec![0u8; CAVE_BLOCK_SIZE];
        mem.read_bytes(s1.cs_base + s1.cave_cs_offset, &mut installed_cave)
            .unwrap();
        assert!(s1.owns_installation());
        assert!(!s2.owns_installation());
        assert_eq!(s1.flag_addr, s2.flag_addr);
        assert_eq!(s1.cave_cs_offset, s2.cave_cs_offset);
        assert_eq!(s1.visibility_snapshot_addr, s2.visibility_snapshot_addr);
        assert_eq!(s2.original_cave, installed_cave);
    }

    #[test]
    fn apply_rejects_incompatible_existing_patch() {
        let (mem, dos_base) = setup_mock();
        let state = apply_patch(&mem, dos_base).expect("first");
        mem.write_u8(
            state.cs_base + state.cave_cs_offset + STATS_CAVE_SIZE + 12,
            0x99,
        )
        .unwrap();

        let err = apply_patch(&mem, dos_base).expect_err("modified cave should not be adopted");
        assert!(
            err.to_string()
                .contains("incompatible existing resident patch"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn apply_rolls_back_if_late_patch_write_fails() {
        let dos_base: usize = 0;
        let cs_base: usize = 0x20000;
        let total_size = SIG_SCAN_SIZE + 0x2000;
        let cave_cs_offset = CAVE_SCAN_START + 1;
        let fail_addr = cs_base + VISIBILITY_CALL_OFFSET;
        let mem = FailingWriteMemory::new(total_size, fail_addr);

        mem.set_bytes(cs_base + REDRAW_OFFSET, &REDRAW_SIGNATURE);
        mem.set_bytes(cs_base + LOOP_JMP_OFFSET, &[0xE9, 0x41, 0xFF]);
        mem.set_bytes(cs_base + LOOP_TOP_OFFSET, &LOOP_TOP_SIGNATURE);
        mem.set_bytes(cs_base + HOOK_OFFSET, &HOOK_BYTES);
        mem.set_bytes(cs_base + VISIBILITY_CALL_OFFSET, &VISIBILITY_CALL_BYTES);

        let err = apply_patch(&mem, dos_base).expect_err("patched call write should fail");
        assert!(
            err.to_string().contains("writing visibility call patch"),
            "unexpected error: {err:#}"
        );

        let mut hook = [0u8; 5];
        mem.read_bytes(cs_base + HOOK_OFFSET, &mut hook).unwrap();
        assert_eq!(hook, HOOK_BYTES);

        let mut call = [0u8; 3];
        mem.read_bytes(cs_base + VISIBILITY_CALL_OFFSET, &mut call)
            .unwrap();
        assert_eq!(call, VISIBILITY_CALL_BYTES);

        let mut cave = vec![0u8; CAVE_BLOCK_SIZE];
        mem.read_bytes(cs_base + cave_cs_offset, &mut cave).unwrap();
        assert_eq!(cave, vec![0u8; CAVE_BLOCK_SIZE]);
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

        let mut call = [0u8; 3];
        mem.read_bytes(state.cs_base + VISIBILITY_CALL_OFFSET, &mut call)
            .unwrap();
        assert_eq!(call, VISIBILITY_CALL_BYTES);
    }

    #[test]
    fn remove_patch_skips_adopted_patch_state() {
        let (mem, dos_base) = setup_mock();
        let installed = apply_patch(&mem, dos_base).unwrap();
        let adopted = apply_patch(&mem, dos_base).unwrap();

        remove_patch(&mem, &adopted);

        let mut hook = [0u8; 5];
        mem.read_bytes(installed.cs_base + HOOK_OFFSET, &mut hook)
            .unwrap();
        assert_eq!(hook, encode_patched_hook(installed.cave_cs_offset));

        let mut call = [0u8; 3];
        mem.read_bytes(installed.cs_base + VISIBILITY_CALL_OFFSET, &mut call)
            .unwrap();
        assert_eq!(
            call,
            encode_visibility_call(installed.cave_cs_offset + STATS_CAVE_SIZE)
        );
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
