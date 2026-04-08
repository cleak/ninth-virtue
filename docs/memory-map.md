# Ultima V Memory Map

Quick reference for all known memory addresses used by ninth-virtue.

## Address Spaces

There are three address spaces to keep straight:

1. **Host process addresses** — the full 64-bit virtual addresses in
   DOSBox's process. These are what `ReadProcessMemory` /
   `WriteProcessMemory` use.

2. **DOS linear addresses** — offsets from `dos_base` (0x00000 to
   0xFFFFF). These map the emulated 1 MB real-mode address space.

3. **Segment:offset addresses** — the 16-bit seg:off notation used in
   the game's x86 code (e.g., `DS:0x55A8`). Linear address =
   `segment * 16 + offset`.

All offsets below are **DOS linear** (relative to `dos_base`) unless
otherwise noted.

## DOS System Areas

| DOS Linear | Size | Description |
|------------|------|-------------|
| `0x00000` | 1024 | Interrupt Vector Table (256 x 4-byte vectors) |
| `0x00400` | 256 | BIOS Data Area (BDA) |
| `0x0041A` | 2 | Keyboard buffer head pointer |
| `0x0041C` | 2 | Keyboard buffer tail pointer |
| `0x0041E` | 32 | Keyboard buffer (16 x 2-byte entries) |
| `0xA0000` | 64K | EGA/VGA graphics memory (segment A000) |
| `0xB8000` | 32K | Text-mode video memory (segment B800) |

## Game Code

| DOS Linear | CS Offset | Description |
|------------|-----------|-------------|
| `cs_base + 0x0000` | `0x0000` | Program entry point / `main()` |
| `cs_base + 0x00B8` | `0x00B8` | Main game loop top |
| `cs_base + 0x0174` | `0x0174` | Main loop backward jump (overlay dispatch) |
| `cs_base + 0x16BA` | `0x16BA` | `putchar` — print one character |
| `cs_base + 0x1850` | `0x1850` | `print_string` — print null-terminated string |
| `cs_base + 0x1A3E` | `0x1A3E` | `print_number` — print number with width/pad |
| `cs_base + 0x266C` | `0x266C` | `get_command` — wait for keyboard input |
| `cs_base + 0x268C` | `0x268C` | `get_command` overlay entry (**hook point**) |
| `cs_base + 0x2726` | `0x2726` | `draw_stats_row` — render one party member |
| `cs_base + 0x2900` | `0x2900` | `redraw_full_stats` — **the target function** |
| `cs_base + 0x3178` | `0x3178` | Overworld command dispatch (A-Z, 0-6) |
| `cs_base + 0x4080` | `0x4080` | Set active player handler |

`cs_base` is discovered at runtime by scanning for the signature of
`redraw_full_stats`: bytes `55 8B EC 83 EC 02 56` at the start of the
function.

## SAVED.GAM Data (Save Offsets)

All relative to `dos_base + SAVE_BASE` where `SAVE_BASE = 0x24826`.
The DS-segment equivalent is `save_offset + 0x55A6`.

### Character Records (save offset 0x02+)

Each record is 32 bytes (`CHAR_RECORD_SIZE = 0x20`). Up to 16 slots.

| Record Offset | DS Offset | Type | Field |
|---------------|-----------|------|-------|
| `0x00` | `+0x00` | 9 bytes | Name (null-terminated) |
| `0x09` | `+0x09` | u8 | Gender (0x0B=M, 0x0C=F) |
| `0x0A` | `+0x0A` | u8 | Class (A/B/F/M) |
| `0x0B` | `+0x0B` | u8 | Status (G/P/S/D) |
| `0x0C` | `+0x0C` | u8 | Strength |
| `0x0D` | `+0x0D` | u8 | Dexterity |
| `0x0E` | `+0x0E` | u8 | Intelligence |
| `0x0F` | `+0x0F` | u8 | Magic Points |
| `0x10` | `+0x10` | u16le | Current HP |
| `0x12` | `+0x12` | u16le | Max HP |
| `0x14` | `+0x14` | u16le | Experience |
| `0x16` | `+0x16` | u8 | Level |
| `0x19` | `+0x19` | 6 bytes | Equipment |

Character 0 base: save offset `0x02`, DS offset `0x55A8`.
Character N base: save offset `0x02 + N * 0x20`.

### Inventory

| Save Offset | DS Offset | Type | Field |
|-------------|-----------|------|-------|
| `0x202` | `0x57A8` | u16le | Food |
| `0x204` | `0x57AA` | u16le | Gold |
| `0x206` | `0x57AC` | u8 | Keys |
| `0x207` | `0x57AD` | u8 | Gems |
| `0x208` | `0x57AE` | u8 | Torches |
| `0x235` | `0x57DB` | u8 | Arrows |
| `0x2AA` | `0x5850` | 8 bytes | Reagents |
| `0x2B5` | `0x585B` | u8 | Party size |
| `0x2E2` | `0x5888` | u8 | Karma |

### Runtime State

| Save Offset | DS Offset | Type | Field |
|-------------|-----------|------|-------|
| `0x2D5` | `0x587B` | u8 | Active player index (0-based, 0xFF=none) |
| `0x2EB` | `0x5891` | u8 | Animations next frame (0, 1, 0xFF) |
| `0x2EC` | `0x5892` | u8 | Wind direction (0-4) |
| `0x2ED` | `0x5893` | u8 | Location / game mode |
| `0x2EF` | `0x5895` | u8 | Party Z coordinate |
| `0x2F0` | `0x5896` | u8 | Party X coordinate |
| `0x2F1` | `0x5897` | u8 | Party Y coordinate |
| `0x2FE` | `0x58A4` | u8 | Update/animate 2D map flag |
| `0x2FF` | `0x58A5` | u8 | Current light intensity |
| `0x300` | `0x58A6` | u8 | Light spell duration (turns) |
| `0x301` | `0x58A7` | u8 | Torch duration (turns) |
| `0x3B0` | `0x5B56` | u8 | New prompt at end of turn |

### Shrine Quest Progress

| Save Offset | DS Offset | Type | Field |
|-------------|-----------|------|-------|
| `0x326` | `0x58CC` | u8 | Ordained shrines (bitmask: bit 0=Honesty, 1=Compassion, 2=Valor, 3=Justice, 4=Sacrifice, 5=Honor, 6=Spirituality, 7=Humility) |
| `0x328` | `0x58CE` | u8 | Codex visited (same bit layout as ordained) |

The two bitmasks encode four quest phases per virtue:

| Ordained | Codex | Phase |
|----------|-------|-------|
| 0 | 0 | Not Started |
| 1 | 0 | Ordained — visit the Codex |
| 1 | 1 | Codex read — return to shrine |
| 0 | 1 | Complete (ordained bit cleared on turn-in) |

### Game Mode Values (save offset 0x2ED / DS:0x5893)

| Value | Mode |
|-------|------|
| `0x00` | Britannia overworld |
| `0x01`-`0x20` | Town or castle interior |
| `0x21`-`0x7F` | Dungeon |
| `0x80`+ | Combat |

## Dirty Flag (Injected by ninth-virtue)

| Save Offset | DS Offset | Description |
|-------------|-----------|-------------|
| `0x3C0` | `0x5966` | Redraw dirty flag — set to 1 by companion app, checked and cleared by injected code cave |

This offset is past the end of the on-disk save file (which ends
around `0x3B2`) but within the runtime-only RAM region.
