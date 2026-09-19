use crate::audio::VIS_BARS;
use crate::config::{AppConfig, AppStats};
use crate::hotkey;
use crate::paste::PasteInjector;
use crate::stt::{models, DownloadPhase, SharedEngine, SttEngine, CATALOG, DEFAULT_MODEL_ID};
use crate::ui::controls;
use crate::ui::preview::{self, Spec as PreviewSpec};
use crate::ui::text::{clip_text, format_mmss, format_time_saved};
use crate::ui::theme::{
    self, accent, foam, focus_ring, gold, hairline, hover, love, muted, pad, panel_bg, pill_bg,
    r_chip, r_section, r_window, selected, success, text, well, GAP_SECTION, GAP_TIGHT, H_CTRL,
    H_TAB, MOTION_MS, MUTED, TAB_FADE_MS, TEXT, TYPE_DESC, TYPE_LABEL, TYPE_META, TYPE_TITLE,
};
use crate::ui::window_util::{
    client_animations_enabled, set_overlay_hidden, set_window_mode, start_window_drag,
    BUBBLE_HEIGHT, BUBBLE_WIDTH, PANEL_CLOSE_MS, PANEL_OPEN_MS,
};
use crate::update::{self, UpdatePhase};
use gpui::prelude::FluentBuilder;
use gpui::*;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Settings control: the hit target paints nothing; the visible
/// highlight is an inset child so it never collides with the pill border.
const SETTINGS_HIT_HEIGHT: f32 = 40.0;
const SETTINGS_SURFACE_HEIGHT: f32 = 28.0;
const WAVE_BARS: usize = VIS_BARS;
const WAVE_BAR_W: f32 = 3.0;
const WAVE_GAP: f32 = 2.0;
const WAVE_FLAT: f32 = 2.0;
const WAVE_MAX: f32 = 22.0;
/// Two clamped lines at 12px in this panel hold about this many characters.
const HISTORY_EXPAND_CHARS: usize = 88;

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
    autostart_enabled: bool,
    autostart_error: Option<String>,
    pub hotkey_label: String,
    pub wave_peaks: [f32; WAVE_BARS],
    pub mode: WindowViewMode,
    pub active_tab: SettingsTab,
    pub stats: AppStats,
    /// Timestamp of the history row showing "Copied" feedback. Keyed by the
    /// row's timestamp, not its index, so a new transcript landing mid-view
    /// cannot move the checkmark to the wrong row.
    pub copied_key: Option<String>,
    expanded_history_key: Option<String>,
    pub copied_at: Option<Instant>,
    /// Bumped on each auto-paste toggle so the knob animation restarts.
    pub toggle_epoch: u64,
    pub injector: PasteInjector,
    pub selected_language: String,
    pub selected_model: String,
    pub stt_engine: SharedEngine,
    pub auto_paste_state: Arc<Mutex<bool>>,
    panel_motion: Option<PanelMotion>,
    update: Arc<Mutex<UpdatePhase>>,
    update_ping: crossbeam_channel::Sender<()>,
    model_phase: Arc<Mutex<DownloadPhase>>,
    model_error: Option<String>,
    pub hotkey_capturing: bool,
}

