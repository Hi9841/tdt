//! Shared visual tokens. Every surface should pull from here.

use gpui::{hsla, linear_color_stop, linear_gradient, px, rgb, rgba, Background, Hsla, Pixels};

// Prism dark app tokens, converted from its OKLCH ink ramp to sRGB.
pub const BG: u32 = 0x0a0b0f;
pub const TEXT: u32 = 0xeff0f2;
pub const MUTED: u32 = 0xc6c8cc;
pub const FOAM: u32 = 0x8ec8ff;
pub const IRIS: u32 = 0x987cf2;
pub const GOLD: u32 = 0xf6c177;
pub const LOVE: u32 = 0xffb2c2;
pub const SUCCESS: u32 = 0x7edbb3;
pub const ACCENT: u32 = IRIS;
pub const WELL: u32 = 0xffffff14;
pub const HOVER: u32 = 0xffffff22;
pub const PRESSED: u32 = 0xffffff2c;
pub const SELECTED: u32 = 0x987cf233;
pub const FOCUS: u32 = 0xffffffff;
pub const PRIMARY_HOVER: u32 = 0xa78bfa;
// The desktop remains visible through this neutral tint. The gradient supplies
// depth without turning the shell edge into a colored accent line.
pub const SHELL_TOP: u32 = 0x14151bf7;
pub const SHELL_BOTTOM: u32 = 0x090a0ff9;

pub const R_WINDOW: f32 = 14.0;
pub const R_SECTION: f32 = 10.0;
pub const R_CHIP: f32 = 8.0;
pub const TAB_FADE_MS: u64 = 140;
pub const PAD: f32 = 12.0;
pub const GAP_SECTION: f32 = 12.0;
pub const GAP_TIGHT: f32 = 6.0;
pub const TYPE_TITLE: f32 = 16.0;
pub const TYPE_LABEL: f32 = 14.0;
pub const TYPE_DESC: f32 = 12.0;
pub const TYPE_META: f32 = 12.0;
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
pub fn selected() -> Hsla {
    rgba(SELECTED).into()
}
pub fn well() -> Hsla {
    rgba(WELL).into()
}
pub fn shell_surface() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgba(SHELL_TOP), 0.0),
        linear_color_stop(rgba(SHELL_BOTTOM), 1.0),
    )
}

/// Ely components read this palette. The shell itself stays the translucent gradient.
pub fn install(cx: &mut gpui::App) {
    use ely_gpui_component::theme::{Density, Mode, Theme};
    let palette = prism_palette();
    Theme::set_mode_now(Mode::Dark, cx);
    Theme::update(cx, |theme| {
        theme.density = Density::Compact;
        theme.colors = palette.clone();
    });
    Theme::set_palette(Mode::Dark, Some(palette), cx);
}

fn prism_palette() -> ely_gpui_component::theme::Palette {
    use ely_gpui_component::theme::Palette;
    let mut palette = Palette::dark(false);
    palette.bg = rgb(BG).into();
    palette.surface = rgba(WELL).into();
    palette.sunken = rgba(HOVER).into();
    palette.overlay = rgba(HOVER).into();
    palette.hover = rgba(HOVER).into();
    palette.active = rgba(PRESSED).into();
    palette.border = rgba(0xffffff22).into();
    palette.border_strong = rgba(0xffffff38).into();
    palette.fg = text();
    palette.fg_muted = muted();
    palette.fg_subtle = rgb(0x9aa0aa).into();
    palette.fg_disabled = rgb(0x6e737c).into();
    palette.accent = accent();
    palette.accent_hover = rgb(PRIMARY_HOVER).into();
    palette.on_accent = rgb(BG).into();
    palette.focus = focus_ring();
    palette.link = foam();
    palette.selection = selected();
    palette.success = success();
    palette.warning = gold();
    palette.danger = love();
    palette.info = foam();
    palette.glass = rgba(SHELL_TOP).into();
    palette
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
pub fn pad() -> Pixels {
    px(PAD)
}

pub fn transparent() -> Hsla {
    hsla(0.0, 0.0, 0.0, 0.0)
}
