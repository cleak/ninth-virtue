# Visibility Reverse Engineering

This document is the canonical notebook for reverse engineering Ultima V's
visibility model before any fog-of-war feature work lands in the app.

## Status

- Static analysis is grounded against a local Windows GOG install of Ultima V.
- Live DOSBox validation is now partially complete for an underworld scene.
- Current recommendation is based on static evidence plus one confirmed runtime
  transition: verify the remaining scene families, then read engine buffers
  directly instead of porting visibility logic.

## Artifact Freeze

Use [scripts/freeze-visibility-artifacts.ps1](../scripts/freeze-visibility-artifacts.ps1)
to regenerate the manifest for the active install. The current static pass was
grounded against this local Windows GOG install:

- `C:\Program Files (x86)\GOG Galaxy\Games\Ultima 5`
- Generated: `2026-04-12T20:35:30.1171346-07:00`

| File | Size | SHA-256 |
|---|---:|---|
| `BLCKTHRN.OVL` | 3184 | `e2f8455c5b1391878f6b7c8ff6574413c8b65267a440614f66f33b34482abc39` |
| `CAST.OVL` | 8560 | `6dce8ffa4262c797ecca717ac0aaa73dd19376c51427073e7e62e407166b6c4a` |
| `CAST2.OVL` | 4544 | `7ececd3fb12211f3a90bf732c88b10f078a406099431fa24257f24d408659425` |
| `CMDS.OVL` | 7440 | `56dccb08f88957383f3dbb6765e5e86049ac6b765cef3e18a3b1da209234a258` |
| `COMBAT.OVL` | 7408 | `e73d5cd382174a449c56786d1596cebd4d4d9b376d0df2b8d8109faf5a9e9619` |
| `COMSUBS.OVL` | 5216 | `cef80d156663da4394a573aaa181cfd7f96cfbfa4d378fea6e15e2ca31d8145e` |
| `DATA.OVL` | 48464 | `da7d10decb8864754d13910e8694c0cb7301b5224f25156730857827f07168c3` |
| `DNGLOOK.OVL` | 5040 | `869ae321f1f1e020bdffbad346cac7bf0a5fbdf697202ae7af47c659e3c7161c` |
| `dosboxULTIMA5.conf` | 11218 | `0c67f27939f80b87d55022551f1ccb7d244dbf1abb53f3bec5875ceb0ac2de02` |
| `dosboxULTIMA5_single.conf` | 270 | `208e12098eb488c5c26582c8538668e0ea75daa69438755267c46b81c44a90fd` |
| `dosboxULTIMA5_single_no_game.conf` | 307 | `abee497e7b80231eea4258b80006ce31b076ce68b4d72477a63a80a426cb2c5c` |
| `DUNGEON.OVL` | 8016 | `4624c97694038bc36ec81cc0e186c837cf484258ecf806dcb06c2e1d0c408518` |
| `ENDGAME.OVL` | 2864 | `409771c71994e6e2ce7850cb10fcab3d6e9b1ad5ff98e517b397795624f9f424` |
| `FLAMES.OVL` | 32 | `e3b075c5cba18cacab558e2e5e261a7c20f531dfde2d0f2ab352f9e4239c11d4` |
| `FONT.OVL` | 3744 | `4dc05e8d75181246151e398b32f9c786897d828fa3cd229809dbc5dcea21d74d` |
| `INTRO.OVL` | 8400 | `074543b5b1d0bca1c4487a677c4aaf5557a28c344caf09da51636d2365923356` |
| `LOOKOBJ.OVL` | 4560 | `91eb595670f8a94a91b43e9912a5755cdc40aac991231d29425c6eedd1eacd57` |
| `MAINOUT.OVL` | 7344 | `522b995016365b5528c3af1400e185d0d062cdd94b1e23d25b0d8fdea4a7c54c` |
| `NPC.OVL` | 4912 | `f1491fb52f685183baf0f5176845105e82e2f31109e7f1f3b7587f98b295151d` |
| `OUTSUBS.OVL` | 2464 | `2e8db7d79e81221b8cdb90bb78692913e0906aebb1605ff43cb5b8fade23e184` |
| `SHOPPES.OVL` | 5936 | `82ea4d043f84b112f5065978818446942a6538377eab89442e3fc8d5a85b270f` |
| `SHOPPES2.OVL` | 2848 | `bea9c2bf72433f86401beb75ec9c47215bb0a7259790a41329fbf18b16468f7f` |
| `SHOPPES3.OVL` | 2528 | `b1e46f145cde60361acc712f1bd368c96db0c60019d39658f59c94f1aaed68f4` |
| `SJOG.OVL` | 8800 | `ab16475d5971e41b422187a0e5214c3ce9249de19dc84852e6d11e378717b299` |
| `TALK.OVL` | 4880 | `fa0abe9ace2e6761a93e28e179b8acd3b73ec2661ca05b552f53988ee32ad60d` |
| `TOWN.OVL` | 6256 | `0deb3ef891e538cd3a53700e43ede2cda6372e4298fbff11ca8310ad2acce062` |
| `ULTIMA.EXE` | 36592 | `e904215f43f45080b499a6de683dddce6f3b0466f25bdce067388ea50a7ac5b0` |
| `ZSTATS.OVL` | 4880 | `6d8c3913249d03b1f77a53e0cb663a2d650c7748f7163628c8a6809fde459933` |

