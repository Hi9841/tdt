use crate::audio::VIS_BARS;
use crate::config::{AppConfig, AppStats};
use crate::hotkey;
use crate::paste::PasteInjector;
use crate::stt::SttEngine;
use crate::ui::text::{clip_text, format_mmss};
use crate::ui::window_util::{
    client_animations_enabled, set_window_mode, start_window_drag, BUBBLE_HEIGHT, BUBBLE_WIDTH,
    PANEL_CLOSE_MS, PANEL_OPEN_MS,
};
use crate::update::{self, UpdatePhase};
use gpui::prelude::FluentBuilder;
use gpui::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

// Rosé Pine Moon
const IRIS: u32 = 0xc4a7e7;
const FOAM: u32 = 0x9ccfd8;
const GOLD: u32 = 0xf6c177;
const LOVE: u32 = 0xeb6f92;
const PINE: u32 = 0x3e8fb0;
const ACCENT: u32 = IRIS;
const SUCCESS: u32 = PINE;
const TEXT: u32 = 0xe0def4;
const TEXT_SECONDARY: u32 = 0xcecae6;
const TEXT_MUTED: u32 = 0xc4bfdb;
const HAIRLINE: u32 = 0xc4a7e73d;
const CARD: u32 = 0x2a283ef0;
const PILL_BG: u32 = 0x232136f5;
const PANEL_BG: u32 = 0x232136fa;
const HOVER: u32 = 0xc4a7e73d;
const SELECTED_FILL: u32 = 0xc4a7e740;
const PILL_OUTLINE: u32 = 0xffffff1a;
/// Settings control: the 44px hit target paints nothing; the visible
/// highlight is an inset child so it never collides with the pill border.
const SETTINGS_HIT_HEIGHT: f32 = 44.0;
const SETTINGS_SURFACE_HEIGHT: f32 = 30.0;
const WAVE_BARS: usize = VIS_BARS;
const WAVE_BAR_W: f32 = 3.0;
const WAVE_GAP: f32 = 2.0;
const WAVE_FLAT: f32 = 2.0;
const WAVE_MAX: f32 = 28.0;
const HOLD_COLOR: u32 = FOAM;
const RELEASE_COLOR: u32 = GOLD;
/// Recent transcripts shown in the stats tab before it scrolls.
const HISTORY_ROWS: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum HudStatus {
    Idle,
    Listening {
        audio_level: f32,
        started_at: Instant,
    },
    Transcribing {
        recorded_for: Duration,
    },
    Success {
        text: String,
        auto_pasted: bool,
        finished_at: Instant,
    },
    Error {
        message: String,
        occurred_at: Instant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowViewMode {
    Bubble,
    StatsAndSettings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsTab {
    Stats,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PanelMotion {
    Opening { started_at: Instant },
    Closing { started_at: Instant },
}

pub struct HudView {
    pub status: HudStatus,
    pub auto_paste_enabled: bool,
    pub hotkey_label: String,
    pub wave_peaks: [f32; WAVE_BARS],
    pub mode: WindowViewMode,
    pub active_tab: SettingsTab,
    pub stats: AppStats,
    /// Timestamp of the history row showing "Copied" feedback. Keyed by the
    /// row's timestamp, not its index, so a new transcript landing mid-view
    /// cannot move the checkmark to the wrong row.
    pub copied_key: Option<String>,
    pub copied_at: Option<Instant>,
    /// Bumped on each auto-paste toggle so the knob animation restarts.
    pub toggle_epoch: u64,
    pub injector: PasteInjector,
    pub selected_language: String,
    pub stt_engine: Option<Arc<SttEngine>>,
    pub auto_paste_state: Arc<Mutex<bool>>,
    panel_motion: Option<PanelMotion>,
    update: Arc<Mutex<UpdatePhase>>,
    update_ping: crossbeam_channel::Sender<()>,
    pub hotkey_capturing: bool,
}

impl HudView {
    pub fn new(
        auto_paste_enabled: bool,
        hotkey_label: String,
        selected_language: String,
        stt_engine: Option<Arc<SttEngine>>,
        auto_paste_state: Arc<Mutex<bool>>,
        update: Arc<Mutex<UpdatePhase>>,
        update_ping: crossbeam_channel::Sender<()>,
    ) -> Self {
        let open_panel = std::env::var_os("VOICE_STT_OPEN_PANEL").is_some();
        if open_panel {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(450));
                set_window_mode(true);
            });
        }

        Self {
            status: HudStatus::Idle,
            auto_paste_enabled,
            hotkey_label,
            wave_peaks: [0.0; WAVE_BARS],
            mode: if open_panel {
                WindowViewMode::StatsAndSettings
            } else {
                WindowViewMode::Bubble
            },
            active_tab: SettingsTab::Stats,
            stats: AppStats::load(),
            copied_key: None,
            copied_at: None,
            toggle_epoch: 0,
            injector: PasteInjector::new(),
            selected_language,
            stt_engine,
            auto_paste_state,
            panel_motion: None,
            update,
            update_ping,
            hotkey_capturing: false,
        }
    }

    pub fn toggle_hotkey_capture(&mut self) {
        if self.hotkey_capturing {
            self.hotkey_capturing = false;
            hotkey::end_capture();
        } else {
            self.hotkey_capturing = true;
            hotkey::begin_capture();
        }
    }

    pub fn start_update_check(&self) {
        // Ignore clicks while a check or download is already running; the
        // shared phase cell has a single writer at a time.
        if matches!(
            &*self.update.lock(),
            UpdatePhase::Checking | UpdatePhase::Downloading
        ) {
            return;
        }
        let cell = Arc::clone(&self.update);
        let ping = self.update_ping.clone();
        let ticket = update::begin_update_check();
        std::thread::spawn(move || {
            *cell.lock() = UpdatePhase::Checking;
            let _ = ping.send(());
            let next = match update::check_latest() {
                Ok(phase) => phase,
                Err(error) => UpdatePhase::Failed(error),
            };
            // A newer user action superseded this check; do not clobber it.
            if update::update_check_is_current(ticket) {
                *cell.lock() = next;
                let _ = ping.send(());
            }
        });
    }

    fn start_update_install(&self) {
        let snapshot = self.update.lock().clone();
        match snapshot {
            UpdatePhase::Available {
                asset_url,
                sums_url,
                ..
            } => {
                let cell = Arc::clone(&self.update);
                let ping = self.update_ping.clone();
                let ticket = update::begin_update_check();
                std::thread::spawn(move || {
                    *cell.lock() = UpdatePhase::Downloading;
                    let _ = ping.send(());
                    let next = match update::download_installer(&asset_url, sums_url.as_deref()) {
                        Ok(path) => match update::launch_installer(&path) {
                            Ok(()) => UpdatePhase::Ready { installer: path },
                            Err(error) => UpdatePhase::Failed(error),
                        },
                        Err(error) => UpdatePhase::Failed(error),
                    };
                    // Only the newest install action may write the phase.
                    if update::update_check_is_current(ticket) {
                        *cell.lock() = next;
                        let _ = ping.send(());
                    }
                });
            }
            UpdatePhase::Ready { installer } => {
                if let Err(error) = update::launch_installer(&installer) {
                    *self.update.lock() = UpdatePhase::Failed(error);
                    let _ = self.update_ping.send(());
                }
            }
            _ => self.start_update_check(),
        }
    }

    pub fn close_stats_settings(&mut self, cx: &mut Context<Self>) {
        if self.mode != WindowViewMode::StatsAndSettings
            && !matches!(self.panel_motion, Some(PanelMotion::Opening { .. }))
        {
            return;
        }
        let started_at = Instant::now();
        self.panel_motion = Some(PanelMotion::Closing { started_at });
        if self.hotkey_capturing {
            self.hotkey_capturing = false;
            hotkey::end_capture();
        }
        set_window_mode(false);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PANEL_CLOSE_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.panel_motion == Some(PanelMotion::Closing { started_at }) {
                    view.mode = WindowViewMode::Bubble;
                    view.panel_motion = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.stats = AppStats::load();
        self.mode = WindowViewMode::StatsAndSettings;
        self.active_tab = SettingsTab::Settings;
        let started_at = Instant::now();
        self.panel_motion = Some(PanelMotion::Opening { started_at });
        set_window_mode(true);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PANEL_OPEN_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.panel_motion == Some(PanelMotion::Opening { started_at }) {
                    view.panel_motion = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn copy_history(&mut self, key: &str, text: &str) {
        let _ = self.injector.copy_to_clipboard(text);
        self.copied_key = Some(key.to_string());
        self.copied_at = Some(Instant::now());
    }

    fn select_language(&mut self, code: &str) {
        self.selected_language = code.to_string();
        if let Some(engine) = &self.stt_engine {
            let _ = engine.set_language(code);
        }
        let mut cfg = AppConfig::load();
        cfg.language = code.to_string();
        let _ = cfg.save();
    }

    fn toggle_auto_paste(&mut self) {
        self.auto_paste_enabled = !self.auto_paste_enabled;
        self.toggle_epoch = self.toggle_epoch.wrapping_add(1);
        let mut cfg = AppConfig::load();
        cfg.auto_paste = self.auto_paste_enabled;
        let _ = cfg.save();
        *self.auto_paste_state.lock() = self.auto_paste_enabled;
    }

    fn clear_recents(&mut self) {
        self.stats.clear_history();
        self.copied_key = None;
        self.copied_at = None;
    }
}

impl Render for HudView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if self.mode == WindowViewMode::StatsAndSettings {
            self.render_stats_settings(cx)
        } else {
            self.render_bubble(cx)
        }
    }
}

impl HudView {
    fn render_bubble(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let waveform_w = WAVE_BARS as f32 * WAVE_BAR_W + (WAVE_BARS as f32 - 1.0) * WAVE_GAP;
        let holding = matches!(&self.status, HudStatus::Listening { .. });
        let released = matches!(&self.status, HudStatus::Transcribing { .. });

        let mut bars = Vec::with_capacity(WAVE_BARS);
        let bar_color = if holding {
            rgb(HOLD_COLOR)
        } else if released {
            rgba(0xf6c17799)
        } else {
            rgba(0x9ccfd899)
        };
        for peak in self.wave_peaks {
            let height = if holding {
                wave_height(peak)
            } else {
                WAVE_FLAT
            };
            bars.push(
                div()
                    .w(px(WAVE_BAR_W))
                    .h(px(height))
                    .rounded_full()
                    .bg(bar_color),
            );
        }

        let waveform = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .items_center()
            .justify_center()
            .gap(px(WAVE_GAP))
            .w(px(waveform_w))
            .h(px(32.0))
            .overflow_hidden()
            .children(bars);

        // Static bars during transcription read as frozen, so pulse them while
        // the model is working.
        let waveform: AnyElement = if released && client_animations_enabled() {
            waveform
                .with_animation(
                    "transcribe_pulse",
                    Animation::new(Duration::from_millis(1100))
                        .repeat()
                        .with_easing(pulse_curve),
                    |this, delta| this.opacity(0.45 + 0.55 * delta),
                )
                .into_any_element()
        } else {
            waveform.into_any_element()
        };

        let center_child = match &self.status {
            HudStatus::Idle => div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgba(0xffffff0a))
                        .text_size(px(12.0))
                        .text_color(rgb(TEXT_SECONDARY))
                        .whitespace_nowrap()
                        .child(format!("Press {}", self.hotkey_label)),
                )
                .into_any_element(),
            HudStatus::Success { text, .. } => overlay_snippet(text, rgb(TEXT)),
            HudStatus::Error { message, .. } => overlay_snippet(message, rgb(LOVE)),
            _ => waveform,
        };

        let state_key = match &self.status {
            HudStatus::Idle => "bubble_ready",
            HudStatus::Listening { .. } => "bubble_listening",
            HudStatus::Transcribing { .. } => "bubble_transcribing",
            HudStatus::Success { .. } => "bubble_success",
            HudStatus::Error { .. } => "bubble_error",
        };
        let center_child = div()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .child(center_child);
        let center_child = if client_animations_enabled() {
            center_child
                .with_animation(
                    state_key,
                    Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                    |element, progress| element.opacity(0.65 + 0.35 * progress),
                )
                .into_any_element()
        } else {
            center_child.into_any_element()
        };

        let (status_label, status_color, status_pulse) = match &self.status {
            HudStatus::Idle => ("Ready", rgb(FOAM), false),
            HudStatus::Listening { .. } => ("Listening", rgb(HOLD_COLOR), true),
            HudStatus::Transcribing { .. } => ("Transcribing", rgb(RELEASE_COLOR), true),
            HudStatus::Success { auto_pasted, .. } => (
                if *auto_pasted { "Pasted" } else { "Copied" },
                rgb(SUCCESS),
                false,
            ),
            HudStatus::Error { .. } => ("Error", rgb(LOVE), false),
        };

        let left_section = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .child(phase_indicator(
                "status_indicator",
                true,
                status_color,
                status_pulse,
            ))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT))
                    .whitespace_nowrap()
                    .child(status_label),
            );

        let timer_str = match &self.status {
            HudStatus::Listening { started_at, .. } => format_mmss(started_at.elapsed().as_secs()),
            HudStatus::Transcribing { recorded_for } => format_mmss(recorded_for.as_secs()),
            _ => String::new(),
        };

        let right_section = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .min_w(px(44.0));

        let right_section = if matches!(&self.status, HudStatus::Idle) {
            right_section.child(
                div()
                    .id("open_settings_btn")
                    .flex()
                    .items_center()
                    .justify_center()
                    // Keep the hit target tall, but inset its painted surface.
                    .h(px(SETTINGS_HIT_HEIGHT))
                    .group("pill_settings")
                    .tab_index(0)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            this.open_settings(cx);
                            cx.notify();
                        }
                    }))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .child(
                        div()
                            .id("pill_settings_surface")
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(SETTINGS_SURFACE_HEIGHT))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            // Reserve the focus border so focus never shifts text.
                            .border_2()
                            .border_color(rgba(0x00000000))
                            .group_hover("pill_settings", |style| style.bg(rgba(0xffffff12)))
                            .group_active("pill_settings", |style| style.bg(rgba(0xffffff20)))
                            .focusable()
                            .in_focus(|style| style.border_color(rgb(FOAM)))
                            .child("Settings"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_settings(cx);
                        cx.notify();
                    })),
            )
        } else if !timer_str.is_empty() {
            right_section.child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(13.0))
                    .font_family("Consolas")
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rgb(TEXT_SECONDARY))
                    .text_right()
                    .whitespace_nowrap()
                    .child(timer_str),
            )
        } else {
            right_section
        };

        div()
            .id("recording_overlay_pill")
            .flex()
            .items_center()
            .justify_between()
            .w(px(BUBBLE_WIDTH))
            .h(px(BUBBLE_HEIGHT))
            .px(px(12.0))
            .gap(px(8.0))
            .rounded(px(14.0))
            .bg(rgba(PILL_BG))
            .border_1()
            .border_color(rgba(PILL_OUTLINE))
            .shadow_xs()
            .overflow_hidden()
            .cursor_move()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, _| {
                    start_window_drag();
                }),
            )
            .child(left_section)
            .child(center_child)
            .child(right_section)
            .into_any_element()
    }

    fn render_stats_settings(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let tab_btn = |id: &'static str,
                       label: &'static str,
                       active: bool,
                       tab: SettingsTab,
                       cx: &mut Context<'_, Self>| {
            div()
                .id(id)
                .tab_index(0)
                .border_2()
                .border_color(rgba(0x00000000))
                .focus(|style| style.border_color(rgb(FOAM)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.active_tab = tab;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .flex_none()
                .h(px(44.0))
                .px(px(10.0))
                .rounded_lg()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .bg(if active {
                    rgba(0x393552cc)
                } else {
                    rgba(0x00000000)
                })
                .text_color(if active { rgb(TEXT) } else { rgb(TEXT_MUTED) })
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|style| if active { style } else { style.bg(rgba(HOVER)) })
                .active(|style| style.opacity(0.92))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_tab = tab;
                    cx.notify();
                }))
        };

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .min_w(px(0.0))
            .gap(px(8.0))
            .pb(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(8.0))
                    .cursor_move()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, event: &MouseDownEvent, _window, _| {
                            if event.click_count == 1 {
                                start_window_drag();
                            }
                        }),
                    )
                    .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgb(ACCENT)))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT))
                            .whitespace_nowrap()
                            .child("TDT"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(2.0))
                            .bg(rgba(0x2a283ecc))
                            .p(px(3.0))
                            .rounded_lg()
                            .child(tab_btn(
                                "tab_stats",
                                "Stats",
                                self.active_tab == SettingsTab::Stats,
                                SettingsTab::Stats,
                                cx,
                            ))
                            .child(tab_btn(
                                "tab_settings",
                                "Settings",
                                self.active_tab == SettingsTab::Settings,
                                SettingsTab::Settings,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .id("close_settings_btn")
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .h(px(44.0))
                            .min_w(px(44.0))
                            .px(px(8.0))
                            .rounded_lg()
                            .tab_index(0)
                            .border_2()
                            .border_color(rgba(0x00000000))
                            .focus(|style| style.border_color(rgb(FOAM)))
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.close_stats_settings(cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT_SECONDARY))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgba(HOVER)).text_color(rgb(TEXT)))
                            .active(|style| style.opacity(0.92))
                            .child("Close")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_stats_settings(cx);
                                cx.notify();
                            })),
                    ),
            );

        let content = match self.active_tab {
            SettingsTab::Stats => self.render_stats_tab(cx),
            SettingsTab::Settings => self.render_settings_tab(cx),
        };

        let tab_id = match self.active_tab {
            SettingsTab::Stats => "panel_tab_stats",
            SettingsTab::Settings => "panel_tab_settings",
        };
        let faded_content = div()
            .id("stats_settings_body")
            .flex()
            .flex_col()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(content);
        let faded_content = if client_animations_enabled() {
            faded_content
                .with_animation(
                    tab_id,
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                    |this, delta| this.opacity(0.82 + 0.18 * delta),
                )
                .into_any_element()
        } else {
            faded_content.into_any_element()
        };

        div()
            .id("stats_settings_container")
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .px(px(12.0))
            .pt(px(12.0))
            .pb(px(12.0))
            .bg(rgba(PANEL_BG))
            .border_1()
            .border_color(rgba(HAIRLINE))
            .rounded(px(14.0))
            .shadow_xs()
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && !this.hotkey_capturing {
                    this.close_stats_settings(cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(header)
            .child(faded_content)
            .into_any_element()
    }

    fn render_stats_tab(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let make_tile = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(6.0))
                .px(px(12.0))
                .py(px(12.0))
                .bg(rgba(CARD))
                .rounded_lg()
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT_MUTED))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(18.0))
                        .line_height(px(22.0))
                        .font_family("Consolas")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(TEXT))
                        .child(value),
                )
        };

        let tile_words = make_tile("Words", format!("{}", self.stats.total_words));
        let tile_time = make_tile("Spoken", format!("{:.1}s", self.stats.total_seconds));
        let tile_lat = make_tile("Latency", format!("{}ms", self.stats.last_latency_ms));
        let tile_count = make_tile("Sessions", format!("{}", self.stats.total_transcriptions));

        let grid = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap(px(8.0))
                    .child(tile_words)
                    .child(tile_time),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap(px(8.0))
                    .child(tile_lat)
                    .child(tile_count),
            );

        let mut history_items = Vec::new();
        if self.stats.history.is_empty() {
            history_items.push(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .px(px(12.0))
                    .py(px(24.0))
                    .rounded_lg()
                    .bg(rgba(CARD))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("No transcripts yet"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!("Tap or hold {} to talk.", self.hotkey_label)),
                    )
                    .into_any_element(),
            );
        } else {
            for (idx, item) in self.stats.history.iter().take(HISTORY_ROWS).enumerate() {
                let text_val = item.text.clone();
                let is_copied = self.copied_key.as_deref() == Some(item.timestamp.as_str());
                let btn_text = if is_copied { "Copied" } else { "Copy" };
                let copy_tx = text_val.clone();
                let row_tx = text_val.clone();
                let click_key = item.timestamp.clone();
                let btn_key = click_key.clone();

                history_items.push(
                    div()
                        .id(ElementId::NamedInteger("history_row".into(), idx as u64))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .px(px(10.0))
                        .py(px(8.0))
                        .bg(rgba(CARD))
                        .rounded_lg()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(HOVER)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.copy_history(&click_key, &row_tx);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_family("Consolas")
                                        .text_color(rgb(TEXT_MUTED))
                                        .whitespace_nowrap()
                                        .child(format!(
                                            "{}  ·  {}ms",
                                            item.timestamp, item.latency_ms
                                        )),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(rgb(TEXT))
                                        .line_clamp(2)
                                        .child(text_val),
                                ),
                        )
                        .child({
                            let anim_key = if is_copied { "copied_btn" } else { "copy_btn" };
                            div()
                                .id(ElementId::NamedInteger(anim_key.into(), idx as u64))
                                .flex_none()
                                .h(px(30.0))
                                .px(px(10.0))
                                .rounded_lg()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .whitespace_nowrap()
                                .bg(if is_copied {
                                    rgba(0x3e8fb033)
                                } else {
                                    rgba(0xe0def412)
                                })
                                .text_color(if is_copied {
                                    rgb(SUCCESS)
                                } else {
                                    rgb(TEXT_SECONDARY)
                                })
                                .cursor_pointer()
                                .hover(|style| {
                                    if is_copied {
                                        style
                                    } else {
                                        style.bg(rgba(0xe0def418))
                                    }
                                })
                                .active(|style| style.opacity(0.92))
                                .child(btn_text)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.copy_history(&btn_key, &copy_tx);
                                    cx.notify();
                                }))
                                .with_animation(
                                    ElementId::NamedInteger(
                                        if is_copied {
                                            "copied_fade"
                                        } else {
                                            "copy_fade"
                                        }
                                        .into(),
                                        idx as u64,
                                    ),
                                    Animation::new(Duration::from_millis(120))
                                        .with_easing(ease_out_quint()),
                                    |this, delta| this.opacity(0.35 + 0.65 * delta),
                                )
                                .into_any_element()
                        })
                        .into_any_element(),
                );
            }
        }

        let has_recents = !self.stats.history.is_empty();
        let total_recents = self.stats.history.len();
        let shown_recents = total_recents.min(HISTORY_ROWS);
        let recents_count = if total_recents > shown_recents {
            format!("{shown_recents} of {total_recents}")
        } else {
            total_recents.to_string()
        };
        let mut history_header = div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT_MUTED))
                            .child("Recent"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_family("Consolas")
                            .text_color(rgb(TEXT_MUTED))
                            .child(recents_count),
                    ),
            );
        if has_recents {
            history_header = history_header.child(
                div()
                    .id("clear_recents_btn")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .h(px(30.0))
                    .px(px(10.0))
                    .rounded_lg()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .bg(rgba(0xe0def412))
                    .text_color(rgb(TEXT_SECONDARY))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(HOVER)).text_color(rgb(TEXT)))
                    .active(|style| style.opacity(0.92))
                    .child("Clear recents")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_recents();
                        cx.notify();
                    })),
            );
        }

        let history_section = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pt(px(16.0))
            .pb(px(4.0))
            .child(history_header)
            .children(history_items);

        div()
            .flex()
            .flex_col()
            .child(grid)
            .child(history_section)
            .into_any_element()
    }

    fn render_settings_tab(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let is_auto_paste = self.auto_paste_enabled;
        let hotkey_str = self.hotkey_label.clone();

        let settings_card = |title: &'static str, subtitle: String, trailing: AnyElement| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .w_full()
                .px(px(12.0))
                .py(px(10.0))
                .bg(rgba(CARD))
                .rounded_lg()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .flex_1()
                        .min_w(px(128.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .line_height(px(14.0))
                                .text_color(rgb(TEXT_MUTED))
                                .line_clamp(2)
                                .child(subtitle),
                        ),
                )
                .child(div().flex_none().child(trailing))
        };

        let on = is_auto_paste;
        let epoch = self.toggle_epoch;
        let animate_toggle = epoch > 0 && client_animations_enabled();
        let knob = div().w(px(16.0)).h(px(16.0)).rounded_full().bg(rgb(TEXT));
        let knob = if animate_toggle {
            knob.with_animation(
                ElementId::NamedInteger("toggle_knob".into(), epoch),
                Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                move |knob, progress| knob.ml(px(toggle_offset(on, progress))),
            )
            .into_any_element()
        } else {
            knob.ml(px(toggle_offset(on, 1.0))).into_any_element()
        };
        let toggle_track = div()
            .id("auto_paste_toggle")
            .flex()
            .flex_none()
            .items_center()
            .w(px(36.0))
            .h(px(20.0))
            .rounded_full()
            .bg(if is_auto_paste {
                rgb(ACCENT)
            } else {
                rgb(0x393552)
            })
            .cursor_pointer()
            .overflow_hidden()
            .child(knob)
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_auto_paste();
                cx.notify();
            }));

        let toggle_track = div()
            .id("auto_paste_hit_target")
            .group("auto_paste")
            .tab_index(0)
            .flex()
            .items_center()
            .justify_center()
            .w(px(44.0))
            .h(px(44.0))
            .rounded(px(10.0))
            .border_2()
            .border_color(rgba(0x00000000))
            .focus(|style| style.border_color(rgb(FOAM)))
            .hover(|style| style.bg(rgba(0xffffff0a)))
            .active(|style| style.bg(rgba(0xffffff14)))
            .cursor_pointer()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.toggle_auto_paste();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_auto_paste();
                cx.notify();
            }))
            .child(toggle_track);

        let auto_paste_row = settings_card(
            "Auto-paste",
            "Insert text into the focused app".to_string(),
            toggle_track.into_any_element(),
        );

        let languages = [
            ("auto", "Auto"),
            ("en", "English"),
            ("zh", "Chinese"),
            ("ja", "Japanese"),
            ("ko", "Korean"),
            ("yue", "Cantonese"),
        ];

        let mut lang_buttons = Vec::new();
        for (i, (code, label)) in languages.iter().enumerate() {
            let is_selected = self.selected_language == *code;
            let code_str = code.to_string();
            let key_code = code_str.clone();

            lang_buttons.push(
                div()
                    .id(ElementId::NamedInteger("lang_btn".into(), i as u64))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .h(px(44.0))
                    .px(px(4.0))
                    .min_w(px(0.0))
                    .rounded_lg()
                    .border_2()
                    .border_color(if is_selected {
                        rgba(0xc4a7e780)
                    } else {
                        rgba(0x00000000)
                    })
                    .tab_index(0)
                    .focus(|style| style.border_color(rgb(FOAM)))
                    .text_size(px(12.0))
                    .font_weight(if is_selected {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .bg(if is_selected {
                        rgba(SELECTED_FILL)
                    } else {
                        rgba(0x39355299)
                    })
                    .text_color(if is_selected {
                        rgb(IRIS)
                    } else {
                        rgb(TEXT_SECONDARY)
                    })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|style| {
                        if is_selected {
                            style
                        } else {
                            style.bg(rgba(HOVER))
                        }
                    })
                    .active(|style| style.opacity(0.92))
                    .child(*label)
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.select_language(&key_code);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_language(&code_str);
                        cx.notify();
                    })),
            );
        }

        let language_row = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(10.0))
            .px(px(12.0))
            .py(px(12.0))
            .bg(rgba(CARD))
            .rounded_lg()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child("Language"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(TEXT_MUTED))
                            .child("Auto-detect, or lock to one language"),
                    ),
            )
            .child({
                let mut rows = Vec::new();
                let mut row = Vec::new();
                for button in lang_buttons {
                    row.push(button);
                    if row.len() == 3 {
                        rows.push(
                            div()
                                .flex()
                                .w_full()
                                .gap(px(8.0))
                                .children(std::mem::take(&mut row))
                                .into_any_element(),
                        );
                    }
                }
                if !row.is_empty() {
                    rows.push(
                        div()
                            .flex()
                            .w_full()
                            .gap(px(8.0))
                            .children(row)
                            .into_any_element(),
                    );
                }
                div().flex().flex_col().w_full().gap(px(8.0)).children(rows)
            });

        let capturing = self.hotkey_capturing;
        let hotkey_sub = if capturing {
            "Press the new shortcut. Esc cancels.".to_string()
        } else {
            "Click the chip to change it.".to_string()
        };
        let hotkey_chip = div()
            .id("hotkey_bind_btn")
            .flex()
            .items_center()
            .justify_center()
            .h(px(32.0))
            .px(px(10.0))
            .rounded_lg()
            .border_1()
            .border_color(rgba(0x00000000))
            .when(capturing, |chip| chip.border_color(rgba(0xc4a7e7aa)))
            .bg(rgba(if capturing { SELECTED_FILL } else { 0x393552cc }))
            .text_size(px(11.0))
            .font_family("Consolas")
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if capturing { IRIS } else { FOAM }))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|style| {
                if capturing {
                    style
                } else {
                    style.bg(rgba(HOVER))
                }
            })
            .active(|style| style.opacity(0.92))
            .child(if capturing {
                "Press shortcut".to_string()
            } else {
                hotkey_str
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_hotkey_capture();
                cx.notify();
            }));

        // Pulse only while listening for keys, so the chip advertises the state
        // that is swallowing the keyboard.
        let hotkey_chip: AnyElement = if capturing && client_animations_enabled() {
            hotkey_chip
                .with_animation(
                    "hotkey_capture_pulse",
                    Animation::new(Duration::from_millis(1100))
                        .repeat()
                        .with_easing(pulse_curve),
                    |this, delta| this.opacity(0.72 + 0.28 * delta),
                )
                .into_any_element()
        } else {
            hotkey_chip.into_any_element()
        };

        let hotkey_row = settings_card("Hotkey", hotkey_sub, hotkey_chip);

        let version_row = settings_card(
            "TDT",
            "Talk Don't Type".to_string(),
            div()
                .px(px(8.0))
                .py(px(5.0))
                .rounded_lg()
                .bg(rgba(0x393552cc))
                .text_size(px(11.0))
                .font_family("Consolas")
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(FOAM))
                .whitespace_nowrap()
                .child(update::current_version())
                .into_any_element(),
        );

        let phase = self.update.lock().clone();
        let (update_title, update_sub, update_action, update_busy) = match &phase {
            UpdatePhase::Idle => (
                "Updates",
                "Checks GitHub when you ask".to_string(),
                "Check for updates",
                false,
            ),
            UpdatePhase::Checking => (
                "Updates",
                "Checking GitHub...".to_string(),
                "Checking",
                true,
            ),
            UpdatePhase::UpToDate => (
                "Updates",
                "You are on the latest version".to_string(),
                "Check again",
                false,
            ),
            UpdatePhase::Available { version, .. } => (
                "Updates",
                format!("Version {version} is available"),
                "Install update",
                false,
            ),
            UpdatePhase::Downloading => (
                "Updates",
                "Downloading installer...".to_string(),
                "Downloading",
                true,
            ),
            UpdatePhase::Ready { .. } => (
                "Updates",
                "Installer ready".to_string(),
                "Install update",
                false,
            ),
            UpdatePhase::Failed(error) => ("Updates", clip_text(error, 48), "Try again", false),
        };

        let update_row = settings_card(
            update_title,
            update_sub,
            div()
                .id("check_updates_btn")
                .flex()
                .items_center()
                .justify_center()
                .h(px(30.0))
                .px(px(10.0))
                .rounded_lg()
                .bg(rgba(if update_busy { 0x39355299 } else { HOVER }))
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|style| {
                    if update_busy {
                        style
                    } else {
                        style.bg(rgba(SELECTED_FILL))
                    }
                })
                .active(|style| style.opacity(0.92))
                .child(update_action)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if matches!(
                        *this.update.lock(),
                        UpdatePhase::Checking | UpdatePhase::Downloading
                    ) {
                        return;
                    }
                    this.start_update_install();
                    cx.notify();
                }))
                .into_any_element(),
        );

        let engine_row = settings_card(
            "Model",
            "SenseVoice Small".to_string(),
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(5.0))
                .rounded_lg()
                .bg(rgba(0x3e8fb022))
                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(rgb(SUCCESS)))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(SUCCESS))
                        .whitespace_nowrap()
                        .child("On demand"),
                )
                .into_any_element(),
        );

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(8.0))
            .child(auto_paste_row)
            .child(language_row)
            .child(hotkey_row)
            .child(engine_row)
            .child(version_row)
            .child(update_row)
            .into_any_element()
    }
}

