//! Ely controls used by the overlay and the panel.
//! Layout helpers stay here so the HUD does not rebuild rows by hand.

use ely_gpui_component::buttons::{Button, ButtonVariant, IconButton};
use ely_gpui_component::motion::ProgressBar;
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, TextSize};
use ely_gpui_component::typography::Kbd;
use gpui::prelude::FluentBuilder;
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

pub fn icon_button(id: &'static str, icon: IconName, tip: &'static str) -> IconButton {
    IconButton::new(id, icon)
        .variant(ButtonVariant::Ghost)
        .size(ControlSize::Sm)
        .tooltip(tip)
}

pub fn hold_click(id: &'static str, child: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(child)
}

/// One setting on a single line. The control stays on the right in a narrow panel.
pub fn labeled(title: impl Into<SharedString>, caption: impl Into<SharedString>) -> LineRow {
    LineRow {
        title: title.into(),
        caption: Some(caption.into()),
        control: None,
    }
}

pub fn line_row(
    title: impl Into<SharedString>,
    caption: impl Into<SharedString>,
    control: impl IntoElement,
) -> LineRow {
    LineRow {
        title: title.into(),
        caption: Some(caption.into()),
        control: Some(control.into_any_element()),
    }
}

/// One figure on the stats page: a short label, the value, and a matching icon.
pub fn stat_tile(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    icon: IconName,
) -> StatTile {
    StatTile {
        label: label.into(),
        value: value.into(),
        icon,
    }
}

#[derive(IntoElement)]
pub struct StatTile {
    label: SharedString,
    value: SharedString,
    icon: IconName,
}

impl RenderOnce for StatTile {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        div()
            .flex()
            .flex_1()
            .flex_col()
            .gap_1()
            .min_w(px(0.0))
            .px_2()
            .py_2()
            .rounded(theme.radius(ely_gpui_component::theme::Radius::Md))
            .bg(colors.sunken)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(theme.text_size(TextSize::Xs))
                            .text_color(colors.fg_muted)
                            .child(self.label),
                    )
                    .child(
                        Icon::new(self.icon)
                            .size(IconSize::Sm)
                            .color(colors.fg_subtle),
                    ),
            )
            .child(
                div()
                    .text_size(theme.text_size(TextSize::Lg))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.fg)
                    .child(self.value),
            )
    }
}

pub fn group_label(title: impl Into<SharedString>) -> GroupLabel {
    GroupLabel {
        title: title.into(),
    }
}

#[derive(IntoElement)]
pub struct GroupLabel {
    title: SharedString,
}

impl RenderOnce for GroupLabel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .text_size(theme.text_size(TextSize::Xs))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.fg_muted)
            .child(self.title)
    }
}

#[derive(IntoElement)]
pub struct LineRow {
    title: SharedString,
    caption: Option<SharedString>,
    control: Option<AnyElement>,
}

impl RenderOnce for LineRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .gap_3()
            .py_1()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(theme.text_size(TextSize::Sm))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.fg)
                            .child(self.title),
                    )
                    .when_some(self.caption, |column, caption| {
                        column.child(
                            div()
                                .text_size(theme.text_size(TextSize::Xs))
                                .text_color(colors.fg_muted)
                                .child(caption),
                        )
                    }),
            )
            .when_some(self.control, |row, control| {
                row.child(div().flex_none().child(control))
            })
    }
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
    // `+` can itself be the shortcut's key ("Ctrl++"), so split off the key
    // from the right and only treat the rest as modifiers.
    let (mods_part, key_part) = if let Some(stripped) = hotkey.strip_suffix("++") {
        (stripped, "+")
    } else if hotkey == "+" {
        ("", "+")
    } else {
        match hotkey.rsplit_once('+') {
            Some((left, right)) if !right.trim().is_empty() => (left, right),
            // A trailing "+" ("Ctrl+") names no key; treat the whole thing
            // as modifiers like the old parser did.
            _ => (hotkey, ""),
        }
    };
    let parts: Vec<&str> = std::iter::once(mods_part)
        .flat_map(|part| part.split('+'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let mut mods: Vec<String> = parts
        .iter()
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "control" => "ctrl".to_string(),
            "windows" => "win".to_string(),
            other => other.to_string(),
        })
        .collect();
    let key = match key_part {
        "Backspace" => "backspace",
        "Tab" => "tab",
        "Enter" => "enter",
        "Space" => "space",
        ";" => ";",
        "-" => "-",
        "+" => "+",
        other => {
            if other.is_empty() {
                return mods.join("-");
            }
            mods.push(other.to_ascii_lowercase());
            return mods.join("-");
        }
    };
    if key == "-" {
        let mut source = mods.join("-");
        if !source.is_empty() {
            source.push('-');
        }
        source.push('-');
        return source;
    }
    mods.push(key.to_string());
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
        assert_eq!(display_to_gpui("Ctrl++"), "ctrl-+");
    }
}
