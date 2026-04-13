//! Research-only snapshot tool for visibility reverse engineering.
//!
//! Captures the current map/light state plus the candidate runtime buffers
//! we care about during the passive-validation phase:
//! - `DS:0xAB02` 11x11 viewport scratch grid (32-byte stride)
//! - `DS:0xAD14` combat terrain scratch grid (32-byte stride)
//! - `DS:0x595A` dungeon terrain buffer (8 floors x 8x8 cells)
//! - `save+MAP_TILES` current 32x32 live terrain window

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use ninth_virtue::game::map::{self, LocationType};
use ninth_virtue::game::offsets::{
    COMBAT_TERRAIN_GRID, COMBAT_TERRAIN_HEIGHT, COMBAT_TERRAIN_STRIDE, COMBAT_TERRAIN_WIDTH,
    DUNGEON_FLOORS, DUNGEON_LEVEL_HEIGHT, DUNGEON_LEVEL_LEN, DUNGEON_LEVEL_WIDTH,
    DUNGEON_TILES_LEN, DUNGEON_TILES_SAVE_OFFSET, LIGHT_INTENSITY, LIGHT_SPELL_DUR, MAP_SCROLL_X,
    MAP_SCROLL_Y, MAP_TILES, MAP_TILES_LEN, TORCH_DUR, ds_addr, inv_addr,
};
use ninth_virtue::memory::access::MemoryAccess;
use ninth_virtue::memory::{process, scanner};

const VIEWPORT_SCRATCH_DS: usize = 0xAB02;
const VIEWPORT_ACTIVE_WIDTH: usize = 11;
const VIEWPORT_STRIDE: usize = 0x20;
const VIEWPORT_ROWS: usize = 11;

struct Options {
    pid: Option<u32>,
    out_dir: PathBuf,
    label: String,
}

fn main() -> Result<()> {
    let options = parse_args()?;
    let output_dir = options.out_dir.join(&options.label);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;

    let (pid, name) = select_process(options.pid)?;
    let process = process::attach(pid)?;
    let scan = scanner::find_dos_base(&process.memory)?;
    if !scan.game_confirmed {
        bail!("Ultima V is not loaded in the selected DOSBox process");
    }

    let mem: &dyn MemoryAccess = &process.memory;
    let map_state = map::read_map_state(mem, scan.dos_base)?;
    let light_intensity = mem.read_u8(inv_addr(scan.dos_base, LIGHT_INTENSITY))?;
    let light_spell_dur = mem.read_u8(inv_addr(scan.dos_base, LIGHT_SPELL_DUR))?;
    let torch_dur = mem.read_u8(inv_addr(scan.dos_base, TORCH_DUR))?;
    let scroll_x = mem.read_u8(inv_addr(scan.dos_base, MAP_SCROLL_X))?;
    let scroll_y = mem.read_u8(inv_addr(scan.dos_base, MAP_SCROLL_Y))?;

    let viewport_scratch = read_bytes(
        mem,
        ds_addr(scan.dos_base, VIEWPORT_SCRATCH_DS),
        VIEWPORT_STRIDE * VIEWPORT_ROWS,
    )?;
    let combat_scratch = read_bytes(
        mem,
        ds_addr(scan.dos_base, COMBAT_TERRAIN_GRID),
        COMBAT_TERRAIN_HEIGHT * COMBAT_TERRAIN_STRIDE,
    )?;
    let dungeon_buffer = read_bytes(
        mem,
        inv_addr(scan.dos_base, DUNGEON_TILES_SAVE_OFFSET),
        DUNGEON_TILES_LEN,
    )?;
    let live_tiles = read_bytes(mem, inv_addr(scan.dos_base, MAP_TILES), MAP_TILES_LEN)?;

    write_text(
        &output_dir.join("summary.txt"),
        &build_summary(
            &name,
            pid,
            scan.dos_base,
            &map_state,
            scroll_x,
            scroll_y,
            light_intensity,
            light_spell_dur,
            torch_dur,
        ),
    )?;
    write_text(
        &output_dir.join("viewport-ab02.txt"),
        &format_strided_grid(
            "DS:0xAB02 viewport scratch",
            &viewport_scratch,
            VIEWPORT_ACTIVE_WIDTH,
            VIEWPORT_STRIDE,
            VIEWPORT_ROWS,
        ),
    )?;
    write_text(
        &output_dir.join("combat-ad14.txt"),
        &format_strided_grid(
            "DS:0xAD14 combat scratch",
            &combat_scratch,
            COMBAT_TERRAIN_WIDTH,
            COMBAT_TERRAIN_STRIDE,
            COMBAT_TERRAIN_HEIGHT,
        ),
    )?;
    write_text(
        &output_dir.join("map-tiles-32x32.txt"),
        &format_dense_grid("save+MAP_TILES 32x32", &live_tiles, 32),
    )?;
    write_text(
        &output_dir.join("dungeon-595a.txt"),
        &format_dungeon_buffer(&dungeon_buffer, usize::from(map_state.z)),
    )?;

    fs::write(output_dir.join("viewport-ab02.bin"), &viewport_scratch)
        .with_context(|| format!("writing {}", output_dir.join("viewport-ab02.bin").display()))?;
    fs::write(output_dir.join("combat-ad14.bin"), &combat_scratch)
        .with_context(|| format!("writing {}", output_dir.join("combat-ad14.bin").display()))?;
    fs::write(output_dir.join("map-tiles-32x32.bin"), &live_tiles).with_context(|| {
        format!(
            "writing {}",
            output_dir.join("map-tiles-32x32.bin").display()
        )
    })?;
    fs::write(output_dir.join("dungeon-595a.bin"), &dungeon_buffer)
        .with_context(|| format!("writing {}", output_dir.join("dungeon-595a.bin").display()))?;

    println!("visibility snapshot written to {}", output_dir.display());
    Ok(())
}

