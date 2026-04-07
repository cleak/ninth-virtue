pub enum ConnectionAction {
    None,
    Attach(u32),
    Detach,
}

#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    process_list: &[(u32, String)],
    selected_pid: &mut Option<u32>,
    is_attached: bool,
    game_confirmed: bool,
    dos_base: Option<usize>,
    auto_refresh: &mut bool,
    refresh_interval: &mut f32,
    status_msg: &str,
) -> ConnectionAction {
    let mut action = ConnectionAction::None;

    ui.horizontal(|ui| {
        // Status indicator
        let (color, text) = if !is_attached {
            (egui::Color32::from_rgb(200, 60, 60), "Disconnected")
        } else if game_confirmed {
            (egui::Color32::from_rgb(60, 200, 60), "Connected")
        } else {
            (egui::Color32::from_rgb(230, 200, 50), "DOS found")
        };

        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 6.0, color);
        ui.colored_label(color, text);

        if is_attached {
            if ui.button("Disconnect").clicked() {
                action = ConnectionAction::Detach;
            }

            if let Some(base) = dos_base {
                ui.label(format!("{base:#x}"));
            }

            ui.separator();
            ui.checkbox(auto_refresh, "Auto");
            if *auto_refresh {
                ui.add(
                    egui::DragValue::new(refresh_interval)
                        .range(0.5..=5.0)
                        .speed(0.1)
                        .suffix("s"),
                );
            }
        } else if !process_list.is_empty() {
            // Process picker -- only visible when not auto-attached
            // (multiple processes, or auto-attach suppressed after disconnect).
            let label = selected_pid
                .and_then(|pid| process_list.iter().find(|(p, _)| *p == pid))
                .map_or_else(
                    || "Select process...".to_string(),
                    |(pid, name)| format!("{name} ({pid})"),
                );

            egui::ComboBox::from_id_salt("process_picker")
                .selected_text(&label)
                .show_ui(ui, |ui| {
                    for (pid, name) in process_list {
                        let text = format!("{name} ({pid})");
                        ui.selectable_value(selected_pid, Some(*pid), &text);
                    }
                });

            if let Some(pid) = *selected_pid
                && ui.button("Connect").clicked()
            {
                action = ConnectionAction::Attach(pid);
            }
        }
    });

    if !status_msg.is_empty() {
        ui.label(status_msg);
    }

    action
}
