"""Disassemble a DOS MZ EXE and search for the mini-stats panel renderer.

We're looking for code that reads character HP ([bx+0x10]) and status
([bx+0x0B]) to render the compact stats panel during gameplay.

Usage:
    python scripts/disasm_exe.py ghidra_projects/ULTIMA.EXE
"""

import sys
import struct
from capstone import Cs, CS_ARCH_X86, CS_MODE_16

def parse_mz_header(data):
    """Parse DOS MZ EXE header, return code start offset and size."""
    if data[:2] != b'MZ' and data[:2] != b'ZM':
        print("Not a DOS MZ executable, treating as raw binary")
        return 0, len(data)

    header_paragraphs = struct.unpack_from('<H', data, 0x08)[0]
    code_start = header_paragraphs * 16
    print(f"MZ header: {header_paragraphs} paragraphs = {code_start} bytes header")
    print(f"Code starts at offset {code_start:#x}")
    print(f"Total file size: {len(data)} bytes")
    print(f"Code size: {len(data) - code_start} bytes")
    return code_start, len(data) - code_start

def disassemble_exe(filepath):
    with open(filepath, "rb") as f:
        data = f.read()

    code_start, code_size = parse_mz_header(data)
    code = data[code_start:]

    md = Cs(CS_ARCH_X86, CS_MODE_16)
    md.detail = True

    instructions = list(md.disasm(code, 0x0000))
    print(f"Total instructions: {len(instructions)}")

    # ----------------------------------------------------------------
    # Search for the mini-stats renderer: code that reads [bx+0x10] (HP)
    # and [bx+0x0B] (status) from character records, AND pushes to
    # a display function.
    # ----------------------------------------------------------------
    print("\n=== HP/STATUS READS (bx+0x10 and bx+0x0B) ===")
    hp_reads = []
    status_reads = []
    for instr in instructions:
        op = instr.op_str
        if "bx + 0x10]" in op or "bx + 0x10]" in op:
            if "0x10]" in op and ("push" in instr.mnemonic or "mov" in instr.mnemonic):
                hp_reads.append(instr)
                hx = " ".join(f"{b:02X}" for b in instr.bytes)
                print(f"  HP:  {instr.address:04X}: {hx:<20s}  {instr.mnemonic:<8s} {op}")
        if "bx + 0xb]" in op:
            if "push" in instr.mnemonic or "mov" in instr.mnemonic:
                status_reads.append(instr)
                hx = " ".join(f"{b:02X}" for b in instr.bytes)
                print(f"  STS: {instr.address:04X}: {hx:<20s}  {instr.mnemonic:<8s} {op}")

    # ----------------------------------------------------------------
    # For each HP read, show surrounding context (20 instructions before/after)
    # ----------------------------------------------------------------
    print(f"\n=== CONTEXT AROUND HP READS ({len(hp_reads)} found) ===")
    instr_by_addr = {i.address: idx for idx, i in enumerate(instructions)}

    for hp_instr in hp_reads:
        idx = instr_by_addr.get(hp_instr.address)
        if idx is None:
            continue
        print(f"\n--- Context around {hp_instr.address:04X} ---")
        start = max(0, idx - 20)
        end = min(len(instructions), idx + 20)
        for i in range(start, end):
            instr = instructions[i]
            marker = " >>>" if i == idx else "    "
            hx = " ".join(f"{b:02X}" for b in instr.bytes)
            print(f"{marker} {instr.address:04X}: {hx:<20s}  {instr.mnemonic:<8s} {instr.op_str}")

    # ----------------------------------------------------------------
    # Also search for the pattern: shl ax, 5; add ax, 0x55A8
    # (character record base calculation we saw in ZSTATS.OVL)
    # ----------------------------------------------------------------
    print("\n=== CHARACTER BASE CALCULATIONS (shl+add 0x55A8) ===")
    for i, instr in enumerate(instructions):
        if instr.mnemonic == "add" and "0x55a8" in instr.op_str:
            print(f"  {instr.address:04X}: {instr.mnemonic} {instr.op_str}")
            # Show context
            start = max(0, i - 5)
            end = min(len(instructions), i + 5)
            for j in range(start, end):
                ci = instructions[j]
                marker = " >>>" if j == i else "    "
                print(f"{marker}   {ci.address:04X}: {ci.mnemonic} {ci.op_str}")

    # ----------------------------------------------------------------
    # Search for "Set Active Plr" or related strings
    # ----------------------------------------------------------------
    print("\n=== EMBEDDED STRINGS ===")
    i = 0
    while i < len(data):
        if 0x20 <= data[i] <= 0x7E:
            start = i
            while i < len(data) and 0x20 <= data[i] <= 0x7E:
                i += 1
            if i - start >= 4:
                s = data[start:i].decode("ascii", errors="replace")
                if any(kw in s.lower() for kw in ["stat", "plr", "player", "active", "party", "name", "hp", "health"]):
                    print(f"  {start:04X} (code {start - code_start:04X}): \"{s}\"")
        else:
            i += 1


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python disasm_exe.py <file>")
        sys.exit(1)
    disassemble_exe(sys.argv[1])
