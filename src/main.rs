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
        Box::new(|_cc| Ok(Box::new(app::UltimaCompanion::new()))),
    )
}
