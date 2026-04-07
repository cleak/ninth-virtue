# Ultima V Reverse Engineering Notes

This document records our reverse engineering of Ultima V's rendering
pipeline, specifically how the party stats panel gets redrawn and what
we can do to trigger a refresh from an external companion app.

## Problem Statement

When ninth-virtue writes to game memory (heal, cure poison, add arrows,
etc.), the in-game stats panel does not update. The display shows stale
values until the player performs an action that triggers a redraw. Our
goal: find a way to force the redraw programmatically.

## Key Files

| File | Size | Purpose |
|------|------|---------|
| `ULTIMA.EXE` | 36,592 bytes | Main executable (MZ DOS EXE) |
| `ZSTATS.OVL` | 4,880 bytes | Z-stats full character sheet overlay |
| `COMSUBS.OVL` | 5,216 bytes | Common gameplay subroutines |
| `MAINOUT.OVL` | 7,344 bytes | Overworld outdoor logic |
| `CMDS.OVL` | 7,440 bytes | Command processing |
| `COMBAT.OVL` | — | Combat mode overlay |
| `TALK.OVL` | — | NPC conversation overlay |
| `SHOPPES.OVL` | — | Shop/trade overlay |

The game uses a DOS overlay system: `ULTIMA.EXE` is the resident core,
and `.OVL` files are loaded/unloaded into a shared overlay area as
needed for different game modes.

## Tools Used

- **Python + capstone** — 16-bit x86 disassembly of raw OVL files and
  the MZ EXE code segment.
- **memdiff** (custom CLI tool) — continuous memory polling across full
  640 KB DOS conventional memory with noise-filtering baseline.
- **poke** (custom CLI tool) — read/write/dump arbitrary DOSBox memory.
- **Memory Watch panel** (GUI) — real-time byte-change logger built
  into the companion app.

## Memory Layout

### Segment Mapping

The game runs in real-mode x86. Through empirical testing we
established the mapping between the game's DS-relative addresses and
our save-file-relative offsets:

```
DS:0x55A8 = save offset 0x0002 (first character record)
DS:0x587B = save offset 0x02D5 (ACTIVE_PLAYER)
DS:0x585B = save offset 0x02B5 (PARTY_SIZE)
DS:0x5893 = save offset 0x02ED (LOCATION / game mode)

General formula:
  save_offset = DS_offset - 0x55A6
  DS_offset   = save_offset + 0x55A6
```

### SAVED.GAM in DOS Memory

The SAVED.GAM image is loaded at DOS linear address `dos_base + 0x24826`
(our `SAVE_BASE` constant). The data segment base is:

```
DS * 16 = dos_base + 0x24826 - 0x55A6 + 0x02
        = dos_base + 0x1F282

(equivalently: dos_base + SAVE_BASE + save_offset = DS*16 + DS_offset)
```

### Key Runtime Variables (not saved to disk)

| DS Offset | Save Offset | Description |
|-----------|-------------|-------------|
| `0x5893` | `0x02ED` | Game mode / location (0=overworld, 1-0x20=town, 0x21-0x7F=dungeon, 0x80+=combat) |
| `0x587B` | `0x02D5` | Active player index (0-based, 0xFF = none) |
| `0x585B` | `0x02B5` | Party size |
| `0x589E` | `0x02F8` | Unknown — used in stats row rendering for magic/combat indicators |

---

## Disassembly Analysis

### ULTIMA.EXE Structure

The MZ header is 2,048 bytes (128 paragraphs). Code begins at file
offset `0x800`. The code segment contains the main game loop,
input handling, stats rendering, and the overlay loader.

### Function Map (code-segment offsets)

| Offset | Name | Description |
|--------|------|-------------|
| `0x0000` | `main` | Program entry point, initialization |
| `0x00B0` | — | Initial call to `redraw_full_stats` |
| `0x00B8` | `main_loop_top` | **Main game loop start** |
| `0x0174` | — | Main loop backward jump (`JMP 0x00B8`) |
| `0x16BA` | `putchar` | Print a single character |
| `0x1850` | `print_string` | Print a null-terminated string |
| `0x1A3E` | `print_number` | Print a number with padding |
| `0x1B94` | `set_display_mode` | Set cursor/window display state |
| `0x1BF2` | `set_cursor_pos` | Position cursor for output |
| `0x1F12` | `get_cursor_x` | Get current cursor X position |
| `0x216C` | `string_length` | Get length of a string for padding |
| `0x266C` | `get_command` | Wait for keyboard input |
| `0x2726` | `draw_stats_row` | Draw one party member's stats row |
| `0x2900` | `redraw_full_stats` | **Redraw entire party stats panel** |
| `0x2A28` | `pre_damage` | Called before applying HP damage |
| `0x3178` | `overworld_cmd_handler` | Full A-Z command dispatch |
| `0x4080` | `set_active_player` | Handle '0'-'6' keypresses |
| `0x7A16`-`0x7A52` | overlay calls | Stubs that call into loaded overlay code |

