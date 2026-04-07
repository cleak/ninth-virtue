# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

ninth-virtue ("The Ninth Virtue: Convenience") is a companion app for Ultima V (classic DOS RPG). It attaches to a running DOSBox/DOSBox Staging process, locates the game's virtual memory base address, and provides quality-of-life features: curing poison, adding arrows, restoring health, and eventually a mini-map overlay. The core challenge is automating the discovery of the memory base pointer within the emulator's address space — once found, all game data offsets are known.

## Build Commands

```bash
cargo build              # debug build
cargo build --release    # release build
cargo run                # build and run
cargo test               # run all tests
cargo test <name>        # run a single test by name
cargo clippy             # lint
cargo fmt                # format code
cargo fmt -- --check     # check formatting without modifying
```

## Architecture

The project is in early stages. The intended architecture:

- **Process attachment** — find and attach to the DOSBox/DOSBox Staging process, locate the virtual memory base where the DOS address space is mapped.
- **Memory reading** — read game state from known offsets relative to the base pointer (party stats, inventory, status effects, map data).
- **GUI** — present game state and provide buttons/controls for common actions (heal, cure, add items).
- **Mini-map** (stretch goal) — render a small map from memory-resident tile data.

## Platform

Windows-only. The app reads memory from another process, which requires Windows APIs (`ReadProcessMemory`, process enumeration). Rust edition 2024.
