use egui_extras::{Column, TableBuilder};

use crate::game::character::{Character, Status};
use crate::game::map::MapState;
use crate::game::offsets::FRIGATE_MAX_HULL;
use crate::game::vehicle::{Frigate, is_frigate_tile, write_frigate_hull, write_frigate_skiffs};
use crate::memory::access::MemoryAccess;

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
    frigates: &mut [Frigate],
    map: Option<&MapState>,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut wrote = false;
    if party.is_empty() {
        ui.label("No party data loaded.");
        return false;
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("⚔").heading());
        ui.label(
            egui::RichText::new("Party")
                .heading()
                .color(egui::Color32::from_rgb(100, 180, 255)),
        );
    });

    TableBuilder::new(ui)
        .column(Column::auto().at_least(70.0)) // Name
        .column(Column::exact(55.0)) // Class
        .column(Column::auto().at_least(85.0)) // Status
        .column(Column::exact(40.0)) // STR
        .column(Column::exact(40.0)) // DEX
        .column(Column::exact(40.0)) // INT
        .column(Column::exact(40.0)) // MP
        .column(Column::exact(45.0)) // HP
        .column(Column::exact(50.0)) // MaxHP
        .column(Column::exact(50.0)) // XP
        .column(Column::exact(35.0)) // Lvl
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
                        .add(egui::DragValue::new(&mut ch.mp).range(0..=99))
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut ch.hp).range(0..=999))
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

                if changed {
                    party[i] = ch;
                    if let Some((mem, dos_base)) = mem {
                        let _ = crate::game::character::write_character(mem, dos_base, &party[i]);
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