### The Stats Panel Redraw Function (0x2900)

This is the function we need to trigger. It loops through all 6 party
slots and draws each one:

```asm
2900: push    bp
2901: mov     bp, sp
2903: sub     sp, 2
2906: push    si
2907: mov     ax, 1
290A: push    ax
290B: call    0x1B94          ; set_display_mode(1)
290E: sub     si, si          ; si = 0 (player index)
2910: push    si
2911: call    0x2726          ; draw_stats_row(si)
2914: inc     si
2915: cmp     si, 6           ; loop for all 6 slots
2918: jl      0x2910
```

### The Stats Row Renderer (0x2726)

Takes a player index as argument. Computes the character record base
address (`player_index * 32 + 0x55A8`), then renders:

1. **Name** — via `print_string` at the character record base
2. **Active player indicator** — arrow/cursor if this is the active player
3. **HP** — read from `[bx + 0x10]` (character record offset 0x10),
   printed as 4-digit number via `print_number`
4. **Status character** — read from `[bx + 0x0B]`:
   - `0x47` = 'G' (Good)
   - `0x50` = 'P' (Poisoned)
   - `0x44` = 'D' (Dead)
   - `0x53` = 'S' (Sleeping)

Key instruction references:
```asm
2795: add     ax, 0x55A8      ; character record base
2804: push    [bx + 0x55B8]   ; push HP (0x55A8 + 0x10)
2843: mov     al, [bx+0x55B3] ; read status (0x55A8 + 0x0B)
```

### Who Calls redraw_full_stats (0x2900)?

Six call sites — **none use a dirty flag**; all are direct calls from
specific event handlers:

| Call Site | Context | Trigger |
|-----------|---------|---------|
| `0x00B0` | `main` initialization | Game start / save load |
| `0x2AA0` | After HP damage | Character takes damage, possibly dies |
| `0x2BC7` | Torch/spell duration tick | End-of-turn time passage |
| `0x2FC7` | Poison application | Character becomes poisoned |
| `0x40AE` | `set_active_player` | Player presses '1'-'6' or '0' |
| `0x509E` | Rest/time passage | Camping, level-up check |

**Critical finding:** There is no dirty flag anywhere. The rendering is
purely event-driven — each handler calls `0x2900` directly when it
knows the display needs updating.

### The Main Game Loop (0x00B8 - 0x0174)

```asm
00B8: mov     word ptr [bp-2], 0      ; clear iteration flag
00BD: cmp     byte ptr [0x5893], 0    ; check game mode
00C2: jne     ...                     ; branch by mode
      ; Overworld: call overlay at 0x7A3A
      ; Town:      call overlay at 0x7A46 / 0x7A52
      ; Dungeon:   call overlay at 0x7A16
      ...
0174: jmp     0x00B8                  ; loop back
```

The main loop delegates to overlay-loaded handlers for each game mode.
The overlay handler reads input, dispatches commands, updates state, and
calls `redraw_full_stats` when appropriate.

### The Set Active Player Handler (0x4080)

This is the handler called when the player presses '1'-'6':

```asm
4080: push    bp
4083: sub     sp, 4
408C: mov     ax, [bp+4]           ; ax = key code
408F: sub     ax, 0x31             ; '1' -> 0, '2' -> 1, etc.
4092: mov     [bp-4], ax           ; store player index
4095: mov     ax, 0xA396           ; "Set Active Plr:\n" string
4098: push    ax
4099: call    0x1850               ; print the message
      ...
40A9: mov     byte ptr [0x587B], 0xFF  ; clear active player (key '0')
40AE: call    0x2900                    ; redraw stats
      ...
40DA: mov     [0x587B], al         ; set active player (keys '1'-'6')
```

**Important:** This always prints "Set Active Plr: <name>" to the game
message area, which is a visible side effect.

