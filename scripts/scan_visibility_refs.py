"""Scan Ultima V binaries for visibility-related memory references.

Usage:
    python scripts/scan_visibility_refs.py <game-dir-or-file>
    python scripts/scan_visibility_refs.py <game-dir> --context 12 --files MAINOUT.OVL DUNGEON.OVL

The scanner understands DOS MZ EXEs and raw OVL binaries. It searches for
references to the runtime bytes and buffers that currently look relevant to
visibility, lighting, and minimap-generation work.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

from capstone import Cs, CS_ARCH_X86, CS_MODE_16


INTERESTING_ADDRS = {
    0x58A4: "UPDATE_MAP",
    0x58A5: "LIGHT_INTENSITY",
    0x58A6: "LIGHT_SPELL_DUR",
    0x58A7: "TORCH_DUR",
    0x5893: "MAP_LOCATION",
    0x5895: "MAP_Z",
    0x5896: "MAP_X",
    0x5897: "MAP_Y",
    0x589B: "SCROLL_X_BASE",
    0x589C: "SCROLL_Y_BASE",
    0x595A: "DUNGEON_TILES",
    0x6603: "DUNGEON_ORIENTATION_LIVE",
    0xAB02: "VIEWPORT_SCRATCH",
    0xAD14: "COMBAT_TERRAIN_GRID",
    0xA000: "VRAM_SEGMENT",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("targets", nargs="+", help="Game directory or one or more files to scan")
    parser.add_argument("--context", type=int, default=12, help="Instruction context on each side")
    parser.add_argument(
        "--files",
        nargs="*",
        default=None,
        help="Optional filename filters when scanning a directory (for example MAINOUT.OVL DUNGEON.OVL)",
    )
    return parser.parse_args()


def iter_files(raw_targets: list[str], filters: list[str] | None) -> list[Path]:
    paths = [Path(target) for target in raw_targets]
    files: list[Path] = []

    for path in paths:
        if path.is_dir():
            wanted = {name.upper() for name in filters or []}
            candidates = [path / "ULTIMA.EXE", *sorted(path.glob("*.OVL"))]
            for candidate in candidates:
                if not candidate.is_file():
                    continue
                if wanted and candidate.name.upper() not in wanted:
                    continue
                files.append(candidate)
        elif path.is_file():
            files.append(path)
        else:
            raise FileNotFoundError(path)

    return files


def load_code(path: Path) -> tuple[bytes, int]:
    data = path.read_bytes()
    if path.suffix.lower() == ".exe" and data[:2] in (b"MZ", b"ZM"):
        header_paragraphs = struct.unpack_from("<H", data, 0x08)[0]
        code_start = header_paragraphs * 16
        return data[code_start:], code_start
    return data, 0


def find_hits(path: Path, context: int) -> list[str]:
    code, _ = load_code(path)
    md = Cs(CS_ARCH_X86, CS_MODE_16)
    instructions = list(md.disasm(code, 0x0000))
    by_index = {ins.address: index for index, ins in enumerate(instructions)}
    hits: list[tuple[int, list[tuple[int, str]]]] = []

    for ins in instructions:
        op_lower = ins.op_str.lower()
        matched = [
            (addr, name)
            for addr, name in INTERESTING_ADDRS.items()
            if f"0x{addr:04x}" in op_lower
        ]
        if matched:
            hits.append((ins.address, matched))

    rendered: list[str] = []
    if not hits:
        return rendered

    rendered.append(f"FILE {path.name}")
    seen_contexts: set[int] = set()
    for address, matched in hits:
        idx = by_index[address]
        if idx in seen_contexts:
            continue
        seen_contexts.add(idx)
        rendered.append("")
        labels = ", ".join(f"0x{addr:04X} {name}" for addr, name in matched)
        rendered.append(f"-- context around {address:04X} ({labels})")
        start = max(0, idx - context)
        end = min(len(instructions), idx + context + 1)
        for entry in instructions[start:end]:
            marker = ">>" if entry.address == address else "  "
            raw = " ".join(f"{byte:02X}" for byte in entry.bytes)
            rendered.append(
                f"{marker} {entry.address:04X}: {raw:<18} {entry.mnemonic:<6} {entry.op_str}"
            )

    return rendered


def main() -> int:
    args = parse_args()
    try:
        files = iter_files(args.targets, args.files)
    except FileNotFoundError as exc:
        print(f"missing target: {exc}", file=sys.stderr)
        return 1

    if not files:
        print("no files to scan", file=sys.stderr)
        return 1

    output: list[str] = []
    for path in files:
        output.extend(find_hits(path, args.context))

    if not output:
        print("no visibility-related references found")
    else:
        print("\n".join(output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
