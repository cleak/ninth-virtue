use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::audio::AudioSession;
use crate::dosbox::config;
use crate::game::character::{
    Character, PartyLocks, apply_party_locks, read_party, write_character,
};
use crate::game::injection::{self, PatchState};
use crate::game::inventory::{
    Inventory, InventoryLocks, apply_inventory_locks, read_inventory, write_inventory,
};
use crate::game::map;
use crate::game::quest::{ShrineQuest, read_shrine_quest};
use crate::game::vehicle::{self, Frigate};
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
/// Keep lock-only polling responsive without forcing full-speed auto-refresh.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub struct UltimaCompanion {
    // Connection state
    process_list: Vec<(u32, String)>,
    selected_pid: Option<u32>,
    attached: Option<AttachedProcess>,

    // Game state (cached)
    party: Vec<Character>,
    inventory: Inventory,
    shrine_quest: ShrineQuest,
    frigates: Vec<Frigate>,
    minimap: MinimapState,
    game_dir: Option<PathBuf>,
    tile_atlas: Option<TileAtlas>,
    tile_atlas_error: Option<String>,
    world_map: Option<WorldMap>,

    // Timing
    last_process_scan: Instant,
    last_rescan: Instant,

    // After manual disconnect, suppress auto-attach until DOSBox exits.
    suppress_auto_attach: bool,

    // UI state
    auto_refresh: bool,
    party_locks: PartyLocks,
    inventory_locks: InventoryLocks,
    refresh_interval_secs: f32,
    last_refresh: Instant,
    status_msg: String,

    // Code injection for stats redraw
    patch_state: Option<PatchState>,

    // Debug: memory watch
    memory_watch: MemoryWatch,

    // Audio volume control (WASAPI)
    audio_session: Option<AudioSession>,
    audio_volume: f32,
    audio_muted: bool,
}

impl UltimaCompanion {
    pub fn new() -> Self {
        let mut app = Self {
            process_list: Vec::new(),
            selected_pid: None,
            attached: None,
            party: Vec::new(),
            inventory: Inventory::default(),
            shrine_quest: ShrineQuest::default(),
            frigates: Vec::new(),
            minimap: MinimapState::new(),
            game_dir: None,
            tile_atlas: None,
            tile_atlas_error: None,
            world_map: None,
            last_process_scan: Instant::now(),
            last_rescan: Instant::now(),
            suppress_auto_attach: false,
            auto_refresh: true,
            party_locks: PartyLocks::default(),
            inventory_locks: InventoryLocks::default(),
            refresh_interval_secs: 0.025,
            last_refresh: Instant::now(),
            status_msg: "Searching for DOSBox...".to_string(),
            patch_state: None,
            memory_watch: MemoryWatch::default(),
            audio_session: None,
            audio_volume: 1.0,
            audio_muted: false,
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
                self.tile_atlas_error = None;
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
                                let load_error = format!(
                                    "Failed to load tile atlas from {}: {e}",
                                    dir.display()
                                );
                                self.status_msg = load_error.clone();
                                self.tile_atlas_error = Some(load_error);
                            }
                        }
                        self.game_dir = Some(dir);
                    }
                    Err(e) => {
                        let game_dir_error = format!("Game dir not found: {e}");
                        self.status_msg = game_dir_error.clone();
                        self.tile_atlas_error = Some(game_dir_error);
                    }
                }

                self.selected_pid = Some(pid);
                self.try_acquire_audio(pid);
                self.attached = Some(AttachedProcess {
                    process: proc,
                    dos_base,
                    game_confirmed,
                });
                self.last_rescan = Instant::now();

                if game_confirmed {
                    self.sync_confirmed_game_state();
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
        self.clear_attached_state("Disconnected");
        self.suppress_auto_attach = true;
    }

    fn clear_attached_state(&mut self, status_msg: &str) {
        self.attached = None;
        self.party.clear();
        self.inventory = Inventory::default();
        self.shrine_quest = ShrineQuest::default();
        self.frigates.clear();
        self.minimap.clear();
        self.game_dir = None;
        self.tile_atlas = None;
        self.tile_atlas_error = None;
        self.world_map = None;
        self.audio_session = None;
        self.memory_watch.stop_recording();
        self.status_msg = status_msg.to_string();
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

    fn sync_confirmed_game_state(&mut self) {
        // Install the redraw hook before the first live refresh so any lock
        // writes during that refresh can trigger an immediate in-game redraw.
        self.try_apply_patch();
        self.refresh_game_state();
    }

    /// Try to find DOSBox's audio session. Non-fatal - the audio controls
    /// are simply disabled until we find the session.
    fn try_acquire_audio(&mut self, pid: u32) {
        if self.audio_session.is_some() {
            return;
        }
        match AudioSession::find_for_pid(pid) {
            Ok(Some(session)) => {
                // Read initial state from the OS mixer.
                self.audio_volume = session.get_volume().unwrap_or(1.0);
                self.audio_muted = session.get_mute().unwrap_or(false);
                self.audio_session = Some(session);
            }
            Ok(None) => {} // No session yet; DOSBox may not be producing audio.
            Err(e) => eprintln!("Audio session lookup failed: {e}"),
        }
    }

    fn handle_process_death(&mut self) {
        self.patch_state = None; // Don't try to unpatch a dead process.
        self.clear_attached_state("Process terminated");
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

        let pid = attached.process.pid;
        let mem: &dyn MemoryAccess = &attached.process.memory;
        let mut game_written = false;

        match read_party(mem, dos_base) {
            Ok(p) => self.party = p,
            Err(e) => {
                self.status_msg = format!("Read party failed: {e}");
                return;
            }
        }

        if self.party_locks.any_active() {
            for ch in &mut self.party {
                if apply_party_locks(ch, &self.party_locks) {
                    if let Err(e) = write_character(mem, dos_base, ch) {
                        self.status_msg = format!("Write character failed: {e}");
                        return;
                    }
                    game_written = true;
                }
            }
        }

        match read_inventory(mem, dos_base) {
            Ok(inv) => self.inventory = inv,
            Err(e) => {
                self.status_msg = format!("Read inventory failed: {e}");
                return;
            }
        }

        if apply_inventory_locks(&mut self.inventory, &self.inventory_locks) {
            if let Err(e) = write_inventory(mem, dos_base, &self.inventory) {
                self.status_msg = format!("Write inventory failed: {e}");
                return;
            }
            game_written = true;
        }

        match read_shrine_quest(mem, dos_base) {
            Ok(sq) => self.shrine_quest = sq,
            Err(e) => {
                self.status_msg = format!("Read shrine quest failed: {e}");
                return;
            }
        }

        match vehicle::read_frigates(mem, dos_base) {
            Ok(f) => self.frigates = f,
            Err(e) => {
                self.frigates.clear();
                self.status_msg = format!("Read frigates failed: {e}");
                return;
            }
        }

        match map::read_map_state(mem, dos_base) {
            Ok(ms) => self.minimap.map = Some(ms),
            Err(e) => {
                self.status_msg = format!("Read map failed: {e}");
            }
        }

        if game_written && let Some(ref patch) = self.patch_state {
            let _ = injection::trigger_redraw(mem, patch);
        }

        // Lazy audio session acquisition: DOSBox may not have an audio
        // session until it starts producing sound.
        self.try_acquire_audio(pid);

        self.last_refresh = Instant::now();
    }

    fn has_active_locks(&self) -> bool {
        self.party_locks.any_active() || self.inventory_locks.any_active()
    }

    fn refresh_interval(&self) -> Option<Duration> {
        let locks_active = self.has_active_locks();
        if !(self.auto_refresh || locks_active) {
            return None;
        }

        if self.auto_refresh {
            let lock_poll_secs = LOCK_POLL_INTERVAL.as_secs_f32();
            if locks_active && self.refresh_interval_secs >= lock_poll_secs {
                Some(LOCK_POLL_INTERVAL)
            } else {
                Some(Duration::from_secs_f32(self.refresh_interval_secs))
            }
        } else {
            Some(LOCK_POLL_INTERVAL)
        }
    }
}

