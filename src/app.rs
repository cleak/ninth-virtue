use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::dosbox::config;
use crate::game::character::{Character, read_party};
use crate::game::injection::{self, PatchState};
use crate::game::inventory::{Inventory, read_inventory};
use crate::game::map;
use crate::game::world_map::WorldMap;
use crate::gui;
use crate::gui::memory_watch_panel::MemoryWatch;
use crate::gui::minimap_panel::MinimapState;
use crate::memory::access::MemoryAccess;
use crate::memory::process::{self, DosBoxProcess};
use crate::memory::scanner;
use crate::tiles::atlas::TileAtlas;

pub struct AttachedProcess {
    pub process: DosBoxProcess,
    pub dos_base: Option<usize>,
    pub game_confirmed: bool,
}

/// How often to scan for DOSBox processes when not attached.
const PROCESS_SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// How often to rescan memory when attached but game not yet confirmed.
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);

pub struct UltimaCompanion {
    // Connection state
    process_list: Vec<(u32, String)>,
    selected_pid: Option<u32>,
    attached: Option<AttachedProcess>,

    // Game state (cached)
    party: Vec<Character>,
    inventory: Inventory,
    minimap: MinimapState,
    game_dir: Option<PathBuf>,
    tile_atlas: Option<TileAtlas>,
    world_map: Option<WorldMap>,

    // Timing
    last_process_scan: Instant,
    last_rescan: Instant,

    // After manual disconnect, suppress auto-attach until DOSBox exits.
    suppress_auto_attach: bool,

    // UI state
    auto_refresh: bool,
    refresh_interval_secs: f32,
    last_refresh: Instant,
    status_msg: String,

    // Code injection for stats redraw
    patch_state: Option<PatchState>,

    // Debug: memory watch
    memory_watch: MemoryWatch,
}

impl UltimaCompanion {
    pub fn new() -> Self {
        let mut app = Self {
            process_list: Vec::new(),
            selected_pid: None,
            attached: None,
            party: Vec::new(),
            inventory: Inventory::default(),
            minimap: MinimapState::new(),
            game_dir: None,
            tile_atlas: None,
            world_map: None,
            last_process_scan: Instant::now(),
            last_rescan: Instant::now(),
            suppress_auto_attach: false,
            auto_refresh: true,
            refresh_interval_secs: 1.0,
            last_refresh: Instant::now(),
            status_msg: "Searching for DOSBox...".to_string(),
            patch_state: None,
            memory_watch: MemoryWatch::default(),
        };
        // Scan immediately on startup so we connect without delay.
        app.scan_processes();
        if app.process_list.len() == 1 {
            let pid = app.process_list[0].0;
            app.attach(pid);
        }
        app
    }

    fn scan_processes(&mut self) {
        match process::list_dosbox_processes() {
            Ok(list) => {
                if list.is_empty() {
                    self.suppress_auto_attach = false;
                    if self.attached.is_none() {
                        self.status_msg = "Searching for DOSBox...".to_string();
                    }
                }
                // Reset selection if the current PID disappeared.
                if !list.iter().any(|(p, _)| Some(*p) == self.selected_pid) {
                    self.selected_pid = list.first().map(|(pid, _)| *pid);
                }
                self.process_list = list;
            }
            Err(e) => {
                self.status_msg = format!("Scan failed: {e}");
            }
        }
        self.last_process_scan = Instant::now();
    }

