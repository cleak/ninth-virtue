use egui_extras::{Column, TableBuilder};

use crate::game::character::{
    Character, MAX_MP, PartyLocks, Status, apply_party_locks, write_character,
};
use crate::game::map::MapState;
use crate::game::offsets::FRIGATE_MAX_HULL;
use crate::game::vehicle::{Frigate, is_frigate_tile, write_frigate_hull, write_frigate_skiffs};
use crate::memory::access::MemoryAccess;

const NAME_COL_WIDTH: f32 = 70.0;
const CLASS_COL_WIDTH: f32 = 55.0;
const STATUS_COL_WIDTH: f32 = 85.0;
const STAT_COL_WIDTH: f32 = 40.0;
const MP_COL_WIDTH: f32 = 40.0;
const HP_COL_WIDTH: f32 = 45.0;
const MAX_HP_COL_WIDTH: f32 = 50.0;
const XP_COL_WIDTH: f32 = 50.0;
const LVL_COL_WIDTH: f32 = 35.0;
const PARTY_COLUMNS: [(&str, f32); 11] = [
    ("Name", NAME_COL_WIDTH),
    ("Class", CLASS_COL_WIDTH),
    ("Status", STATUS_COL_WIDTH),
    ("STR", STAT_COL_WIDTH),
    ("DEX", STAT_COL_WIDTH),
    ("INT", STAT_COL_WIDTH),
    ("MP", MP_COL_WIDTH),
    ("HP", HP_COL_WIDTH),
    ("MaxHP", MAX_HP_COL_WIDTH),
    ("XP", XP_COL_WIDTH),
    ("Lvl", LVL_COL_WIDTH),
];
const MP_COLUMN_INDEX: usize = 6;
const HP_COLUMN_INDEX: usize = 7;

fn party_columns(table: TableBuilder<'_>) -> TableBuilder<'_> {
    PARTY_COLUMNS.iter().fold(table, |table, (_, width)| {
        table.column(Column::exact(*width))
    })
}

fn column_width(index: usize) -> f32 {
    PARTY_COLUMNS[index].1
}

fn column_span_width(start: usize, end: usize, gap: f32) -> f32 {
    let widths = PARTY_COLUMNS[start..end]
        .iter()
        .map(|(_, width)| *width)
        .sum::<f32>();
    let gaps = (end - start).saturating_sub(1) as f32 * gap;
    widths + gaps
}

/// Find the frigate the party is currently aboard, if any.
fn current_ship<'a>(
    frigates: &'a mut [Frigate],
    map: Option<&MapState>,
) -> Option<&'a mut Frigate> {
    let map = map?;
    if !is_frigate_tile(map.transport) {
        return None;
    }
    frigates.iter_mut().find(|f| f.x == map.x && f.y == map.y)
}

