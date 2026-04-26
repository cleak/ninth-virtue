use crate::game::map::{MapState, write_position};
use crate::memory::access::MemoryAccess;

const LOCATION_HEADING: egui::Color32 = egui::Color32::from_rgb(120, 200, 255);

/// Render the location tracker / position editor.
///
/// Shows the current scene name plus dungeon facing when applicable, and
/// exposes X / Y / Z DragValues that write back through [`write_position`].
/// Returns `true` when the player position was written to game memory.
pub fn show(
    ui: &mut egui::Ui,
    map: Option<&mut MapState>,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("📍").heading());
        ui.label(
            egui::RichText::new("Location")
                .heading()
                .color(LOCATION_HEADING),
        );
    });

    let Some(map) = map else {
        ui.label(egui::RichText::new("(no map loaded)").italics());
        return false;
    };

    let mut changed = false;

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        ui.label("Place:");
        ui.label(egui::RichText::new(map.display_location_name()).strong());

        if let Some(facing) = map.dungeon_facing {
            ui.separator();
            ui.label("Facing:");
            ui.label(egui::RichText::new(facing.name()).strong());
        }

        ui.separator();
        ui.label("X:");
        if ui
            .add(egui::DragValue::new(&mut map.x).range(0..=u8::MAX))
            .on_hover_text("In-scene X coordinate (0-255).")
            .changed()
        {
            changed = true;
        }
        ui.label("Y:");
        if ui
            .add(egui::DragValue::new(&mut map.y).range(0..=u8::MAX))
            .on_hover_text("In-scene Y coordinate (0-255).")
            .changed()
        {
            changed = true;
        }
        ui.label("Z:");
        if ui
            .add(egui::DragValue::new(&mut map.z).range(0..=u8::MAX))
            .on_hover_text(
                "Z byte (0-255): dungeon floor in dungeons; outdoors, values above 0x7F are the Underworld.",
            )
            .changed()
        {
            changed = true;
        }
    });

    if changed && let Some((mem, dos_base)) = mem {
        let _ = write_position(mem, dos_base, map.x, map.y, map.z);
        return true;
    }

    false
}
