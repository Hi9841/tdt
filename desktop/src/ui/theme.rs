//! Shared visual tokens. Every surface should pull from here.
#![allow(dead_code)]

use gpui::{
    hsla, linear_color_stop, linear_gradient, px, rgb, rgba, Background, FontWeight, Hsla, Pixels,
};

// Prism dark app tokens, converted from its OKLCH ink ramp to sRGB.
pub const BG: u32 = 0x0a0b0f;
pub const SURFACE: u32 = 0x0e0f13;
pub const SURFACE_RAISED: u32 = 0x1c1e23;
pub const TEXT: u32 = 0xeff0f2;
pub const MUTED: u32 = 0xc6c8cc;
pub const FOAM: u32 = 0x8ec8ff;
pub const IRIS: u32 = 0x987cf2;
pub const GOLD: u32 = 0xf6c177;
pub const LOVE: u32 = 0xffb2c2;
pub const SUCCESS: u32 = 0x7edbb3;
pub const ACCENT: u32 = IRIS;
pub const TOGGLE_ON: u32 = 0xa88dff;
pub const TRACK_OFF: u32 = 0xffffff70;
pub const WELL: u32 = 0xffffff14;
pub const HOVER: u32 = 0xffffff22;
pub const PRESSED: u32 = 0xffffff2c;
pub const SELECTED: u32 = 0x987cf233;
pub const HAIRLINE: u32 = 0xffffff70;
pub const FOCUS: u32 = 0xffffffff;
pub const PRIMARY_HOVER: u32 = 0xa78bfa;
pub const PRIMARY_ACTIVE: u32 = 0x8b6ce8;
// The desktop remains visible through this neutral tint. The gradient supplies
// depth without turning the shell edge into a colored accent line.
pub const SHELL_TOP: u32 = 0x14151bf7;
pub const SHELL_BOTTOM: u32 = 0x090a0ff9;
pub const SHELL_LINE_TOP: u32 = 0xffffff42;
pub const SHELL_LINE_BOTTOM: u32 = 0xffffff2e;

pub const R_WINDOW: f32 = 14.0;
pub const R_SECTION: f32 = 10.0;
pub const R_CHIP: f32 = 8.0;
pub const H_CTRL: f32 = 30.0;
pub const H_TAB: f32 = 24.0;
pub const H_BTN: f32 = 28.0;
pub const H_BTN_PRIMARY: f32 = 30.0;
pub const H_ICON: f32 = 28.0;
pub const TAB_FADE_MS: u64 = 140;
pub const PAD: f32 = 12.0;
pub const GAP_SECTION: f32 = 12.0;
pub const GAP_ROW: f32 = 6.0;
pub const GAP_TIGHT: f32 = 6.0;
pub const TYPE_TITLE: f32 = 16.0;
pub const TYPE_SECTION: f32 = 13.0;
pub const TYPE_LABEL: f32 = 14.0;
pub const TYPE_DESC: f32 = 12.0;
pub const TYPE_META: f32 = 12.0;
pub const TYPE_STAT: f32 = 20.0;
pub const ICON: f32 = 14.0;
pub const TOGGLE_W: f32 = 36.0;
pub const TOGGLE_H: f32 = 20.0;
pub const TOGGLE_KNOB: f32 = 16.0;
pub const METER_H: f32 = 6.0;
pub const MOTION_MS: u64 = 160;

pub fn text() -> Hsla {
    rgb(TEXT).into()
}
pub fn muted() -> Hsla {
    rgb(MUTED).into()
}
pub fn accent() -> Hsla {
    rgb(ACCENT).into()
}
pub fn foam() -> Hsla {
    rgb(FOAM).into()
}
pub fn gold() -> Hsla {
    rgb(GOLD).into()
}
pub fn love() -> Hsla {
    rgb(LOVE).into()
}
pub fn success() -> Hsla {
    rgb(SUCCESS).into()
}
pub fn iris() -> Hsla {
    rgb(IRIS).into()
}
pub fn hover() -> Hsla {
    rgba(HOVER).into()
}
pub fn pressed() -> Hsla {
    rgba(PRESSED).into()
}
pub fn selected() -> Hsla {
    rgba(SELECTED).into()
}
pub fn well() -> Hsla {
    rgba(WELL).into()
}
pub fn hairline() -> Hsla {
    rgba(HAIRLINE).into()
}
pub fn track_off() -> Hsla {
    rgba(TRACK_OFF).into()
}
pub fn toggle_on() -> Hsla {
    rgb(TOGGLE_ON).into()
}
pub fn surface() -> Hsla {
    rgb(SURFACE).into()
}
pub fn surface_raised() -> Hsla {
    rgb(SURFACE_RAISED).into()
}
pub fn shell_border() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgba(SHELL_LINE_TOP), 0.0),
        linear_color_stop(rgba(SHELL_LINE_BOTTOM), 1.0),
    )
}
pub fn shell_surface() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgba(SHELL_TOP), 0.0),
        linear_color_stop(rgba(SHELL_BOTTOM), 1.0),
    )
}
pub fn focus_ring() -> Hsla {
    rgba(FOCUS).into()
}

pub fn r_window() -> Pixels {
    px(R_WINDOW)
}
pub fn r_section() -> Pixels {
    px(R_SECTION)
}
pub fn r_chip() -> Pixels {
    px(R_CHIP)
}
pub fn h_ctrl() -> Pixels {
    px(H_CTRL)
}
pub fn h_btn() -> Pixels {
    px(H_BTN)
}
pub fn h_icon() -> Pixels {
    px(H_ICON)
}
pub fn pad() -> Pixels {
    px(PAD)
}

pub fn medium() -> FontWeight {
    FontWeight::MEDIUM
}
pub fn semibold() -> FontWeight {
    FontWeight::SEMIBOLD
}

pub fn transparent() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.0)
}
