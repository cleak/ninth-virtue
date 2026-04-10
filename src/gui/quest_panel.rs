use crate::game::quest::{QuestPhase, ShrineQuest, Virtue};

const QUEST_HEADING: egui::Color32 = egui::Color32::from_rgb(100, 200, 160);
const PHASE_NOT_STARTED: egui::Color32 = egui::Color32::from_rgb(128, 128, 128);
const PHASE_ORDAINED: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const PHASE_CODEX_READ: egui::Color32 = egui::Color32::from_rgb(100, 180, 255);
const PHASE_COMPLETE: egui::Color32 = egui::Color32::from_rgb(80, 220, 120);
const MANTRA_HIDDEN_LABEL: &str = "[hidden]";
const MANTRA_MASKED_FILL: egui::Color32 = egui::Color32::from_rgb(48, 52, 72);
const MANTRA_REVEALED_FILL: egui::Color32 = egui::Color32::from_rgb(32, 38, 58);
const MANTRA_MASKED_TEXT: egui::Color32 = egui::Color32::from_rgb(120, 128, 156);
const MANTRA_REVEALED_TEXT: egui::Color32 = egui::Color32::from_rgb(180, 180, 220);

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
                mantra_spoiler(ui, virtue.mantra());
                ui.label(egui::RichText::new(label).color(color));
                ui.end_row();
            }
        });
}

fn mantra_spoiler(ui: &mut egui::Ui, mantra: &str) {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let hidden_size = ui
        .painter()
        .layout_no_wrap(
            MANTRA_HIDDEN_LABEL.to_owned(),
            font_id.clone(),
            MANTRA_MASKED_TEXT,
        )
        .size();
    let revealed_size = ui
        .painter()
        .layout_no_wrap(mantra.to_owned(), font_id.clone(), MANTRA_REVEALED_TEXT)
        .size();
    // Keep the cell width stable so the grid does not jump as the mantra is revealed.
    let desired_size = egui::vec2(
        hidden_size.x.max(revealed_size.x) + 12.0,
        hidden_size.y.max(revealed_size.y).max(14.0) + 6.0,
    );
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let (display, fill, text_color) = if response.hovered() {
        (mantra, MANTRA_REVEALED_FILL, MANTRA_REVEALED_TEXT)
    } else {
        (MANTRA_HIDDEN_LABEL, MANTRA_MASKED_FILL, MANTRA_MASKED_TEXT)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(display.to_owned(), font_id, text_color);
    let text_pos = egui::pos2(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5,
    );

    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter().galley(text_pos, galley, text_color);
}
