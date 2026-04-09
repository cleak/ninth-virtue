use std::time::{Duration, Instant};

use crate::audio::AudioSession;
use crate::controller::{GameController, TickResult};
use crate::game::character::Character;
use crate::game::inventory::Inventory;
use crate::game::quest::ShrineQuest;
use crate::game::save_state;
use crate::game::vehicle::Frigate;
use crate::gui;
use crate::gui::memory_watch_panel::MemoryWatch;
use crate::gui::minimap_panel::MinimapState;
use crate::memory::process;

/// How often to scan for DOSBox processes when not attached.
const PROCESS_SCAN_INTERVAL: Duration = Duration::from_secs(2);

pub struct UltimaCompanion {
    ctrl: GameController,

    process_list: Vec<(u32, String)>,
    selected_pid: Option<u32>,
    last_process_scan: Instant,
    suppress_auto_attach: bool,

    party: Vec<Character>,
    inventory: Inventory,
    shrine_quest: ShrineQuest,
    frigates: Vec<Frigate>,
    minimap: MinimapState,

    auto_refresh: bool,
    refresh_interval_secs: f32,
    last_refresh: Instant,
    status_msg: String,

    selected_save_slot: usize,
    save_slots: Vec<Option<save_state::SlotInfo>>,

    memory_watch: MemoryWatch,

    audio_session: Option<AudioSession>,
    audio_volume: f32,
    audio_muted: bool,
}

impl UltimaCompanion {
    pub fn new() -> Self {
        let mut app = Self {
            ctrl: GameController::new(),
            process_list: Vec::new(),
            selected_pid: None,
            last_process_scan: Instant::now(),
            suppress_auto_attach: false,
            party: Vec::new(),
            inventory: Inventory::default(),
            shrine_quest: ShrineQuest::default(),
            frigates: Vec::new(),
            minimap: MinimapState::new(),
            auto_refresh: true,
            refresh_interval_secs: 0.025,
            last_refresh: Instant::now(),
            status_msg: "Searching for DOSBox...".to_string(),
            selected_save_slot: 0,
            save_slots: vec![None; save_state::NUM_SLOTS],
            memory_watch: MemoryWatch::default(),
            audio_session: None,
            audio_volume: 1.0,
            audio_muted: false,
        };
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
                    if self.ctrl.is_disconnected() {
                        self.status_msg = "Searching for DOSBox...".to_string();
                    }
                }
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
        match self.ctrl.attach(pid) {
            Ok(()) => {
                self.selected_pid = Some(pid);
                if let Some(name) = self.ctrl.process_name() {
                    if self.ctrl.game_confirmed() {
                        self.status_msg = format!("Connected to {name} (PID {pid})");
                    } else {
                        self.status_msg = "Waiting for game to load...".to_string();
                    }
                }
                if let Some(dir) = self.ctrl.game_dir.as_ref() {
                    self.save_slots = save_state::list_slots(dir);
                }
                self.try_acquire_audio(pid);
                if self.ctrl.game_confirmed() {
                    self.refresh_game_state();
                }
            }
            Err(e) => {
                self.status_msg = format!("Attach failed: {e}");
            }
        }
    }

    fn detach(&mut self) {
        self.ctrl.request_detach();
        self.suppress_auto_attach = true;
    }

    fn reset_cached_state(&mut self) {
        self.party.clear();
        self.inventory = Inventory::default();
        self.shrine_quest = ShrineQuest::default();
        self.frigates.clear();
        self.minimap.map = None;
        self.save_slots = vec![None; save_state::NUM_SLOTS];
    }

    fn refresh_game_state(&mut self) {
        match self.ctrl.read_party() {
            Ok(p) => self.party = p,
            Err(e) => {
                self.status_msg = format!("Read party failed: {e}");
                return;
            }
        }
        match self.ctrl.read_inventory() {
            Ok(inv) => self.inventory = inv,
            Err(e) => {
                self.status_msg = format!("Read inventory failed: {e}");
                return;
            }
        }
        match self.ctrl.read_shrine_quest() {
            Ok(sq) => self.shrine_quest = sq,
            Err(e) => {
                self.status_msg = format!("Read shrine quest failed: {e}");
                return;
            }
        }
        match self.ctrl.read_frigates() {
            Ok(f) => self.frigates = f,
            Err(e) => {
                self.frigates.clear();
                self.status_msg = format!("Read frigates failed: {e}");
                return;
            }
        }
        match self.ctrl.read_map_state() {
            Ok(ms) => self.minimap.map = Some(ms),
            Err(e) => {
                self.status_msg = format!("Read map failed: {e}");
            }
        }

        if let Some(pid) = self.ctrl.process_pid() {
            self.try_acquire_audio(pid);
        }

        self.last_refresh = Instant::now();
    }

    fn try_acquire_audio(&mut self, pid: u32) {
        if self.audio_session.is_some() {
            return;
        }
        if let Ok(Some(session)) = AudioSession::find_for_pid(pid) {
            self.audio_volume = session.get_volume().unwrap_or(1.0);
            self.audio_muted = session.get_mute().unwrap_or(false);
            self.audio_session = Some(session);
        }
    }
}

