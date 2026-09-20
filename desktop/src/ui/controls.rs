//! Compact desktop controls shared by the overlay and the panel.
#![allow(dead_code)]

use super::theme::{self, *};
use crate::stt::models;
use gpui::*;

pub fn ghost_button(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .items_center()
        .justify_center()
        .h(h_btn())
        .px(px(10.0))
        .rounded(r_chip())
        .text_size(px(TYPE_META))
        .line_height(px(16.0))
        .font_weight(medium())
        .text_color(muted())
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| {
            s.bg(hover())
                .text_color(text())
                .text_size(px(TYPE_META))
                .font_weight(medium())
        })
        .active(|s| {
            s.bg(pressed())
                .text_color(text())
                .text_size(px(TYPE_META))
                .font_weight(medium())
        })
        .focus(|s| {
            s.bg(pressed())
                .text_color(text())
                .text_size(px(TYPE_META))
                .font_weight(medium())
        })
        .child(label.into())
}

pub fn secondary_button(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .items_center()
        .justify_center()
        .h(h_btn())
        .px(px(12.0))
        .rounded(r_chip())
        .bg(well())
        .text_size(px(TYPE_META))
        .line_height(px(16.0))
        .font_weight(medium())
        .text_color(text())
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(hover()))
        .active(|s| s.bg(pressed()))
        .focus(|s| s.bg(pressed()))
        .child(label.into())
}

pub fn primary_button(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .items_center()
        .justify_center()
        .h(px(H_BTN_PRIMARY))
        .px(px(12.0))
        .rounded(r_chip())
        .bg(rgb(ACCENT))
        .text_size(px(TYPE_META))
        .font_weight(semibold())
        .text_color(rgb(BG))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(PRIMARY_HOVER)))
        .active(|s| s.bg(rgb(PRIMARY_ACTIVE)))
        .focus(|s| s.bg(rgb(PRIMARY_HOVER)))
        .child(label.into())
}

pub fn section_label(title: &'static str) -> Div {
    div()
        .px(px(2.0))
        .text_size(px(TYPE_META))
        .font_weight(semibold())
        .text_color(muted())
        .child(title)
}

pub fn setting_row(
    title: &'static str,
    subtitle: impl Into<SharedString>,
    trailing: AnyElement,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(GAP_TIGHT))
        .w_full()
        .min_h(px(44.0))
        .px(px(2.0))
        .py(px(4.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(TYPE_LABEL))
                        .font_weight(medium())
                        .text_color(text())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(TYPE_DESC))
                        .line_height(px(15.0))
                        .text_color(muted())
                        .child(subtitle.into()),
                ),
        )
        .child(div().flex_none().child(trailing))
}

pub fn grouped_section(title: &'static str, rows: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(GAP_TIGHT))
        .child(section_label(title))
        .child(div().flex().flex_col().w_full().children(rows))
}

pub fn labeled_block(
    title: &'static str,
    subtitle: impl Into<SharedString>,
    body: impl IntoIterator<Item = AnyElement>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(GAP_TIGHT))
        .child(section_label(title))
        .child(
            div()
                .px(px(2.0))
                .text_size(px(TYPE_DESC))
                .line_height(px(15.0))
                .text_color(muted())
                .child(subtitle.into()),
        )
        .children(body)
}

pub fn chip_row(chips: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .flex()
        .w_full()
        .gap(px(GAP_TIGHT))
        .children(chips)
        .into_any_element()
}

pub fn choice_chip(id: ElementId, label: impl Into<SharedString>, is_on: bool) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .h(h_ctrl())
        .px(px(8.0))
        .min_w(px(0.0))
        .rounded(r_chip())
        .bg(theme::transparent())
        .text_size(px(TYPE_DESC))
        .font_weight(FontWeight::NORMAL)
        .text_color(muted())
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.opacity(0.88))
        .active(|s| s.opacity(0.66))
        .focus(|s| s)
        .child(label)
        .child(div().w(px(18.0)).h(px(2.0)).rounded_full().bg(if is_on {
            accent()
        } else {
            theme::transparent()
        }))
}

pub fn chip_well(rows: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(GAP_TIGHT))
        .children(rows)
}

pub fn progress_bar(done: u64, total: u64) -> AnyElement {
    let fraction = if total == 0 {
        0.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    };
    div()
        .w_full()
        .h(px(METER_H))
        .rounded_full()
        .bg(well())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(foam())
                .w(relative(fraction.max(0.02))),
        )
        .into_any_element()
}

pub fn keycap(label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h(px(24.0))
        .px(px(2.0))
        .text_size(px(TYPE_META))
        .line_height(px(16.0))
        .font_family("Consolas")
        .font_weight(medium())
        .text_color(text())
        .whitespace_nowrap()
        .child(label.into())
}

pub fn shortcut_keys(hotkey: &str) -> AnyElement {
    let mut parts = Vec::new();
    for (i, part) in hotkey.split('+').enumerate() {
        if i > 0 {
            parts.push(
                div()
                    .text_size(px(TYPE_META))
                    .line_height(px(16.0))
                    .text_color(muted())
                    .child("+")
                    .into_any_element(),
            );
        }
        parts.push(keycap(part.trim().to_string()).into_any_element());
    }
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .h(px(24.0))
        .px(px(2.0))
        .children(parts)
        .into_any_element()
}

pub fn toggle_hit(id: &'static str, track: Div) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(44.0))
        .min_h(px(44.0))
        .cursor_pointer()
        .rounded(r_chip())
        .hover(|s| s.opacity(0.88))
        .focus(|s| s.opacity(0.78))
        .active(|s| s.opacity(0.72))
        .child(track)
}

pub fn toggle_track(on: bool, knob: AnyElement) -> Div {
    div()
        .flex()
        .items_center()
        .px(px(2.0))
        .w(px(TOGGLE_W))
        .h(px(TOGGLE_H))
        .rounded_full()
        .bg(if on { toggle_on() } else { track_off() })
        .overflow_hidden()
        .child(knob)
}

pub fn toggle_knob() -> Div {
    div()
        .w(px(TOGGLE_KNOB))
        .h(px(TOGGLE_KNOB))
        .rounded_full()
        .bg(rgb(BG))
}

pub fn stat_block(label: &'static str, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(TYPE_META))
                .font_weight(medium())
                .text_color(muted())
                .child(label),
        )
        .child(
            div()
                .text_size(px(TYPE_STAT))
                .line_height(px(24.0))
                .font_family("Consolas")
                .font_weight(semibold())
                .text_color(text())
                .child(value.into()),
        )
}

pub fn download_copy(
    done: u64,
    total: u64,
    file: Option<&str>,
    file_index: usize,
    file_count: usize,
) -> String {
    let pct = models::percent(done, total);
    match file {
        Some(name) if file_count > 1 => format!(
            "{}% - {} of {} - {} - {} of {}",
            pct,
            file_index,
            file_count,
            models::file_label(name),
            models::format_mb(done),
            models::format_mb(total)
        ),
        _ => format!(
            "{}% - {} of {}",
            pct,
            models::format_mb(done),
            models::format_mb(total)
        ),
    }
}