impl HudView {
    pub fn new(
        auto_paste_enabled: bool,
        hotkey_label: String,
        selected_language: String,
        stt_engine: SharedEngine,
        auto_paste_state: Arc<Mutex<bool>>,
        update: Arc<Mutex<UpdatePhase>>,
        update_ping: crossbeam_channel::Sender<()>,
    ) -> Self {
        let open_panel =
            std::env::var_os("VOICE_STT_OPEN_PANEL").is_some() || preview::wants_panel();
        if open_panel {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(450));
                set_window_mode(true);
            });
        }

        let mut view = Self {
            status: HudStatus::Idle,
            auto_paste_enabled,
            autostart_enabled: crate::autostart::is_enabled(),
            autostart_error: None,
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
            expanded_history_key: None,
            copied_at: None,
            toggle_epoch: 0,
            injector: PasteInjector::new(),
            selected_language,
            selected_model: models::resolve(&AppConfig::load().model_id).id.to_string(),
            stt_engine,
            auto_paste_state,
            panel_motion: None,
            update,
            update_ping,
            model_phase: Arc::new(Mutex::new(DownloadPhase::Idle)),
            model_error: None,
            hotkey_capturing: false,
        };
        view.apply_preview_state();
        view
    }

    fn apply_preview_state(&mut self) {
        let Some(spec) = preview::parse() else {
            return;
        };
        let is_panel = preview::wants_panel();
        if is_panel {
            self.mode = WindowViewMode::StatsAndSettings;
        } else {
            self.mode = WindowViewMode::Bubble;
        }

        self.auto_paste_enabled = true;
        self.autostart_enabled = false;
        self.autostart_error = None;
        self.selected_language = "auto".into();
        self.selected_model = DEFAULT_MODEL_ID.into();
        self.hotkey_capturing = false;
        *self.model_phase.lock() = DownloadPhase::Idle;
        self.model_error = None;
        self.copied_key = None;
        self.expanded_history_key = None;
        self.copied_at = None;
        self.wave_peaks = [0.0; WAVE_BARS];
        self.status = HudStatus::Idle;
        self.active_tab = if is_panel {
            SettingsTab::Settings
        } else {
            SettingsTab::Stats
        };
        self.stats = AppStats::default();

        match spec {
            PreviewSpec::BubbleIdle => {}
            PreviewSpec::BubbleListening { loud } => {
                self.status = HudStatus::Listening {
                    audio_level: if loud { 0.8 } else { 0.12 },
                    started_at: Instant::now() - Duration::from_secs(12),
                };
                self.wave_peaks = preview_wave(loud);
            }
            PreviewSpec::BubbleTranscribing => {
                self.status = HudStatus::Transcribing {
                    recorded_for: Duration::from_secs(8),
                };
                self.wave_peaks = [0.08; WAVE_BARS];
            }
            PreviewSpec::BubbleSuccess { pasted } => {
                self.status = HudStatus::Success {
                    text: "this is a sample transcript".into(),
                    auto_pasted: pasted,
                    finished_at: Instant::now(),
                };
            }
            PreviewSpec::BubbleError => {
                self.status = HudStatus::Error {
                    message: "Microphone was disconnected".into(),
                    occurred_at: Instant::now(),
                };
            }
            PreviewSpec::PanelStatsEmpty => {
                self.active_tab = SettingsTab::Stats;
            }
            PreviewSpec::PanelStatsHistory => {
                self.active_tab = SettingsTab::Stats;
                self.stats = preview::fixture_stats(3);
            }
            PreviewSpec::PanelStatsOverflow => {
                self.active_tab = SettingsTab::Stats;
                self.stats = preview::fixture_stats(8);
            }
            PreviewSpec::PanelStatsExpanded => {
                self.active_tab = SettingsTab::Stats;
                self.stats = preview::fixture_stats(3);
                self.expanded_history_key = self
                    .stats
                    .history
                    .first()
                    .map(|item| item.timestamp.clone());
            }
            PreviewSpec::PanelStatsCopied => {
                self.active_tab = SettingsTab::Stats;
                self.stats = preview::fixture_stats(3);
                self.copied_key = self
                    .stats
                    .history
                    .first()
                    .map(|item| item.timestamp.clone());
                self.copied_at = Some(Instant::now());
            }
            PreviewSpec::PanelSettingsDefaults => {}
            PreviewSpec::PanelSettingsAutoPasteOff => {
                self.auto_paste_enabled = false;
            }
            PreviewSpec::PanelSettingsStartupOn => {
                self.autostart_enabled = true;
            }
            PreviewSpec::PanelSettingsStartupError => {
                self.autostart_error = Some(
                    "Could not change startup. Check Windows startup permissions and try again."
                        .into(),
                );
            }
            PreviewSpec::PanelSettingsLang(code) => {
                self.selected_language = code.to_string();
            }
            PreviewSpec::PanelSettingsHotkeyCapture => {
                self.hotkey_capturing = true;
            }
            PreviewSpec::PanelSettingsUpdateChecking
            | PreviewSpec::PanelSettingsUpdateUpToDate
            | PreviewSpec::PanelSettingsUpdateAvailable
            | PreviewSpec::PanelSettingsUpdateDownloading
            | PreviewSpec::PanelSettingsUpdateReady
            | PreviewSpec::PanelSettingsUpdateFailed => {}
        }

        if let Some(phase) = preview::update_phase(&spec) {
            *self.update.lock() = phase;
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
            UpdatePhase::Checking | UpdatePhase::Downloading { .. }
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
                version,
                asset_url,
                asset_name,
                sums_url,
            } => {
                let cell = Arc::clone(&self.update);
                let ping = self.update_ping.clone();
                let ticket = update::begin_update_check();
                std::thread::spawn(move || {
                    *cell.lock() = UpdatePhase::Downloading { done: 0, total: 1 };
                    let _ = ping.send(());
                    let mut last_ping = Instant::now();
                    let next = match update::download_installer(
                        &asset_url,
                        &asset_name,
                        sums_url.as_deref(),
                        |done, total| {
                            *cell.lock() = UpdatePhase::Downloading { done, total };
                            if last_ping.elapsed() >= Duration::from_millis(100) {
                                let _ = ping.send(());
                                last_ping = Instant::now();
                            }
                        },
                    ) {
                        Ok(path) => {
                            if update::is_app_binary(&asset_name) {
                                match update::replace_running_exe(&path, &version) {
                                    Ok(()) => std::process::exit(0),
                                    Err(error) => UpdatePhase::Failed(error),
                                }
                            } else {
                                match update::launch_installer(&path) {
                                    Ok(()) => UpdatePhase::Ready { installer: path },
                                    Err(error) => UpdatePhase::Failed(error),
                                }
                            }
                        }
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

    pub fn hide_overlay(&mut self, cx: &mut Context<Self>) {
        if self.hotkey_capturing {
            self.hotkey_capturing = false;
            hotkey::end_capture();
        }
        self.mode = WindowViewMode::Bubble;
        self.panel_motion = None;
        set_window_mode(false);
        set_overlay_hidden(true);
        cx.notify();
    }

    pub fn reveal_overlay(&mut self) {
        set_overlay_hidden(false);
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.reveal_overlay();
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
        if let Some(engine) = self.stt_engine.lock().clone() {
            let _ = engine.set_language(code);
        }
        let mut cfg = AppConfig::load();
        cfg.language = code.to_string();
        let _ = cfg.save();
    }

    fn model_busy(&self) -> bool {
        matches!(
            self.status,
            HudStatus::Listening { .. } | HudStatus::Transcribing { .. }
        )
    }

    pub fn take_ready_model(&mut self) {
        let ready_id = match &*self.model_phase.lock() {
            DownloadPhase::Ready { id } => Some(id.clone()),
            _ => None,
        };
        let Some(id) = ready_id else {
            return;
        };
        *self.model_phase.lock() = DownloadPhase::Idle;
        if self.selected_model == id {
            self.install_engine(&id);
        }
    }

    fn install_engine(&mut self, id: &str) {
        let spec = models::resolve(id);
        let override_dir = AppConfig::load().model_dir;
        let Some(dir) = models::find_dir(spec, override_dir.as_deref()) else {
            *self.stt_engine.lock() = None;
            return;
        };
        match SttEngine::new(spec, &dir, &self.selected_language) {
            Ok(engine) => {
                *self.stt_engine.lock() = Some(Arc::new(engine));
                self.model_error = None;
            }
            Err(error) => {
                *self.stt_engine.lock() = None;
                self.model_error = Some(format!("Could not load {}. {error}", spec.label));
            }
        }
    }

    fn select_model(&mut self, id: &str) {
        if self.model_busy() {
            self.model_error =
                Some("Wait until transcription finishes, then switch models.".into());
            return;
        }
        let spec = models::resolve(id);
        self.selected_model = spec.id.to_string();
        let mut cfg = AppConfig::load();
        cfg.model_id = spec.id.to_string();
        let _ = cfg.save();
        if models::find_dir(spec, cfg.model_dir.as_deref()).is_some() {
            self.install_engine(spec.id);
        } else {
            *self.stt_engine.lock() = None;
            self.model_error = None;
        }
    }

    fn start_model_download(&mut self) {
        if let DownloadPhase::Downloading { id, .. } = &*self.model_phase.lock() {
            if id != &self.selected_model {
                self.model_error = Some("Wait for the current download to finish.".into());
            }
            return;
        }
        let spec = models::resolve(&self.selected_model);
        let override_dir = AppConfig::load().model_dir;
        if models::find_dir(spec, override_dir.as_deref()).is_some() {
            self.install_engine(spec.id);
            return;
        }
        let dest = models::install_dir(spec);
        let id = spec.id.to_string();
        let phase = Arc::clone(&self.model_phase);
        let ping = self.update_ping.clone();
        let first_file = spec
            .files
            .first()
            .map(|file| file.name.to_string())
            .unwrap_or_default();
        *phase.lock() = DownloadPhase::Downloading {
            id: id.clone(),
            done: 0,
            total: spec.size_bytes.max(1),
            file: first_file,
            file_index: 1,
            file_count: spec.files.len().max(1),
        };
        std::thread::spawn(move || {
            let spec = models::resolve(&id);
            let mut last_ping = Instant::now();
            let mut last_pct = 255u8;
            let result = models::download(spec, &dest, |progress| {
                let pct = models::percent(progress.done, progress.total);
                *phase.lock() = DownloadPhase::Downloading {
                    id: spec.id.to_string(),
                    done: progress.done,
                    total: progress.total,
                    file: progress.file,
                    file_index: progress.file_index,
                    file_count: progress.file_count,
                };
                if last_ping.elapsed() >= Duration::from_millis(100) || pct != last_pct {
                    let _ = ping.send(());
                    last_ping = Instant::now();
                    last_pct = pct;
                }
            });
            match result {
                Ok(()) => {
                    *phase.lock() = DownloadPhase::Ready {
                        id: spec.id.to_string(),
                    };
                }
                Err(error) => {
                    eprintln!("{error}");
                    *phase.lock() = DownloadPhase::Failed {
                        id: spec.id.to_string(),
                        message: format!(
                            "Could not download {}. Check your network and try again.",
                            spec.label
                        ),
                    };
                }
            }
            let _ = ping.send(());
        });
    }

    fn toggle_auto_paste(&mut self) {
        self.auto_paste_enabled = !self.auto_paste_enabled;
        self.toggle_epoch = self.toggle_epoch.wrapping_add(1);
        let mut cfg = AppConfig::load();
        cfg.auto_paste = self.auto_paste_enabled;
        let _ = cfg.save();
        *self.auto_paste_state.lock() = self.auto_paste_enabled;
    }

    fn toggle_autostart(&mut self) {
        let enabled = !crate::autostart::is_enabled();
        match crate::autostart::set_enabled(enabled) {
            Ok(()) => {
                self.autostart_enabled = enabled;
                self.autostart_error = None;
            }
            Err(error) => {
                eprintln!("{error}");
                self.autostart_enabled = crate::autostart::is_enabled();
                self.autostart_error = Some(
                    "Could not change startup. Check Windows startup permissions and try again."
                        .into(),
                );
            }
        }
    }

    fn toggle_history_preview(&mut self, key: &str) {
        self.expanded_history_key = if self.expanded_history_key.as_deref() == Some(key) {
            None
        } else {
            Some(key.to_owned())
        };
    }

    fn clear_recents(&mut self) {
        self.stats.clear_history();
        self.expanded_history_key = None;
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
        let holding = matches!(&self.status, HudStatus::Listening { .. });
        let released = matches!(&self.status, HudStatus::Transcribing { .. });

        let waveform = if holding || released {
            let mut bars = Vec::with_capacity(WAVE_BARS);
            let bar_color = if holding { foam() } else { gold() };
            for peak in self.wave_peaks {
                let height = if holding {
                    wave_height(peak)
                } else {
                    WAVE_FLAT + 2.0
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
                .flex_none()
                .items_center()
                .justify_center()
                .gap(px(WAVE_GAP))
                .h(px(28.0))
                .overflow_hidden()
                .children(bars);
            if released && client_animations_enabled() {
                waveform
                    .with_animation(
                        "transcribe_pulse",
                        Animation::new(Duration::from_millis(1000))
                            .repeat()
                            .with_easing(pulse_curve),
                        |this, delta| this.opacity(0.45 + 0.55 * delta),
                    )
                    .into_any_element()
            } else {
                waveform.into_any_element()
            }
        } else {
            div().into_any_element()
        };

        let center_child = match &self.status {
            HudStatus::Idle => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(controls::shortcut_keys(&self.hotkey_label))
                .into_any_element(),
            HudStatus::Success { auto_pasted, .. } => {
                overlay_snippet(if *auto_pasted { "Pasted" } else { "Copied" }, success())
            }
            HudStatus::Error { message, .. } => overlay_snippet(message, love()),
            _ => waveform,
        };

        let state_key = match &self.status {
            HudStatus::Idle => "bubble_ready",
            HudStatus::Listening { .. } => "bubble_listening",
            HudStatus::Transcribing { .. } => "bubble_transcribing",
            HudStatus::Success { .. } => "bubble_success",
            HudStatus::Error { .. } => "bubble_error",
        };
        let center_child = div().flex_none().overflow_hidden().child(center_child);
        let center_child = if client_animations_enabled() {
            center_child
                .with_animation(
                    state_key,
                    Animation::new(Duration::from_millis(MOTION_MS)).with_easing(ease_out_quint()),
                    |element, progress| element.opacity(0.7 + 0.3 * progress),
                )
                .into_any_element()
        } else {
            center_child.into_any_element()
        };

        let (status_label, status_color, status_pulse) = match &self.status {
            HudStatus::Idle => ("Ready", muted(), false),
            HudStatus::Listening { .. } => ("Listening", foam(), true),
            HudStatus::Transcribing { .. } => ("Transcribing", gold(), true),
            HudStatus::Success { auto_pasted, .. } => (
                if *auto_pasted { "Pasted" } else { "Copied" },
                success(),
                false,
            ),
            HudStatus::Error { .. } => ("Error", love(), false),
        };

        let left_section = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(GAP_TIGHT))
            .child(phase_indicator(
                "status_indicator",
                true,
                status_color,
                status_pulse,
            ))
            .child(
                div()
                    .text_size(px(TYPE_LABEL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(status_color)
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
            .gap(px(GAP_TIGHT));

        let right_section = if matches!(&self.status, HudStatus::Idle) {
            right_section
                .child(
                    controls::ghost_button("hide_overlay_btn", "Hide")
                        .h(px(SETTINGS_HIT_HEIGHT))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.hide_overlay(cx);
                                cx.notify();
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| cx.stop_propagation()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.hide_overlay(cx);
                            cx.notify();
                        })),
                )
                .child(
                    controls::secondary_button("open_settings_btn", "Settings")
                        .h(px(SETTINGS_HIT_HEIGHT))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.open_settings(cx);
                                cx.notify();
                            }
                        }))
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
                    .text_size(px(TYPE_META))
                    .line_height(px(14.0))
                    .font_family("Consolas")
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted())
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
            .w(px(BUBBLE_WIDTH))
            .h(px(BUBBLE_HEIGHT))
            .px(pad())
            .gap(px(GAP_TIGHT))
            .rounded(r_window())
            .bg(pill_bg())
            .border_1()
            .border_color(hairline())
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
            .child(div().flex_1().min_w(px(0.0)))
            .child(center_child)
            .child(div().flex_1().min_w(px(0.0)))
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
                .border_1()
                .border_color(theme::transparent())
                .focus(|style| style.border_color(focus_ring()))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.active_tab = tab;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .flex_none()
                .h(px(H_TAB))
                .px(px(10.0))
                .rounded(r_chip())
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TYPE_DESC))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .bg(if active {
                    selected()
                } else {
                    theme::transparent()
                })
                .text_color(if active { accent() } else { muted() })
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|style| {
                    if active {
                        style
                    } else {
                        style.bg(hover()).text_color(text())
                    }
                })
                .active(|style| style.opacity(0.9))
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
            .pb(px(8.0))
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
                    .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(accent()))
                    .child(
                        div()
                            .text_size(px(TYPE_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text())
                            .whitespace_nowrap()
                            .child("TDT"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(2.0))
                            .p(px(2.0))
                            .rounded(r_section())
                            .bg(well())
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
                        controls::ghost_button("close_settings_btn", "Close")
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.close_stats_settings(cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
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
            .min_h(px(0.0));
        let faded_content = if self.active_tab == SettingsTab::Stats {
            faded_content.overflow_hidden()
        } else {
            faded_content.overflow_y_scroll()
        };
        let faded_content = faded_content.child(content);
        let faded_content = if client_animations_enabled() {
            faded_content
                .with_animation(
                    tab_id,
                    Animation::new(Duration::from_millis(TAB_FADE_MS))
                        .with_easing(ease_out_quint()),
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
            .px(pad())
            .pt(pad())
            .pb(pad())
            .bg(panel_bg())
            .border_1()
            .border_color(hairline())
            .rounded(r_window())
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
        let grid = div()
            .flex()
            .w_full()
            .gap(px(12.0))
            .px(px(2.0))
            .pt(px(4.0))
            .child(controls::stat_block(
                "Transcriptions",
                format!("{}", self.stats.total_transcriptions),
            ))
            .child(controls::stat_block(
                "Words",
                format!("{}", self.stats.total_words),
            ))
            .child(controls::stat_block(
                "Time saved",
                format_time_saved(self.stats.total_seconds),
            ));

        let mut history_items = Vec::new();
        if self.stats.history.is_empty() {
            history_items.push(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(32.0))
                    .child(
                        div()
                            .text_size(px(TYPE_LABEL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text())
                            .child("No transcriptions yet"),
                    )
                    .child(
                        div()
                            .text_size(px(TYPE_DESC))
                            .text_color(muted())
                            .child(format!("Press or hold {} to talk.", self.hotkey_label)),
                    )
                    .into_any_element(),
            );
        } else {
            for (idx, item) in self.stats.history.iter().enumerate() {
                let text_val = item.text.clone();
                let is_copied = self.copied_key.as_deref() == Some(item.timestamp.as_str());
                let btn_text = if is_copied { "Copied" } else { "Copy" };
                let copy_tx = text_val.clone();
                let keyboard_copy_tx = text_val.clone();
                let click_key = item.timestamp.clone();
                let btn_key = click_key.clone();
                let keyboard_key = click_key.clone();
                let keyboard_copy_key = click_key.clone();
                let expanded =
                    self.expanded_history_key.as_deref() == Some(item.timestamp.as_str());
                let can_expand = history_text_overflows(&text_val);

                let copy_btn = {
                    let anim_key = if is_copied { "copied_btn" } else { "copy_btn" };
                    div()
                        .id(ElementId::NamedInteger(anim_key.into(), idx as u64))
                        .tab_index(0)
                        .focus(|style| style.bg(hover()))
                        .flex_none()
                        .h(px(H_CTRL))
                        .px(px(8.0))
                        .rounded(r_chip())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(TYPE_META))
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .bg(if is_copied {
                            rgba(0x3e8fb033).into()
                        } else {
                            theme::transparent()
                        })
                        .text_color(if is_copied { success() } else { muted() })
                        .cursor_pointer()
                        .hover(|style| {
                            if is_copied {
                                style
                            } else {
                                style.bg(hover()).text_color(text())
                            }
                        })
                        .active(|style| style.opacity(0.9))
                        .child(btn_text)
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.copy_history(&keyboard_copy_key, &keyboard_copy_tx);
                                cx.notify();
                            }
                        }))
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
                };

                history_items.push(
                    div()
                        .id(ElementId::NamedInteger("history_row".into(), idx as u64))
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(px(4.0))
                        .px(px(10.0))
                        .py(px(10.0))
                        .rounded(r_chip())
                        .tab_index(0)
                        .cursor_pointer()
                        .focus(|style| style.bg(hover()))
                        .hover(|style| style.bg(hover()))
                        .active(|style| style.opacity(0.92))
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.toggle_history_preview(&keyboard_key);
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_history_preview(&click_key);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .w_full()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_size(px(11.0))
                                        .font_family("Consolas")
                                        .text_color(rgb(MUTED))
                                        .whitespace_nowrap()
                                        .child(format!(
                                            "{}  ·  {}ms",
                                            item.timestamp, item.latency_ms
                                        )),
                                )
                                .child(copy_btn),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(rgb(TEXT))
                                .when(!expanded, |text| text.line_clamp(2))
                                .child(text_val),
                        )
                        .when(can_expand, |row| {
                            row.child(div().text_size(px(11.0)).text_color(rgb(MUTED)).child(
                                if expanded {
                                    "Collapse text"
                                } else {
                                    "Expand text"
                                },
                            ))
                        })
                        .into_any_element(),
                );
            }
        }

        let has_recents = !self.stats.history.is_empty();
        let recents_count = self.stats.history.len().to_string();
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
                            .text_color(rgb(MUTED))
                            .child("Recent"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_family("Consolas")
                            .text_color(rgb(MUTED))
                            .child(recents_count),
                    ),
            );
        if has_recents {
            history_header = history_header.child(
                controls::ghost_button("clear_recents_btn", "Clear recents").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.clear_recents();
                        cx.notify();
                    },
                )),
            );
        }

        let history_section = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(GAP_TIGHT))
            .child(history_header)
            .child(
                div()
                    .id("history_scroller")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .gap(px(GAP_TIGHT))
                    .children(history_items),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(GAP_SECTION))
            .child(grid)
            .child(history_section)
            .into_any_element()
    }

    fn render_settings_tab(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let is_auto_paste = self.auto_paste_enabled;
        let hotkey_str = self.hotkey_label.clone();

        let settings_row = |title: &'static str, subtitle: String, trailing: AnyElement| {
            controls::setting_row(title, subtitle, trailing)
        };

        let on = is_auto_paste;
        let epoch = self.toggle_epoch;
        let animate_toggle = epoch > 0 && client_animations_enabled();
        let knob = controls::toggle_knob();
        let knob = if animate_toggle {
            knob.with_animation(
                ElementId::NamedInteger("toggle_knob".into(), epoch),
                Animation::new(Duration::from_millis(MOTION_MS)).with_easing(ease_out_quint()),
                move |knob, progress| knob.ml(px(toggle_offset(on, progress))),
            )
            .into_any_element()
        } else {
            knob.ml(px(toggle_offset(on, 1.0))).into_any_element()
        };
        let toggle_track = controls::toggle_hit(
            "auto_paste_hit_target",
            controls::toggle_track(is_auto_paste, knob),
        )
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
        }));

        let auto_paste_row = settings_row(
            "Auto-paste",
            "Insert text into the focused app".to_string(),
            toggle_track.into_any_element(),
        );

        let startup_toggle = controls::toggle_hit(
            "autostart_hit_target",
            controls::toggle_track(
                self.autostart_enabled,
                controls::toggle_knob()
                    .ml(px(toggle_offset(self.autostart_enabled, 1.0)))
                    .into_any_element(),
            ),
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                this.toggle_autostart();
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_autostart();
            cx.notify();
        }));
        let startup_row = settings_row(
            "Start with Windows",
            "Launch TDT when you sign in".to_string(),
            startup_toggle.into_any_element(),
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
                controls::choice_chip(
                    ElementId::NamedInteger("lang_btn".into(), i as u64),
                    *label,
                    is_selected,
                )
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

        let language_row = {
            let mut rows = Vec::new();
            let mut row = Vec::new();
            for button in lang_buttons {
                row.push(button.into_any_element());
                if row.len() == 3 {
                    rows.push(controls::chip_row(std::mem::take(&mut row)));
                }
            }
            if !row.is_empty() {
                rows.push(controls::chip_row(row));
            }
            controls::labeled_block(
                "Language",
                "Auto-detect, or lock to one language",
                [controls::chip_well(rows).into_any_element()],
            )
        };

        let capturing = self.hotkey_capturing;
        let hotkey_sub = if capturing {
            "Press the new shortcut. Esc cancels.".to_string()
        } else {
            "Click the keys to change it.".to_string()
        };
        let hotkey_chip = div()
            .id("hotkey_bind_btn")
            .flex()
            .items_center()
            .justify_center()
            .h(px(H_CTRL))
            .px(px(8.0))
            .rounded(r_chip())
            .border_1()
            .border_color(if capturing {
                rgba(0x9ccfd8aa).into()
            } else {
                theme::transparent()
            })
            .bg(if capturing { selected() } else { well() })
            .cursor_pointer()
            .active(|style| style.opacity(0.9))
            .child(if capturing {
                div()
                    .text_size(px(TYPE_META))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(accent())
                    .child("Press shortcut...")
                    .into_any_element()
            } else {
                controls::shortcut_keys(&hotkey_str)
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

        let hotkey_row = settings_row("Hotkey", hotkey_sub, hotkey_chip);

        let phase = self.update.lock().clone();
        let (_update_title, update_sub, update_action, update_busy) = match &phase {
            UpdatePhase::Idle => (
                "Updates",
                format!("Current version v{}", update::current_version()),
                "Check for updates".to_string(),
                false,
            ),
            UpdatePhase::Checking => (
                "Updates",
                "Checking for updates...".to_string(),
                "Checking".to_string(),
                true,
            ),
            UpdatePhase::UpToDate => (
                "Updates",
                format!("You're up to date · v{}", update::current_version()),
                "Check again".to_string(),
                false,
            ),
            UpdatePhase::Available { version, .. } => (
                "Updates",
                format!("v{version} is ready. Downloads the app only, not the speech model."),
                "Download and restart".to_string(),
                false,
            ),
            UpdatePhase::Downloading { done, total } => {
                let pct = models::percent(*done, *total);
                (
                    "Updates",
                    format!(
                        "{}% · {} of {}",
                        pct,
                        models::format_mb(*done),
                        models::format_mb(*total)
                    ),
                    format!("{pct}%"),
                    true,
                )
            }
            UpdatePhase::Ready { .. } => (
                "Updates",
                "Update downloaded. Install when ready.".to_string(),
                "Install update".to_string(),
                false,
            ),
            UpdatePhase::Failed(error) => (
                "Updates",
                clip_text(error, 48),
                "Try again".to_string(),
                false,
            ),
        };

        let update_btn = if matches!(
            phase,
            UpdatePhase::Available { .. } | UpdatePhase::Ready { .. }
        ) {
            controls::primary_button("check_updates_btn", update_action)
        } else {
            controls::secondary_button("check_updates_btn", update_action)
        };
        let update_meter = match &phase {
            UpdatePhase::Downloading { done, total } => Some(controls::progress_bar(*done, *total)),
            _ => None,
        };
        let update_row = div()
            .flex()
            .flex_col()
            .w_full()
            .pt(px(10.0))
            .gap(px(10.0))
            .child(controls::section_label("Updates"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .w_full()
                    .px(px(2.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(TYPE_DESC))
                            .line_height(px(16.0))
                            .text_color(muted())
                            .child(update_sub),
                    )
                    .child(
                        update_btn
                            .when(update_busy, |btn| btn.opacity(0.7))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if matches!(
                                    *this.update.lock(),
                                    UpdatePhase::Checking | UpdatePhase::Downloading { .. }
                                ) {
                                    return;
                                }
                                this.start_update_install();
                                cx.notify();
                            })),
                    ),
            )
            .when_some(update_meter, |row, meter| {
                row.child(div().px(px(2.0)).pt(px(6.0)).child(meter))
            });

        let spec = models::resolve(&self.selected_model);
        let installed = models::find_dir(spec, None).is_some();
        let phase = match &*self.model_phase.lock() {
            DownloadPhase::Downloading {
                id,
                done,
                total,
                file,
                file_index,
                file_count,
            } if id == spec.id => DownloadPhase::Downloading {
                id: id.clone(),
                done: *done,
                total: *total,
                file: file.clone(),
                file_index: *file_index,
                file_count: *file_count,
            },
            DownloadPhase::Failed { id, message } if id == spec.id => DownloadPhase::Failed {
                id: id.clone(),
                message: message.clone(),
            },
            DownloadPhase::Ready { id } if id == spec.id => DownloadPhase::Ready { id: id.clone() },
            _ => DownloadPhase::Idle,
        };
        let downloading = matches!(phase, DownloadPhase::Downloading { .. });
        let model_sub = match &phase {
            DownloadPhase::Downloading {
                done,
                total,
                file,
                file_index,
                file_count,
                ..
            } => format!(
                "{}% · {} of {} · {} · {} of {}",
                models::percent(*done, *total),
                file_index,
                file_count,
                models::file_label(file),
                models::format_mb(*done),
                models::format_mb(*total)
            ),
            DownloadPhase::Failed { message, .. } => message.clone(),
            _ if installed => format!("{}, {}", spec.label, spec.blurb),
            _ => format!("{}, not downloaded, {}", spec.label, spec.size_label),
        };
        let mut model_buttons = Vec::new();
        for (i, option) in CATALOG.iter().enumerate() {
            let is_selected = self.selected_model == option.id;
            let option_id = option.id;
            model_buttons.push(
                controls::choice_chip(
                    ElementId::NamedInteger("model_btn".into(), i as u64),
                    option.chip,
                    is_selected,
                )
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.select_model(option_id);
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_model(option_id);
                    cx.notify();
                })),
            );
        }
        let mut model_rows = Vec::new();
        let mut row = Vec::new();
        for button in model_buttons {
            row.push(button.into_any_element());
            if row.len() == 2 {
                model_rows.push(controls::chip_row(std::mem::take(&mut row)));
            }
        }
        if !row.is_empty() {
            model_rows.push(controls::chip_row(row));
        }
        let download_action = match &phase {
            DownloadPhase::Downloading { done, total, .. } => {
                format!("{}%", models::percent(*done, *total))
            }
            DownloadPhase::Failed { .. } => "Try again".to_string(),
            _ => format!("Download model · {}", spec.size_label),
        };
        let show_download = !installed || matches!(phase, DownloadPhase::Failed { .. });
        let model_meter = match &phase {
            DownloadPhase::Downloading { done, total, .. } => {
                Some(controls::progress_bar(*done, *total))
            }
            _ => None,
        };
        let mut model_body = vec![controls::chip_well(model_rows).into_any_element()];
        if let Some(meter) = model_meter {
            model_body.push(meter);
        }
        if show_download {
            model_body.push(
                div()
                    .id("download_model_btn")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(H_CTRL))
                    .px(px(10.0))
                    .rounded(r_chip())
                    .tab_index(0)
                    .border_1()
                    .border_color(theme::transparent())
                    .focus(|style| style.border_color(focus_ring()))
                    .text_size(px(TYPE_META))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if downloading { muted() } else { foam() })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|style| {
                        if downloading {
                            style
                        } else {
                            style.text_color(text())
                        }
                    })
                    .active(|style| style.opacity(0.92))
                    .child(download_action)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.start_model_download();
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_model_download();
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let model_row = controls::labeled_block("Model", model_sub, model_body);

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(GAP_SECTION))
            .child(controls::grouped_section(
                "General",
                [
                    auto_paste_row.into_any_element(),
                    startup_row.into_any_element(),
                ],
            ))
            .when_some(self.autostart_error.clone(), |view, error| {
                view.child(
                    div()
                        .px(px(2.0))
                        .mt(px(-4.0))
                        .text_size(px(TYPE_DESC))
                        .line_height(px(15.0))
                        .text_color(love())
                        .child(error),
                )
            })
            .child(language_row)
            .child(controls::grouped_section(
                "Shortcut",
                [hotkey_row.into_any_element()],
            ))
            .child(model_row)
            .when_some(self.model_error.clone(), |view, error| {
                view.child(
                    div()
                        .px(px(10.0))
                        .text_size(px(TYPE_DESC))
                        .line_height(px(15.0))
                        .text_color(love())
                        .child(error),
                )
            })
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
    let travel = 16.0;
    if on {
        travel * progress
    } else {
        travel * (1.0 - progress)
    }
}

