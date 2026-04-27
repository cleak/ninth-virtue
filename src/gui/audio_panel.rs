use crate::audio::AudioSession;

const HEADING_COLOR: egui::Color32 = egui::Color32::from_rgb(100, 220, 180);
const AUDIO_FILL: egui::Color32 = egui::Color32::from_rgb(55, 55, 100);
const BTN_TEXT: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);

/// Render volume/mute controls. Reads and writes the audio session directly
/// (no game memory involved).
///
/// `muted` reflects the current WASAPI mute state (which auto-mute may
/// change without the user's input). `user_muted` is the user's persisted
/// intent, updated only when they click the mute button here.
pub fn show(
    ui: &mut egui::Ui,
    session: &Option<AudioSession>,
    volume: &mut f32,
    muted: &mut bool,
    user_muted: &mut bool,
    mute_on_lost_focus: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(egui::RichText::new("🔊").heading());
        ui.label(egui::RichText::new("Sound").heading().color(HEADING_COLOR));
    });

    let enabled = session.is_some();

    // Mute toggle button + "Mute on lost focus" checkbox on the same line.
    let mute_label = if *muted { "🔇 Unmute" } else { "🔈 Mute" };
    // Keep a consistent button height without stretching the control to the
    // full card width.
    let button_size = egui::vec2(0.0, 24.0);

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(mute_label).color(BTN_TEXT))
                    .fill(AUDIO_FILL)
                    .min_size(button_size),
            )
            .clicked()
        {
            let new_muted = !*muted;
            if let Some(sess) = session.as_ref() {
                match sess.set_mute(new_muted) {
                    Ok(()) => {
                        *muted = new_muted;
                        // Record the click as the user's intent. Auto-mute
                        // never reaches this path, so this only captures
                        // explicit clicks.
                        *user_muted = new_muted;
                    }
                    Err(e) => eprintln!("set_mute failed: {e}"),
                }
            }
        }

        ui.checkbox(mute_on_lost_focus, "Mute on lost focus")
            .on_hover_text("Auto-mute the game when its window loses focus");
    });

    // Volume slider.
    ui.add_space(2.0);
    ui.add_enabled_ui(enabled, |ui| {
        let mut pct = *volume * 100.0;
        let slider = egui::Slider::new(&mut pct, 0.0..=100.0)
            .text("%")
            .fixed_decimals(0);
        if ui.add(slider).changed() {
            let new_vol = pct / 100.0;
            if let Some(sess) = session.as_ref() {
                match sess.set_volume(new_vol) {
                    Ok(()) => *volume = new_vol,
                    Err(e) => eprintln!("set_volume failed: {e}"),
                }
            }
        }
    });
}
