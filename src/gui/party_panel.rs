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

    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        // `ui.horizontal` inserts one gap between these cells, so the lead
        // width only needs the first five table gaps to land the next cell on
        // the MP column start.
        let lead_width = NAME_COL_WIDTH
            + CLASS_COL_WIDTH
            + STATUS_COL_WIDTH
            + (STAT_COL_WIDTH * 3.0)
            + (gap * 5.0);

        ui.allocate_ui_with_layout(
            egui::vec2(lead_width, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(egui::RichText::new("⚔").heading());
                ui.label(
                    egui::RichText::new("Party")
                        .heading()
                        .color(egui::Color32::from_rgb(100, 180, 255)),
                );
            },
        );

        ui.allocate_ui_with_layout(
            egui::vec2(MP_COL_WIDTH, ui.spacing().interact_size.y),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                crate::gui::infinity_checkbox(
                    ui,
                    &mut locks.mana,
                    "Lock mana at 99 for the whole party.",
                );
            },
        );

        ui.allocate_ui_with_layout(
            egui::vec2(HP_COL_WIDTH, ui.spacing().interact_size.y),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| {
                crate::gui::infinity_checkbox(
                    ui,
                    &mut locks.health,
                    "Lock health at max and force Good status for the whole party.",
                );
            },
        );

        let trailing_width = MAX_HP_COL_WIDTH + XP_COL_WIDTH + LVL_COL_WIDTH + (gap * 2.0);
        ui.allocate_ui(
            egui::vec2(trailing_width, ui.spacing().interact_size.y),
            |_| {},
        );
    });

    if party.is_empty() {
        ui.label("No party data loaded.");
        return false;
    }

    TableBuilder::new(ui)
        .column(Column::exact(NAME_COL_WIDTH)) // Name
        .column(Column::exact(CLASS_COL_WIDTH)) // Class
        .column(Column::exact(STATUS_COL_WIDTH)) // Status
        .column(Column::exact(STAT_COL_WIDTH)) // STR
        .column(Column::exact(STAT_COL_WIDTH)) // DEX
        .column(Column::exact(STAT_COL_WIDTH)) // INT
        .column(Column::exact(MP_COL_WIDTH)) // MP
        .column(Column::exact(HP_COL_WIDTH)) // HP
        .column(Column::exact(MAX_HP_COL_WIDTH)) // MaxHP
        .column(Column::exact(XP_COL_WIDTH)) // XP
        .column(Column::exact(LVL_COL_WIDTH)) // Lvl
        .striped(true)
        .header(20.0, |mut header| {
            let hdr = egui::Color32::from_rgb(140, 180, 255);
            header.col(|ui| {
                ui.colored_label(hdr, "Name");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "Class");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "Status");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "STR");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "DEX");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "INT");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "MP");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "HP");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "MaxHP");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "XP");
            });
            header.col(|ui| {
                ui.colored_label(hdr, "Lvl");
            });
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
            ui.label(egui::RichText::new(format!("⛵ {} Hull:", ship.label())).color(ship_color));
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