// A full cosine cycle has matching values and slopes at the repeat boundary.
fn pulse_curve(delta: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::TAU * delta).cos()
}

fn toggle_offset(on: bool, progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    if on {
        2.0 + 16.0 * progress
    } else {
        18.0 - 16.0 * progress
    }
}

fn wave_height(peak: f32) -> f32 {
    let driven = (1.0 - (-peak * 16.0).exp()).clamp(0.0, 1.0);
    WAVE_FLAT + (WAVE_MAX - WAVE_FLAT) * driven
}

fn phase_indicator(id: &'static str, on: bool, color: impl Into<Hsla>, pulse: bool) -> AnyElement {
    let color = color.into();
    let dot = div()
        .id(id)
        .flex_none()
        .w(px(8.0))
        .h(px(8.0))
        .rounded_full()
        .bg(if on { color } else { rgba(0x6e6a8688).into() });
    if on && pulse && client_animations_enabled() {
        dot.with_animation(
            id,
            Animation::new(Duration::from_millis(1200))
                .repeat()
                .with_easing(pulse_curve),
            |this, delta| this.opacity(0.55 + 0.45 * delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

fn overlay_snippet(text: &str, color: impl Into<Hsla>) -> AnyElement {
    let color = color.into();
    div()
        .flex()
        .flex_none()
        .items_center()
        .w(px(
            WAVE_BARS as f32 * WAVE_BAR_W + (WAVE_BARS as f32 - 1.0) * WAVE_GAP
        ))
        .h(px(32.0))
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .text_size(px(11.0))
                .line_height(px(14.0))
                .text_color(color)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(clip_text(text, 22)),
        )
        .into_any_element()
}

// Compile-time invariant: the painted settings surface must stay inset within
// the pill so its highlight never collides with the border and 14px corners.
const _: () = assert!(SETTINGS_SURFACE_HEIGHT < SETTINGS_HIT_HEIGHT);
const _: () = assert!(SETTINGS_SURFACE_HEIGHT <= BUBBLE_HEIGHT - 14.0);

#[cfg(test)]
mod motion_tests {
    use super::{pulse_curve, toggle_offset};

    #[test]
    fn pulse_is_continuous_at_repeat_boundary() {
        assert!((pulse_curve(0.0) - pulse_curve(1.0)).abs() < 1e-6);
        assert!((pulse_curve(0.5) - 1.0).abs() < 1e-6);
        for step in 0..=100 {
            assert!((0.0..=1.0).contains(&pulse_curve(step as f32 / 100.0)));
        }
    }

    #[test]
    fn toggle_finishes_inside_track_in_both_directions() {
        assert_eq!(toggle_offset(true, 0.0), 2.0);
        assert_eq!(toggle_offset(true, 1.0), 18.0);
        assert_eq!(toggle_offset(false, 0.0), 18.0);
        assert_eq!(toggle_offset(false, 1.0), 2.0);
        for step in 0..=100 {
            let progress = step as f32 / 100.0;
            for on in [true, false] {
                assert!((2.0..=18.0).contains(&toggle_offset(on, progress)));
            }
        }
    }
}
