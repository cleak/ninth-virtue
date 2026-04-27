mod app;
mod audio;
mod dosbox;
mod game;
mod gui;
mod icon;
mod memory;
mod preferences;
#[cfg(test)]
mod test_support;
mod tiles;
mod window_focus;

fn main() -> eframe::Result {
    env_logger::init();

    // Initialize COM for WASAPI audio session control (apartment-threaded
    // to match egui's single-thread model).
    if let Err(e) = audio::init_com() {
        eprintln!("COM init failed (audio controls unavailable): {e}");
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 1000.0])
            .with_title("The Ninth Virtue")
            .with_icon(icon::load_app_icon()),
        // The minimap uses egui_glow paint callbacks, so keep the native renderer on Glow.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "ninth-virtue",
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

            cc.egui_ctx.global_style_mut(|style| {
                style.spacing.item_spacing = egui::vec2(8.0, 4.0);
                for ws in [
                    &mut style.visuals.widgets.inactive,
                    &mut style.visuals.widgets.hovered,
                    &mut style.visuals.widgets.active,
                    &mut style.visuals.widgets.open,
                ] {
                    ws.corner_radius = egui::CornerRadius::same(4);
                }
            });
            Ok(Box::new(app::UltimaCompanion::new()))
        }),
    )
}
