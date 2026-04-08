pub mod actions_panel;
pub mod audio_panel;
pub mod connection_bar;
pub mod inventory_panel;
pub mod memory_watch_panel;
pub mod minimap_panel;
pub mod party_panel;

/// Styled frame for visually grouping a UI section.
pub fn section_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::group(ui.style()).inner_margin(8.0)
}
