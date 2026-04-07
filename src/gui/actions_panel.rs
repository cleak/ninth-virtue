use crate::game::character::{Character, Status, write_character};
use crate::game::inventory::{Inventory, write_inventory};
use crate::memory::access::MemoryAccess;

pub fn show(
    ui: &mut egui::Ui,
    party: &mut [Character],
    inventory: &mut Inventory,
    mem: Option<(&dyn MemoryAccess, usize)>,
) {
    ui.heading("⚡ Quick Actions");

    let enabled = mem.is_some();
    let button_size = egui::vec2(ui.available_width(), 24.0);

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("💚 Heal All").min_size(button_size),
        )
        .clicked()
    {
        for ch in party.iter_mut() {
            ch.hp = ch.max_hp;
            ch.status = Status::Good;
            if let Some((mem, base)) = mem {
                let _ = write_character(mem, base, ch);
            }
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("🩹 Cure Poison").min_size(button_size),
        )
        .clicked()
    {
        for ch in party.iter_mut() {
            if ch.status == Status::Poisoned {
                ch.status = Status::Good;
                if let Some((mem, base)) = mem {
                    let _ = write_character(mem, base, ch);
                }
            }
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("✨ Resurrect All").min_size(button_size),
        )
        .clicked()
    {
        for ch in party.iter_mut() {
            if ch.status == Status::Dead {
                ch.status = Status::Good;
                ch.hp = ch.max_hp;
                if let Some((mem, base)) = mem {
                    let _ = write_character(mem, base, ch);
                }
            }
        }
    }

    ui.add_space(4.0);

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("💰 Max Gold").min_size(button_size),
        )
        .clicked()
    {
        inventory.gold = 9999;
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("🍖 Max Food").min_size(button_size),
        )
        .clicked()
    {
        inventory.food = 9999;
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("🏹 Refill Arrows").min_size(button_size),
        )
        .clicked()
    {
        inventory.arrows = 255;
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
        }
    }

    if ui
        .add_enabled(
            enabled,
            egui::Button::new("🧪 Max Reagents").min_size(button_size),
        )
        .clicked()
    {
        inventory.reagents = [99; 8];
        if let Some((mem, base)) = mem {
            let _ = write_inventory(mem, base, inventory);
        }
    }
}
