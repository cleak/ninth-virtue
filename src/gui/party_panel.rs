use egui_extras::{Column, TableBuilder};

use crate::game::character::{
    Character, MAX_MP, PartyLocks, Status, apply_party_locks, write_character,
};
use crate::game::map::MapState;
use crate::game::offsets::FRIGATE_MAX_HULL;
use crate::game::vehicle::{Frigate, is_frigate_tile, write_frigate_hull, write_frigate_skiffs};
use crate::memory::access::MemoryAccess;

const NAME_COL_WIDTH: f32 = 70.0;
const CLASS_COL_WIDTH: f32 = 55.0;
const STATUS_COL_WIDTH: f32 = 85.0;
const STAT_COL_WIDTH: f32 = 40.0;
const MP_COL_WIDTH: f32 = 40.0;
const HP_COL_WIDTH: f32 = 45.0;
const MAX_HP_COL_WIDTH: f32 = 50.0;
const XP_COL_WIDTH: f32 = 50.0;
const LVL_COL_WIDTH: f32 = 35.0;
const PARTY_COLUMNS: [(&str, f32); 11] = [
    ("Name", NAME_COL_WIDTH),
    ("Class", CLASS_COL_WIDTH),
    ("Status", STATUS_COL_WIDTH),
    ("STR", STAT_COL_WIDTH),
    ("DEX", STAT_COL_WIDTH),
    ("INT", STAT_COL_WIDTH),
    ("MP", MP_COL_WIDTH),
    ("HP", HP_COL_WIDTH),
    ("MaxHP", MAX_HP_COL_WIDTH),
    ("XP", XP_COL_WIDTH),
    ("Lvl", LVL_COL_WIDTH),
];
const MP_COLUMN_INDEX: usize = 6;
const HP_COLUMN_INDEX: usize = 7;

