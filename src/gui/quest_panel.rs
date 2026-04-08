use crate::game::quest::{QuestPhase, ShrineQuest, Virtue};

const QUEST_HEADING: egui::Color32 = egui::Color32::from_rgb(100, 200, 160);
const PHASE_NOT_STARTED: egui::Color32 = egui::Color32::from_rgb(128, 128, 128);
const PHASE_ORDAINED: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const PHASE_CODEX_READ: egui::Color32 = egui::Color32::from_rgb(100, 180, 255);
const PHASE_COMPLETE: egui::Color32 = egui::Color32::from_rgb(80, 220, 120);

pub fn show(ui: &mut egui::Ui, quest: &ShrineQuest) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("🏛").heading());
        ui.label(
            egui::RichText::new("Shrine Quests")
                .heading()
                .color(QUEST_HEADING),
        );
    });

    let mut completed = 0u32;
    let mut in_progress = 0u32;
    for v in Virtue::ALL {
        match quest.phase(v) {
            QuestPhase::Complete => completed += 1,
            QuestPhase::Ordained | QuestPhase::CodexRead => in_progress += 1,
            QuestPhase::NotStarted => {}
        }
    }
    ui.label(format!("{completed}/8 complete, {in_progress} in progress"));
    ui.add_space(4.0);

    egui::Grid::new("shrine_quests")
        .num_columns(3)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Virtue").strong());
            ui.label(egui::RichText::new("Mantra").strong());
            ui.label(egui::RichText::new("Phase").strong());
            ui.end_row();

            for virtue in Virtue::ALL {
                let phase = quest.phase(virtue);
                let (color, label) = match phase {
                    QuestPhase::NotStarted => (PHASE_NOT_STARTED, "Not Started"),
                    QuestPhase::Ordained => (PHASE_ORDAINED, "Visit Codex"),
                    QuestPhase::CodexRead => (PHASE_CODEX_READ, "Return to Shrine"),
                    QuestPhase::Complete => (PHASE_COMPLETE, "Complete"),
                };

                ui.label(virtue.name());
                ui.label(
                    egui::RichText::new(virtue.mantra())
                        .monospace()
                        .color(egui::Color32::from_rgb(180, 180, 220)),
                );
                ui.label(egui::RichText::new(label).color(color));
                ui.end_row();
            }
        });
}
