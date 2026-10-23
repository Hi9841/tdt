//! Ely controls used by the overlay and the panel.
//! Layout helpers stay here so the HUD does not rebuild rows by hand.

use crate::ui::theme;
use ely_gpui_component::buttons::{Button, ButtonVariant, IconButton};
use ely_gpui_component::motion::ProgressBar;
use ely_gpui_component::primitives::{Icon, IconName};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize, TextSize};
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

/// One setting on a single line. The control stays on the right in a narrow panel.
pub fn line_row(
    title: impl Into<SharedString>,
    caption: impl Into<SharedString>,
    control: impl IntoElement,
) -> LineRow {
    LineRow {
        title: title.into(),
        caption: Some(caption.into()),
        control: Some(control.into_any_element()),
        click: None,
    }
}

/// A titled group of settings: the title, then the rows on one card, with a
/// hairline between rows.
pub fn section(title: impl Into<SharedString>) -> Section {
    Section {
        title: title.into(),
        children: Vec::new(),
    }
}

/// Content on a section card that is not a row, aligned with the row text.
pub fn inset() -> Div {
    div().px(px(ROW_PAD_X)).py(px(4.0))
}

const ROW_PAD_X: f32 = 8.0;
const CARD_PAD: f32 = 4.0;

#[derive(IntoElement)]
pub struct Section {
    title: SharedString,
    /// Each child, and whether a hairline separates it from the one before.
    children: Vec<(AnyElement, bool)>,
}

impl Section {
    /// Content that belongs to the row above it (a message, a progress
    /// bar): no hairline between them.
    pub fn note(mut self, element: impl IntoElement) -> Self {
        self.children.push((element.into_any_element(), false));
        self
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children
            .extend(elements.into_iter().map(|element| (element, true)));
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(6.0))
            .child(
                div()
                    .pl(px(CARD_PAD + ROW_PAD_X))
                    .child(group_label(self.title)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .p(px(CARD_PAD))
                    .rounded(theme::r_section())
                    .bg(theme::raised())
                    .border_1()
                    .border_color(theme::edge())
                    .children(self.children.into_iter().enumerate().flat_map(
                        |(index, (element, divided))| {
                            let hairline = (index > 0 && divided).then(|| {
                                div()
                                    .h(px(1.0))
                                    .mx(px(ROW_PAD_X))
                                    .my(px(2.0))
                                    .bg(theme::edge())
                                    .into_any_element()
                            });
                            hairline.into_iter().chain(std::iter::once(element))
                        },
                    )),
            )
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

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct LineRow {
    title: SharedString,
    caption: Option<SharedString>,
    control: Option<AnyElement>,
    click: Option<(&'static str, ClickHandler)>,
}

impl LineRow {
    /// The whole row acts like its control: a larger target, with hover
    /// and press states. A click on the control itself goes only to the
    /// control, so a switch does not toggle twice.
    pub fn on_click(
        mut self,
        id: &'static str,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.click = Some((id, Box::new(handler)));
        self
    }
}

impl RenderOnce for LineRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let row = div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .gap_3()
            .px(px(ROW_PAD_X))
            .py(px(8.0))
            .rounded(crate::ui::theme::r_chip())
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
            );
        let Some((id, handler)) = self.click else {
            return row
                .when_some(self.control, |row, control| {
                    row.child(div().flex_none().child(control))
                })
                .into_any_element();
        };
        row.id(id)
            .cursor_pointer()
            .hover(|style| style.bg(crate::ui::theme::hover()))
            .active(|style| style.bg(crate::ui::theme::pressed()))
            .on_click(move |event, window, cx| handler(event, window, cx))
            .when_some(self.control, |row, control| {
                row.child(
                    div()
                        .id(SharedString::from(format!("{id}:control")))
                        .flex_none()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(control),
                )
            })
            .into_any_element()
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

/// Flat keycaps. `hotkey` is TDT's display form, such as `Ctrl+;`.
pub fn shortcut_keys(hotkey: &str) -> AnyElement {
    let keys = hotkey_parts(hotkey).into_iter().map(|key| {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(18.0))
            .px(px(5.0))
            .rounded(px(4.5))
            .bg(theme::chip())
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::text())
            .child(key.to_string())
    });
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.0))
        .children(keys)
        .into_any_element()
}

/// The keys of a shortcut label. `+` can itself be the key ("Ctrl++").
fn hotkey_parts(hotkey: &str) -> Vec<&str> {
    let (mods, key) = if let Some(mods) = hotkey.strip_suffix("++") {
        (mods, "+")
    } else if hotkey == "+" {
        ("", "+")
    } else {
        hotkey.rsplit_once('+').unwrap_or(("", hotkey))
    };
    mods.split('+')
        .chain(std::iter::once(key))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}