fn party_columns(table: TableBuilder<'_>) -> TableBuilder<'_> {
    PARTY_COLUMNS.iter().fold(table, |table, (_, width)| {
        table.column(Column::exact(*width))
    })
}

fn column_width(index: usize) -> f32 {
    PARTY_COLUMNS[index].1
}

fn column_span_width(start: usize, end: usize, gap: f32) -> f32 {
    let widths = PARTY_COLUMNS[start..end]
        .iter()
        .map(|(_, width)| *width)
        .sum::<f32>();
    let gaps = (end - start).saturating_sub(1) as f32 * gap;
    widths + gaps
}

/// Find the frigate the party is currently aboard, if any.
fn resolve_current_ship_slot(
    frigates: &[Frigate],
    map: Option<&MapState>,
    previous_slot: Option<usize>,
) -> Option<usize> {
    let map = map?;
    if !is_frigate_tile(map.transport) {
        return None;
    }

    if let Some(slot) = frigates
        .iter()
        .find(|f| f.x == map.x && f.y == map.y)
        .map(|f| f.slot)
    {
        return Some(slot);
    }

    if let Some(slot) = previous_slot.filter(|slot| frigates.iter().any(|f| f.slot == *slot)) {
        return Some(slot);
    }

    None
}

fn current_ship_by_slot_mut(frigates: &mut [Frigate], slot: usize) -> Option<&mut Frigate> {
    frigates.iter_mut().find(|frigate| frigate.slot == slot)
}

/// Returns `true` if any character data was written to game memory.
pub fn show(
    ui: &mut egui::Ui,
    party: &mut [Character],
    locks: &mut PartyLocks,
    frigates: &mut [Frigate],
    current_ship_slot: &mut Option<usize>,
    map: Option<&MapState>,
    mem: Option<(&dyn MemoryAccess, usize)>,
) -> bool {
    let mut wrote = false;
    let has_party = !party.is_empty();
    let lock_row_height = ui.spacing().interact_size.y;
    let column_gap = ui.spacing().item_spacing.x;
    // Match the title span to the same column widths used by the party table so
    // the MP/HP lock cells line up with their column headers below.
    let lead_width = column_span_width(0, MP_COLUMN_INDEX, column_gap);
    let mp_width = column_width(MP_COLUMN_INDEX);
    let hp_width = column_width(HP_COLUMN_INDEX);

    ui.horizontal(|ui| {
        // `allocate_ui_with_layout` advances the parent cursor by the child's
        // `min_rect`, not the requested size, so each cell calls `set_min_width`
        // to consume the full column width and keep alignment with the headers.
        ui.allocate_ui_with_layout(
            egui::vec2(lead_width, lock_row_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(lead_width);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(egui::RichText::new("\u{2694}").heading());
                ui.label(
                    egui::RichText::new("Party")
                        .heading()
                        .color(egui::Color32::from_rgb(100, 180, 255)),
                );
            },
        );

        if has_party {
            ui.allocate_ui_with_layout(
                egui::vec2(mp_width, lock_row_height),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.set_min_width(mp_width);
                    crate::gui::infinity_checkbox(
                        ui,
                        &mut locks.mana,
                        "Lock mana at 99 for the whole party.",
                    );
                },
            );

            ui.allocate_ui_with_layout(
                egui::vec2(hp_width, lock_row_height),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    ui.set_min_width(hp_width);
                    crate::gui::infinity_checkbox(
                        ui,
                        &mut locks.health,
                        "Lock health at max and force Good status for the whole party.",
                    );
                },
            );
        }
    });

    if !has_party {
        ui.label("No party data loaded.");
        return false;
    }

    party_columns(TableBuilder::new(ui))
        .striped(true)
        .header(20.0, |mut header| {
            let hdr = egui::Color32::from_rgb(140, 180, 255);
            for &(label, _) in &PARTY_COLUMNS {
                header.col(|ui| {
                    ui.colored_label(hdr, label);
                });
            }
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
                    ui.add_enabled_ui(!locks.health, |ui| {
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
                        .add_enabled(
                            !locks.mana,
                            egui::DragValue::new(&mut ch.mp).range(0..=MAX_MP),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });

                row.col(|ui| {
                    if ui
                        .add_enabled(
                            !locks.health,
                            egui::DragValue::new(&mut ch.hp).range(0..=999),
                        )
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

                if mem.is_some() {
                    changed |= apply_party_locks(&mut ch, locks);
                }

                if changed {
                    party[i] = ch;
                    if let Some((mem, dos_base)) = mem {
                        let _ = write_character(mem, dos_base, &party[i]);
                        wrote = true;
                    }
                }
            });
        });

    let ship_panel_visible = map.is_some_and(|map| is_frigate_tile(map.transport));
    *current_ship_slot = resolve_current_ship_slot(frigates, map, *current_ship_slot);

    // Keep the ship row mounted whenever the transport byte says we are
    // sailing so transient mismatches between map and object snapshots cannot
    // resize the dashboard above the minimap.
    if ship_panel_visible {
        let ship_color = egui::Color32::from_rgb(120, 200, 220);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut placeholder_hull = 0u8;
            let mut placeholder_skiffs = 0u8;

            if let Some(ship) =
                current_ship_slot.and_then(|slot| current_ship_by_slot_mut(frigates, slot))
            {
                ui.label(
                    egui::RichText::new(format!("\u{26F5} {} Hull:", ship.label()))
                        .color(ship_color),
                );
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
            } else {
                ui.label(egui::RichText::new("\u{26F5} Ship Hull:").color(ship_color));
                ui.add_enabled(
                    false,
                    egui::DragValue::new(&mut placeholder_hull).range(0..=FRIGATE_MAX_HULL),
                );
                ui.label(format!("/ {FRIGATE_MAX_HULL}"));
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Skiffs:").color(ship_color));
                ui.add_enabled(
                    false,
                    egui::DragValue::new(&mut placeholder_skiffs).range(0..=u8::MAX),
                );
            }
        });
    }

    wrote
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::map::LocationType;
    use crate::game::offsets::MAP_TILES_LEN;

    fn test_map(transport: u8, x: u8, y: u8) -> MapState {
        MapState {
            location: LocationType::Overworld,
            z: 0,
            x,
            y,
            dungeon_facing: None,
            transport,
            scroll_x: 0,
            scroll_y: 0,
            tiles: [0; MAP_TILES_LEN],
            combat_tiles: None,
            visibility_tiles: None,
            objects: Vec::new(),
        }
    }

    #[test]
    fn resolve_current_ship_slot_prefers_exact_match() {
        let frigates = vec![
            Frigate {
                slot: 1,
                tile: 36,
                x: 10,
                y: 10,
                hull: 50,
                skiffs: 2,
            },
            Frigate {
                slot: 2,
                tile: 36,
                x: 40,
                y: 40,
                hull: 80,
                skiffs: 1,
            },
        ];

        let map = test_map(36, 40, 40);

        assert_eq!(
            resolve_current_ship_slot(&frigates, Some(&map), Some(1)),
            Some(2)
        );
    }

    #[test]
    fn resolve_current_ship_slot_keeps_previous_slot_across_snapshot_mismatch() {
        let frigates = vec![
            Frigate {
                slot: 1,
                tile: 36,
                x: 10,
                y: 10,
                hull: 50,
                skiffs: 2,
            },
            Frigate {
                slot: 2,
                tile: 36,
                x: 60,
                y: 60,
                hull: 80,
                skiffs: 1,
            },
        ];

        let map = test_map(36, 11, 10);

        assert_eq!(
            resolve_current_ship_slot(&frigates, Some(&map), Some(1)),
            Some(1)
        );
    }

    #[test]
    fn resolve_current_ship_slot_clears_when_not_sailing() {
        let frigates = vec![Frigate {
            slot: 1,
            tile: 36,
            x: 10,
            y: 10,
            hull: 50,
            skiffs: 2,
        }];

        let map = test_map(0, 10, 10);

        assert_eq!(
            resolve_current_ship_slot(&frigates, Some(&map), Some(1)),
            None
        );
    }
}
