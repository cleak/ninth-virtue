use crate::controller::GameController;
use crate::game::character::{Character, Status};
use crate::game::inventory::Inventory;
use crate::game::offsets::FRIGATE_MAX_HULL;
use crate::game::save_state::{NUM_SLOTS, SlotInfo};
use crate::game::vehicle::Frigate;

/// Request from the actions panel that requires mutable controller access.
pub enum SaveAction {
    None,
    Save(usize),
    Load(usize),
}

const HEADING_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 220, 180);
const HEAL_FILL: egui::Color32 = egui::Color32::from_rgb(35, 100, 55);
const INV_FILL: egui::Color32 = egui::Color32::from_rgb(110, 85, 35);
const SAVE_FILL: egui::Color32 = egui::Color32::from_rgb(35, 70, 110);
const BTN_TEXT: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);

/// Returns (wrote_memory, save_action).
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    party: &mut [Character],
    inventory: &mut Inventory,
    frigates: &mut [Frigate],
    ctrl: &GameController,
    selected_slot: &mut usize,
    save_slots: &mut [Option<SlotInfo>],
    _status_msg: &mut String,
) -> (bool, SaveAction) {
    let mut wrote = false;
    let mut action = SaveAction::None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("⚡").heading());
        ui.label(
            egui::RichText::new("Quick Actions")
                .heading()
                .color(HEADING_COLOR),
        );
    });

    let enabled = ctrl.is_ready();
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
            let _ = ctrl.write_character(ch);
            wrote = true;
        }
        for f in frigates.iter_mut() {
            f.hull = FRIGATE_MAX_HULL;
            let _ = ctrl.write_frigate_hull(f);
            wrote = true;
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
                let _ = ctrl.write_character(ch);
                wrote = true;
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
                let _ = ctrl.write_character(ch);
                wrote = true;
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
        let _ = ctrl.write_inventory(inventory);
        wrote = true;
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
        let _ = ctrl.write_inventory(inventory);
        wrote = true;
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
        inventory.arrows = 99;
        let _ = ctrl.write_inventory(inventory);
        wrote = true;
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
        let _ = ctrl.write_inventory(inventory);
        wrote = true;
    }

    // ----- Save States -----
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("💾").heading());
        ui.label(
            egui::RichText::new("Save States")
                .heading()
                .color(HEADING_COLOR),
        );
    });

    let save_enabled = ctrl.is_ready() && ctrl.game_dir.is_some();

    egui::ComboBox::from_id_salt("save_slot")
        .width(ui.available_width() - 8.0)
        .selected_text(format!(
            "Slot {} — {}",
            *selected_slot + 1,
            slot_label(save_slots, *selected_slot),
        ))
        .show_ui(ui, |ui| {
            for i in 0..NUM_SLOTS {
                ui.selectable_value(
                    selected_slot,
                    i,
                    format!("Slot {} — {}", i + 1, slot_label(save_slots, i)),
                );
            }
        });

    if ui
        .add_enabled(
            save_enabled && !ctrl.is_busy(),
            egui::Button::new(egui::RichText::new("💾 Save").color(BTN_TEXT))
                .fill(SAVE_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        action = SaveAction::Save(*selected_slot);
    }

    let load_enabled = save_enabled && !ctrl.is_busy() && save_slots[*selected_slot].is_some();
    if ui
        .add_enabled(
            load_enabled,
            egui::Button::new(egui::RichText::new("📂 Load").color(BTN_TEXT))
                .fill(SAVE_FILL)
                .min_size(button_size),
        )
        .clicked()
    {
        action = SaveAction::Load(*selected_slot);
    }

    if let Some(info) = &save_slots[*selected_slot] {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(format!(
                "{} — {} — {}",
                info.leader_name,
                info.location,
                format_timestamp(info.timestamp),
            ))
            .small()
            .color(egui::Color32::from_rgb(160, 160, 160)),
        );
    }

    (wrote, action)
}

fn slot_label(slots: &[Option<SlotInfo>], index: usize) -> &str {
    match slots.get(index) {
        Some(Some(info)) => &info.location,
        _ => "Empty",
    }
}

fn format_timestamp(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let ago = now - ts;
    if ago < 60 {
        "just now".to_string()
    } else if ago < 3600 {
        format!("{}m ago", ago / 60)
    } else if ago < 86400 {
        format!("{}h ago", ago / 3600)
    } else {
        format!("{}d ago", ago / 86400)
    }
}
