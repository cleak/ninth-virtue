# Stats Panel Redraw Mechanism

This document describes the in-memory code patch that forces the Ultima V
stats panel to refresh after our companion app modifies game state.

## Background

Ultima V has no dirty flag for the stats panel. The `redraw_full_stats`
function at `CS:0x2900` is called directly by six event handlers (damage,
poison, player selection, etc.) and never via a polled flag. See
[reverse-engineering.md](reverse-engineering.md) for the full analysis.

Our solution: patch the game's main loop in emulated DOS memory to add a
dirty-flag check that calls `0x2900` when set.

## Architecture

```
 ┌─────────────────────────────────────────────────────────┐
 │                   DOSBox Process                        │
 │                                                         │
 │  Emulated DOS Memory (640 KB)                           │
 │  ┌───────────────────────────────────────────────────┐  │
 │  │                                                   │  │
 │  │  CS:0x0174  ──JMP──►  CS:code_cave               │  │
 │  │  (was: JMP 0x00B8)    │                           │  │
 │  │                       ▼                           │  │
 │  │                 ┌─────────────────┐               │  │
 │  │                 │ CMP [flag], 0   │               │  │
 │  │                 │ JE skip         │               │  │
 │  │                 │ MOV [flag], 0   │◄── clear flag │  │
 │  │                 │ CALL 0x2900     │◄── redraw!    │  │
 │  │                 │ skip:           │               │  │
 │  │                 │ JMP 0x00B8      │◄── resume     │  │
 │  │                 └─────────────────┘    loop       │  │
 │  │                                                   │  │
 │  │  DS:flag_addr  ◄──── WriteProcessMemory(1)        │  │
 │  │                       from ninth-virtue           │  │
 │  └───────────────────────────────────────────────────┘  │
 │                                                         │
 └─────────────────────────────────────────────────────────┘
          ▲
          │ WriteProcessMemory
          │
 ┌────────┴─────────┐
 │  ninth-virtue     │
 │  companion app    │
 │                   │
 │  nudge_redraw():  │
 │    write 1 to     │
 │    flag_addr      │
 └───────────────────┘
```

## Step-by-Step Process

### Step 1: Locate the Code Segment Base

The game's code segment (CS) base address in DOSBox's emulated memory is
not fixed — it depends on how DOS loaded the executable. We locate it by
searching for a known byte signature.

**Signature:** The `redraw_full_stats` function at `CS:0x2900` begins with:

```
55 8B EC 83 EC 02 56    push bp; mov bp,sp; sub sp,2; push si
```

We scan the full 640 KB of DOS memory for these 7 bytes. When found at
linear address `L`:

```
cs_base = L - 0x2900
```

We validate by checking a second known sequence at another offset (e.g.,
the `main_loop_top` at `CS:0x00B8` or the `print_string` function at
`CS:0x1850`).

### Step 2: Choose a Flag Byte Address

We need a byte in the data segment that:
- The game never reads or writes
- Is addressable from our code cave
- We can reliably locate via `WriteProcessMemory`

**Candidate:** We use a byte at a known unused location in the save-data
area. Several bytes in the 0x2E3-0x2E4 range are documented as "Unknown"
in the Ultima V internal formats wiki and appear inert in our memory-diff
testing.

Alternatively, we can use a byte just past the end of the documented save
format (e.g., save offset `0x1060`), since the game's RAM image extends
well beyond what gets saved to disk.

The DS-relative address is computed as:

```
flag_ds_offset = 0x55A6 + chosen_save_offset
```

### Step 3: Find a Code Cave

We need ~15-20 bytes of unused space in the code segment to place our
stub. Candidates:

1. **NOP padding** — the disassembly shows single `0x90` NOP bytes at
   function boundaries (e.g., `0x0081`, `0x0277`, `0x02A7`). These are
   alignment padding and are never executed.

2. **Slack at end of code** — if the code segment extends past the last
   real instruction, there may be zeroed or garbage bytes we can use.

3. **Unused function** — if we can identify a function that is never
   called at runtime, we can overwrite it.

The stub is small enough (~17 bytes) that even a cluster of 2-3 NOP
bytes at function boundaries could work if we chain them, but ideally
we find a contiguous block.

**Safer approach:** Scan the code segment for a run of 20+ bytes that
are all `0x00` or `0x90` (NOP). These are guaranteed to be unused
padding.

### Step 4: Write the Code Cave

The code cave stub in 16-bit x86 real mode:

