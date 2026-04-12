use crate::game::character::{Character, Status, write_character};
use crate::game::inventory::{Inventory, MAX_CONSUMABLE_COUNT, write_inventory};
use crate::game::offsets::FRIGATE_MAX_HULL;
use crate::game::vehicle::{Frigate, write_frigate_hull};
use crate::memory::access::MemoryAccess;

const HEADING_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 220, 180);
const HEAL_FILL: egui::Color32 = egui::Color32::from_rgb(35, 100, 55);
const INV_FILL: egui::Color32 = egui::Color32::from_rgb(110, 85, 35);
const BTN_TEXT: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);

/// Returns `true` if any game memory was written.
pub fn show(
    ui: &mut egui::Ui,
    party: &mut [Character],
    inventory: &mut Inventory,
    frigates: &mut [Frigate],
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut wrote = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("⚡").heading());
        ui.label(
            egui::RichText::new("Quick Actions")
                .heading()
                .color(HEADING_COLOR),
        );
    });

    let enabled = mem.is_some();
    let button_size = egui::vec2(ui.available_width(), 24.0);

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("💚 Heal All").color(BTN_TEXT))
                .fill(HEAL_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        for ch in party.iter_mut() {
            ch.hp = ch.max_hp;
            ch.status = Status::Good;
            if let Some((mem, base)) = mem {
                let _ = write_character(mem, base, ch);
                wrote = true;
            }
        }
        for f in frigates.iter_mut() {
            f.hull = FRIGATE_MAX_HULL;
            if let Some((mem, base)) = mem {
                let _ = write_frigate_hull(mem, base, f);
                wrote = true;
            }
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("🩹 Cure Poison").color(BTN_TEXT))
                .fill(HEAL_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        for ch in party.iter_mut() {
            if ch.status == Status::Poisoned {
                ch.status = Status::Good;
                if let Some((mem, base)) = mem {
                    let _ = write_character(mem, base, ch);
                    wrote = true;
                }
            }
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("✨ Resurrect All").color(BTN_TEXT))
                .fill(HEAL_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        for ch in party.iter_mut() {
            if ch.status == Status::Dead {
                ch.status = Status::Good;
                ch.hp = ch.max_hp;
                if let Some((mem, base)) = mem {
                    let _ = write_character(mem, base, ch);
                    wrote = true;
                }
            }
        }
    }

    ui.add_space(4.0);

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("💰 Max Gold").color(BTN_TEXT))
                .fill(INV_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        inventory.gold = 9999;
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
            wrote = true;
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("🍖 Max Food").color(BTN_TEXT))
                .fill(INV_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        inventory.food = 9999;
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
            wrote = true;
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("🏹 Refill Arrows").color(BTN_TEXT))
                .fill(INV_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        inventory.arrows = MAX_CONSUMABLE_COUNT;
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
            wrote = true;
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new("🧪 Max Reagents").color(BTN_TEXT))
                .fill(INV_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        inventory.reagents = [99; 8];
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
            wrote = true;
        }
    }

    wrote
}
