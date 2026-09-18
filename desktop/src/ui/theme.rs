//! Shared visual tokens. Every surface should pull from here.
#![allow(dead_code)]

use gpui::{hsla, px, rgb, rgba, FontWeight, Hsla, Pixels};

pub const BG: u32 = 0x191724;
pub const SURFACE: u32 = 0x232136;
pub const SURFACE_RAISED: u32 = 0x2a273f;
pub const TEXT: u32 = 0xe0def4;
pub const MUTED: u32 = 0xa6a1bb;
pub const FOAM: u32 = 0x9ccfd8;
pub const IRIS: u32 = 0xc4a7e7;
pub const GOLD: u32 = 0xf6c177;
pub const LOVE: u32 = 0xeb6f92;
pub const PINE: u32 = 0x3e8fb0;
pub const SUCCESS: u32 = PINE;
pub const ACCENT: u32 = FOAM;
pub const TRACK_OFF: u32 = 0x6e6a86;
pub const WELL: u32 = 0xffffff0c;
pub const HOVER: u32 = 0xffffff14;
pub const PRESSED: u32 = 0xffffff1a;
pub const SELECTED: u32 = 0x9ccfd838;
pub const HAIRLINE: u32 = 0xffffff28;
pub const PILL_BG: u32 = 0x232136f5;
pub const PANEL_BG: u32 = 0x232136fa;

pub const R_WINDOW: f32 = 16.0;
pub const R_SECTION: f32 = 12.0;
pub const R_CHIP: f32 = 8.0;
pub const H_CTRL: f32 = 26.0;
pub const H_TAB: f32 = 24.0;
pub const H_BTN: f32 = 26.0;
pub const H_BTN_PRIMARY: f32 = 28.0;
pub const H_ICON: f32 = 26.0;
pub const TAB_FADE_MS: u64 = 140;
pub const PAD: f32 = 12.0;
pub const GAP_SECTION: f32 = 12.0;
pub const GAP_ROW: f32 = 6.0;
pub const GAP_TIGHT: f32 = 6.0;
pub const TYPE_TITLE: f32 = 16.0;
pub const TYPE_SECTION: f32 = 13.0;
pub const TYPE_LABEL: f32 = 13.0;
pub const TYPE_DESC: f32 = 11.0;
pub const TYPE_META: f32 = 11.0;
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
    rgb(TRACK_OFF).into()
}
pub fn surface() -> Hsla {
    rgb(SURFACE).into()
}
pub fn surface_raised() -> Hsla {
    rgb(SURFACE_RAISED).into()
}
pub fn pill_bg() -> Hsla {
    rgba(PILL_BG).into()
}
pub fn panel_bg() -> Hsla {
    rgba(PANEL_BG).into()
}
pub fn focus_ring() -> Hsla {
    rgb(FOAM).into()
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