## Validation Matrix

Capture pre-action, during-action, and settled snapshots for each case:

| Case | Goal |
|---|---|
| Overworld daytime, open terrain | Establish baseline daylight radius |
| Overworld daytime, mountain blocking | Confirm terrain occlusion |
| Overworld nighttime, no torch/spell | Confirm night radius clamp |
| Overworld nighttime, torch lit | Confirm torch minimum light |
| Overworld with light spell active | Confirm spell minimum light |
| Town/interior with walls/buildings | Confirm local-scene occlusion |
| Dungeon corridor/room/facing changes | Confirm first-person dungeon visibility |
| Underworld | Confirm separate outdoor rules |
| Combat | Confirm that combat uses a separate fully materialized terrain path |

Use [src/bin/visibility_watch.rs](../src/bin/visibility_watch.rs)
for passive snapshots once a DOSBox process is running.

## Static Findings

### 1. Global light timers and current intensity live in the resident EXE

`ULTIMA.EXE` owns the runtime light bytes:

- `0x58A6` is the light-spell duration.
- `0x58A7` is the torch duration.
- `0x58A5` is the current effective light intensity used by map generation.

Key routine:

- `ULTIMA.EXE:0x4F7C..0x514A`

Observed behavior:

- `0x4FB4` and `0x4FBE` decrement `0x58A7` and `0x58A6` via helper `0x3F36`.
- `0x50A1..0x5110` recomputes `0x58A5`.
- `0x5115..0x5136` clamp `0x58A5` upward when `0x58A6` or `0x58A7` are non-zero.
- The spell clamp is `0x12`; the torch clamp is `0x0A`.

High-confidence conclusion:

- Exact visibility must treat `0x58A5` as the authoritative light radius or
  intensity byte, not derive radius directly from spell/torch duration.

### 2. The resident EXE builds a reusable 11x11 viewport scratch grid at `DS:0xAB02`

Key routine:

- `ULTIMA.EXE:0x5910`

Observed behavior:

- `0x5929` gates the redraw path on `0x58A4`.
- `0x5965` passes `0x58A5` plus player-relative offsets into `0x5D0A`.
- `0x5992..0x59EF` fills `DS:0xAB02` directly from the world-tile getter when
  the expensive redraw path is skipped.
- `0x59FB` points `DI` at `0xAB02`, confirming that `0xAB02` is the viewport scratch buffer.

Key helper:

- `ULTIMA.EXE:0x5D0A`

Observed behavior:

