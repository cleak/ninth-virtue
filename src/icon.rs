const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");

pub(crate) fn load_app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).expect("app icon should decode")
}

#[cfg(test)]
mod tests {
    use super::load_app_icon;

    #[test]
    fn app_icon_decodes() {
        let icon = load_app_icon();

        assert_eq!(icon.width, 256);
        assert_eq!(icon.height, 256);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
