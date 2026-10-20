//! Shared visual tokens. Every surface should pull from here.
//!
//! One black theme, after Aqua Voice: black surfaces, zinc hairlines, white
//! text, and one blue accent. Errors take the only other strong color.

use gpui::{hsla, px, rgb, rgba, Hsla, Pixels};

pub const BG: u32 = 0x000000;
pub const EDGE: u32 = 0x27272a;
pub const EDGE_STRONG: u32 = 0x3f3f46;
pub const TEXT: u32 = 0xfafafa;
pub const MUTED: u32 = 0xa1a1aa;
pub const SUBTLE: u32 = 0x71717a;
pub const DISABLED: u32 = 0x52525b;
pub const SOFT: u32 = 0xffffff9e;
pub const ACCENT: u32 = 0x3b7ddd;
pub const ACCENT_HOVER: u32 = 0x5a93e6;
pub const LINK: u32 = 0x8ab8f5;
pub const WARNING: u32 = 0xf6c177;
pub const DANGER: u32 = 0xfca5a5;
pub const SUCCESS: u32 = 0x7edbb3;
pub const WELL: u32 = 0xffffff0f;
/// Keycaps and chips.
pub const CHIP: u32 = 0xffffff24;
pub const HOVER: u32 = 0xffffff1f;
pub const PRESSED: u32 = 0xffffff2c;
pub const SELECTED: u32 = 0x3b7ddd40;
pub const FOCUS: u32 = 0xffffffff;
// At rest the pill is see-through, with a grey hairline.
pub const REST_BG: u32 = 0x000000a3;
pub const REST_EDGE: u32 = 0x9ca3af99;

pub const R_WINDOW: f32 = 18.0;
pub const R_SECTION: f32 = 12.0;
pub const R_CHIP: f32 = 8.0;
pub const TAB_FADE_MS: u64 = 140;
pub const PAD: f32 = 12.0;
pub const GAP_SECTION: f32 = 12.0;
pub const GAP_TIGHT: f32 = 6.0;
pub const TYPE_TITLE: f32 = 16.0;
pub const TYPE_LABEL: f32 = 14.0;
pub const TYPE_DESC: f32 = 12.0;
pub const TYPE_META: f32 = 12.0;

pub fn bg() -> Hsla {
    rgb(BG).into()
}
pub fn edge() -> Hsla {
    rgb(EDGE).into()
}
pub fn text() -> Hsla {
    rgb(TEXT).into()
}
pub fn muted() -> Hsla {
    rgb(MUTED).into()
}
pub fn soft() -> Hsla {
    rgba(SOFT).into()
}
pub fn danger() -> Hsla {
    rgb(DANGER).into()
}
pub fn well() -> Hsla {
    rgba(WELL).into()
}
pub fn chip() -> Hsla {
    rgba(CHIP).into()
}
pub fn hover() -> Hsla {
    rgba(HOVER).into()
}
pub fn pressed() -> Hsla {
    rgba(PRESSED).into()
}
pub fn rest_bg() -> Hsla {
    rgba(REST_BG).into()
}
pub fn rest_edge() -> Hsla {
    rgba(REST_EDGE).into()
}
pub fn focus_ring() -> Hsla {
    rgba(FOCUS).into()
}

/// Ely components read this palette, so the panel follows the same theme.
pub fn install(cx: &mut gpui::App) {
    use ely_gpui_component::theme::{Density, Mode, Theme};
    let palette = palette();
    Theme::set_mode_now(Mode::Dark, cx);
    Theme::update(cx, |theme| {
        theme.density = Density::Compact;
        // Round enough that buttons and tabs read as pills, like the overlay.
        theme.radius_scale = 3.0;
        theme.colors = palette.clone();
    });
    Theme::set_palette(Mode::Dark, Some(palette), cx);
}

fn palette() -> ely_gpui_component::theme::Palette {
    use ely_gpui_component::theme::Palette;
    let mut palette = Palette::dark(false);
    palette.bg = bg();
    palette.surface = well();
    palette.sunken = well();
    palette.overlay = bg();
    palette.hover = hover();
    palette.active = rgba(PRESSED).into();
    palette.border = edge();
    palette.border_strong = rgb(EDGE_STRONG).into();
    palette.fg = text();
    palette.fg_muted = muted();
    palette.fg_subtle = rgb(SUBTLE).into();
    palette.fg_disabled = rgb(DISABLED).into();
    palette.accent = rgb(ACCENT).into();
    palette.accent_hover = rgb(ACCENT_HOVER).into();
    palette.on_accent = text();
    palette.focus = focus_ring();
    palette.link = rgb(LINK).into();
    palette.selection = rgba(SELECTED).into();
    palette.success = rgb(SUCCESS).into();
    palette.warning = rgb(WARNING).into();
    palette.danger = danger();
    palette.info = rgb(LINK).into();
    palette.glass = bg();
    palette
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
