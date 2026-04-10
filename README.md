# The Ninth Virtue

The Ninth Virtue: Convenience.

`ninth-virtue` is an unofficial companion app for Ultima V that plugs into a live DOSBox or DOSBox Staging session and turns the game's opaque memory into a modern control panel. It is built for players who still want the original game and original atmosphere, but would not mind a second screen that can surface party state, quest progress, map data, and a few mercy buttons when Britannia gets rough.

This is not a ROM hack, not a save editor in disguise, and not a replacement client. It is a live companion that attaches to the emulator process, discovers the emulated DOS memory base at runtime, and reads or writes known Ultima V data structures in place.

## What It Does

- Attaches to a running DOSBox or DOSBox Staging process on Windows
- Locates Ultima V's emulated DOS memory automatically
- Reads party state, inventory, shrine quest progress, map state, and object data
- Provides quick recovery actions such as healing, curing poison, resurrecting the party, refilling arrows, and topping off supplies
- Exposes direct editing surfaces for party and inventory values
- Tracks shrine quest progress with virtue, mantra, and completion state
- Controls the attached DOSBox audio session volume and mute state
- Renders an experimental live minimap from in-memory tile and object data
- Includes debugging tools for memory watching and reverse-engineering work

## Why This Exists

Ultima V is brilliant, but it was never designed to have an observer's dashboard. `ninth-virtue` treats the running game as the source of truth and builds a companion UI around it. The result is a tool that feels more like a cockpit than a cheat menu: you can inspect the party at a glance, see the shape of the world, understand shrine progression, and intervene quickly without digging through hex dumps or restarting the session.

For players, it is a quality-of-life layer.

For reverse engineers, it is a concrete example of process attachment, emulator memory scanning, and live state extraction from a classic DOS game.

## Screenshots

This section is intentionally left open so screenshots can be dropped in without rewriting the README.

Suggested captures:

- Main window while attached to a live game
- Party and inventory panels
- Shrine quest tracker
- Overworld minimap
- Audio controls and connection bar

Example placement:

```md
![Main window](docs/screenshots/main-window.png)
![Overworld minimap](docs/screenshots/minimap.png)
```

## Running It

This project is currently aimed at local development and experimentation.

Requirements:

- Windows
- Rust toolchain
- A running DOSBox or DOSBox Staging process
- Ultima V available in that process; loading into the game before attaching is recommended for the fastest startup path

Build and run:

```bash
cargo run --release
```

Typical workflow:

1. Launch DOSBox or DOSBox Staging with Ultima V.
2. Load into the game.
3. Start `ninth-virtue`.
4. If only one DOSBox process is available, the app will try to attach automatically.
5. If multiple processes are present, select the right one from the connection bar.

## How It Works

At a high level, the app:

1. Enumerates DOSBox processes and opens the selected process with Windows APIs.
2. Scans committed memory regions to find the emulated 1 MB DOS address space.
3. Confirms that the expected Ultima V data layout is present.
4. Reads and writes game state using known save-relative offsets.
5. Applies a small in-memory redraw hook so the party stats panel refreshes after companion-driven changes.

The redraw mechanism is intentionally runtime-only. It does not modify game files on disk.

## Project Structure

- [src/app.rs](src/app.rs): application state and refresh loop
- [src/memory](src/memory): process attachment and DOS memory scanning
- [src/game](src/game): Ultima V data structures, offsets, and read/write logic
- [src/gui](src/gui): egui panels for party, inventory, actions, quests, audio, and minimap
- [src/tiles](src/tiles): tile decoding and atlas support for map rendering
- [src/bin](src/bin): CLI tools for scanning, poking, and memory diff experiments

If you want the lower-level details, start here:

- [docs/dosbox-internals.md](docs/dosbox-internals.md)
- [docs/memory-map.md](docs/memory-map.md)
- [docs/redraw-mechanism.md](docs/redraw-mechanism.md)
- [docs/reverse-engineering.md](docs/reverse-engineering.md)

## Caveats

- Windows only
- Reads and writes another process's memory via Windows APIs, so appropriate local permissions are required
- The minimap path depends on locating the mounted Ultima V game directory from the DOSBox configuration
- This project is still evolving, and the reverse-engineering notes are part of the product, not just background material

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
