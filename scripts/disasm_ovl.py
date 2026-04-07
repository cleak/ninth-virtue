"""Disassemble a DOS OVL file (raw 16-bit x86 code) and search for
instructions that reference character-data offsets.

Usage:
    python scripts/disasm_ovl.py <file.OVL>
    python scripts/disasm_ovl.py ghidra_projects/ZSTATS.OVL
"""

import sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_16

# Known offsets we're looking for in operands
INTERESTING_VALS = {
    0x0B: "CHAR_STATUS",
    0x10: "CHAR_HP",
    0x11: "CHAR_HP+1",
    0x12: "CHAR_MAXHP",
    0x13: "CHAR_MAXHP+1",
    0x20: "CHAR_RECORD_SIZE",
    0x02: "CHAR_RECORDS_OFFSET",
}

def disassemble(filepath):
    with open(filepath, "rb") as f:
        code = f.read()

    print(f"File: {filepath}")
    print(f"Size: {len(code)} bytes")
    print()

    md = Cs(CS_ARCH_X86, CS_MODE_16)
    md.detail = True

    instructions = list(md.disasm(code, 0x0000))
    print(f"Total instructions: {len(instructions)}")

    # ----------------------------------------------------------------
    # Full disassembly
    # ----------------------------------------------------------------
    print("\n=== FULL DISASSEMBLY ===")
    for instr in instructions:
        hexbytes = " ".join(f"{b:02X}" for b in instr.bytes)
        print(f"  {instr.address:04X}: {hexbytes:<20s}  {instr.mnemonic:<8s} {instr.op_str}")

    # ----------------------------------------------------------------
    # Find call/jmp targets (function boundaries)
    # ----------------------------------------------------------------
    print("\n=== CALL TARGETS (likely function entry points) ===")
    call_targets = set()
    for instr in instructions:
        if instr.mnemonic in ("call", "jmp") and instr.op_str.startswith("0x"):
            try:
                target = int(instr.op_str, 16)
                call_targets.add(target)
                print(f"  {instr.address:04X}: {instr.mnemonic} {instr.op_str}")
            except ValueError:
                pass

    # ----------------------------------------------------------------
    # Find instructions referencing interesting scalar values
    # ----------------------------------------------------------------
    print("\n=== INSTRUCTIONS WITH INTERESTING OFFSETS ===")
    for instr in instructions:
        op_str = instr.op_str
        # Check for interesting values in operands
        for val, name in INTERESTING_VALS.items():
            # Look for the value as a displacement or immediate
            hex_patterns = [f"0x{val:x}", f"+ 0x{val:x}", f"- 0x{val:x}"]
            for pat in hex_patterns:
                if pat in op_str:
                    hexbytes = " ".join(f"{b:02X}" for b in instr.bytes)
                    print(f"  {instr.address:04X}: {hexbytes:<20s}  {instr.mnemonic:<8s} {instr.op_str}  [{name}]")
                    break

    # ----------------------------------------------------------------
    # Find INT 21h calls (DOS API) and INT 10h (BIOS video)
    # ----------------------------------------------------------------
    print("\n=== INTERRUPT CALLS ===")
    for instr in instructions:
        if instr.mnemonic == "int":
            hexbytes = " ".join(f"{b:02X}" for b in instr.bytes)
            print(f"  {instr.address:04X}: {hexbytes:<20s}  {instr.mnemonic:<8s} {instr.op_str}")

    # ----------------------------------------------------------------
    # Find string data (sequences of printable ASCII)
    # ----------------------------------------------------------------
    print("\n=== EMBEDDED STRINGS ===")
    i = 0
    while i < len(code):
        if 0x20 <= code[i] <= 0x7E:
            start = i
            while i < len(code) and 0x20 <= code[i] <= 0x7E:
                i += 1
            if i - start >= 4:  # at least 4 printable chars
                s = code[start:i].decode("ascii", errors="replace")
                print(f"  {start:04X}: \"{s}\"")
        else:
            i += 1


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python disasm_ovl.py <file>")
        sys.exit(1)
    disassemble(sys.argv[1])
