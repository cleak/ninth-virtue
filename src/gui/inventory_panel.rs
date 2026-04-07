use crate::game::inventory::{Inventory, REAGENT_NAMES, write_inventory};
use crate::memory::access::MemoryAccess;

const RESOURCE_HEADING: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const REAGENT_HEADING: egui::Color32 = egui::Color32::from_rgb(180, 140, 255);

fn colored_heading(ui: &mut egui::Ui, emoji: &str, text: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new(emoji).heading());
        ui.label(egui::RichText::new(text).heading().color(color));
    });
}

/// Returns `true` if inventory was written to game memory.
pub fn show_resources(
    ui: &mut egui::Ui,
    inventory: &mut Inventory,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut changed = false;

    colored_heading(ui, "🎒", "Resources", RESOURCE_HEADING);
    egui::Grid::new("inv_resources")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("🍖 Food:");
            if ui
                .add(egui::DragValue::new(&mut inventory.food).range(0..=9999))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("💰 Gold:");
            if ui
                .add(egui::DragValue::new(&mut inventory.gold).range(0..=9999))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("🔑 Keys:");
            if ui
                .add(egui::DragValue::new(&mut inventory.keys).range(0..=99))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("💎 Gems:");
            if ui
                .add(egui::DragValue::new(&mut inventory.gems).range(0..=99))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("🔥 Torches:");
            if ui
                .add(egui::DragValue::new(&mut inventory.torches).range(0..=99))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("🏹 Arrows:");
            if ui
                .add(egui::DragValue::new(&mut inventory.arrows).range(0..=99))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("☯ Karma:");
            if ui
                .add(egui::DragValue::new(&mut inventory.karma).range(0..=99))
                .changed()
            {
                changed = true;
            }
            ui.end_row();
        });

    if changed && let Some((mem, dos_base)) = mem {
        let _ = write_inventory(mem, dos_base, inventory);
        return true;
    }
    false
}

/// Returns `true` if inventory was written to game memory.
pub fn show_reagents(
    ui: &mut egui::Ui,
    inventory: &mut Inventory,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut changed = false;

    colored_heading(ui, "🧪", "Reagents", REAGENT_HEADING);
    egui::Grid::new("inv_reagents")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (i, name) in REAGENT_NAMES.iter().enumerate() {
                ui.label(*name);
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
        return true;
    }
    false
}
