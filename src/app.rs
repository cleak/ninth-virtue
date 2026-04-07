use std::time::{Duration, Instant};

use crate::game::character::{Character, read_party};
use crate::game::inventory::{Inventory, read_inventory};
use crate::gui;
use crate::memory::access::MemoryAccess;
use crate::memory::process::{self, DosBoxProcess};
use crate::memory::scanner;

pub struct AttachedProcess {
    pub process: DosBoxProcess,
    pub dos_base: Option<usize>,
    pub game_confirmed: bool,
}

pub struct UltimaCompanion {
    // Connection state
    process_list: Vec<(u32, String)>,
    selected_pid: Option<u32>,
    attached: Option<AttachedProcess>,

    // Game state (cached)
    party: Vec<Character>,
    inventory: Inventory,

    // UI state
    auto_refresh: bool,
    refresh_interval_secs: f32,
    last_refresh: Instant,
    status_msg: String,
}

impl UltimaCompanion {
    pub fn new() -> Self {
        Self {
            process_list: Vec::new(),
            selected_pid: None,
            attached: None,
            party: Vec::new(),
            inventory: Inventory::default(),
            auto_refresh: true,
            refresh_interval_secs: 1.0,
            last_refresh: Instant::now(),
            status_msg: String::new(),
        }
    }

    fn scan_processes(&mut self) {
        match process::list_dosbox_processes() {
            Ok(list) => {
                self.status_msg = format!("Found {} DOSBox process(es)", list.len());
                if self.selected_pid.is_none() {
                    self.selected_pid = list.first().map(|(pid, _)| *pid);
                }
                self.process_list = list;
            }
            Err(e) => {
                self.status_msg = format!("Scan failed: {e}");
            }
        }
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

                if let Some(base) = dos_base {
                    self.status_msg = format!(
                        "Attached to {} (PID {}), base={base:#x}{}",
                        proc.name,
                        proc.pid,
                        if game_confirmed {
                            ""
                        } else {
                            " (game not loaded)"
                        }
                    );
                }

                self.attached = Some(AttachedProcess {
                    process: proc,
                    dos_base,
                    game_confirmed,
                });
                self.refresh_game_state();
            }
            Err(e) => {
                self.status_msg = format!("Attach failed: {e}");
            }
        }
    }

    fn detach(&mut self) {
        self.attached = None;
        self.party.clear();
        self.inventory = Inventory::default();
        self.status_msg = "Disconnected".to_string();
    }

    fn rescan_memory(&mut self) {
        let Some(ref mut attached) = self.attached else {
            return;
        };
        match scanner::find_dos_base(&attached.process.memory) {
            Ok(result) => {
                attached.dos_base = Some(result.dos_base);
                attached.game_confirmed = result.game_confirmed;
                self.status_msg = format!(
                    "Rescan: base={:#x}{}",
                    result.dos_base,
                    if result.game_confirmed {
                        ""
                    } else {
                        " (game not loaded)"
                    }
                );
            }
            Err(e) => {
                self.status_msg = format!("Rescan failed: {e}");
            }
        }
    }

    fn refresh_game_state(&mut self) {
        let Some(ref attached) = self.attached else {
            return;
        };
        let Some(dos_base) = attached.dos_base else {
            return;
        };

        if !attached.process.is_alive() {
            self.status_msg = "Process terminated".to_string();
            self.attached = None;
            self.party.clear();
            self.inventory = Inventory::default();
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

        self.last_refresh = Instant::now();
    }
}

impl eframe::App for UltimaCompanion {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Auto-refresh
        if self.auto_refresh && self.attached.is_some() {
            let interval = Duration::from_secs_f32(self.refresh_interval_secs);
            if self.last_refresh.elapsed() >= interval {
                self.refresh_game_state();
            }
            ctx.request_repaint_after(interval);
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
                &self.status_msg,
            );
        });

        match conn_action {
            gui::connection_bar::ConnectionAction::ScanProcesses => self.scan_processes(),
            gui::connection_bar::ConnectionAction::Attach(pid) => self.attach(pid),
            gui::connection_bar::ConnectionAction::Detach => self.detach(),
            gui::connection_bar::ConnectionAction::RescanMemory => self.rescan_memory(),
            gui::connection_bar::ConnectionAction::None => {}
        }

        // --- Main content ---
        // Destructure for disjoint borrow splitting
        let UltimaCompanion {
            attached,
            party,
            inventory,
            ..
        } = self;

        let mem: Option<(&dyn MemoryAccess, usize)> = attached.as_ref().and_then(|a| {
            a.dos_base
                .map(|base| (&a.process.memory as &dyn MemoryAccess, base))
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            gui::party_panel::show(ui, party, mem);
            ui.separator();
            ui.columns(2, |cols| {
                gui::inventory_panel::show(&mut cols[0], inventory, mem);
                gui::actions_panel::show(&mut cols[1], party, inventory, mem);
            });
        });
    }
}