- Initializes an `11 x 11` active window in a `0x20`-stride buffer at `DS:0xAB02`.
- Accepts the current light intensity as an argument.
- Delegates to generic helper `0x5A28`, which also feeds the combat scratch grid.

High-confidence conclusion:

- For overworld, underworld, towns, and other 2D scenes, `DS:0xAB02` is the
  first buffer to verify at runtime. It is already the engine-owned viewport
  scratch grid, and `0x58A5` is wired directly into its generation path.

### 3. Live runtime validation confirms `DS:0xAB02` behaves like the current 2D visibility window

Validated snapshot pair:

- `artifacts/visibility-watch/baseline-live`
- `artifacts/visibility-watch/torch-expired-step1`

Observed behavior:

- Baseline scene: underworld, player at `x=55 y=23 z=255`, torch active,
  `0x58A5=0x0A`, `0x58A7=0x96`.
- Follow-up scene: player moved one tile west to `x=54 y=23 z=255`, torch
  expired, `0x58A5=0x02`, `0x58A7=0x00`.
- `DS:0xAB02` remained player-centered across the move, but the visible mask
  collapsed from `25` non-`FF` cells to `9` non-`FF` cells when the torch
  expired.
- Hidden cells in the active `11 x 11` window read back as `0xFF`.

High-confidence conclusion:

- `DS:0xAB02` is directly usable as the current-frame visibility mask for 2D
  scenes.
- The buffer already tracks live light changes, so fog implementation should
  read it rather than recompute night or torch radius.

### 4. Torch activation is driven from `CMDS.OVL`

Key routine:

- `CMDS.OVL:0x0D98..0x0DDB`

Observed behavior:

- Decrements inventory torches at `save+0x208` / `0x57AE`.
- Writes `0x58A7` directly or through a helper, depending on scene type.
- Uses `0xF0` as one immediate duration value for `0x58A7`.

High-confidence conclusion:

- Torch duration is set in command-processing code, then consumed later by the
  resident EXE's light-intensity routine.

### 5. `MAINOUT.OVL` seeds map redraw state during scene setup

Key routines:

- `MAINOUT.OVL:0x0011`
- `MAINOUT.OVL:0x0A38`

Observed behavior:

- `0x0011` writes `0x58A4`, then recalculates `0x589B` / `0x589C` from the
  current player tile before the main redraw runs.
- `0x0A38` can zero `0x58A5` on specific entry paths before later code rebuilds
  the current scene's lighting state.

High-confidence conclusion:

- Scene entry does not just render whatever state already exists in memory.
  `MAINOUT.OVL` actively seeds redraw and scroll state, so runtime validation
  must capture snapshots both before and after a scene transition settles.

### 6. Dungeon visibility likely mutates the live dungeon buffer at `DS:0x595A`

Key routines:

- `DUNGEON.OVL:0x1AD6`
- `DUNGEON.OVL:0x150A`
- `DNGLOOK.OVL:0x0013`

Observed behavior:

- `DUNGEON.OVL:0x1AD6` short-circuits to a darker path when both `0x58A6` and
  `0x58A7` are zero.
- `DNGLOOK.OVL:0x0013` uses the same `0x58A6` / `0x58A7` gating before reading
  the current dungeon cell from `0x595A`.
- `DUNGEON.OVL:0x150A` reads the dungeon floor data from `0x595A` and, in
  selected cases, writes back to the same buffer after masking the low three bits.

High-confidence conclusion:

- Dungeon visibility may not use a separate mask. The engine appears to encode
  view-state directly into the live dungeon buffer at `DS:0x595A`.
- Runtime verification must compare `0x595A` before and after moves/rotations
  with and without torch/light spell active.

### 7. Combat already uses a separate fully materialized scratch grid

Key routine:

- `ULTIMA.EXE:0x5E4A`

Observed behavior:

- Initializes `DS:0xAD14` to `0xFF`.
- Fills it through the same generic helper family used by `0x5D0A`.
- `COMBAT.OVL` itself does not reference `0x58A5`, `0x58A6`, or `0x58A7`.