fn parse_args() -> Result<Options> {
    let mut pid = None;
    let mut out_dir = PathBuf::from("artifacts").join("visibility-watch");
    let mut label = default_label();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pid" => {
                let value = args.next().context("missing value for --pid")?;
                pid = Some(value.parse().context("invalid pid")?);
            }
            "--out" => {
                let value = args.next().context("missing value for --out")?;
                out_dir = PathBuf::from(value);
            }
            "--label" => {
                label = args.next().context("missing value for --label")?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    Ok(Options {
        pid,
        out_dir,
        label,
    })
}

fn print_usage() {
    println!(
        "Usage: cargo run --bin visibility_watch -- [--pid <pid>] [--out <dir>] [--label <name>]"
    );
    println!();
    println!(
        "Captures the current map/light state and candidate visibility buffers into a timestamped directory."
    );
}

fn default_label() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("snapshot-{secs}")
}

fn select_process(requested_pid: Option<u32>) -> Result<(u32, String)> {
    let processes = process::list_dosbox_processes()?;
    if processes.is_empty() {
        bail!("no DOSBox process found");
    }

    if let Some(pid) = requested_pid {
        return processes
            .into_iter()
            .find(|(candidate, _)| *candidate == pid)
            .with_context(|| format!("DOSBox process {pid} not found"));
    }

    if processes.len() > 1 {
        let ids = processes
            .iter()
            .map(|(pid, name)| format!("{name} ({pid})"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("multiple DOSBox processes found; rerun with --pid. Candidates: {ids}");
    }

    Ok(processes[0].clone())
}

fn read_bytes(mem: &dyn MemoryAccess, addr: usize, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    mem.read_bytes(addr, &mut buf)?;
    Ok(buf)
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn build_summary(
    process_name: &str,
    pid: u32,
    dos_base: usize,
    map_state: &map::MapState,
    scroll_x: u8,
    scroll_y: u8,
    light_intensity: u8,
    light_spell_dur: u8,
    torch_dur: u8,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Process: {process_name} (PID {pid})");
    let _ = writeln!(out, "DOS base: 0x{dos_base:05X}");
    let _ = writeln!(out, "Location: {}", map_state.display_location_name());
    let _ = writeln!(out, "Location enum: {:?}", map_state.location);
    let _ = writeln!(
        out,
        "Position: x={} y={} z={}",
        map_state.x, map_state.y, map_state.z
    );
    let _ = writeln!(out, "Scroll: x={scroll_x} y={scroll_y}");
    let _ = writeln!(out, "Dungeon facing: {:?}", map_state.dungeon_facing);
    let _ = writeln!(out, "Objects: {}", map_state.objects.len());
    let _ = writeln!(
        out,
        "Light intensity (save+0x2FF): 0x{light_intensity:02X} ({light_intensity})"
    );
    let _ = writeln!(
        out,
        "Light spell dur (save+0x300): 0x{light_spell_dur:02X} ({light_spell_dur})"
    );
    let _ = writeln!(
        out,
        "Torch dur (save+0x301): 0x{torch_dur:02X} ({torch_dur})"
    );
    let _ = writeln!(out, "Scene notes:");
    match map_state.location {
        LocationType::Combat(_) => {
            let _ = writeln!(
                out,
                "- Combat scene active; compare DS:0xAD14 against the minimap."
            );
        }
        LocationType::Dungeon(_) => {
            let _ = writeln!(
                out,
                "- Dungeon scene active; compare save+0x3B4 / DS:0x595A with first-person lighting."
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "- Outdoor/interior scene active; compare DS:0xAB02 with live 2D visibility."
            );
        }
    }
    out
}

fn format_strided_grid(
    title: &str,
    data: &[u8],
    active_width: usize,
    stride: usize,
    rows: usize,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "active={} stride={} rows={}",
        active_width, stride, rows
    );
    for row in 0..rows {
        let start = row * stride;
        let active = &data[start..start + active_width];
        let raw = &data[start..start + stride];
        let _ = writeln!(
            out,
            "row {:02}: active={} | raw={}",
            row,
            hex_bytes(active),
            hex_bytes(raw)
        );
    }
    out
}

fn format_dense_grid(title: &str, data: &[u8], width: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{title}");
    for (row_idx, row) in data.chunks_exact(width).enumerate() {
        let _ = writeln!(out, "row {:02}: {}", row_idx, hex_bytes(row));
    }
    out
}

fn format_dungeon_buffer(data: &[u8], active_floor: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "save+0x3B4 / DS:0x595A dungeon buffer");
    let clamped_floor = active_floor.min(DUNGEON_FLOORS.saturating_sub(1));
    let _ = writeln!(out, "active_floor={clamped_floor}");

    for floor in 0..DUNGEON_FLOORS {
        let _ = writeln!(out);
        let marker = if floor == clamped_floor { "*" } else { " " };
        let _ = writeln!(out, "{marker} floor {floor}");
        let base = floor * DUNGEON_LEVEL_LEN;
        for row in 0..DUNGEON_LEVEL_HEIGHT {
            let start = base + row * DUNGEON_LEVEL_WIDTH;
            let end = start + DUNGEON_LEVEL_WIDTH;
            let _ = writeln!(out, "row {:02}: {}", row, hex_bytes(&data[start..end]));
        }
    }

    out
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
