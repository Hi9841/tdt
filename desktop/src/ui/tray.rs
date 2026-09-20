use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_SIZE: u32 = 32;

pub struct SystemTray {
    pub tray_icon: TrayIcon,
    pub auto_paste_item: CheckMenuItem,
    pub settings_item: MenuItem,
    pub show_item: MenuItem,
    pub updates_item: MenuItem,
    pub quit_item: MenuItem,
}

impl SystemTray {
    pub fn new(auto_paste_enabled: bool, hotkey_label: &str) -> Result<Self, String> {
        let tray_menu = Menu::new();

        let show_item = MenuItem::new("Show overlay", true, None);
        let settings_item = MenuItem::new("Open settings", true, None);
        let auto_paste_item = CheckMenuItem::new(
            "Auto-paste into the focused app",
            true,
            auto_paste_enabled,
            None,
        );
        let updates_item = MenuItem::new("Check for updates", true, None);
        let quit_item = MenuItem::new("Quit TDT", true, None);

        tray_menu
            .append(&show_item)
            .map_err(|e| format!("Menu error: {e}"))?;
        tray_menu
            .append(&settings_item)
            .map_err(|e| format!("Menu error: {e}"))?;
        tray_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("Menu error: {e}"))?;
        tray_menu
            .append(&auto_paste_item)
            .map_err(|e| format!("Menu error: {e}"))?;
        tray_menu
            .append(&updates_item)
            .map_err(|e| format!("Menu error: {e}"))?;
        tray_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("Menu error: {e}"))?;
        tray_menu
            .append(&quit_item)
            .map_err(|e| format!("Menu error: {e}"))?;

        let icon = Icon::from_rgba(render_tray_icon(), ICON_SIZE, ICON_SIZE)
            .map_err(|e| format!("Failed to create tray icon: {e}"))?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip(tooltip_text(hotkey_label))
            .with_icon(icon)
            .build()
            .map_err(|e| format!("Failed to build tray icon: {e}"))?;

        Ok(Self {
            tray_icon,
            auto_paste_item,
            settings_item,
            show_item,
            updates_item,
            quit_item,
        })
    }
}

pub fn tooltip_text(hotkey_label: &str) -> String {
    format!("TDT. Shortcut {hotkey_label}. Click to show.")
}

/// 32px D-bubble mark so the tray matches the Start-menu icon.
fn render_tray_icon() -> Vec<u8> {
    const RAW: &[u8] = include_bytes!("../../assets/tray-32.rgba");
    debug_assert_eq!(RAW.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    RAW.to_vec()
}

#[cfg(test)]
mod tests {
    use super::{render_tray_icon, tooltip_text, ICON_SIZE};

    #[test]
    fn icon_is_32_rgba_with_opaque_tile() {
        let pixels = render_tray_icon();
        assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let mut opaque = 0usize;
        let mut colored = 0usize;
        for px in pixels.as_chunks::<4>().0 {
            if px[3] == 255 {
                opaque += 1;
            }
            if px[3] > 0 && (px[0] > 40 || px[1] > 40 || px[2] > 80) {
                colored += 1;
            }
        }
        assert!(
            opaque > 500,
            "tile should fill most of the 32px icon, got {opaque}"
        );
        assert!(
            colored > 40,
            "D-bubble should be visible, got {colored} colored pixels"
        );
    }

    #[test]
    fn tooltip_names_the_shortcut() {
        assert_eq!(
            tooltip_text("Ctrl+;"),
            "TDT. Shortcut Ctrl+;. Click to show."
        );
    }
}