    fn attach(&mut self, pid: u32) {
        match process::attach(pid) {
            Ok(proc) => {
                let (dos_base, game_confirmed) = match scanner::find_dos_base(&proc.memory) {
                    Ok(result) => (Some(result.dos_base), result.game_confirmed),
                    Err(e) => {
                        self.status_msg = format!("Attached, scan failed: {e}");
                        (None, false)
                    }
                };

                if dos_base.is_some() {
                    if game_confirmed {
                        self.status_msg = format!("Connected to {} (PID {})", proc.name, proc.pid,);
                    } else {
                        self.status_msg = "Waiting for game to load...".to_string();
                    }
                }

                // Try to locate game data files and load tile atlas + world map
                match config::find_game_directory(proc.memory.handle()) {
                    Ok(dir) => {
                        match TileAtlas::load(&dir) {
                            Ok(atlas) => {
                                self.tile_atlas = Some(atlas);
                                // Only load the world map if the atlas succeeded
                                match WorldMap::load(&dir) {
                                    Ok(wm) => self.world_map = Some(wm),
                                    Err(e) => {
                                        self.status_msg = format!(
                                            "World map failed: {e} (dir: {})",
                                            dir.display()
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                self.status_msg =
                                    format!("Tiles failed: {e} (dir: {})", dir.display());
                            }
                        }
                        self.game_dir = Some(dir);
                    }
                    Err(e) => {
                        self.status_msg = format!("Game dir not found: {e}");
                    }
                }

                self.selected_pid = Some(pid);
                self.attached = Some(AttachedProcess {
                    process: proc,
                    dos_base,
                    game_confirmed,
                });
                self.last_rescan = Instant::now();

                if game_confirmed {
                    self.refresh_game_state();
                    self.try_apply_patch();
                }
            }
            Err(e) => {
                self.status_msg = format!("Attach failed: {e}");
            }
        }
    }

    fn detach(&mut self) {
        // Remove the code patch before dropping the process handle.
        if let (Some(attached), Some(state)) = (&self.attached, &self.patch_state) {
            injection::remove_patch(&attached.process.memory, state);
        }
        self.patch_state = None;
        self.attached = None;
        self.party.clear();
        self.inventory = Inventory::default();
        self.minimap.map = None;
        self.game_dir = None;
        self.tile_atlas = None;
        self.world_map = None;
        self.suppress_auto_attach = true;
        self.status_msg = "Disconnected".to_string();
    }

    /// Try to apply the code-cave patch for stats redraw.  Non-fatal on
    /// failure — the app continues to work, just without on-demand redraw.
    fn try_apply_patch(&mut self) {
        // Don't re-apply if already patched.
        if self.patch_state.is_some() {
            return;
        }

        let Some(ref attached) = self.attached else {
            return;
        };
        let Some(dos_base) = attached.dos_base else {
            return;
        };

        match injection::apply_patch(&attached.process.memory, dos_base) {
            Ok(state) => {
                self.patch_state = Some(state);
                eprintln!("Redraw patch applied successfully");
            }
            Err(e) => {
                eprintln!("Redraw patch failed (non-fatal): {e}");
            }
        }
    }

    fn handle_process_death(&mut self) {
        self.patch_state = None; // Don't try to unpatch a dead process.
        self.attached = None;
        self.party.clear();
        self.inventory = Inventory::default();
        self.minimap.map = None;
        self.status_msg = "Process terminated".to_string();
    }

    fn rescan_memory(&mut self) {
        let Some(ref mut attached) = self.attached else {
            return;
        };
        match scanner::find_dos_base(&attached.process.memory) {
            Ok(result) => {
                attached.dos_base = Some(result.dos_base);
                let was_confirmed = attached.game_confirmed;
                attached.game_confirmed = result.game_confirmed;
                if result.game_confirmed {
                    self.status_msg = format!(
                        "Connected to {} (PID {})",
                        attached.process.name, attached.process.pid,
                    );
                    if !was_confirmed {
                        // Game just became confirmed — apply patch.
                        // (refresh_game_state is called by the caller)
                    }
                }
            }
            Err(e) => {
                self.status_msg = format!("Rescan failed: {e}");
            }
        }
        self.last_rescan = Instant::now();
    }

    fn refresh_game_state(&mut self) {
        let Some(ref attached) = self.attached else {
            return;
        };
        let Some(dos_base) = attached.dos_base else {
            return;
        };

        if !attached.process.is_alive() {
            self.handle_process_death();
            return;
        }

        let mem: &dyn MemoryAccess = &attached.process.memory;

        match read_party(mem, dos_base) {
            Ok(p) => self.party = p,
            Err(e) => {
                self.status_msg = format!("Read party failed: {e}");
                return;
            }
        }

        match read_inventory(mem, dos_base) {
            Ok(inv) => self.inventory = inv,
            Err(e) => {
                self.status_msg = format!("Read inventory failed: {e}");
                return;
            }
        }

        match map::read_map_state(mem, dos_base) {
            Ok(ms) => self.minimap.map = Some(ms),
            Err(e) => {
                self.status_msg = format!("Read map failed: {e}");
            }
        }

        self.last_refresh = Instant::now();
    }
}

impl eframe::App for UltimaCompanion {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // --- Auto-management ---
        if self.attached.is_none() {
            // Periodically scan for DOSBox processes.
            if self.last_process_scan.elapsed() >= PROCESS_SCAN_INTERVAL {
                self.scan_processes();
                // Auto-attach if exactly one process and not suppressed.
                if !self.suppress_auto_attach && self.process_list.len() == 1 {
                    let pid = self.process_list[0].0;
                    self.attach(pid);
                }
            }
            ctx.request_repaint_after(PROCESS_SCAN_INTERVAL);
        } else if !self.attached.as_ref().unwrap().process.is_alive() {
            // Process died -- clean up.
            self.handle_process_death();
        } else {
            let game_confirmed = self.attached.as_ref().unwrap().game_confirmed;

            if !game_confirmed {
                // Attached but game not loaded: periodically rescan memory.
                if self.last_rescan.elapsed() >= RESCAN_INTERVAL {
                    self.rescan_memory();
                    if self.attached.as_ref().is_some_and(|a| a.game_confirmed) {
                        self.refresh_game_state();
                        self.try_apply_patch();
                    }
                }
                ctx.request_repaint_after(RESCAN_INTERVAL);
            } else if self.auto_refresh {
                let interval = Duration::from_secs_f32(self.refresh_interval_secs);
                if self.last_refresh.elapsed() >= interval {
                    self.refresh_game_state();
                }
                ctx.request_repaint_after(interval);
            }
        }

        // --- Connection bar ---
        let mut conn_action = gui::connection_bar::ConnectionAction::None;
        let is_attached = self.attached.is_some();
        let game_confirmed = self.attached.as_ref().is_some_and(|a| a.game_confirmed);
        let dos_base = self.attached.as_ref().and_then(|a| a.dos_base);

        egui::TopBottomPanel::top("connection").show(ctx, |ui| {
            conn_action = gui::connection_bar::show(
                ui,
                &self.process_list,
                &mut self.selected_pid,
                is_attached,
                game_confirmed,
                dos_base,
                &mut self.auto_refresh,
                &mut self.refresh_interval_secs,
                &mut self.memory_watch.visible,
                &self.status_msg,
            );
        });

        match conn_action {
            gui::connection_bar::ConnectionAction::Attach(pid) => {
                self.suppress_auto_attach = false;
                self.attach(pid);
            }
            gui::connection_bar::ConnectionAction::Detach => self.detach(),
            gui::connection_bar::ConnectionAction::None => {}
        }

        // --- Memory watch polling (every frame while recording) ---
        if self.memory_watch.recording {
            if let Some(ref attached) = self.attached
                && let Some(dos_base) = attached.dos_base
            {
                self.memory_watch.poll(&attached.process.memory, dos_base);
            }
            // Keep repainting at max rate while recording.
            ctx.request_repaint();
        }

        // --- Main content ---
        // Destructure for disjoint borrow splitting
        let UltimaCompanion {
            attached,
            party,
            inventory,
            minimap,
            tile_atlas,
            world_map,
            game_dir,
            patch_state,
            memory_watch,
            ..
        } = self;

        let mem: Option<(&dyn MemoryAccess, usize)> = attached.as_ref().and_then(|a| {
            a.dos_base
                .map(|base| (&a.process.memory as &dyn MemoryAccess, base))
        });

        // --- Memory watch window ---
        gui::memory_watch_panel::show(ctx, memory_watch, mem);

        // --- Mini-map (anchored bottom panel) ---
        egui::TopBottomPanel::bottom("minimap")
            .min_height(300.0)
            .resizable(true)
            .show(ctx, |ui| {
                gui::section_frame(ui).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if let Some(atlas) = tile_atlas.as_ref() {
                        gui::minimap_panel::show(ui, minimap, atlas, world_map.as_ref());
                    } else if attached.is_some() {
                        // Only show load errors when actually attached to a process
                        let status = match game_dir {
                            Some(dir) => format!("Tiles not found in {}", dir.display()),
                            None => "Game directory not found — could not locate DOSBox config"
                                .to_string(),
                        };
                        gui::minimap_panel::show_no_atlas(ui, &status);
                    }
                });
            });

        // --- Party, inventory, actions ---
        let mut game_written = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            gui::section_frame(ui).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                game_written |= gui::party_panel::show(ui, party, mem);
            });

            ui.add_space(4.0);

            ui.columns(3, |cols| {
                gui::section_frame(&cols[0]).show(&mut cols[0], |ui| {
                    ui.set_min_size(ui.available_size());
                    game_written |= gui::inventory_panel::show_resources(ui, inventory, mem);
                });
                gui::section_frame(&cols[1]).show(&mut cols[1], |ui| {
                    ui.set_min_size(ui.available_size());
                    game_written |= gui::inventory_panel::show_reagents(ui, inventory, mem);
                });
                gui::section_frame(&cols[2]).show(&mut cols[2], |ui| {
                    ui.set_min_size(ui.available_size());
                    game_written |= gui::actions_panel::show(ui, party, inventory, mem);
                });
            });
        });

        // After all panels have run, trigger a single redraw if any
        // panel wrote to game memory.
        if game_written && let (Some((mem, _)), Some(patch)) = (mem, patch_state.as_ref()) {
            let _ = injection::trigger_redraw(mem, patch);
        }
    }
}
