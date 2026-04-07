use crate::game::inventory::{Inventory, REAGENT_NAMES, write_inventory};
use crate::memory::access::MemoryAccess;

const RESOURCE_HEADING: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const REAGENT_HEADING: egui::Color32 = egui::Color32::from_rgb(180, 140, 255);
const LABEL_COLOR: egui::Color32 = egui::Color32::from_rgb(180, 190, 210);

pub fn show_resources(
    ui: &mut egui::Ui,
    inventory: &mut Inventory,
    mem: Option<(&dyn MemoryAccess, usize)>,
) {
    let mut changed = false;

    ui.label(
        egui::RichText::new("Resources")
            .heading()
            .color(RESOURCE_HEADING),
    );
    egui::Grid::new("inv_resources")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Food:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.food).range(0..=9999))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label(egui::RichText::new("Gold:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.gold).range(0..=9999))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label(egui::RichText::new("Keys:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.keys).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label(egui::RichText::new("Gems:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.gems).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label(egui::RichText::new("Torches:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.torches).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label(egui::RichText::new("Arrows:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.arrows).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label(egui::RichText::new("Karma:").color(LABEL_COLOR));
            if ui
                .add(egui::DragValue::new(&mut inventory.karma).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();
        });

    if changed && let Some((mem, dos_base)) = mem {
        let _ = write_inventory(mem, dos_base, inventory);
    }
}

pub fn show_reagents(
    ui: &mut egui::Ui,
    inventory: &mut Inventory,
    mem: Option<(&dyn MemoryAccess, usize)>,
) {
    let mut changed = false;

    ui.label(
        egui::RichText::new("Reagents")
            .heading()
            .color(REAGENT_HEADING),
    );
    egui::Grid::new("inv_reagents")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (i, name) in REAGENT_NAMES.iter().enumerate() {
                ui.label(egui::RichText::new(*name).color(LABEL_COLOR));
                if ui
                    .add(egui::DragValue::new(&mut inventory.reagents[i]).range(0..=99))
                    .changed()
                {
                    changed = true;
                }
                ui.end_row();
            }
        });

    if changed && let Some((mem, dos_base)) = mem {
        let _ = write_inventory(mem, dos_base, inventory);
    }
}
