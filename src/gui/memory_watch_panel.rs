use std::time::Instant;

use crate::game::offsets::{self, SAVE_BASE};
use crate::memory::access::MemoryAccess;

/// A single observed byte change.
pub struct MemoryChange {
    pub time_ms: u128,
    pub offset: usize, // save-relative
    pub old_val: u8,
    pub new_val: u8,
}

/// Persistent state for the memory-watch debug window.
pub struct MemoryWatch {
    pub visible: bool,
    pub recording: bool,
    pub region_offset: usize,
    pub region_size: usize,
    prev_snapshot: Vec<u8>,
    has_snapshot: bool,
    pub log: Vec<MemoryChange>,
    recording_start: Instant,
}

impl Default for MemoryWatch {
    fn default() -> Self {
        Self {
            visible: false,
            recording: false,
            region_offset: 0x000,
            region_size: 0x3C0,
            prev_snapshot: Vec::new(),
            has_snapshot: false,
            log: Vec::new(),
            recording_start: Instant::now(),
        }
    }
}

impl MemoryWatch {
    /// Poll the watched region, diff against the previous snapshot, and
    /// append any changes to the log.  Called every frame while recording.
    pub fn poll(&mut self, mem: &dyn MemoryAccess, dos_base: usize) {
        let start_addr = dos_base + SAVE_BASE + self.region_offset;
        let mut buf = vec![0u8; self.region_size];
        if mem.read_bytes(start_addr, &mut buf).is_err() {
            return;
        }

        if self.has_snapshot && self.prev_snapshot.len() == buf.len() {
            let now = self.recording_start.elapsed().as_millis();
            for (i, (old, new)) in self.prev_snapshot.iter().zip(buf.iter()).enumerate() {
                if old != new {
                    self.log.push(MemoryChange {
                        time_ms: now,
                        offset: self.region_offset + i,
                        old_val: *old,
                        new_val: *new,
                    });
                }
            }
        }

        self.prev_snapshot = buf;
        self.has_snapshot = true;
    }

    /// Reset recording state for a fresh session.
    pub fn start_recording(&mut self) {
        self.recording = true;
        self.has_snapshot = false;
        self.prev_snapshot.clear();
        self.log.clear();
        self.recording_start = Instant::now();
    }

    pub fn stop_recording(&mut self) {
        self.recording = false;
    }
}

// --- UI ---

const HEADING_COLOR: egui::Color32 = egui::Color32::from_rgb(180, 140, 220);
const KNOWN_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 180, 255);
const UNKNOWN_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);

pub fn show(ctx: &egui::Context, watch: &mut MemoryWatch, mem: Option<(&dyn MemoryAccess, usize)>) {
    if !watch.visible {
        return;
    }

    let mut open = watch.visible;
    egui::Window::new("Memory Watch")
        .open(&mut open)
        .default_width(520.0)
        .default_height(400.0)
        .show(ctx, |ui| {
            // Controls row
            ui.horizontal(|ui| {
                if watch.recording {
                    if ui.button("Stop").clicked() {
                        watch.stop_recording();
                    }
                    ui.colored_label(egui::Color32::from_rgb(255, 80, 80), "Recording...");
                } else if ui.button("Start").clicked() && mem.is_some() {
                    watch.start_recording();
                }

                if ui.button("Clear").clicked() {
                    watch.log.clear();
                }

                ui.separator();

                ui.label("Offset:");
                let mut offset_val = watch.region_offset as u32;
                if ui
                    .add(
                        egui::DragValue::new(&mut offset_val)
                            .range(0..=0x1060_u32)
                            .hexadecimal(4, false, true)
                            .speed(1),
                    )
                    .changed()
                {
                    watch.region_offset = offset_val as usize;
                }

                ui.label("Size:");
                let mut size_val = watch.region_size as u32;
                if ui
                    .add(
                        egui::DragValue::new(&mut size_val)
                            .range(1..=0x1060_u32)
                            .hexadecimal(4, false, true)
                            .speed(1),
                    )
                    .changed()
                {
                    watch.region_size = size_val as usize;
                }
            });

            ui.separator();

            // Stats
            ui.horizontal(|ui| {
                ui.label(format!("{} changes logged", watch.log.len()));
                if let Some(last) = watch.log.last() {
                    ui.separator();
                    ui.label(format!("latest: {}ms", last.time_ms));
                }
            });

            ui.separator();

            // Log table (most recent first)
            let row_height = 18.0;
            let num_rows = watch.log.len();

            egui_extras::TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(egui_extras::Column::exact(60.0)) // time
                .column(egui_extras::Column::exact(60.0)) // offset
                .column(egui_extras::Column::exact(100.0)) // label
                .column(egui_extras::Column::exact(60.0)) // old
                .column(egui_extras::Column::exact(60.0)) // new
                .header(row_height, |mut header| {
                    header.col(|ui| {
                        ui.strong("Time ms");
                    });
                    header.col(|ui| {
                        ui.strong("Offset");
                    });
                    header.col(|ui| {
                        ui.strong("Label");
                    });
                    header.col(|ui| {
                        ui.strong("Old");
                    });
                    header.col(|ui| {
                        ui.strong("New");
                    });
                })
                .body(|body| {
                    body.rows(row_height, num_rows, |mut row| {
                        // Show newest first
                        let idx = num_rows - 1 - row.index();
                        let change = &watch.log[idx];

                        row.col(|ui| {
                            ui.label(format!("{}", change.time_ms));
                        });
                        row.col(|ui| {
                            ui.label(format!("{:#05X}", change.offset));
                        });
                        row.col(|ui| {
                            if let Some(lbl) = offsets::label_for_save_offset(change.offset) {
                                ui.colored_label(KNOWN_COLOR, lbl);
                            } else {
                                ui.colored_label(UNKNOWN_COLOR, "???");
                            }
                        });
                        row.col(|ui| {
                            ui.label(format!("{:#04X} ({})", change.old_val, change.old_val));
                        });
                        row.col(|ui| {
                            ui.label(format!("{:#04X} ({})", change.new_val, change.new_val));
                        });
                    });
                });

            // Tip
            if watch.log.is_empty() && !watch.recording {
                ui.add_space(8.0);
                ui.colored_label(
                    HEADING_COLOR,
                    "Tip: Downclock DOSBox (Ctrl+F11) before recording \
                     to catch transient flags.",
                );
            }
        });
    watch.visible = open;
}