impl eframe::App for UltimaCompanion {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                        self.sync_confirmed_game_state();
                    }
                }
                ctx.request_repaint_after(RESCAN_INTERVAL);
            } else if let Some(interval) = self.refresh_interval() {
                if self.last_refresh.elapsed() >= interval {
                    self.refresh_game_state();
                }
                ctx.request_repaint_after(interval);
            }
        }

        // --- Memory watch polling (every frame while recording) ---
        if self.memory_watch.recording {
            if let Some(ref attached) = self.attached {
                if let Some(dos_base) = attached.dos_base {
                    self.memory_watch.poll(&attached.process.memory, dos_base);
                    // Keep repainting at max rate while recording while we have a source to poll.
                    ctx.request_repaint();
                }
            } else {
                self.memory_watch.stop_recording();
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // --- Connection bar ---
        let mut conn_action = gui::connection_bar::ConnectionAction::None;
        let is_attached = self.attached.is_some();
        let game_confirmed = self.attached.as_ref().is_some_and(|a| a.game_confirmed);
        let dos_base = self.attached.as_ref().and_then(|a| a.dos_base);

        egui::Panel::top("connection").show_inside(ui, |ui| {
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

        // --- Main content ---
        // Destructure for disjoint borrow splitting
        let UltimaCompanion {
            attached,
            party,
            inventory,
            party_locks,
            inventory_locks,
            shrine_quest,
            frigates,
            minimap,
            tile_atlas,
            tile_atlas_error,
            world_map,
            game_dir,
            patch_state,
            memory_watch,
            audio_session,
            audio_volume,
            audio_muted,
            ..
        } = self;

        let mem: Option<(&dyn MemoryAccess, usize)> = attached.as_ref().and_then(|a| {
            a.dos_base
                .map(|base| (&a.process.memory as &dyn MemoryAccess, base))
        });

        // --- Memory watch window ---
        gui::memory_watch_panel::show(&ctx, memory_watch, mem);

        // --- Party, inventory, actions ---
        let mut game_written = false;

        // Let the dashboard keep its natural content height so the minimap
        // can take the rest of the window.
        egui::Panel::top("dashboard").show_inside(ui, |ui| {
            gui::section_frame(ui).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                game_written |= gui::party_panel::show(
                    ui,
                    party,
                    party_locks,
                    frigates,
                    minimap.map.as_ref(),
                    mem,
                );
            });

            ui.add_space(4.0);

            ui.columns(4, |cols| {
                gui::section_frame(&cols[0]).show(&mut cols[0], |ui| {
                    ui.set_min_width(ui.available_width());
                    game_written |=
                        gui::inventory_panel::show_resources(ui, inventory, inventory_locks, mem);
                });
                gui::section_frame(&cols[1]).show(&mut cols[1], |ui| {
                    ui.set_min_width(ui.available_width());
                    game_written |=
                        gui::inventory_panel::show_reagents(ui, inventory, inventory_locks, mem);
                });
                gui::section_frame(&cols[2]).show(&mut cols[2], |ui| {
                    ui.set_min_width(ui.available_width());
                    game_written |= gui::actions_panel::show(ui, party, inventory, frigates, mem);
                    ui.add_space(8.0);
                    gui::audio_panel::show(ui, audio_session, audio_volume, audio_muted);
                });
                gui::section_frame(&cols[3]).show(&mut cols[3], |ui| {
                    ui.set_min_width(ui.available_width());
                    gui::quest_panel::show(ui, shrine_quest);
                });
            });
        });

        // --- Mini-map (fills all remaining space) ---
        egui::CentralPanel::default().show_inside(ui, |ui| {
            gui::section_frame(ui).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                if let Some(atlas) = tile_atlas.as_ref() {
                    gui::minimap_panel::show(ui, minimap, atlas, world_map.as_ref());
                } else if attached.is_some() {
                    // Only show load errors when actually attached to a process
                    let status = tile_atlas_error.clone().unwrap_or_else(|| match game_dir {
                        Some(dir) => format!("Failed to load tile atlas from {}", dir.display()),
                        None => {
                            "Game directory not found; could not locate DOSBox config".to_string()
                        }
                    });
                    gui::minimap_panel::show_no_atlas(ui, &status);
                }
            });
        });

        // After all panels have run, trigger a single redraw if any
        // panel wrote to game memory.
        if game_written && let (Some((mem, _)), Some(patch)) = (mem, patch_state.as_ref()) {
            let _ = injection::trigger_redraw(mem, patch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> UltimaCompanion {
        let mut memory_watch = MemoryWatch::default();
        memory_watch.visible = true;
        memory_watch.recording = true;

        UltimaCompanion {
            process_list: vec![(1234, "dosbox".to_string())],
            selected_pid: Some(1234),
            attached: None,
            party: Vec::new(),
            inventory: Inventory::default(),
            shrine_quest: ShrineQuest::default(),
            frigates: Vec::new(),
            minimap: MinimapState::new(),
            game_dir: Some(PathBuf::from("C:/games/u5")),
            tile_atlas: None,
            tile_atlas_error: None,
            world_map: None,
            last_process_scan: Instant::now(),
            last_rescan: Instant::now(),
            suppress_auto_attach: false,
            auto_refresh: true,
            party_locks: PartyLocks::default(),
            inventory_locks: InventoryLocks::default(),
            refresh_interval_secs: 0.025,
            last_refresh: Instant::now(),
            status_msg: "Connected".to_string(),
            patch_state: None,
            memory_watch,
            audio_session: None,
            audio_volume: 1.0,
            audio_muted: false,
        }
    }

    #[test]
    fn detach_stops_memory_watch_recording() {
        let mut app = test_app();

        app.detach();

        assert!(!app.memory_watch.recording);
        assert!(app.suppress_auto_attach);
        assert_eq!(app.status_msg, "Disconnected");
    }

    #[test]
    fn handle_process_death_stops_memory_watch_recording() {
        let mut app = test_app();

        app.handle_process_death();

        assert!(!app.memory_watch.recording);
        assert_eq!(app.status_msg, "Process terminated");
    }

    #[test]
    fn refresh_interval_uses_auto_refresh_when_no_locks_are_active() {
        let mut app = test_app();
        app.refresh_interval_secs = 0.25;

        assert_eq!(app.refresh_interval(), Some(Duration::from_secs_f32(0.25)));
    }

    #[test]
    fn refresh_interval_is_capped_when_locks_are_active() {
        let mut app = test_app();
        app.refresh_interval_secs = 0.25;
        app.party_locks.mana = true;

        assert_eq!(app.refresh_interval(), Some(LOCK_POLL_INTERVAL));
    }

    #[test]
    fn refresh_interval_uses_lock_poll_when_auto_refresh_is_disabled() {
        let mut app = test_app();
        app.auto_refresh = false;
        app.inventory_locks.food = true;

        assert_eq!(app.refresh_interval(), Some(LOCK_POLL_INTERVAL));
    }

    #[test]
    fn refresh_interval_is_none_when_auto_refresh_and_locks_are_disabled() {
        let mut app = test_app();
        app.auto_refresh = false;

        assert_eq!(app.refresh_interval(), None);
    }
}