```asm
code_cave:
    cmp     byte [flag_ds_offset], 0    ; 4 bytes: 80 3E xx xx 00
    je      skip                        ; 2 bytes: 74 06
    mov     byte [flag_ds_offset], 0    ; 4 bytes: C6 06 xx xx 00
    call    0x2900                      ; 3 bytes: E8 xx xx
skip:
    jmp     0x00B8                      ; 3 bytes: E9 xx xx
                                        ; TOTAL: 16 bytes
```

The `call` and `jmp` displacements are relative to the instruction's
own address, so they must be computed based on where the cave is placed:

```
call_displacement = 0x2900 - (cave_addr + offset_of_call_instr + 3)
jmp_displacement  = 0x00B8 - (cave_addr + offset_of_jmp_instr + 3)
```

We compute the displacement as a signed 16-bit value (little-endian).

### Step 5: Patch the Main Loop Jump

The original backward jump at `CS:0x0174`:

```
E9 41 FF    jmp 0x00B8    (displacement = 0x00B8 - 0x0177 = 0xFF41)
```

We overwrite it with a jump to our code cave:

```
E9 xx xx    jmp code_cave (displacement = cave_addr - 0x0177)
```

This is a 3-byte overwrite — same instruction size, so no alignment
issues.

### Step 6: Trigger a Redraw

From the companion app, `nudge_redraw()` becomes trivial:

```rust
pub fn nudge_redraw(mem: &dyn MemoryAccess, dos_base: usize) -> Result<()> {
    let flag_addr = dos_base + SAVE_BASE + FLAG_SAVE_OFFSET;
    mem.write_u8(flag_addr, 1)?;
    Ok(())
}
```

The game's main loop picks up the flag on its next iteration (typically
within one frame, ~16ms at 60fps), calls `redraw_full_stats`, clears
the flag, and continues normally.

## Safety Analysis

### Reentrancy

**Not a concern.** The code cave runs in the main game loop — the same
context where `0x2900` is normally called. There is no interrupt
involvement and no concurrent execution.

### Game Mode Compatibility

The main loop at `0x00B8-0x0174` runs in **all game modes** (overworld,
town, dungeon, combat). The `0x2900` function is safe to call in all
these modes — it simply redraws the panel based on current game state.

During overlay-driven modal states (conversations, shopping), the main
loop is suspended while the overlay's own input loop runs. Our flag
will simply accumulate and be processed when the overlay returns control
to the main loop. This is the correct behavior — we don't want to
redraw mid-conversation anyway.

### DOSBox Core Compatibility

Ultima V is a real-mode DOS game. DOSBox Staging defaults to `core=auto`,
which uses the **interpreter (normal) core** for real-mode programs.
The interpreter reads instructions directly from emulated memory on
every cycle, so our code patches take effect immediately.

If someone forces `core=dynamic` (JIT), our patches could theoretically
be ignored due to code cache. However:
- The dynamic core is intended for protected-mode games
- Real-mode games almost always use the normal core
- The code cave is in previously-unexecuted memory, so it won't be in
  any JIT cache

### Cleanup

When ninth-virtue detaches from DOSBox, it should restore the original
bytes at `CS:0x0174` to unpatch the main loop. This ensures the game
runs normally after disconnection.

## Memory Map Summary

```
dos_base + 0x00000              Start of emulated DOS memory
dos_base + 0x00070              INT 0x1C vector (not used by us)
dos_base + 0x0041A              BIOS keyboard buffer head pointer
dos_base + 0x0041C              BIOS keyboard buffer tail pointer
dos_base + 0x0041E              BIOS keyboard buffer data (32 bytes)
dos_base + 0x1F282              DS segment base (DS * 16)
dos_base + cs_base              CS segment base (discovered at runtime)
dos_base + cs_base + 0x00B8     Main loop top
dos_base + cs_base + 0x0174     Main loop backward jump (PATCH POINT)
dos_base + cs_base + 0x2900     redraw_full_stats function
dos_base + cs_base + cave_off   Code cave (INJECTED)
dos_base + 0x24826              SAVE_BASE (SAVED.GAM image start)
dos_base + 0x24826 + flag_off   Dirty flag byte (WRITTEN BY APP)
dos_base + 0xA0000              End of conventional DOS memory
```

## Verification Plan

1. **Unit test:** Encode the code cave bytes with known addresses and
   verify the disassembly matches expectations (using capstone).
2. **Integration test:** Attach to DOSBox, apply patch, write flag=1,
   verify the stats panel redraws without any user interaction.
3. **Stability test:** Play the game for several minutes with the patch
   active — enter/exit towns, combat, conversations, shops — to confirm
   no crashes or visual glitches.
4. **Cleanup test:** Detach the companion app and verify the game
   continues running normally with the original bytes restored.