fn preview_wave(loud: bool) -> [f32; WAVE_BARS] {
    let mut peaks = [0.0; WAVE_BARS];
    for (index, slot) in peaks.iter_mut().enumerate() {
        let t = index as f32 / WAVE_BARS as f32;
        let wave = (t * std::f32::consts::PI * 3.0).sin().abs();
        *slot = if loud {
            0.35 + 0.6 * wave
        } else {
            0.04 + 0.12 * wave
        };
    }
    peaks
}

fn wave_height(peak: f32) -> f32 {
    let driven = peak.clamp(0.0, 1.0);
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
        .justify_center()
        .h(px(28.0))
        .overflow_hidden()
        .child(
            div()
                .text_size(px(TYPE_META))
                .line_height(px(14.0))
                .text_color(color)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(clip_text(text, 40)),
        )
        .into_any_element()
}

fn history_text_overflows(text: &str) -> bool {
    text.chars().count() > HISTORY_EXPAND_CHARS
}

// Compile-time invariant: the painted settings surface must stay inset within
// the pill so its highlight never collides with the border and 14px corners.
const _: () = assert!(SETTINGS_SURFACE_HEIGHT < SETTINGS_HIT_HEIGHT);
const _: () = assert!(SETTINGS_SURFACE_HEIGHT <= BUBBLE_HEIGHT - 14.0);

#[cfg(test)]
mod motion_tests {
    use super::{history_text_overflows, pulse_curve, toggle_offset, HISTORY_EXPAND_CHARS};

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
        assert_eq!(toggle_offset(true, 0.0), 0.0);
        assert_eq!(toggle_offset(true, 1.0), 16.0);
        assert_eq!(toggle_offset(false, 0.0), 16.0);
        assert_eq!(toggle_offset(false, 1.0), 0.0);
        for step in 0..=100 {
            let progress = step as f32 / 100.0;
            for on in [true, false] {
                assert!((0.0..=16.0).contains(&toggle_offset(on, progress)));
            }
        }
    }

    #[test]
    fn short_history_text_does_not_offer_expand() {
        assert!(!history_text_overflows(
            "Short transcript 1: ready when you are"
        ));
        let long = "word ".repeat(HISTORY_EXPAND_CHARS);
        assert!(history_text_overflows(&long));
    }
}