impl eframe::App for UltimaCompanion {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // --- FSM tick ---
        let tick = self.ctrl.tick();
        match tick {
            TickResult::NeedsRepaint => ctx.request_repaint(),
            TickResult::SaveComplete(slot, info) => {
                self.status_msg = format!("Saved slot {}", slot + 1);
                self.save_slots[slot] = Some(info);
            }
            TickResult::LoadComplete(slot) => {
                self.status_msg = format!("Loaded slot {}", slot + 1);
                self.refresh_game_state();
            }
            TickResult::Detached => {
                self.reset_cached_state();
                self.status_msg = "Disconnected".to_string();
            }
            TickResult::PatchApplied => {
                if self.ctrl.game_confirmed() {
                    self.refresh_game_state();
                    if let (Some(name), Some(pid)) =
                        (self.ctrl.process_name(), self.ctrl.process_pid())
                    {
                        self.status_msg = format!("Connected to {name} (PID {pid})");
                    }
                }
            }
            TickResult::Error(e) => {
                self.status_msg = e;
                if self.ctrl.is_disconnected() {
                    self.reset_cached_state();
                }
            }
            TickResult::Idle => {}
        }

        // --- Process scanning ---
        if self.ctrl.is_disconnected() {
            if self.last_process_scan.elapsed() >= PROCESS_SCAN_INTERVAL {
                self.scan_processes();
                if !self.suppress_auto_attach && self.process_list.len() == 1 {
                    let pid = self.process_list[0].0;
                    self.attach(pid);
                }
            }
            ctx.request_repaint_after(PROCESS_SCAN_INTERVAL);
        } else if self.ctrl.is_ready() && self.auto_refresh {
            let interval = Duration::from_secs_f32(self.refresh_interval_secs);
            if self.last_refresh.elapsed() >= interval {
                self.refresh_game_state();
            }
            ctx.request_repaint_after(interval);
        }

        // --- Connection bar ---
        let mut conn_action = gui::connection_bar::ConnectionAction::None;

        egui::TopBottomPanel::top("connection").show(ctx, |ui| {
            conn_action = gui::connection_bar::show(
                ui,
                &self.process_list,
                &mut self.selected_pid,
                self.ctrl.is_attached(),
                self.ctrl.game_confirmed(),
                self.ctrl.dos_base(),
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

        let phase_label = self.ctrl.phase_label();
        if !phase_label.is_empty() && self.status_msg != phase_label {
            self.status_msg = phase_label.to_string();
        }

        // --- Memory watch polling ---
        if self.memory_watch.recording {
            self.ctrl.poll_memory_watch(&mut self.memory_watch);
            ctx.request_repaint();
        }

        // --- Main content ---
        let UltimaCompanion {
            ctrl,
            party,
            inventory,
            shrine_quest,
            frigates,
            minimap,
            selected_save_slot,
            save_slots,
            status_msg,
            memory_watch,
            audio_session,
            audio_volume,
            audio_muted,
            ..
        } = self;

        gui::memory_watch_panel::show_with_ctrl(ctx, memory_watch, ctrl);

        egui::TopBottomPanel::bottom("minimap")
            .min_height(300.0)
            .resizable(true)
            .show(ctx, |ui| {
                gui::section_frame(ui).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    if let Some(atlas) = ctrl.tile_atlas.as_ref() {
                        gui::minimap_panel::show(ui, minimap, atlas, ctrl.world_map.as_ref());
                    } else if ctrl.is_attached() {
                        let status = match &ctrl.game_dir {
                            Some(dir) => format!("Tiles not found in {}", dir.display()),
                            None => "Game directory not found".to_string(),
                        };
                        gui::minimap_panel::show_no_atlas(ui, &status);
                    }
                });
            });

        let mut game_written = false;
        let mut save_action = gui::actions_panel::SaveAction::None;

        egui::CentralPanel::default().show(ctx, |ui| {
            gui::section_frame(ui).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                game_written |=
                    gui::party_panel::show(ui, party, frigates, minimap.map.as_ref(), ctrl);
            });

            ui.add_space(4.0);

            ui.columns(4, |cols| {
                gui::section_frame(&cols[0]).show(&mut cols[0], |ui| {
                    ui.set_min_size(ui.available_size());
                    game_written |= gui::inventory_panel::show_resources(ui, inventory, ctrl);
                });
                gui::section_frame(&cols[1]).show(&mut cols[1], |ui| {
                    ui.set_min_size(ui.available_size());
                    game_written |= gui::inventory_panel::show_reagents(ui, inventory, ctrl);
                });
                gui::section_frame(&cols[2]).show(&mut cols[2], |ui| {
                    ui.set_min_size(ui.available_size());
                    let (w, action) = gui::actions_panel::show(
                        ui,
                        party,
                        inventory,
                        frigates,
                        ctrl,
                        selected_save_slot,
                        save_slots,
                        status_msg,
                    );
                    game_written |= w;
                    save_action = action;
                    ui.add_space(8.0);
                    gui::audio_panel::show(ui, audio_session, audio_volume, audio_muted);
                });
                gui::section_frame(&cols[3]).show(&mut cols[3], |ui| {
                    ui.set_min_size(ui.available_size());
                    gui::quest_panel::show(ui, shrine_quest);
                });
            });
        });

        if game_written {
            let _ = self.ctrl.trigger_redraw();
            self.refresh_game_state();
        }

        match save_action {
            gui::actions_panel::SaveAction::Save(slot) => {
                if let Err(e) = self.ctrl.request_save(slot) {
                    self.status_msg = format!("Save failed: {e}");
                }
            }
            gui::actions_panel::SaveAction::Load(slot) => {
                if let Err(e) = self.ctrl.request_load(slot) {
                    self.status_msg = format!("Load failed: {e}");
                }
            }
            gui::actions_panel::SaveAction::None => {}
        }
    }
}
