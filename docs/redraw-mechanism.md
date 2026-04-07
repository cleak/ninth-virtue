# Stats Panel Redraw Mechanism

This document describes the in-memory code patch that forces the Ultima V
stats panel to refresh after our companion app modifies game state.

## Background

Ultima V has no dirty flag for the stats panel. The `redraw_full_stats`
function at `CS:0x2900` is called directly by six event handlers (damage,
poison, player selection, etc.) and never via a polled flag. See
[reverse-engineering.md](reverse-engineering.md) for the full analysis.

Our solution: inject a code cave into the game's `get_command` function
that checks a dirty flag and calls `0x2900` when set.

## Why `get_command`?

The game uses a DOS overlay system. The main loop at `CS:0x00B8` merely
dispatches to overlay code — the overlay runs its own inner loop and only
returns when the game mode changes. The actual input-processing loop lives
in overlay segment `0x0FC6`, which is 2 paragraphs (32 bytes) above CS.

Because overlay code enters `get_command` at `CS:0x268C` (32 bytes past
the prologue at `CS:0x266C`), our hook is placed at `CS:0x268C`. This is
reached on every input poll regardless of which overlay is loaded, making
it the ideal hook point.

## Architecture

```
 ┌─────────────────────────────────────────────────────────┐
 │                   DOSBox Process                        │
 │                                                         │
 │  Emulated DOS Memory                                    │
 │  ┌───────────────────────────────────────────────────┐  │
 │  │                                                   │  │
 │  │  CS:0x268C  ──JMP──►  CS:code_cave               │  │
 │  │  (was: CMP [0x5893],  │                           │  │
 │  │         0x21)         ▼                           │  │
 │  │                 ┌─────────────────┐               │  │
 │  │                 │ CMP [flag], 0   │               │  │
 │  │                 │ JE skip         │               │  │
 │  │                 │ MOV [flag], 0   │◄── clear flag │  │
 │  │                 │ CALL 0x2900     │◄── redraw!    │  │
 │  │                 │ skip:           │               │  │
 │  │                 │ CMP [0x5893],21 │◄── displaced  │  │
 │  │                 │ JMP 0x2691      │◄── resume     │  │
 │  │                 └─────────────────┘               │  │
 │  │                                                   │  │
 │  │  DS:0x5966  ◄──── WriteProcessMemory(1)           │  │
 │  │  (save+0x3C0)     from ninth-virtue               │  │
 │  └───────────────────────────────────────────────────┘  │
 └─────────────────────────────────────────────────────────┘
          ▲
          │ WriteProcessMemory
 ┌────────┴─────────┐
 │  ninth-virtue     │
 │  companion app    │
 │                   │
 │  trigger_redraw():│
 │    write 1 to     │
 │    flag byte      │
 └───────────────────┘
```

## Patch Details

### Step 1: Locate the Code Segment Base

The game's CS base is found by scanning DOS memory for the 7-byte
signature of `redraw_full_stats` at CS:0x2900: `55 8B EC 83 EC 02 56`.
Cross-validated against the loop-top bytes at CS:0x00B8 and a JMP opcode
at CS:0x0174.

### Step 2: Find a Code Cave

Scan CS:0x4000–0x8000 for a contiguous run of 25+ zero bytes.

### Step 3: Write the Code Cave (23 bytes)

```
Offset  Bytes              Instruction
------  -----              -----------
 0      80 3E xx xx 00     CMP byte [DS:flag], 0
 5      74 08              JE skip  (→ byte 15)
 7      C6 06 xx xx 00     MOV byte [DS:flag], 0
12      E8 xx xx           CALL 0x2900
15      80 3E 93 58 21     CMP byte [0x5893], 0x21  (displaced)
20      E9 xx xx           JMP 0x2691               (resume)
```

### Step 4: Patch the Hook Site (5 bytes at CS:0x268C)

Replace the 5-byte `CMP byte [0x5893], 0x21` with:

```
E9 xx xx 90 90     JMP cave; NOP; NOP
```

### Flag Byte

Save offset `0x3C0` (DS:0x5966) — past the end of the on-disk save file,
in the runtime-only RAM region.

## Safety

- **No reentrancy risk** — the cave runs in the game's normal input-poll
  context, not from an interrupt.
- **All game modes** — `get_command` is called from every overlay.
- **Idempotent** — detects and adopts patches left by previous sessions.
- **Verified writes** — every write is read back and compared.
- **Rollback on failure** — if any write fails verification, all changes
  are reverted before returning an error.
- **Clean detach** — original bytes restored when ninth-virtue disconnects.
