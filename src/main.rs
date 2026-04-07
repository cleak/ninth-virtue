mod app;
mod game;
mod gui;
mod memory;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_title("Ultima V Companion"),
        ..Default::default()
    };
    eframe::run_native(
        "u5-companion",
        options,
        Box::new(|cc| {
            // Add Segoe UI Emoji as a fallback font for emoji support.
            if let Ok(emoji_data) = std::fs::read("C:/Windows/Fonts/seguiemj.ttf") {
                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert(
                    "emoji".to_owned(),
                    egui::FontData::from_owned(emoji_data).into(),
                );
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("emoji".to_owned());
                cc.egui_ctx.set_fonts(fonts);
            }

            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.item_spacing = egui::vec2(8.0, 4.0);
            for ws in [
                &mut style.visuals.widgets.inactive,
                &mut style.visuals.widgets.hovered,
                &mut style.visuals.widgets.active,
                &mut style.visuals.widgets.open,
            ] {
                ws.corner_radius = egui::CornerRadius::same(4);
            }
            cc.egui_ctx.set_style(style);
            Ok(Box::new(app::UltimaCompanion::new()))
        }),
    )
}