High-confidence conclusion:

- Combat should be treated as a control case. The engine already materializes a
  dedicated combat terrain grid at `DS:0xAD14`, and current static evidence does
  not point to any additional combat-only light/visibility mask.

## Scene-To-Routine Map

| Scene | Candidate producer | Candidate runtime buffer | Notes |
|---|---|---|---|
| Overworld / Underworld / 2D interiors | `ULTIMA.EXE:0x5910`, `0x5D0A`, `0x5A28` | `DS:0xAB02` | Light intensity `0x58A5` is passed directly into generation |
| Global light state | `ULTIMA.EXE:0x4F7C..0x514A` | `0x58A5`, `0x58A6`, `0x58A7` | `0x58A5` is the authoritative current intensity |
| Torch activation | `CMDS.OVL:0x0D98..0x0DDB` | `0x58A7` | Decrements inventory torch count |
| Dungeon first-person | `DUNGEON.OVL:0x1AD6`, `0x150A` | `DS:0x595A` | Likely mutates the live dungeon buffer itself |
| Dungeon look command | `DNGLOOK.OVL:0x0013` | `DS:0x595A` | Shares the same light gating as dungeon walk mode |
| Combat | `ULTIMA.EXE:0x5E4A` | `DS:0xAD14` | Separate terrain scratch grid |

## Runtime Capture Plan

When DOSBox is running, use this flow:

1. Freeze the active binary set with `.\scripts\freeze-visibility-artifacts.ps1 -GameDir '<path-to-ultima5>'`.
2. Capture a baseline snapshot with `& cargo run --bin visibility_watch -- --label baseline`.
3. Perform exactly one in-game action.
4. Capture `during` and `settled` snapshots.
5. Diff:
   - `summary.txt`
   - `viewport-ab02.bin`
   - `dungeon-595a.bin`
   - `combat-ad14.bin`

Priority checks:

- Overworld/night transitions should change `0x58A5`, then alter `DS:0xAB02`.
- Torch/light spell should clamp `0x58A5` before `DS:0xAB02` changes.
- Dungeon movement and turning should change `DS:0x595A` even if `save+MAP_TILES`
  stays unrelated.
- Combat should keep using `DS:0xAD14` without involving `0x58A5`.

Confirmed so far:

- Torch expiry changed `0x58A5` from `0x0A` to `0x02`.
- The same step reduced `DS:0xAB02` from a `25`-cell visible region to a
  `9`-cell visible region.

## Decision And Recommendation

### Confirmed inputs

- Time/light state: `0x58A5`, `0x58A6`, `0x58A7`
- Scene type: `0x5893`
- Player position: `0x5895`, `0x5896`, `0x5897`
- Outdoor/local scroll origin: `0x589B`, `0x589C`
- Dungeon orientation/live state: `0x6603` and `0x595A`
- Occluders: not yet proven from live runtime capture; static evidence only shows
  that the engine rebuilds scene-local buffers before render, not how the
  blocking test is encoded in each scene family

### Reusable buffer decision

- **Overworld / Underworld / towns / other 2D scenes:** treat `DS:0xAB02` as the
  primary candidate reusable visibility buffer.
- **Dungeons:** treat `DS:0x595A` as the primary candidate visibility-bearing
  buffer, with the expectation that visibility is encoded into the live dungeon
  terrain bytes rather than a separate mask.
- **Combat:** use `DS:0xAD14` only as a control/reference buffer; fog work should
  continue to ignore combat scenes.

### Final integration recommendation

- **Prefer reading live engine-produced buffers first.**
- Verify `DS:0xAB02` and `DS:0x595A` under runtime snapshots before porting any algorithm.
- Only port the exact algorithm if runtime validation shows that the buffers are
  too transient, too late in the pipeline, or incomplete for the minimap.
- Treat combat as out of scope for fog unless runtime capture disproves the
  current static conclusion that `DS:0xAD14` is already the fully materialized
  combat terrain grid.
