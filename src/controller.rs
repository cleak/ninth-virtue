//! Centralized DOSBox memory controller with finite state machine.
//!
//! All reads and writes to DOSBox process memory go through
//! [`GameController`].  The FSM tracks the controller lifecycle and
//! gates operations — writes are only allowed in the `Patched` state,
//! save/load transitions are non-blocking (polled per frame), and the
//! detach cleanup sequence is fully asynchronous.
//!
//! # Serialization contract
//!
//! Only one `GameController` instance should exist.  All DOSBox memory
//! access must go through it.  The controller is designed for
//! single-threaded use (one call to [`GameController::tick`] per egui
//! frame).  If save/load ever moves to a background thread, the
//! controller must be wrapped in a mutex.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::dosbox::config;
use crate::game::character::{self, Character};
use crate::game::injection::{self, PatchState};
use crate::game::inventory::{self, Inventory};
use crate::game::map::{self, MapState};
use crate::game::quest::{self, ShrineQuest};
use crate::game::save_state::{self, SlotInfo};
use crate::game::vehicle::{self, Frigate};
use crate::game::world_map::WorldMap;
use crate::gui::memory_watch_panel::MemoryWatch;
use crate::memory::access::MemoryAccess;
use crate::memory::process::{self, DosBoxProcess};
use crate::memory::scanner;
use crate::tiles::atlas::TileAtlas;

// ---------------------------------------------------------------------------
// FSM types
// ---------------------------------------------------------------------------

/// Controller lifecycle phase.
#[derive(Debug)]
enum Phase {
    /// No process attached.
    Disconnected,
    /// Process attached, periodically rescanning for game confirmation.
    Scanning { last_rescan: Instant },
    /// Game confirmed, dos_base found, but patch not yet applied.
    Attached,
    /// Cave + hooks installed.  Ready for normal operations.
    Patched,
    /// Trap flag set to 1, polling each frame for ack (flag == 2).
    Trapping {
        since: Instant,
        purpose: TrapPurpose,
    },
    /// Hooks restored, trap released.  Waiting for IP to leave cave.
    Detaching { released_at: Instant },
}

/// What to do once the game is trapped.
#[derive(Debug)]
enum TrapPurpose {
    Save { slot: usize },
    Load { slot: usize },
    Detach,
}

/// Result of a single [`GameController::tick`] call.
pub enum TickResult {
    /// Nothing to do.
    Idle,
    /// A transition is in progress; request a repaint.
    NeedsRepaint,
    /// Save completed successfully (slot index, metadata).
    SaveComplete(usize, SlotInfo),
    /// Load completed successfully (slot index).
    LoadComplete(usize),
    /// Detach completed (cave restored).
    Detached,
    /// Patch was just applied.
    PatchApplied,
    /// An error occurred (displayed in status bar).
    Error(String),
}

/// How often to rescan for game confirmation.
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Trap polling timeout.
const TRAP_TIMEOUT: Duration = Duration::from_secs(10);

/// Time to wait after releasing trap before restoring cave bytes.
const DETACH_WAIT: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// GameController
// ---------------------------------------------------------------------------

pub struct GameController {
    phase: Phase,
    process: Option<DosBoxProcess>,
    dos_base: Option<usize>,
    patch: Option<PatchState>,
    pub game_dir: Option<PathBuf>,
    pub tile_atlas: Option<TileAtlas>,
    pub world_map: Option<WorldMap>,
    game_confirmed: bool,
}

impl Default for GameController {
    fn default() -> Self {
        Self::new()
    }
}

impl GameController {
    pub fn new() -> Self {
        Self {
            phase: Phase::Disconnected,
            process: None,
            dos_base: None,
            patch: None,
            game_dir: None,
            tile_atlas: None,
            world_map: None,
            game_confirmed: false,
        }
    }

    // --- State queries ---

    /// Raw memory access for legacy callers (memory watch panel).
    /// Returns None if not connected.
    pub fn mem_access(&self) -> Option<(&dyn MemoryAccess, usize)> {
        let proc = self.process.as_ref()?;
        let base = self.dos_base?;
        Some((&proc.memory as &dyn MemoryAccess, base))
    }

