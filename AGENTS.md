# AGENTS.md

Instructions for coding agents working in this repository.

## Project Snapshot

- `ninth-virtue` is a Windows-only Rust companion app for Ultima V running inside DOSBox or DOSBox Staging.
- The app attaches to a live emulator process, locates the emulated DOS memory region, and reads or writes known Ultima V runtime structures.
- The UI is an `egui`/`eframe` control panel with party, inventory, shrine, audio, and experimental minimap features.
- Treat the running game state as the source of truth. This is not a save-file editor, replacement engine, or general-purpose memory editor.

## Operating Constraints

- Windows only. Process attachment and memory access rely on Windows APIs and appropriate local permissions.
- Rust edition is `2024`; minimum toolchain is Rust `1.92`.
- Do not commit Ultima V game files, extracted assets, ROMs, or other proprietary third-party material.
- Do not commit secrets, credentials, or machine-specific private data.
- Keep reverse-engineering notes, offsets, and code comments factual and reproducible.

## Repository Map

- `src/main.rs`, `src/app.rs`: app startup, attachment flow, and refresh loop.
- `src/dosbox`: DOSBox configuration parsing and emulator-specific integration helpers.
- `src/memory`: process enumeration, handle management, region scanning, and DOS memory discovery.
- `src/game`: Ultima V data structures, offsets, and read/write logic.
- `src/gui`: `egui` panels and UI wiring.
- `src/tiles`: tile decoding and minimap rendering support.
- `src/audio.rs`: DOSBox audio session control.
- `src/bin`: CLI/debug utilities for scanning, poking, and reverse-engineering experiments.
- `docs/dosbox-internals.md`, `docs/memory-map.md`, `docs/redraw-mechanism.md`, `docs/reverse-engineering.md`: low-level reference material for memory layout and runtime behavior.

## Working Style

- Prefer small, focused changes that preserve existing behavior unless the task explicitly asks for broader refactoring.
- If you change behavior, update tests or documentation where practical.
- For changes touching offsets, memory scanning, redraw behavior, or emulator integration, consult the relevant file under `docs/` before editing code.
- Prefer PowerShell examples and commands in docs meant for contributors; this is a Windows-only project.
- Use `README.md` as the canonical end-user project overview and `CONTRIBUTING.md` for contributor policy. Keep `AGENTS.md` focused on agent workflow and repository-specific guardrails.

## Build Commands

```powershell
cargo build
cargo build --release
cargo run --release
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
./scripts/check-no-build-warnings.ps1
```

## Pull Request Checks

- Do not introduce new build warnings.
- Always run `./scripts/check-no-build-warnings.ps1` before creating or updating a PR.
- Before creating or updating a PR, also run `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, and `cargo test --locked`.
