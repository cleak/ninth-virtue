pub mod actions_panel;
pub mod audio_panel;
pub mod connection_bar;
pub mod inventory_panel;
pub mod memory_watch_panel;
pub mod minimap_fog;
pub mod minimap_gl;
pub mod minimap_panel;
pub mod party_panel;
pub mod quest_panel;

/// Styled frame for visually grouping a UI section.
pub fn section_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().panel_fill)
        .inner_margin(8.0)
}

pub fn infinity_checkbox(ui: &mut egui::Ui, locked: &mut bool, tooltip: &str) -> egui::Response {
    ui.checkbox(locked, egui::RichText::new("∞"))
        .on_hover_text(tooltip)
}