    pub fn is_disconnected(&self) -> bool {
        matches!(self.phase, Phase::Disconnected)
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Patched)
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Trapping { .. } | Phase::Detaching { .. })
    }

    pub fn is_attached(&self) -> bool {
        self.process.is_some()
    }

    pub fn game_confirmed(&self) -> bool {
        self.game_confirmed
    }

    pub fn dos_base(&self) -> Option<usize> {
        self.dos_base
    }

    pub fn process_name(&self) -> Option<&str> {
        self.process.as_ref().map(|p| p.name.as_str())
    }

    pub fn process_pid(&self) -> Option<u32> {
        self.process.as_ref().map(|p| p.pid)
    }

    pub fn phase_label(&self) -> &'static str {
        match &self.phase {
            Phase::Disconnected => "",
            Phase::Scanning { .. } => "Waiting for game to load...",
            Phase::Attached => "Applying patch...",
            Phase::Patched => "",
            Phase::Trapping { purpose, .. } => match purpose {
                TrapPurpose::Save { .. } => "Saving...",
                TrapPurpose::Load { .. } => "Loading...",
                TrapPurpose::Detach => "Detaching...",
            },
            Phase::Detaching { .. } => "Restoring...",
        }
    }

    // --- Lifecycle ---

    pub fn attach(&mut self, pid: u32) -> Result<()> {
        let proc = process::attach(pid)?;

        let (dos_base, game_confirmed) = match scanner::find_dos_base(&proc.memory) {
            Ok(result) => (Some(result.dos_base), result.game_confirmed),
            Err(_) => (None, false),
        };

        // Try to locate game directory and load assets
        if let Ok(dir) = config::find_game_directory(proc.memory.handle()) {
            match TileAtlas::load(&dir) {
                Ok(atlas) => {
                    self.tile_atlas = Some(atlas);
                    match WorldMap::load(&dir) {
                        Ok(wm) => self.world_map = Some(wm),
                        Err(e) => log::warn!("World map failed: {e}"),
                    }
                }
                Err(e) => log::warn!("Tiles failed: {e}"),
            }
            self.game_dir = Some(dir);
        }

        self.dos_base = dos_base;
        self.game_confirmed = game_confirmed;
        self.process = Some(proc);

        if game_confirmed {
            self.phase = Phase::Attached;
        } else {
            self.phase = Phase::Scanning {
                last_rescan: Instant::now(),
            };
        }

        Ok(())
    }

    pub fn request_detach(&mut self) {
        match &self.phase {
            Phase::Patched => {
                if let (Some(patch), Some(proc)) = (&self.patch, &self.process) {
                    let _ = proc.memory.write_u8(patch.trap_flag_addr(), 1);
                    self.phase = Phase::Trapping {
                        since: Instant::now(),
                        purpose: TrapPurpose::Detach,
                    };
                    return;
                }
                self.reset();
            }
            _ => {
                // Best-effort: clear trap flag and restore hooks if we're
                // mid-transition (e.g. Trapping for a save when user hits
                // Disconnect).
                self.release_trap();
                self.restore_hooks();
                self.reset();
            }
        }
    }

    // --- Frame tick ---

    pub fn tick(&mut self) -> TickResult {
        // Check process liveness.  If the process is dead, attempt to
        // clean up the trap flag before resetting (best-effort — the
        // write will fail if the process is truly gone, which is fine).
        if self.process.as_ref().is_some_and(|p| !p.is_alive()) {
            if let (Some(proc), Some(patch)) = (&self.process, &self.patch) {
                let _ = proc.memory.write_u8(patch.trap_flag_addr(), 0);
                let _ = proc.memory.write_u8(patch.flag_addr(), 0);
            }
            self.reset();
            return TickResult::Error("Process terminated".into());
        }

        match &self.phase {
            Phase::Scanning { last_rescan } => {
                if last_rescan.elapsed() >= RESCAN_INTERVAL {
                    self.do_rescan();
                    if self.game_confirmed {
                        self.phase = Phase::Attached;
                        return self.try_apply_patch();
                    }
                    // Update rescan timer
                    self.phase = Phase::Scanning {
                        last_rescan: Instant::now(),
                    };
                }
                TickResult::NeedsRepaint
            }

            Phase::Attached => self.try_apply_patch(),

            Phase::Trapping { since, .. } => {
                let since = *since;
                let mem = match self.mem() {
                    Some(m) => m,
                    None => {
                        self.reset();
                        return TickResult::Error("Lost memory access".into());
                    }
                };
                let trap_addr = match &self.patch {
                    Some(p) => p.trap_flag_addr(),
                    None => {
                        self.reset();
                        return TickResult::Error("No patch state".into());
                    }
                };

                match mem.read_u8(trap_addr) {
                    Ok(2) => self.execute_trapped(),
                    Ok(_) if since.elapsed() > TRAP_TIMEOUT => {
                        let _ = mem.write_u8(trap_addr, 0);
                        let final_val = mem.read_u8(trap_addr).unwrap_or(0xFF);
                        self.phase = Phase::Patched;
                        TickResult::Error(format!(
                            "Trap timeout — flag is {final_val} after {TRAP_TIMEOUT:?}"
                        ))
                    }
                    Err(e) => {
                        // Best-effort: clear the trap flag so the game doesn't
                        // stay frozen in the spin loop.
                        let _ = mem.write_u8(trap_addr, 0);
                        self.reset();
                        TickResult::Error(format!("Memory read failed: {e}"))
                    }
                    _ => TickResult::NeedsRepaint,
                }
            }

            Phase::Detaching { released_at } => {
                if released_at.elapsed() >= DETACH_WAIT {
                    self.restore_cave_and_disconnect();
                    return TickResult::Detached;
                }
                TickResult::NeedsRepaint
            }

            _ => TickResult::Idle,
        }
    }

    // --- Read operations (valid in Patched, Scanning, Attached) ---

    pub fn read_party(&self) -> Result<Vec<Character>> {
        let (mem, base) = self.require_mem()?;
        character::read_party(mem, base)
    }

    pub fn read_inventory(&self) -> Result<Inventory> {
        let (mem, base) = self.require_mem()?;
        inventory::read_inventory(mem, base)
    }

    pub fn read_map_state(&self) -> Result<MapState> {
        let (mem, base) = self.require_mem()?;
        map::read_map_state(mem, base)
    }

    pub fn read_shrine_quest(&self) -> Result<ShrineQuest> {
        let (mem, base) = self.require_mem()?;
        quest::read_shrine_quest(mem, base)
    }

    pub fn read_frigates(&self) -> Result<Vec<Frigate>> {
        let (mem, base) = self.require_mem()?;
        vehicle::read_frigates(mem, base)
    }

    // --- Write operations (only valid in Patched) ---

    pub fn write_character(&self, ch: &Character) -> Result<()> {
        let (mem, base) = self.require_patched()?;
        character::write_character(mem, base, ch)
    }

    pub fn write_inventory(&self, inv: &Inventory) -> Result<()> {
        let (mem, base) = self.require_patched()?;
        inventory::write_inventory(mem, base, inv)
    }

    pub fn write_frigate_hull(&self, f: &Frigate) -> Result<()> {
        let (mem, base) = self.require_patched()?;
        vehicle::write_frigate_hull(mem, base, f)
    }

    pub fn write_frigate_skiffs(&self, f: &Frigate) -> Result<()> {
        let (mem, base) = self.require_patched()?;
        vehicle::write_frigate_skiffs(mem, base, f)
    }

    pub fn trigger_redraw(&self) -> Result<()> {
        let (mem, _) = self.require_patched()?;
        let patch = self
            .patch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no patch state"))?;
        injection::trigger_redraw(mem, patch)
    }

    pub fn request_save(&mut self, slot: usize) -> Result<()> {
        let (_, _) = self.require_patched()?;
        let patch = self
            .patch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no patch state"))?;
        let mem = &self
            .process
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("not connected"))?
            .memory;
        mem.write_u8(patch.trap_flag_addr(), 1)?;
        self.phase = Phase::Trapping {
            since: Instant::now(),
            purpose: TrapPurpose::Save { slot },
        };
        Ok(())
    }

    pub fn request_load(&mut self, slot: usize) -> Result<()> {
        let (_, _) = self.require_patched()?;
        let patch = self
            .patch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no patch state"))?;
        let mem = &self
            .process
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("not connected"))?
            .memory;
        mem.write_u8(patch.trap_flag_addr(), 1)?;
        self.phase = Phase::Trapping {
            since: Instant::now(),
            purpose: TrapPurpose::Load { slot },
        };
        Ok(())
    }

    // --- Memory watch (read-only, valid when attached) ---

    pub fn poll_memory_watch(&self, watch: &mut MemoryWatch) {
        if let (Some(proc), Some(base)) = (&self.process, self.dos_base) {
            watch.poll(&proc.memory, base);
        }
    }

    // --- Private helpers ---

    fn mem(&self) -> Option<&dyn MemoryAccess> {
        self.process
            .as_ref()
            .map(|p| &p.memory as &dyn MemoryAccess)
    }

    fn require_mem(&self) -> Result<(&dyn MemoryAccess, usize)> {
        let mem = self.mem().ok_or_else(|| anyhow::anyhow!("not connected"))?;
        let base = self
            .dos_base
            .ok_or_else(|| anyhow::anyhow!("dos_base not found"))?;
        Ok((mem, base))
    }

    fn require_patched(&self) -> Result<(&dyn MemoryAccess, usize)> {
        if !self.is_ready() {
            bail!("not ready (phase: {:?})", self.phase_label());
        }
        self.require_mem()
    }

    fn reset(&mut self) {
        self.phase = Phase::Disconnected;
        self.process = None;
        self.dos_base = None;
        self.patch = None;
        self.game_dir = None;
        self.tile_atlas = None;
        self.world_map = None;
        self.game_confirmed = false;
    }

    fn do_rescan(&mut self) {
        let Some(ref proc) = self.process else { return };
        match scanner::find_dos_base(&proc.memory) {
            Ok(result) => {
                self.dos_base = Some(result.dos_base);
                self.game_confirmed = result.game_confirmed;
            }
            Err(e) => log::debug!("Rescan failed: {e}"),
        }
    }

    fn try_apply_patch(&mut self) -> TickResult {
        if self.patch.is_some() {
            self.phase = Phase::Patched;
            return TickResult::Idle;
        }
        let Some(ref proc) = self.process else {
            return TickResult::Idle;
        };
        let Some(dos_base) = self.dos_base else {
            return TickResult::Idle;
        };

        match injection::apply_patch(&proc.memory, dos_base) {
            Ok(state) => {
                self.patch = Some(state);
                self.phase = Phase::Patched;
                log::info!("Patch applied");
                TickResult::PatchApplied
            }
            Err(e) => {
                // Stay in Attached — Patched requires self.patch to be Some,
                // and trigger_redraw/request_save/request_load unwrap it.
                self.phase = Phase::Attached;
                TickResult::Error(format!("Patch failed: {e}"))
            }
        }
    }

    fn execute_trapped(&mut self) -> TickResult {
        // Take the purpose out of the phase
        let purpose = match std::mem::replace(&mut self.phase, Phase::Patched) {
            Phase::Trapping { purpose, .. } => purpose,
            other => {
                self.phase = other;
                return TickResult::Idle;
            }
        };

        match purpose {
            TrapPurpose::Save { slot } => {
                let result = self.do_save_trapped(slot);
                self.release_trap();
                match result {
                    Ok(info) => TickResult::SaveComplete(slot, info),
                    Err(e) => TickResult::Error(format!("Save failed: {e}")),
                }
            }
            TrapPurpose::Load { slot } => {
                let result = self.do_load_trapped(slot);
                self.release_trap();
                match result {
                    Ok(()) => TickResult::LoadComplete(slot),
                    Err(e) => TickResult::Error(format!("Load failed: {e}")),
                }
            }
            TrapPurpose::Detach => {
                self.restore_hooks();
                self.release_trap();
                self.phase = Phase::Detaching {
                    released_at: Instant::now(),
                };
                TickResult::NeedsRepaint
            }
        }
    }

    fn do_save_trapped(&self, slot: usize) -> Result<SlotInfo> {
        let (mem, dos_base) = self.require_mem()?;
        let game_dir = self
            .game_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no game directory"))?;
        save_state::save_trapped(mem, dos_base, slot, game_dir)
    }

    fn do_load_trapped(&self, slot: usize) -> Result<()> {
        let (mem, dos_base) = self.require_mem()?;
        let patch = self
            .patch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no patch state"))?;
        let game_dir = self
            .game_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no game directory"))?;
        save_state::load_trapped(mem, dos_base, patch, slot, game_dir)
    }

    fn release_trap(&self) {
        if let (Some(proc), Some(patch)) = (&self.process, &self.patch) {
            let _ = proc.memory.write_u8(patch.trap_flag_addr(), 0);
        }
    }

    fn restore_hooks(&self) {
        if let (Some(proc), Some(patch)) = (&self.process, &self.patch) {
            let hook_addr = patch.cs_base() + injection::HOOK_OFFSET;
            let _ = proc.memory.write_bytes(hook_addr, patch.original_hook());
            if patch.has_putchar_hook() {
                let putchar_addr = patch.cs_base() + injection::PUTCHAR_OFFSET;
                let _ = proc
                    .memory
                    .write_bytes(putchar_addr, patch.original_putchar());
            }
        }
    }

    fn restore_cave_and_disconnect(&mut self) {
        if let (Some(proc), Some(patch)) = (&self.process, &self.patch) {
            let cave_addr = patch.cs_base() + patch.cave_cs_offset();
            let _ = proc.memory.write_bytes(cave_addr, patch.original_cave());
            let _ = proc.memory.write_u8(patch.flag_addr(), 0);
            let _ = proc.memory.write_u8(patch.trap_flag_addr(), 0);
        }
        self.reset();
    }
}
