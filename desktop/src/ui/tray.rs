use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct SystemTray {
    #[allow(dead_code)]
    pub tray_icon: TrayIcon,
    pub auto_paste_item: CheckMenuItem,
    pub settings_item: MenuItem,
    pub updates_item: MenuItem,
    pub quit_item: MenuItem,
}

impl SystemTray {
    pub fn new(auto_paste_enabled: bool, hotkey_label: &str) -> Result<Self, String> {
        let tray_menu = Menu::new();

        let title_item = MenuItem::new("TDT - Talk Don't Type", false, None);
        let shortcut_item = MenuItem::new(format!("Shortcut: {}", hotkey_label), false, None);
        let auto_paste_item = CheckMenuItem::new(
            "Auto-paste into the focused app",
            true,
            auto_paste_enabled,
            None,
        );
        let settings_item = MenuItem::new("Open settings", true, None);
        let updates_item = MenuItem::new("Check for updates", true, None);
        let quit_item = MenuItem::new("Quit TDT", true, None);

        tray_menu
            .append(&title_item)
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&shortcut_item)
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&auto_paste_item)
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&settings_item)
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&updates_item)
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("Menu error: {}", e))?;
        tray_menu
            .append(&quit_item)
            .map_err(|e| format!("Menu error: {}", e))?;

        // Rosé Pine Moon iris circle (RGBA: 196, 167, 231, 255)
        let mut icon_data = Vec::with_capacity(32 * 32 * 4);
        for y in 0..32 {
            for x in 0..32 {
                let dx = (x as f32 - 15.5).powi(2);
                let dy = (y as f32 - 15.5).powi(2);
                let dist = (dx + dy).sqrt();
                if dist <= 13.0 {
                    icon_data.extend_from_slice(&[196, 167, 231, 255]);
                } else {
                    icon_data.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }

        let icon = Icon::from_rgba(icon_data, 32, 32)
            .map_err(|e| format!("Failed to create tray icon: {}", e))?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("TDT - Talk Don't Type. Tap or hold Ctrl+; to talk.")
            .with_icon(icon)
            .build()
            .map_err(|e| format!("Failed to build tray icon: {}", e))?;

        Ok(Self {
            tray_icon,
            auto_paste_item,
            settings_item,
            updates_item,
            quit_item,
        })
    }
}
