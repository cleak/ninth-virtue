use crate::game::inventory::{Inventory, REAGENT_NAMES, write_inventory};
use crate::memory::access::MemoryAccess;

pub fn show_resources(
    ui: &mut egui::Ui,
    inventory: &mut Inventory,
    mem: Option<(&dyn MemoryAccess, usize)>,
) {
    let mut changed = false;

    ui.heading("🎒 Resources");
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
                .add(egui::DragValue::new(&mut inventory.keys).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("💎 Gems:");
            if ui
                .add(egui::DragValue::new(&mut inventory.gems).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("🔥 Torches:");
            if ui
                .add(egui::DragValue::new(&mut inventory.torches).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("🏹 Arrows:");
            if ui
                .add(egui::DragValue::new(&mut inventory.arrows).range(0..=255))
                .changed()
            {
                changed = true;
            }
            ui.end_row();

            ui.label("☯️ Karma:");
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

    ui.heading("🧪 Reagents");
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
    }
}
