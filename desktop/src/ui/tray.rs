use super::hud::HudStatus;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Ordinal of the icon in `desktop/assets/tdt.rc`. Same image as the exe icon.
const APP_ICON_ID: u16 = 1;

pub struct SystemTray {
    pub tray_icon: TrayIcon,
    pub auto_paste_item: CheckMenuItem,
    pub settings_item: MenuItem,
    pub show_item: MenuItem,
    pub updates_item: MenuItem,
    pub quit_item: MenuItem,
    pub stop_item: MenuItem,
    last_tooltip: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Listening,
    Transcribing,
    Success,
    Attention,
}

impl TrayState {
    pub fn from_status(status: &HudStatus) -> Self {
        match status {
            HudStatus::Idle => Self::Idle,
            HudStatus::Listening { .. } => Self::Listening,
            HudStatus::Transcribing { .. } => Self::Transcribing,
            HudStatus::Success { .. } => Self::Success,
            HudStatus::Error { .. } | HudStatus::NoSpeech => Self::Attention,
        }
    }
}

impl SystemTray {
    pub fn new(auto_paste_enabled: bool, hotkey_label: &str) -> Result<Self, String> {
        let tray_menu = Menu::new();

        let show_item = MenuItem::new("Open TDT", true, None);
        let stop_item = MenuItem::new("Stop recording", false, None);
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
            .append(&stop_item)
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

        let icon = app_icon()?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_menu_on_left_click(false)
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
            stop_item,
            last_tooltip: String::new(),
        })
    }

    pub fn update(&mut self, status: &HudStatus, hotkey: &str) {
        let state = TrayState::from_status(status);
        let recording = state == TrayState::Listening;
        if self.stop_item.is_enabled() != recording {
            self.stop_item.set_enabled(recording);
        }
        let tooltip = status_tooltip(status, hotkey);
        if tooltip != self.last_tooltip && self.tray_icon.set_tooltip(Some(&tooltip)).is_ok() {
            self.last_tooltip = tooltip;
        }
    }
}

fn app_icon() -> Result<Icon, String> {
    let side = tray_icon_px();
    Icon::from_resource(APP_ICON_ID, Some((side, side)))
        .map_err(|error| format!("Failed to load the app icon: {error}"))
}

fn tray_icon_px() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    let side = unsafe { GetSystemMetrics(SM_CXSMICON) };
    side.max(16) as u32
}

pub fn tooltip_text(hotkey_label: &str) -> String {
    format!("TDT. Shortcut {hotkey_label}. Click to open.")
}

fn status_tooltip(status: &HudStatus, hotkey: &str) -> String {
    let text = match status {
        HudStatus::Idle => format!("TDT. Ready. Shortcut {hotkey}. Click to open."),
        HudStatus::Listening { .. } => format!("TDT. Listening. Press {hotkey} to stop."),
        HudStatus::Transcribing { .. } => "TDT. Transcribing.".into(),
        HudStatus::Success {
            text,
            auto_pasted,
            latency_ms,
            ..
        } => format!(
            "TDT. {} {}: {text}",
            if *auto_pasted { "Inserted" } else { "Copied" },
            crate::ui::text::format_latency_ms(*latency_ms)
        ),
        HudStatus::NoSpeech => "TDT. No speech detected. Try again or open settings.".into(),
        HudStatus::Error { message, .. } => format!("TDT. {message}"),
    };
    // NOTIFYICONDATAW reserves 128 UTF-16 units including its terminator.
    let mut units = 0;
    text.chars()
        .take_while(|ch| {
            units += ch.len_utf16();
            units <= 127
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::tooltip_text;

    #[test]
    fn tray_uses_the_executable_app_icon() {
        let rc = include_str!("../../assets/tdt.rc");
        assert!(rc.contains("1 ICON \"tdt.ico\""));
        assert_eq!(super::APP_ICON_ID, 1);
    }

    #[test]
    fn tooltip_names_the_shortcut() {
        assert_eq!(
            tooltip_text("Ctrl+;"),
            "TDT. Shortcut Ctrl+;. Click to open."
        );
    }

    #[test]
    fn transcript_tooltip_fits_windows_unicode_limit() {
        let status = super::HudStatus::Success {
            text: "hello 🎙️".repeat(40),
            auto_pasted: false,
            latency_ms: 182,
            finished_at: std::time::Instant::now(),
        };
        let tooltip = super::status_tooltip(&status, "Ctrl+;");
        assert!(tooltip.starts_with("TDT. Copied 182ms: hello"));
        assert!(tooltip.encode_utf16().count() <= 127);
    }
}
