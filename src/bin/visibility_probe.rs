//! Live probe for outdoor visibility stability.
//!
//! Attaches to a running DOSBox process, installs the same resident patch the
//! GUI uses, and prints repeated samples of:
//! - raw `DS:0xAB02` active 11x11 bytes
//! - synchronized save-region visibility snapshot
//! - the window returned by `read_map_state_with_visibility_snapshot`
//!
//! This is meant for reverse-engineering and regression checks while a scene is
//! held stationary in-game.

use std::collections::BTreeSet;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use ninth_virtue::game::injection::{
    self, VISIBILITY_SNAPSHOT_LIGHT_IDX, VISIBILITY_SNAPSHOT_LOCATION_IDX,
    VISIBILITY_SNAPSHOT_READY_MARKER, VISIBILITY_SNAPSHOT_READY_OFFSET,
    VISIBILITY_SNAPSHOT_SCROLL_X_IDX, VISIBILITY_SNAPSHOT_SCROLL_Y_IDX,
    VISIBILITY_SNAPSHOT_TILES_OFFSET, VISIBILITY_SNAPSHOT_TOTAL_LEN, VISIBILITY_SNAPSHOT_X_IDX,
    VISIBILITY_SNAPSHOT_Y_IDX, VISIBILITY_SNAPSHOT_Z_IDX,
};
use ninth_virtue::game::map;
use ninth_virtue::game::offsets::{
    LIGHT_INTENSITY, MAP_LOCATION, MAP_SCROLL_X, MAP_SCROLL_Y, MAP_X, MAP_Y, MAP_Z,
    VIEWPORT_VISIBILITY_GRID, VIEWPORT_VISIBILITY_HEIGHT, VIEWPORT_VISIBILITY_LEN,
    VIEWPORT_VISIBILITY_STRIDE, VIEWPORT_VISIBILITY_WIDTH, ds_addr, inv_addr,
};
use ninth_virtue::memory::access::MemoryAccess;
use ninth_virtue::memory::{process, scanner};

const VIEWPORT_TERRAIN_FALLBACK_GRID: usize = 0xAC64;
const VIEWPORT_TERRAIN_FALLBACK_STRIDE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisibilityKey {
    location_id: u8,
    z: u8,
    x: u8,
    y: u8,
    scroll_x: u8,
    scroll_y: u8,
    light: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SampleHash {
    raw_ab02: u64,
    raw_ac64: u64,
    snapshot: Option<u64>,
    app: Option<u64>,
}

struct Options {
    pid: Option<u32>,
    count: usize,
    interval_ms: u64,
    warmup_ms: u64,
}

struct PatchGuard<'a> {
    mem: &'a dyn MemoryAccess,
    patch: Option<injection::PatchState>,
}

impl<'a> PatchGuard<'a> {
    fn new(mem: &'a dyn MemoryAccess, patch: injection::PatchState) -> Self {
        Self {
            mem,
            patch: Some(patch),
        }
    }

    fn patch(&self) -> &injection::PatchState {
        self.patch
            .as_ref()
            .expect("patch guard must be initialized")
    }
}

impl Drop for PatchGuard<'_> {
    fn drop(&mut self) {
        if let Some(patch) = self.patch.take() {
            injection::remove_patch(self.mem, &patch);
        }
    }
}

