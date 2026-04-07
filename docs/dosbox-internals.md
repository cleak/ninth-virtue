# DOSBox Internals for Code Injection

Notes on how DOSBox's emulation works and what constraints apply when
modifying emulated memory from an external process.

## DOSBox Memory Architecture

DOSBox allocates a single large contiguous block of host memory to
represent the emulated DOS address space. This block is pointed to
internally by the `MemBase` global variable.

Our scanner (`memory/scanner.rs`) locates this block by enumerating
committed RW memory regions in the DOSBox process and matching against
known sizes:

| Config `memsize` | Region Size | With Guard Page |
|------------------|-------------|-----------------|
| 4 MB | `0x0400000` | `0x0401000` |
| 8 MB | `0x0800000` | `0x0801000` |
| 16 MB (default) | `0x1000000` | `0x1001000` |
| 32 MB | `0x2000000` | `0x2001000` |

The first 1 MB of this block is conventional DOS memory (0x00000-0xFFFFF).
Everything above is extended memory (XMS/EMS), which Ultima V does not use.

## CPU Core Types

DOSBox has three CPU emulation cores:

### Normal (Interpreter)

- Reads each x86 instruction from emulated memory, decodes, executes.
- **No code caching.** Writes to emulated memory via `WriteProcessMemory`
  are immediately visible on the next instruction fetch.
- This is the core used for real-mode programs like Ultima V.

### Dynamic (JIT Recompiler)

- Translates blocks of emulated x86 into native host x86 and caches
  the result.
- **Code caching problem:** `WriteProcessMemory` from an external process
  bypasses DOSBox's internal page handlers, so no cache invalidation is
  triggered. Modified code may not be seen until the cache block is
  evicted for unrelated reasons.
- Used for protected-mode programs (Windows 3.x, DOS4GW games).
- DOSBox does have self-modifying code (SMC) detection, but only for
  writes that go through the emulated CPU — not external writes.

### Simple

- Simplified interpreter, falls back to normal for complex situations.
- Same behavior as normal for our purposes.

### Which Core Does Ultima V Use?

With `core=auto` (the default in DOSBox Staging), real-mode programs
use the **normal** core. Since Ultima V is a real-mode DOS game, our
code patches work correctly.

## WriteProcessMemory Behavior

When we call `WriteProcessMemory` to modify DOSBox's emulated memory:

1. **Data writes** (game state, flags, inventory) — always safe on any
   core. The emulated CPU reads these as data, not instructions.

2. **Code writes** (injecting new instructions, patching jumps) — safe
   on the normal/simple core. The interpreter re-reads from memory on
   every instruction, so patches take effect immediately.

3. **IVT writes** (modifying interrupt vectors) — same as data writes.
   The emulated CPU reads the IVT on each interrupt dispatch. Changes
   via `WriteProcessMemory` are visible on the next interrupt.

4. **Timing** — `WriteProcessMemory` is not synchronized with DOSBox's
   emulation loop. There is a theoretical race where we write mid-
   instruction-decode. In practice this is negligible because:
   - Writes are fast (nanoseconds for a few bytes)
   - DOSBox emulates in batched cycles
   - We can order writes carefully (write the cave first, then the
     hook jump last)

## DOSBox Callback Mechanism

DOSBox uses a special invalid opcode `0xFE 0x38 <uint16>` as a trap
instruction. When the emulated CPU encounters this opcode, DOSBox
intercepts it and calls a registered native C++ callback function.
BIOS services, DOS INT 21h, and device drivers all use this mechanism.

This is **not useful** for our purposes — callback registration happens
inside DOSBox's C++ code and cannot be done from an external process.

## Implications for ninth-virtue

### Safe Operations (what we already do)

- Read/write game data (character stats, inventory, flags)
- Read/write the BIOS Data Area (keyboard buffer)
- Modify the IVT (interrupt vectors)

### New: Code Injection (what we plan to do)

- Write a small code cave (~16 bytes) into unused space in the game's
  code segment.
- Patch a 3-byte JMP instruction in the main game loop.
- The normal core ensures our injected code executes correctly.

### Constraints

- **Always write the cave before the hook.** The cave must be complete
  and correct before we redirect the JMP to it. Otherwise the game
  could jump to incomplete code.
- **Restore on detach.** When ninth-virtue disconnects, restore the
  original JMP bytes to unpatch the main loop.
- **Don't assume JIT works.** If someone forces `core=dynamic`, our
  code patch may silently fail. We could detect this by reading back
  the game mode byte after patching and checking if the flag gets
  cleared — if it doesn't, the patch isn't being executed.