/// Returns `true` if any character data was written to game memory.
pub fn show(
    ui: &mut egui::Ui,
    party: &mut [Character],
    locks: &mut PartyLocks,
    frigates: &mut [Frigate],
    map: Option<&MapState>,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut wrote = false;
    let has_party = !party.is_empty();
    let lock_row_height = ui.spacing().interact_size.y;
    let column_gap = ui.spacing().item_spacing.x;
    // Keep the title span and trailing spacer derived from the same column
    // widths as the party table so the MP/HP lock cells stay aligned.
    let lead_width = column_span_width(0, MP_COLUMN_INDEX, column_gap);
    let trailing_width = column_span_width(HP_COLUMN_INDEX + 1, PARTY_COLUMNS.len(), column_gap);

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(lead_width, lock_row_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(egui::RichText::new("\u{2694}").heading());
                ui.label(
                    egui::RichText::new("Party")
                        .heading()
                        .color(egui::Color32::from_rgb(100, 180, 255)),
                );
            },
        );

        ui.allocate_ui_with_layout(
            egui::vec2(column_width(MP_COLUMN_INDEX), lock_row_height),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                if has_party {
                    crate::gui::infinity_checkbox(
                        ui,
                        &mut locks.mana,
                        "Lock mana at 99 for the whole party.",
                    );
                }
            },
        );

        ui.allocate_ui_with_layout(
            egui::vec2(column_width(HP_COLUMN_INDEX), lock_row_height),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                if has_party {
                    crate::gui::infinity_checkbox(
                        ui,
                        &mut locks.health,
                        "Lock health at max and force Good status for the whole party.",
                    );
                }
            },
        );

        ui.allocate_ui(egui::vec2(trailing_width, lock_row_height), |_| {});
    });

    if !has_party {
        ui.label("No party data loaded.");
        return false;
    }

    party_columns(TableBuilder::new(ui))
        .striped(true)
        .header(20.0, |mut header| {
            let hdr = egui::Color32::from_rgb(140, 180, 255);
            for &(label, _) in &PARTY_COLUMNS {
                header.col(|ui| {
                    ui.colored_label(hdr, label);
                });
            }
        })
        .body(|body| {
            body.rows(22.0, party.len(), |mut row| {
                let i = row.index();
                let mut ch = party[i].clone();
                let mut changed = false;

                row.col(|ui| {
                    let color = match ch.status {
                        Status::Dead => egui::Color32::from_rgb(255, 100, 100),
                        Status::Poisoned => egui::Color32::from_rgb(255, 255, 100),
                        Status::Asleep => egui::Color32::from_rgb(180, 180, 180),
                        Status::Good => ui.visuals().text_color(),
                    };
                    ui.colored_label(color, &ch.name);
                });

                row.col(|ui| {
                    ui.label(ch.class.label());
                });

                row.col(|ui| {
                    ui.add_enabled_ui(!locks.health, |ui| {
                        egui::ComboBox::from_id_salt(format!("status_{i}"))
                            .selected_text(ch.status.label())
                            .show_ui(ui, |ui| {
                                for &s in Status::ALL {
                                    if ui.selectable_value(&mut ch.status, s, s.label()).changed() {
                                        changed = true;
                                    }
                                }
                            });
                    });
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.str_).range(1..=99))
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.dex).range(1..=99))
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.int).range(1..=99))
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add_enabled(
                            !locks.mana,
                            egui::DragValue::new(&mut ch.mp).range(0..=MAX_MP),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add_enabled(
                            !locks.health,
                            egui::DragValue::new(&mut ch.hp).range(0..=999),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.max_hp).range(1..=999))
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.xp).range(0..=9999))
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.level).range(1..=8))
                        .changed()
                    {
                        changed = true;
                    }
                });

                if mem.is_some() {
                    changed |= apply_party_locks(&mut ch, locks);
                }

                if changed {
                    party[i] = ch;
                    if let Some((mem, dos_base)) = mem {
                        let _ = write_character(mem, dos_base, &party[i]);
                        wrote = true;
                    }
                }
            });
        });

    // Show current ship stats when the party is aboard a frigate
    if let Some(ship) = current_ship(frigates, map) {
        let ship_color = egui::Color32::from_rgb(120, 200, 220);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                egui::RichText::new(format!("\u{26F5} {} Hull:", ship.label())).color(ship_color),
            );
            if ui
                .add(egui::DragValue::new(&mut ship.hull).range(0..=FRIGATE_MAX_HULL))
                .changed()
                && let Some((mem, dos_base)) = mem
            {
                let _ = write_frigate_hull(mem, dos_base, ship);
                wrote = true;
            }
            ui.label(format!("/ {FRIGATE_MAX_HULL}"));

            if !ship.is_pirate() {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Skiffs:").color(ship_color));
                if ui
                    .add(egui::DragValue::new(&mut ship.skiffs).range(0..=u8::MAX))
                    .changed()
                    && let Some((mem, dos_base)) = mem
                {
                    let _ = write_frigate_skiffs(mem, dos_base, ship);
                    wrote = true;
                }
            }
        });
    }

    wrote
}
