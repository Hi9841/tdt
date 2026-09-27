use super::{hud::HudStatus, theme, window_util::client_animations_enabled};
use std::time::{Duration, Instant};
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
    pub stop_item: MenuItem,
    last_frame: Option<(TrayState, usize, u8)>,
    last_tooltip: String,
    next_frame: Instant,
    started: Instant,
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

        let icon = Icon::from_rgba(
            render_bubble_icon(TrayState::Idle, 0, 0),
            ICON_SIZE,
            ICON_SIZE,
        )
        .map_err(|e| format!("Failed to create tray icon: {e}"))?;

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
            last_frame: None,
            last_tooltip: String::new(),
            next_frame: Instant::now(),
            started: Instant::now(),
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
        let now = Instant::now();
        if now < self.next_frame && self.last_frame.is_some_and(|(last, _, _)| last == state) {
            return;
        }
        self.next_frame = now + Duration::from_millis(100);
        let motion = client_animations_enabled();
        let frame = animation_frame(state, self.started.elapsed(), motion);
        let level = match status {
            HudStatus::Listening { audio_level, .. } if motion => {
                (audio_level.clamp(0.0, 1.0).sqrt() * 8.0).round() as u8
            }
            _ => 4,
        };
        if self.last_frame == Some((state, frame, level)) {
            return;
        }
        if let Ok(icon) = Icon::from_rgba(
            render_bubble_icon(state, frame, level),
            ICON_SIZE,
            ICON_SIZE,
        ) {
            if self.tray_icon.set_icon(Some(icon)).is_ok() {
                self.last_frame = Some((state, frame, level));
            }
        }
    }
}

fn animation_frame(state: TrayState, elapsed: Duration, motion: bool) -> usize {
    if motion && matches!(state, TrayState::Listening | TrayState::Transcribing) {
        (elapsed.as_millis() / 100 % 12) as usize
    } else {
        0
    }
}

/// Antialiased bubble with a state glyph, legible on light and dark taskbars.
fn render_bubble_icon(state: TrayState, frame: usize, level: u8) -> Vec<u8> {
    let color = match state {
        TrayState::Idle => theme::IRIS,
        TrayState::Listening => theme::FOAM,
        TrayState::Transcribing => theme::GOLD,
        TrayState::Success => theme::SUCCESS,
        TrayState::Attention => theme::LOVE,
    };
    let mut pixels = vec![0; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let phase = frame as f32 / 12.0 * std::f32::consts::TAU;
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let distance = dx.hypot(dy);
            let alpha = (14.5 - distance).clamp(0.0, 1.0);
            let glyph = match state {
                TrayState::Idle => dx.hypot(dy) < 3.0,
                TrayState::Listening => {
                    let column = ((dx + 8.0) / 5.0).floor();
                    (-8.0..8.0).contains(&dx)
                        && (dx + 8.0) % 5.0 < 3.0
                        && dy.abs()
                            < 2.0 + level as f32 * (0.4 + 0.6 * (phase + column * 1.5).sin().abs())
                }
                TrayState::Transcribing => {
                    (7.0..10.0).contains(&distance)
                        && (dy.atan2(dx) - phase).rem_euclid(std::f32::consts::TAU) < 4.7
                }
                TrayState::Success => {
                    ((dx + 4.0).abs() < 4.0 && (dy - dx - 4.0).abs() < 1.8)
                        || ((-1.0..8.0).contains(&dx) && (dy + dx - 3.0).abs() < 1.8)
                }
                TrayState::Attention => {
                    dx.abs() < 1.8 && ((-8.0..3.0).contains(&dy) || (5.0..8.0).contains(&dy))
                }
            };
            let rgb = if glyph {
                theme::BG
            } else if distance > 12.0 {
                color
            } else {
                theme::TEXT
            };
            let offset = ((y * ICON_SIZE + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&[
                (rgb >> 16) as u8,
                (rgb >> 8) as u8,
                rgb as u8,
                (alpha * 255.0) as u8,
            ]);
        }
    }
    pixels
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
    use super::{animation_frame, render_bubble_icon, TrayState};
    use super::{tooltip_text, ICON_SIZE};
    use std::time::Duration;

    #[test]
    fn motion_only_runs_for_active_states_and_respects_reduced_motion() {
        for state in [TrayState::Idle, TrayState::Success, TrayState::Attention] {
            assert_eq!(animation_frame(state, Duration::from_millis(500), true), 0);
        }
        for state in [TrayState::Listening, TrayState::Transcribing] {
            assert_eq!(animation_frame(state, Duration::from_millis(500), false), 0);
            assert_eq!(animation_frame(state, Duration::from_millis(500), true), 5);
            assert_ne!(
                render_bubble_icon(state, 0, 8),
                render_bubble_icon(state, 5, 8)
            );
        }
    }

    #[test]
    fn bubble_icon_is_visible_with_transparent_corners() {
        let pixels = render_bubble_icon(TrayState::Idle, 0, 0);
        assert_eq!(pixels[3], 0);
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
            "bubble should fill most of the 32px icon, got {opaque}"
        );
        assert!(
            colored > 40,
            "bubble should be visible, got {colored} colored pixels"
        );
    }

    #[test]
    fn tooltip_names_the_shortcut() {
        assert_eq!(
            tooltip_text("Ctrl+;"),
            "TDT. Shortcut Ctrl+;. Click to open."
        );
    }

    #[test]
    fn tray_wave_responds_to_voice_level() {
        assert_ne!(
            render_bubble_icon(TrayState::Listening, 0, 0),
            render_bubble_icon(TrayState::Listening, 0, 8)
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
