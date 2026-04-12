use crate::game::inventory::{
    Inventory, InventoryLocks, MAX_CONSUMABLE_COUNT, MAX_FOOD, MAX_GOLD, REAGENT_NAMES,
    apply_reagent_locks, apply_resource_locks, write_inventory,
};
use crate::memory::access::MemoryAccess;

const RESOURCE_HEADING: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const REAGENT_HEADING: egui::Color32 = egui::Color32::from_rgb(180, 140, 255);
const RESOURCE_VALUE_WIDTH: f32 = 60.0;
const REAGENT_VALUE_WIDTH: f32 = 48.0;

fn colored_heading(ui: &mut egui::Ui, emoji: &str, text: &str, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new(emoji).heading());
        ui.label(egui::RichText::new(text).heading().color(color));
    });
}

fn value_cell(
    ui: &mut egui::Ui,
    width: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        add_contents,
    )
    .inner
}

/// Returns `true` if inventory was written to game memory.
pub fn show_resources(
    ui: &mut egui::Ui,
    inventory: &mut Inventory,
    locks: &mut InventoryLocks,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut changed = false;

    colored_heading(ui, "🎒", "Resources", RESOURCE_HEADING);
    egui::Grid::new("inv_resources")
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("🍖 Food:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add_enabled(
                    !locks.food,
                    egui::DragValue::new(&mut inventory.food).range(0..=MAX_FOOD),
                )
            })
            .changed()
            {
                changed = true;
            }
            crate::gui::infinity_checkbox(ui, &mut locks.food, "Lock food at 9999.");
            ui.end_row();

            ui.label("💰 Gold:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add_enabled(
                    !locks.gold,
                    egui::DragValue::new(&mut inventory.gold).range(0..=MAX_GOLD),
                )
            })
            .changed()
            {
                changed = true;
            }
            crate::gui::infinity_checkbox(ui, &mut locks.gold, "Lock gold at 9999.");
            ui.end_row();

            ui.label("🔑 Keys:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add_enabled(
                    !locks.keys,
                    egui::DragValue::new(&mut inventory.keys).range(0..=MAX_CONSUMABLE_COUNT),
                )
            })
            .changed()
            {
                changed = true;
            }
            crate::gui::infinity_checkbox(ui, &mut locks.keys, "Lock keys at 99.");
            ui.end_row();

            ui.label("💎 Gems:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add_enabled(
                    !locks.gems,
                    egui::DragValue::new(&mut inventory.gems).range(0..=MAX_CONSUMABLE_COUNT),
                )
            })
            .changed()
            {
                changed = true;
            }
            crate::gui::infinity_checkbox(ui, &mut locks.gems, "Lock gems at 99.");
            ui.end_row();

            ui.label("🔥 Torches:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add_enabled(
                    !locks.torches,
                    egui::DragValue::new(&mut inventory.torches).range(0..=MAX_CONSUMABLE_COUNT),
                )
            })
            .changed()
            {
                changed = true;
            }
            crate::gui::infinity_checkbox(ui, &mut locks.torches, "Lock torches at 99.");
            ui.end_row();

            ui.label("🏹 Arrows:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add_enabled(
                    !locks.arrows,
                    egui::DragValue::new(&mut inventory.arrows).range(0..=MAX_CONSUMABLE_COUNT),
                )
            })
            .changed()
            {
                changed = true;
            }
            crate::gui::infinity_checkbox(ui, &mut locks.arrows, "Lock arrows at 99.");
            ui.end_row();

            ui.label("☯ Karma:");
            if value_cell(ui, RESOURCE_VALUE_WIDTH, |ui| {
                ui.add(egui::DragValue::new(&mut inventory.karma).range(0..=99))
            })
            .changed()
            {
                changed = true;
            }
            ui.label("");
            ui.end_row();
        });

    if mem.is_some() && apply_resource_locks(inventory, locks) {
        changed = true;
    }

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
    locks: &mut InventoryLocks,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut changed = false;

    colored_heading(ui, "🧪", "Reagents", REAGENT_HEADING);
    egui::Grid::new("inv_reagents")
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (i, name) in REAGENT_NAMES.iter().enumerate() {
                ui.label(*name);
                if value_cell(ui, REAGENT_VALUE_WIDTH, |ui| {
                    ui.add_enabled(
                        !locks.reagents[i],
                        egui::DragValue::new(&mut inventory.reagents[i])
                            .range(0..=MAX_CONSUMABLE_COUNT),
                    )
                })
                .changed()
                {
                    changed = true;
                }
                crate::gui::infinity_checkbox(ui, &mut locks.reagents[i], "Lock reagent at 99.");
                ui.end_row();
            }
        });

    if mem.is_some() && apply_reagent_locks(inventory, locks) {
        changed = true;
    }

    if changed && let Some((mem, dos_base)) = mem {
        let _ = write_inventory(mem, dos_base, inventory);
        return true;
    }
    false
}
