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

/// Dark rounded tile with a cyan waveform so the icon reads on both
/// the dark taskbar and the light Windows 11 overflow flyout.
fn render_tray_icon() -> Vec<u8> {
    let size = ICON_SIZE as i32;
    let mut pixels = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let bg = (0x19u8, 0x17, 0x24);
    let foam = (0x9cu8, 0xcf, 0xd8);
    let edge = (0x2a, 0x27, 0x3f);

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if !inside_round_rect(px, py, 1.0, 1.0, 31.0, 31.0, 7.0) {
                continue;
            }
            let on_edge = !inside_round_rect(px, py, 2.2, 2.2, 29.8, 29.8, 6.0);
            let (r, g, b) = if on_edge { edge } else { bg };
            put_pixel(&mut pixels, x, y, r, g, b, 255);
        }
    }

    let bars: [(i32, i32); 5] = [(9, 6), (13, 12), (17, 16), (21, 10), (25, 7)];
    for (cx, height) in bars {
        let top = 16 - height / 2;
        let bottom = top + height;
        for y in top..=bottom {
            for x in (cx - 1)..=cx {
                if y >= 4 && y <= 27 {
                    put_pixel(&mut pixels, x, y, foam.0, foam.1, foam.2, 255);
                }
            }
        }
    }

    pixels
}

fn inside_round_rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> bool {
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn put_pixel(buf: &mut [u8], x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
    if x < 0 || y < 0 || x >= ICON_SIZE as i32 || y >= ICON_SIZE as i32 {
        return;
    }
    let i = ((y * ICON_SIZE as i32 + x) * 4) as usize;
    buf[i] = r;
    buf[i + 1] = g;
    buf[i + 2] = b;
    buf[i + 3] = a;
}

#[cfg(test)]
mod tests {
    use super::{render_tray_icon, tooltip_text, ICON_SIZE};

    #[test]
    fn icon_is_32_rgba_with_opaque_tile() {
        let pixels = render_tray_icon();
        assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let mut opaque = 0usize;
        let mut foam = 0usize;
        for px in pixels.chunks_exact(4) {
            if px[3] == 255 {
                opaque += 1;
            }
            if px[0] == 0x9c && px[1] == 0xcf && px[2] == 0xd8 {
                foam += 1;
            }
        }
        assert!(opaque > 500, "tile should fill most of the 32px icon, got {opaque}");
        assert!(foam > 40, "waveform should be visible, got {foam} foam pixels");
        assert_eq!(pixels[0..4], [0, 0, 0, 0]);
    }

    #[test]
    fn tooltip_names_the_shortcut() {
        assert_eq!(
            tooltip_text("Ctrl+;"),
            "TDT. Shortcut Ctrl+;. Click to show."
        );
    }
}