fn main() -> Result<()> {
    let options = parse_args()?;
    let (pid, name) = select_process(options.pid)?;
    let proc = process::attach(pid)?;
    let scan = scanner::find_dos_base(&proc.memory)?;
    if !scan.game_confirmed {
        bail!("Ultima V is not loaded in the selected DOSBox process");
    }

    let mem: &dyn MemoryAccess = &proc.memory;
    let patch = PatchGuard::new(mem, injection::apply_patch(mem, scan.dos_base)?);
    let snapshot_addr = patch.patch().visibility_snapshot_addr();
    println!(
        "Attached to {name} (PID {pid}), dos_base={:#x}, snapshot_addr={:#x}",
        scan.dos_base, snapshot_addr
    );
    if options.warmup_ms > 0 {
        thread::sleep(Duration::from_millis(options.warmup_ms));
    }

    let mut unique = BTreeSet::new();
    for idx in 0..options.count {
        let key = read_visibility_key(mem, scan.dos_base)?;
        let raw_ab02 = read_active_grid(
            mem,
            ds_addr(scan.dos_base, VIEWPORT_VISIBILITY_GRID),
            VIEWPORT_VISIBILITY_STRIDE,
        )?;
        let raw_ac64 = read_active_grid(
            mem,
            ds_addr(scan.dos_base, VIEWPORT_TERRAIN_FALLBACK_GRID),
            VIEWPORT_TERRAIN_FALLBACK_STRIDE,
        )?;
        let snapshot = read_snapshot(mem, snapshot_addr, key)?;
        let app_state =
            map::read_map_state_with_visibility_snapshot(mem, scan.dos_base, Some(snapshot_addr))?;
        let app_tiles = app_state.visibility_tiles;

        let snapshot_tiles = snapshot.as_ref().map(|(_, tiles)| *tiles);
        let source = match (snapshot_tiles.as_ref(), app_tiles.as_ref()) {
            (Some(snapshot_tiles), Some(app_tiles)) if snapshot_tiles == app_tiles => "snapshot",
            (_, Some(app_tiles)) if app_tiles == &raw_ab02 => "raw",
            (_, Some(_)) => "other",
            _ => "none",
        };

        let sample_hash = SampleHash {
            raw_ab02: fnv1a64(&raw_ab02),
            raw_ac64: fnv1a64(&raw_ac64),
            snapshot: snapshot_tiles.as_ref().map(|tiles| fnv1a64(tiles)),
            app: app_tiles.as_ref().map(|tiles| fnv1a64(tiles)),
        };
        unique.insert(sample_hash);

        println!(
            "#{idx:02} loc={:02X} z={:02X} pos=({}, {}) scroll=({}, {}) light={:02X} \
raw_ab02={:016X} vis={} center={} raw_ac64={:016X} \
snap={} app={} source={}",
            key.location_id,
            key.z,
            key.x,
            key.y,
            key.scroll_x,
            key.scroll_y,
            key.light,
            sample_hash.raw_ab02,
            visible_count(&raw_ab02),
            raw_ab02[(VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
                + VIEWPORT_VISIBILITY_WIDTH / 2],
            sample_hash.raw_ac64,
            format_optional_tiles(snapshot_tiles.as_ref()),
            format_optional_tiles(app_tiles.as_ref()),
            source,
        );

        if idx + 1 < options.count {
            thread::sleep(Duration::from_millis(options.interval_ms));
        }
    }

    println!();
    println!(
        "Unique sample tuples across {} samples: {}",
        options.count,
        unique.len()
    );
    for (idx, sample) in unique.iter().enumerate() {
        println!(
            "  [{idx}] raw_ab02={:016X} raw_ac64={:016X} snapshot={} app={}",
            sample.raw_ab02,
            sample.raw_ac64,
            format_optional_hash(sample.snapshot),
            format_optional_hash(sample.app),
        );
    }
    Ok(())
}

fn parse_args() -> Result<Options> {
    let mut pid = None;
    let mut count = 16usize;
    let mut interval_ms = 200u64;
    let mut warmup_ms = 300u64;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pid" => {
                let value = args.next().context("missing value for --pid")?;
                pid = Some(value.parse().context("invalid pid")?);
            }
            "--count" => {
                let value = args.next().context("missing value for --count")?;
                count = value.parse().context("invalid count")?;
            }
            "--interval-ms" => {
                let value = args.next().context("missing value for --interval-ms")?;
                interval_ms = value.parse().context("invalid interval")?;
            }
            "--warmup-ms" => {
                let value = args.next().context("missing value for --warmup-ms")?;
                warmup_ms = value.parse().context("invalid warmup")?;
            }
            "--help" | "-h" => {
                println!(
                    "Usage: cargo run --bin visibility_probe -- [--pid <pid>] [--count <n>] [--interval-ms <ms>] [--warmup-ms <ms>]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    Ok(Options {
        pid,
        count,
        interval_ms,
        warmup_ms,
    })
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

fn read_visibility_key(mem: &dyn MemoryAccess, dos_base: usize) -> Result<VisibilityKey> {
    Ok(VisibilityKey {
        location_id: mem.read_u8(inv_addr(dos_base, MAP_LOCATION))?,
        z: mem.read_u8(inv_addr(dos_base, MAP_Z))?,
        x: mem.read_u8(inv_addr(dos_base, MAP_X))?,
        y: mem.read_u8(inv_addr(dos_base, MAP_Y))?,
        scroll_x: mem.read_u8(inv_addr(dos_base, MAP_SCROLL_X))?,
        scroll_y: mem.read_u8(inv_addr(dos_base, MAP_SCROLL_Y))?,
        light: mem.read_u8(inv_addr(dos_base, LIGHT_INTENSITY))?,
    })
}

fn read_active_grid(
    mem: &dyn MemoryAccess,
    addr: usize,
    stride: usize,
) -> Result<[u8; VIEWPORT_VISIBILITY_LEN]> {
    let mut scratch = vec![0u8; stride * VIEWPORT_VISIBILITY_HEIGHT];
    mem.read_bytes(addr, &mut scratch)?;

    let mut active = [0u8; VIEWPORT_VISIBILITY_LEN];
    for row in 0..VIEWPORT_VISIBILITY_HEIGHT {
        let src = row * stride;
        let dst = row * VIEWPORT_VISIBILITY_WIDTH;
        active[dst..dst + VIEWPORT_VISIBILITY_WIDTH]
            .copy_from_slice(&scratch[src..src + VIEWPORT_VISIBILITY_WIDTH]);
    }
    Ok(active)
}

fn read_snapshot(
    mem: &dyn MemoryAccess,
    snapshot_addr: usize,
    key: VisibilityKey,
) -> Result<Option<(VisibilityKey, [u8; VIEWPORT_VISIBILITY_LEN])>> {
    let mut snapshot = [0u8; VISIBILITY_SNAPSHOT_TOTAL_LEN];
    mem.read_bytes(snapshot_addr, &mut snapshot)?;

    if snapshot[VISIBILITY_SNAPSHOT_READY_OFFSET] != VISIBILITY_SNAPSHOT_READY_MARKER {
        return Ok(None);
    }

    let snapshot_key = VisibilityKey {
        location_id: snapshot[VISIBILITY_SNAPSHOT_LOCATION_IDX],
        z: snapshot[VISIBILITY_SNAPSHOT_Z_IDX],
        x: snapshot[VISIBILITY_SNAPSHOT_X_IDX],
        y: snapshot[VISIBILITY_SNAPSHOT_Y_IDX],
        scroll_x: snapshot[VISIBILITY_SNAPSHOT_SCROLL_X_IDX],
        scroll_y: snapshot[VISIBILITY_SNAPSHOT_SCROLL_Y_IDX],
        light: snapshot[VISIBILITY_SNAPSHOT_LIGHT_IDX],
    };

    let mut tiles = [0u8; VIEWPORT_VISIBILITY_LEN];
    tiles.copy_from_slice(
        &snapshot[VISIBILITY_SNAPSHOT_TILES_OFFSET
            ..VISIBILITY_SNAPSHOT_TILES_OFFSET + VIEWPORT_VISIBILITY_LEN],
    );

    if snapshot_key != key {
        return Ok(Some((snapshot_key, tiles)));
    }

    Ok(Some((snapshot_key, tiles)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn visible_count(tiles: &[u8; VIEWPORT_VISIBILITY_LEN]) -> usize {
    tiles.iter().filter(|&&tile| tile != 0xFF).count()
}

fn format_optional_tiles(tiles: Option<&[u8; VIEWPORT_VISIBILITY_LEN]>) -> String {
    match tiles {
        Some(tiles) => format!(
            "{:016X}/vis{}/ctr{:02X}",
            fnv1a64(tiles),
            visible_count(tiles),
            tiles[(VIEWPORT_VISIBILITY_HEIGHT / 2) * VIEWPORT_VISIBILITY_WIDTH
                + VIEWPORT_VISIBILITY_WIDTH / 2]
        ),
        None => "none".to_string(),
    }
}

fn format_optional_hash(hash: Option<u64>) -> String {
    hash.map(|hash| format!("{hash:016X}"))
        .unwrap_or_else(|| "none".to_string())
}
