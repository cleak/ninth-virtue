use egui_extras::{Column, TableBuilder};

use crate::game::character::{Character, Status};
use crate::memory::access::MemoryAccess;

pub fn show(ui: &mut egui::Ui, party: &mut [Character], mem: Option<(&dyn MemoryAccess, usize)>) {
    if party.is_empty() {
        ui.label("No party data loaded.");
        return;
    }

    ui.heading("Party");

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
            header.col(|ui| {
                ui.strong("Name");
            });
            header.col(|ui| {
                ui.strong("Class");
            });
            header.col(|ui| {
                ui.strong("Status");
            });
            header.col(|ui| {
                ui.strong("STR");
            });
            header.col(|ui| {
                ui.strong("DEX");
            });
            header.col(|ui| {
                ui.strong("INT");
            });
            header.col(|ui| {
                ui.strong("MP");
            });
            header.col(|ui| {
                ui.strong("HP");
            });
            header.col(|ui| {
                ui.strong("MaxHP");
            });
            header.col(|ui| {
                ui.strong("XP");
            });
            header.col(|ui| {
                ui.strong("Lvl");
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
                        .add(egui::DragValue::new(&mut ch.mp).range(0..=255))
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
                    }
                }
            });
        });
}
