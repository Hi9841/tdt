//! Ely controls used by the overlay and the panel.
//! Layout helpers stay here so the HUD does not rebuild rows by hand.

use ely_gpui_component::buttons::{Button, ButtonVariant};
use ely_gpui_component::motion::ProgressBar;
use ely_gpui_component::settings::SettingsRow;
use ely_gpui_component::theme::ControlSize;
use ely_gpui_component::typography::Kbd;
use gpui::*;

pub fn ghost_button(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .variant(ButtonVariant::Ghost)
        .size(ControlSize::Sm)
}

pub fn secondary_button(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .variant(ButtonVariant::Secondary)
        .size(ControlSize::Sm)
}

pub fn primary_button(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .variant(ButtonVariant::Primary)
        .size(ControlSize::Sm)
}

pub fn danger_button(id: &'static str, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .variant(ButtonVariant::Danger)
        .size(ControlSize::Sm)
}

pub fn hold_click(id: &'static str, child: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(child)
}

pub fn setting_row(
    title: &'static str,
    subtitle: impl Into<SharedString>,
    trailing: impl IntoElement,
) -> SettingsRow {
    SettingsRow::new(title)
        .description(subtitle)
        .control(trailing)
}

pub fn progress_bar(id: &'static str, done: u64, total: u64) -> ProgressBar {
    let fraction = if total == 0 {
        0.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    };
    ProgressBar::new(id, fraction)
}

pub fn indeterminate_bar(id: &'static str) -> ProgressBar {
    ProgressBar::indeterminate(id)
}

/// Ely keycaps. `hotkey` is TDT's display form, such as `Ctrl+;`.
pub fn shortcut_keys(hotkey: &str) -> AnyElement {
    Kbd::new(&display_to_gpui(hotkey)).into_any_element()
}

fn display_to_gpui(hotkey: &str) -> String {
    let parts: Vec<&str> = hotkey
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return "ctrl-semicolon".into();
    }
    let mut mods = Vec::new();
    let mut key = String::new();
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if !last {
            mods.push(match part.to_ascii_lowercase().as_str() {
                "control" => "ctrl".to_string(),
                "windows" => "win".to_string(),
                other => other.to_string(),
            });
            continue;
        }
        key = match *part {
            "Backspace" => "backspace".into(),
            "Tab" => "tab".into(),
            "Enter" => "enter".into(),
            "Space" => "space".into(),
            ";" => ";".into(),
            "-" => "-".into(),
            other => other.to_ascii_lowercase(),
        };
    }
    if key == "-" {
        let mut source = mods.join("-");
        if !source.is_empty() {
            source.push('-');
        }
        source.push('-');
        return source;
    }
    mods.push(key);
    mods.join("-")
}

#[cfg(test)]
mod tests {
    use super::display_to_gpui;

    #[test]
    fn display_shortcuts_become_gpui_keystrokes() {
        assert_eq!(display_to_gpui("Ctrl+;"), "ctrl-;");
        assert_eq!(display_to_gpui("Ctrl+Shift+Space"), "ctrl-shift-space");
        assert_eq!(display_to_gpui("Ctrl+-"), "ctrl--");
        assert_eq!(display_to_gpui("F5"), "f5");
        assert_eq!(display_to_gpui("Ctrl+Win+A"), "ctrl-win-a");
    }
}