### The Command Dispatch (0x3178)

The overworld command handler dispatches on ASCII key values:

```
Keys A-E, SPACE: CMP/JE chain at 0x31A0
Keys F-L:        Jump table at 0x3490
Key  M:          Direct compare at 0x3186
Keys N-Z:        CMP/JNE chain at 0x34A8
Keys 1-6:        Handled at 0x3B12 (sub ax, 0x31, bounds-check against party size)
Key  0:          Jumps to 0x3AF7 (reset active player)
Unrecognized:    Falls through to 0x34D8 (error)
```

---

## Approaches Investigated and Rejected

### 1. Toggle Save-Data Flags

**Tried:** Setting `UPDATE_MAP` (0x2FE), `ANIM_NEXT_FRAME` (0x2EB), and
`NEW_PROMPT` (0x3B0) to 1 after memory writes.

**Result:** No effect on the stats panel. These flags control map
animation and turn processing, not the character stats display.

### 2. Toggle ACTIVE_PLAYER (0x2D5)

**Tried:** Write a different value then immediately restore the original.

**Result:** No effect. The game doesn't poll this byte — it only acts on
it when the keyboard handler sets it.

### 3. Memory Diff Scanning

**Tried:** Used memdiff to capture all 640 KB of DOS memory while
switching active players. Identified ~560 candidate addresses that
changed and reverted.

**Result:** The candidates fell into clear clusters:
- ~480 addresses at 4-byte stride (EGA screen buffer — the *result*
  of the redraw, not the trigger)
- ~20 addresses in display workspace regions
- ~46 addresses in small clusters (game engine variables, cursor state)

**Poking all 46 small-cluster candidates** produced no stats redraw.
Cluster E (just before SAVE_BASE) moved the display cursor around but
didn't trigger a redraw. **Confirmed: no dirty flag exists.**

### 4. Direct Screen Buffer Writes

**Researched:** Write the correct pixel data directly to the EGA screen
buffer (the 4-byte-stride block at save+0x6BDED).

**Rejected:** Too complex — requires reverse-engineering the font
bitmaps, EGA plane interleaving, and character-to-pixel mapping. Also
fragile across DOSBox versions.

### 5. INT 0x1C Timer Hook

**Researched:** Hook the user timer tick interrupt to call `0x2900`
when a flag is set.

**Rejected:** The stats redraw function is not reentrant. Calling it
from interrupt context while the game is mid-draw would corrupt global
display state, video registers, and potentially crash.

### 6. BIOS Keyboard Buffer Injection

**Researched:** Write a keypress directly into the BIOS keyboard buffer
at `dos_base + 0x41E` (bypassing SendInput/host OS entirely).

**Partially viable:** Works mechanically, but:
- Always prints "Set Active Plr: <name>" message (visible side effect)
- Dangerous in conversations, combat, shopping (key has different meaning)
- Requires game mode checking via save offset 0x2ED

### 7. SendInput / PostMessage

**Rejected by user:** Sending keypresses to the DOSBox window is unsafe:
- Passing a turn (`Space`) can be lethal in combat
- Opening screens (`Z`) is disruptive
- Re-selecting the active player prints a message
- Any key could interfere with conversations or menus

---

## Chosen Approach: `get_command` Hook via Code Cave

See [redraw-mechanism.md](redraw-mechanism.md) for the full design.

**Summary:** Hook the overlay's entry into `get_command` at `CS:0x268C`
with a JMP to a 23-byte code cave. The cave checks a dirty flag byte
at `DS:0x5966` (save offset `0x3C0`); when set, it clears the flag and
calls `redraw_full_stats` before proceeding to the real function.

### Key Discovery: Overlay Segment Offset

The main loop at `CS:0x00B8-0x0174` was our first patch target, but it
never fires during gameplay — the overlay manager redirects execution
into overlay segment `0x0FC6` which has its own inner loop. The overlay
segment is 2 paragraphs (32 bytes) above CS (`0x0FC4`), so overlay code
calling `get_command` at offset `0x266C` actually enters `CS:0x268C`.

### Why This Works

1. **No reentrancy risk** — runs in the game's normal input-poll context.
2. **No visible side effects** — calls `0x2900` directly, no messages.
3. **Works in any game mode** — `get_command` is called from every overlay.
4. **DOSBox compatible** — real-mode interpreter core, no JIT cache issues.
5. **Reversible** — original bytes restored on detach.
