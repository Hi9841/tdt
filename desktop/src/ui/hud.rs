use crate::audio::VIS_BARS;
use crate::config::{AppConfig, AppStats};
use crate::hotkey;
use crate::paste::PasteInjector;
use crate::ui::controls;
use crate::ui::preview::{self, Spec as PreviewSpec};
use crate::ui::text::{clip_text, first_sentence, format_latency_ms, format_time_saved};
use crate::ui::theme::{
    self, bg, chip, danger, edge, muted, pad, r_chip, r_section, r_window, rest_bg, rest_edge,
    soft, text, well, GAP_SECTION, GAP_TIGHT, MUTED, TAB_FADE_MS, TEXT, TYPE_DESC, TYPE_LABEL,
    TYPE_META, TYPE_TITLE,
};
use crate::ui::window_util::{
    client_animations_enabled, set_pill_hit_rect, set_window_mode,
};
use crate::update::{self, UpdatePhase};
use ely_gpui_component::buttons::{ButtonVariant, IconButton, SegmentedControl};
use ely_gpui_component::feedback::{Alert, InlineMessage};
use ely_gpui_component::forms::{Choice, ChoiceChips, Switch};
use ely_gpui_component::layout::ScrollArea;
use ely_gpui_component::primitives::{FocusScope, Icon, IconName, Severity};
use ely_gpui_component::theme::{ActiveTheme, ControlSize, IconSize};
use gpui::prelude::FluentBuilder;
use gpui::*;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use voice_stt_desktop::stt::{models, DownloadPhase, EngineSlot, SttEngine, CATALOG};

/// Compact actions leave seven pixels above and below inside the overlay.
const SETTINGS_SURFACE_HEIGHT: f32 = 28.0;
const WAVE_BARS: usize = VIS_BARS;
/// Two clamped lines at 12px in this panel hold about this many characters.
const HISTORY_EXPAND_CHARS: usize = 88;
/// The latest row clamps four lines at 14px, so it holds about twice as much.
const LATEST_EXPAND_CHARS: usize = 170;

// The overlay pill follows Aqua Voice: a small black capsule at rest that
// grows for live audio and for a short message, then settles back.
const REST: (f32, f32) = (28.0, 11.0);
const REST_HOVER: (f32, f32) = (36.0, 14.0);
const FINISHING: (f32, f32) = (34.0, 14.0);
const LIVE_W: f32 = 76.0;
const LIVE_H: f32 = 25.0;
const LIVE_ORB: f32 = 9.0;
const LIVE_BARS: usize = 11;
const LIVE_BAR_W: f32 = 2.0;
const LIVE_BAR_GAP: f32 = 2.0;
const LIVE_BAR_MIN: f32 = 3.0;
const LIVE_BAR_MAX: f32 = 13.0;
const MESSAGE_H: f32 = 41.0;
const MESSAGE_PAD: f32 = 18.0;
const MESSAGE_TEXT: f32 = 13.5;
const MESSAGE_MAX_W: f32 = 360.0;
const SNIPPET_MAX_W: f32 = 170.0;
const ERROR_MAX_W: f32 = 230.0;
const SNIPPET_CHARS: usize = 80;
const PILL_GAP: f32 = 10.0;
const CHIP_PAD: f32 = 9.0;
const HIT_PAD: f32 = 6.0;
const TIP_H: f32 = 30.0;
const TIP_GAP: f32 = 6.0;
const PILL_MOTION_MS: u64 = 320;
/// The pill grows into the panel, and shrinks back faster than it grew.
const PANEL_IN_MS: u64 = 260;
const PANEL_OUT_MS: u64 = 180;

/// The pill's size moves from `from` to `to` with a small overshoot.
#[derive(Debug, Clone, Copy)]
struct PillMotion {
    from: (f32, f32),
    to: (f32, f32),
    started: Instant,
}

impl PillMotion {
    fn new() -> Self {
        Self {
            from: REST,
            to: REST,
            started: Instant::now(),
        }
    }

    fn progress(&self) -> f32 {
        if !client_animations_enabled() {
            return 1.0;
        }
        (self.started.elapsed().as_secs_f32() * 1000.0 / PILL_MOTION_MS as f32).min(1.0)
    }

    fn current(&self) -> (f32, f32) {
        let eased = ease_pop(self.progress());
        (
            self.from.0 + (self.to.0 - self.from.0) * eased,
            self.from.1 + (self.to.1 - self.from.1) * eased,
        )
    }

    /// Size for this frame, and whether the motion still runs.
    fn sample(&mut self, target: (f32, f32)) -> ((f32, f32), bool) {
        if target != self.to {
            self.from = self.current();
            self.to = target;
            self.started = Instant::now();
        }
        (self.current(), self.progress() < 1.0)
    }

    /// Room the pill can take while it moves, overshoot included.
    fn span(&self) -> (f32, f32) {
        if self.progress() >= 1.0 {
            return self.to;
        }
        (
            self.from.0.max(self.to.0) * 1.08 + 2.0,
            self.from.1.max(self.to.1) * 1.08 + 2.0,
        )
    }
}

/// Ease out with a small overshoot, so the pill pops open.
fn ease_pop(t: f32) -> f32 {
    let c1 = 1.1;
    let c3 = c1 + 1.0;
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + c3 * u * u * u + c1 * u * u
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PillAction {
    Start,
    Stop,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PillKind {
    Label,
    Snippet(f32),
    Meta,
    Chip,
}

/// One run of words in the message pill.
struct PillPart {
    text: SharedString,
    color: Hsla,
    kind: PillKind,
}

impl PillPart {
    fn label(text: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            text: text.into(),
            color,
            kind: PillKind::Label,
        }
    }

    fn snippet(text: &str, color: Hsla, max_width: f32) -> Self {
        let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        Self {
            text: clip_text(&line, SNIPPET_CHARS).into(),
            color,
            kind: PillKind::Snippet(max_width),
        }
    }

    fn meta(text: String) -> Self {
        Self {
            text: text.into(),
            color: soft(),
            kind: PillKind::Meta,
        }
    }

    fn chip(words: &'static str) -> Self {
        Self {
            text: words.into(),
            color: text(),
            kind: PillKind::Chip,
        }
    }

    fn font(&self, family: &SharedString) -> (Font, f32) {
        match self.kind {
            PillKind::Meta => (font("Consolas"), 12.0),
            PillKind::Chip => (
                Font {
                    weight: FontWeight::MEDIUM,
                    ..font(family.clone())
                },
                12.0,
            ),
            PillKind::Label => (
                Font {
                    weight: FontWeight::MEDIUM,
                    ..font(family.clone())
                },
                MESSAGE_TEXT,
            ),
            PillKind::Snippet(_) => (font(family.clone()), MESSAGE_TEXT),
        }
    }

    fn width(&self, window: &Window, family: &SharedString) -> f32 {
        let (font, size) = self.font(family);
        let run = TextRun {
            len: self.text.len(),
            font,
            color: self.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(self.text.clone(), px(size), &[run], None);
        let width = (shaped.width / px(1.0)).ceil();
        match self.kind {
            PillKind::Snippet(max) => width.min(max),
            PillKind::Chip => width + 2.0 * CHIP_PAD,
            _ => width,
        }
    }

    fn render(self, window: &Window, family: &SharedString) -> AnyElement {
        let width = self.width(window, family);
        let (font, size) = self.font(family);
        let part = div()
            .flex_none()
            .w(px(width))
            .text_size(px(size))
            .line_height(px(20.0))
            .font_family(font.family)
            .font_weight(font.weight)
            .text_color(self.color)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis();
        match self.kind {
            PillKind::Chip => part
                .flex()
                .items_center()
                .justify_center()
                .h(px(22.0))
                .rounded_full()
                .bg(chip())
                .child(self.text)
                .into_any_element(),
            _ => part.child(self.text).into_any_element(),
        }
    }
}

/// The app's orb logo, shown in the live pill. Decoded once and shared, so
/// GPUI keeps one texture for it.
fn orb_image() -> Arc<Image> {
    static ORB: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    ORB.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../../assets/orb.png").to_vec(),
        ))
    })
    .clone()
}

/// Fold the recorder's bars into the few the live pill shows.
fn live_peaks(peaks: &[f32; WAVE_BARS]) -> [f32; LIVE_BARS] {
    let mut out = [0.0; LIVE_BARS];
    for (index, slot) in out.iter_mut().enumerate() {
        let start = index * WAVE_BARS / LIVE_BARS;
        let end = ((index + 1) * WAVE_BARS / LIVE_BARS).max(start + 1);
        *slot = peaks[start..end]
            .iter()
            .copied()
            .fold(0.0, f32::max)
            .clamp(0.0, 1.0);
    }
    out
}

/// The pill grows into the panel. A black shell widens first and then rises
/// (height trails width), staying a pill until it is tall, and the content
/// fades in at the end. Closing runs the same path back: the content goes
/// first, then the shell flattens into the pill.
fn morph(panel: AnyElement, open: f32, viewport: Size<Pixels>) -> AnyElement {
    let lerp = |from: f32, to: f32, at: f32| from + (to - from) * at;
    let (width, height) = (viewport.width / px(1.0), viewport.height / px(1.0));
    let shell_h = lerp(REST.1, height, open * open);
    let content = ((open - 0.8) / 0.2).clamp(0.0, 1.0);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_end()
        .pb(px(lerp(HIT_PAD, 0.0, open)))
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .justify_end()
                .w(px(lerp(REST.0, width, open)))
                .h(px(shell_h))
                .rounded(px((shell_h / 2.0).min(theme::R_WINDOW)))
                .bg(bg())
                .border_1()
                .border_color(edge())
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .w(px(width))
                        .h(px(height))
                        .opacity(content)
                        .child(panel),
                ),
        )
        .into_any_element()
}

/// Three dots that pulse in turn while the words are worked out.
fn finishing_dots() -> AnyElement {
    let dots = (0..3usize).map(|index| {
        let dot = div().flex_none().size(px(3.0)).rounded_full().bg(text());
        if client_animations_enabled() {
            dot.with_animation(
                ("finishing_dot", index),
                Animation::new(Duration::from_millis(900)).repeat(),
                move |dot, progress| {
                    let phase = (progress - index as f32 * 0.18).rem_euclid(1.0);
                    dot.opacity(0.3 + 0.7 * (phase * std::f32::consts::PI).sin())
                },
            )
            .into_any_element()
        } else {
            dot.opacity(0.7).into_any_element()
        }
    });
    div()
        .flex()
        .items_center()
        .gap(px(3.0))
        .children(dots)
        .into_any_element()
}

#[derive(Debug, Clone, PartialEq)]
pub enum HudStatus {
    Idle,
    NoSpeech,
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
        latency_ms: u64,
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

impl PanelMotion {
    /// How open the panel is now (0 is the resting pill, 1 the full panel),
    /// and whether it still moves.
    fn openness(&self) -> (f32, bool) {
        let (started_at, ms, opening) = match *self {
            Self::Opening { started_at } => (started_at, PANEL_IN_MS, true),
            Self::Closing { started_at } => (started_at, PANEL_OUT_MS, false),
        };
        let t = (started_at.elapsed().as_secs_f32() * 1000.0 / ms as f32).min(1.0);
        // Cubic, not quint: a gentler start spreads the motion over more
        // frames, so a 143 Hz display shows small even steps, not jumps.
        let eased = 1.0 - (1.0 - t).powi(3);
        (if opening { eased } else { 1.0 - eased }, t < 1.0)
    }
}

struct PreparedModel {
    id: String,
    engine: Arc<SttEngine>,
}

pub struct HudView {
    pub status: HudStatus,
    pub auto_paste_enabled: bool,
    remove_fillers_enabled: bool,
    remove_fillers_state: Arc<AtomicBool>,
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
    pub injector: PasteInjector,
    pub selected_model: String,
    pub stt_engine: EngineSlot,
    pub auto_paste_state: Arc<Mutex<bool>>,
    panel_motion: Option<PanelMotion>,
    update: Arc<Mutex<UpdatePhase>>,
    update_ping: crossbeam_channel::Sender<()>,
    model_phase: Arc<Mutex<DownloadPhase>>,
    model_error: Option<String>,
    /// Why the dictionary file could not be opened, shown under the Dictionary row.
    dictionary_error: Option<String>,
    pub hotkey_capturing: bool,
    pill_hovered: bool,
    pill_motion: PillMotion,
    /// A click on the resting pill asks the main loop to start a take.
    pub click_dictation: bool,
    pub stop_requested: bool,
    pub retry_microphone: bool,
    pub recovery_message: Option<String>,
    pub recovery_is_microphone: bool,
    clear_history_pending: bool,
    active_model: Option<String>,
    installed_models: Vec<String>,
    model_dir: Option<std::path::PathBuf>,
    model_loading: Option<String>,
    prepared_model: Option<PreparedModel>,
    model_ready_tx: crossbeam_channel::Sender<Result<PreparedModel, String>>,
    model_ready_rx: crossbeam_channel::Receiver<Result<PreparedModel, String>>,
    preview_ready: Option<bool>,
    root_focus: FocusHandle,
    panel_focus: FocusHandle,
    panel_needs_focus: bool,
}

impl HudView {
    pub fn new(
        hotkey_label: String,
        stt_engine: EngineSlot,
        auto_paste_state: Arc<Mutex<bool>>,
        remove_fillers_state: Arc<AtomicBool>,
        update: Arc<Mutex<UpdatePhase>>,
        update_ping: crossbeam_channel::Sender<()>,
        cx: &mut Context<Self>,
    ) -> Self {
        let open_panel =
            std::env::var_os("VOICE_STT_OPEN_PANEL").is_some() || preview::wants_panel();
        if open_panel {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(450));
                set_window_mode(true);
            });
        }

        let config = if preview::is_active() {
            AppConfig::default()
        } else {
            AppConfig::load()
        };
        let selected_model = models::resolve(&config.model_id).id.to_string();
        let active_model = stt_engine.is_some().then(|| selected_model.clone());
        let installed_models = CATALOG
            .iter()
            .filter(|model| models::find_dir(model, config.model_dir.as_deref()).is_some())
            .map(|model| model.id.to_string())
            .collect();
        let (model_ready_tx, model_ready_rx) = crossbeam_channel::unbounded();
        let auto_paste_enabled = *auto_paste_state.lock();
        let mut view = Self {
            status: HudStatus::Idle,
            auto_paste_enabled,
            remove_fillers_enabled: remove_fillers_state.load(Ordering::Relaxed),
            remove_fillers_state,
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
            stats: if preview::is_active() {
                AppStats::default()
            } else {
                AppStats::load()
            },
            copied_key: None,
            expanded_history_key: None,
            copied_at: None,
            pill_hovered: false,
            pill_motion: PillMotion::new(),
            click_dictation: false,
            injector: PasteInjector::new(),
            selected_model,
            stt_engine,
            auto_paste_state,
            panel_motion: None,
            update,
            update_ping,
            model_phase: Arc::new(Mutex::new(DownloadPhase::Idle)),
            model_error: None,
            dictionary_error: None,
            hotkey_capturing: false,
            stop_requested: false,
            retry_microphone: false,
            recovery_message: None,
            recovery_is_microphone: false,
            clear_history_pending: false,
            active_model,
            installed_models,
            model_dir: config.model_dir,
            model_loading: None,
            prepared_model: None,
            model_ready_tx,
            model_ready_rx,
            preview_ready: None,
            root_focus: cx.focus_handle(),
            panel_focus: cx.focus_handle().tab_stop(false),
            panel_needs_focus: open_panel,
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
        self.remove_fillers_enabled = true;
        self.autostart_enabled = false;
        self.autostart_error = None;

        self.selected_model = models::DEFAULT_MODEL_ID.into();
        self.hotkey_capturing = false;
        *self.model_phase.lock() = DownloadPhase::Idle;
        self.model_error = None;
        self.copied_key = None;
        self.expanded_history_key = None;
        self.copied_at = None;
        self.wave_peaks = [0.0; WAVE_BARS];
        self.status = HudStatus::Idle;
        self.preview_ready = Some(true);
        self.active_model = Some(models::DEFAULT_MODEL_ID.into());
        self.installed_models = vec![models::DEFAULT_MODEL_ID.into()];
        self.active_tab = if is_panel {
            SettingsTab::Settings
        } else {
            SettingsTab::Stats
        };
        self.stats = AppStats::default();

        match spec {
            PreviewSpec::BubbleIdle => {}
            PreviewSpec::BubbleHover => self.pill_hovered = true,
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
                    latency_ms: 182,
                    finished_at: Instant::now(),
                };
            }
            PreviewSpec::BubbleError => {
                self.status = HudStatus::Error {
                    message: "Microphone was disconnected".into(),
                    occurred_at: Instant::now(),
                };
            }
            PreviewSpec::BubbleSetup | PreviewSpec::PanelSetup => {
                self.preview_ready = Some(false);
                self.active_model = None;
                self.installed_models.clear();
            }
            PreviewSpec::BubbleNoSpeech => {
                self.status = HudStatus::NoSpeech;
                self.recovery_message = Some("No speech detected. Try again and check your microphone.".into());
                self.recovery_is_microphone = true;
            }
            PreviewSpec::BubbleCopyFailed => self.set_error("Could not copy text. Open History to copy the saved transcript.".into()),
            PreviewSpec::BubblePasteFallback => {
                self.status = HudStatus::Success {
                    text: "This transcript is ready to paste.".into(),
                    auto_pasted: false,
                    latency_ms: 240,
                    finished_at: Instant::now(),
                };
                self.recovery_message = Some("Copied. Could not insert all text. Check the destination before pasting with Ctrl+V.".into());
                self.recovery_is_microphone = false;
            }
            PreviewSpec::BubbleLimit => {
                self.status = HudStatus::Listening {
                    audio_level: 0.5,
                    started_at: Instant::now() - Duration::from_secs(115),
                };
                self.wave_peaks = preview_wave(true);
            }
            PreviewSpec::PanelModelFailed => {
                self.selected_model = models::DEFAULT.id.to_string();
                *self.model_phase.lock() = DownloadPhase::Failed { id: self.selected_model.clone(), message: format!("Download failed. Check your connection and try again. {} is still active.", models::DEFAULT.label) };
            }
            PreviewSpec::PanelRecovery => self.set_error("Microphone disconnected. Connect a microphone, check Windows microphone access, then choose Retry microphone.".into()),
            PreviewSpec::PanelClearHistory => {
                self.active_tab = SettingsTab::Stats;
                self.stats = preview::fixture_stats(3);
                self.clear_history_pending = true;
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
            if !preview::is_active() {
                hotkey::end_capture();
            }
        } else {
            self.hotkey_capturing = true;
            if !preview::is_active() {
                hotkey::begin_capture();
            }
        }
    }

    pub fn start_update_check(&self) {
        if preview::is_active() {
            return;
        }
        // Ignore clicks while a check or download is already running; the
        // shared phase cell has a single writer at a time.
        let Some(ticket) = update::claim(&self.update, UpdatePhase::Checking, false) else {
            return;
        };
        let cell = Arc::clone(&self.update);
        let ping = self.update_ping.clone();
        std::thread::spawn(move || {
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
        if preview::is_active() {
            return;
        }
        let snapshot = self.update.lock().clone();
        match snapshot {
            UpdatePhase::Available {
                version,
                asset_url,
                asset_name,
                sums_url,
            } => {
                let busy = UpdatePhase::Downloading { done: 0, total: 1 };
                let Some(ticket) = update::claim(&self.update, busy, false) else {
                    return;
                };
                let cell = Arc::clone(&self.update);
                let ping = self.update_ping.clone();
                std::thread::spawn(move || {
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
                                // The main loop quits on Restarting, so the tray
                                // icon goes away before the new TDT starts.
                                match update::replace_running_exe(&path, &version) {
                                    Ok(()) => UpdatePhase::Restarting,
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
        self.clear_history_pending = false;
        self.panel_motion = Some(PanelMotion::Closing { started_at });
        if self.hotkey_capturing {
            self.hotkey_capturing = false;
            hotkey::end_capture();
        }
        // The morph shrinks the panel back into the pill; when it ends,
        // finish_panel_motion leaves panel mode.
        if !client_animations_enabled() {
            self.finish_panel_motion();
        }
        cx.notify();
    }

    /// The morph reached its end: settle into the panel or back into the pill.
    fn finish_panel_motion(&mut self) {
        if let Some(PanelMotion::Closing { .. }) = self.panel_motion {
            self.mode = WindowViewMode::Bubble;
            set_window_mode(false);
        }
        self.panel_motion = None;
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        // Already open or opening: a second click (or a double click on the
        // tray icon) must not restart the morph.
        if self.mode == WindowViewMode::StatsAndSettings
            && !matches!(self.panel_motion, Some(PanelMotion::Closing { .. }))
        {
            return;
        }
        self.mode = WindowViewMode::StatsAndSettings;
        self.pill_hovered = false;
        if !preview::is_active() {
            self.stats = AppStats::load();
        }
        self.active_tab = SettingsTab::Settings;
        self.panel_needs_focus = true;
        let started_at = Instant::now();
        self.panel_motion = Some(PanelMotion::Opening { started_at });
        set_window_mode(true);
        if !client_animations_enabled() {
            self.finish_panel_motion();
        }
        cx.notify();
    }

    fn copy_history(&mut self, key: &str, text: &str) {
        if !preview::is_active() {
            if let Err(error) = self.injector.copy_to_clipboard(text) {
                self.set_error(format!("Could not copy text. Try Copy again. {error}"));
                return;
            }
        }
        self.copied_key = Some(key.to_string());
        self.copied_at = Some(Instant::now());
    }

    fn model_busy(&self) -> bool {
        matches!(
            self.status,
            HudStatus::Listening { .. } | HudStatus::Transcribing { .. }
        )
    }

    pub fn take_ready_model(&mut self) {
        while let Ok(result) = self.model_ready_rx.try_recv() {
            self.model_loading = None;
            match result {
                Ok(model) => self.prepared_model = Some(model),
                Err(error) => self.model_error = Some(error),
            }
        }
        if self.model_busy() {
            return;
        }
        let matches_selection = self
            .prepared_model
            .as_ref()
            .is_some_and(|model| model.id == self.selected_model);
        if matches_selection {
            if let Some(model) = self.prepared_model.take() {
                let mut config = AppConfig::load();
                config.model_id = model.id.clone();
                match config.save() {
                    Ok(()) => {
                        self.stt_engine.set(Some(model.engine));
                        self.active_model = Some(model.id);
                        self.model_error = None;
                        self.recovery_message = None;
                        self.recovery_is_microphone = false;
                        if matches!(self.status, HudStatus::Error { .. } | HudStatus::NoSpeech) {
                            self.status = HudStatus::Idle;
                        }
                    }
                    Err(error) => {
                        self.model_error = Some(format!(
                            "Could not save the model choice. Try again. {error}"
                        ))
                    }
                }
            }
        }
        let ready_id = match &*self.model_phase.lock() {
            DownloadPhase::Ready { id } => Some(id.clone()),
            _ => None,
        };
        let Some(id) = ready_id else {
            return;
        };
        *self.model_phase.lock() = DownloadPhase::Idle;
        if !self.installed_models.contains(&id) {
            self.installed_models.push(id.clone());
        }
        if self.selected_model == id {
            self.install_engine(&id);
        }
    }

    fn install_engine(&mut self, id: &str) {
        if self.model_loading.is_some() {
            return;
        }
        if preview::is_active() {
            self.active_model = Some(id.into());
            self.preview_ready = Some(true);
            return;
        }
        let spec = models::resolve(id);
        let Some(dir) = models::find_dir(spec, self.model_dir.as_deref()) else {
            self.model_error = Some("Download this model before using it.".into());
            return;
        };
        self.model_loading = Some(id.into());
        self.model_error = None;
        let tx = self.model_ready_tx.clone();
        let ping = self.update_ping.clone();
        std::thread::spawn(move || {
            let result = SttEngine::new(spec, &dir)
                .and_then(|engine| {
                    engine.prepare()?;
                    Ok(PreparedModel {
                        id: spec.id.into(),
                        engine: Arc::new(engine),
                    })
                })
                .map_err(|error| {
                    format!(
                        "Could not load {}. Your previous model is unchanged. {error}",
                        spec.label
                    )
                });
            let _ = tx.send(result);
            let _ = ping.send(());
        });
    }

    fn select_model(&mut self, id: &str) {
        if self.model_busy() || self.model_loading.is_some() {
            self.model_error =
                Some("Wait until transcription finishes, then switch models.".into());
            return;
        }
        let spec = models::resolve(id);
        self.selected_model = spec.id.to_string();
        if self.active_model.as_deref() == Some(id) {
            self.model_error = None;
            return;
        }
        if self
            .installed_models
            .iter()
            .any(|installed| installed == id)
        {
            self.install_engine(spec.id);
        } else {
            self.model_error = None;
        }
    }

    fn start_model_download(&mut self) {
        if preview::is_active() || self.model_loading.is_some() {
            return;
        }
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
        if preview::is_active() {
            return;
        }
        let mut cfg = AppConfig::load();
        cfg.auto_paste = self.auto_paste_enabled;
        let _ = cfg.save();
        *self.auto_paste_state.lock() = self.auto_paste_enabled;
    }

    fn set_remove_fillers(&mut self, on: bool) {
        self.remove_fillers_enabled = on;
        if preview::is_active() {
            return;
        }
        let mut cfg = AppConfig::load();
        cfg.remove_fillers = on;
        let _ = cfg.save();
        self.remove_fillers_state.store(on, Ordering::Relaxed);
    }

    fn toggle_autostart(&mut self) {
        if preview::is_active() {
            self.autostart_enabled = !self.autostart_enabled;
            return;
        }
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
        self.clear_history_pending = false;
        if preview::is_active() {
            self.stats.history.clear();
        } else {
            self.stats.clear_history();
        }
        self.expanded_history_key = None;
        self.copied_key = None;
        self.copied_at = None;
    }

    pub fn set_error(&mut self, message: String) {
        self.recovery_is_microphone = message.to_lowercase().contains("microphone");
        self.recovery_message = Some(message.clone());
        self.status = HudStatus::Error {
            message,
            occurred_at: Instant::now(),
        };
    }

    fn model_ready(&self) -> bool {
        self.preview_ready
            .unwrap_or_else(|| self.stt_engine.is_some())
    }
}

impl Render for HudView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if self.mode == WindowViewMode::StatsAndSettings && self.panel_needs_focus {
            window.focus(&self.panel_focus, cx);
            self.panel_needs_focus = false;
        }
        let body = if self.mode == WindowViewMode::StatsAndSettings {
            let panel = self.render_stats_settings(cx);
            match self.panel_motion.filter(|_| client_animations_enabled()) {
                Some(motion) => {
                    let (open, moving) = motion.openness();
                    if moving {
                        window.request_animation_frame();
                    } else {
                        self.finish_panel_motion();
                    }
                    morph(panel, open, window.viewport_size())
                }
                None => panel,
            }
        } else {
            self.render_bubble(window, cx)
        };
        FocusScope::new(&self.root_focus)
            .root()
            .size_full()
            .child(body)
    }
}

impl HudView {
    fn render_bubble(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> AnyElement {
        let font_family = cx.theme().font_family.clone();
        let ready = self.model_ready();
        // A mouse that leaves through the window edge sends no hover-out to
        // the zone, but the window does know it lost the mouse.
        if !preview::is_active() && !window.is_window_hovered() {
            self.pill_hovered = false;
        }
        let hover_tip = self.pill_hovered && ready && matches!(self.status, HudStatus::Idle);
        let limit_left = match &self.status {
            HudStatus::Listening { started_at, .. } => {
                let elapsed = started_at.elapsed().as_secs();
                let limit = crate::audio::MAX_RECORDING_SECONDS as u64;
                (elapsed >= limit.saturating_sub(10)).then(|| limit.saturating_sub(elapsed))
            }
            _ => None,
        };

        // The message is laid out first: its measured width sizes the pill.
        let message = self.pill_message(ready);
        let message_width = message.as_ref().map(|parts| {
            let widths: f32 = parts
                .iter()
                .map(|part| part.width(window, &font_family))
                .sum();
            let gaps = PILL_GAP * parts.len().saturating_sub(1) as f32;
            (widths + gaps + 2.0 * MESSAGE_PAD).min(MESSAGE_MAX_W)
        });
        let target = match (&self.status, message_width) {
            (_, Some(width)) => (width, MESSAGE_H),
            (HudStatus::Listening { .. }, _) if limit_left.is_some() => (LIVE_W + 30.0, LIVE_H),
            (HudStatus::Listening { .. }, _) => (LIVE_W, LIVE_H),
            (HudStatus::Transcribing { .. }, _) => FINISHING,
            _ if hover_tip => REST_HOVER,
            _ => REST,
        };
        let (size, moving) = self.pill_motion.sample(target);
        if moving {
            window.request_animation_frame();
        }

        // Input reaches only the pill, plus the tip while it shows.
        let span = self.pill_motion.span();
        let hit = (!hover_tip).then(|| {
            let w = span.0 + 2.0 * HIT_PAD;
            let h = span.1 + 2.0 * HIT_PAD;
            let canvas = window.viewport_size();
            ((canvas.width / px(1.0) - w) / 2.0, canvas.height / px(1.0) - h, w, h)
        });
        set_pill_hit_rect(hit);

        let at_rest = matches!(self.status, HudStatus::Idle) && ready && !hover_tip;
        let content: AnyElement = match (&self.status, message) {
            (_, Some(parts)) => div()
                .flex()
                .items_center()
                .gap(px(PILL_GAP))
                .px(px(MESSAGE_PAD))
                .children(
                    parts
                        .into_iter()
                        .map(|part| part.render(window, &font_family)),
                )
                .into_any_element(),
            (HudStatus::Listening { .. }, _) => self.live_wave(limit_left),
            (HudStatus::Transcribing { .. }, _) => finishing_dots(),
            _ => div().into_any_element(),
        };
        let state_key = match &self.status {
            HudStatus::Idle => "pill_rest",
            HudStatus::Listening { .. } => "pill_live",
            HudStatus::Transcribing { .. } => "pill_finishing",
            HudStatus::Success { .. } => "pill_success",
            HudStatus::Error { .. } => "danger",
            HudStatus::NoSpeech => "pill_no_speech",
        };
        let content = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h_full()
            .child(content);
        let content = if client_animations_enabled() {
            content
                .with_animation(
                    state_key,
                    Animation::new(Duration::from_millis(PILL_MOTION_MS)),
                    |element, progress| element.opacity(((progress - 0.35) / 0.65).clamp(0.0, 1.0)),
                )
                .into_any_element()
        } else {
            content.into_any_element()
        };

        let pill = div()
            .id("recording_overlay_pill")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(size.0))
            .h(px(size.1))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .when(at_rest, |pill| {
                pill.bg(rest_bg()).border_color(rest_edge())
            })
            .when(!at_rest, |pill| {
                pill.bg(bg()).border_color(edge())
            })
            .when(self.pill_action().is_some(), |pill| pill.cursor_pointer())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.pill_clicked(cx)),
            )
            .child(content);

        let zone = div()
            .id("pill_zone")
            .flex()
            .flex_col()
            .items_center()
            .gap(px(TIP_GAP))
            .p(px(HIT_PAD))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.pill_hovered != *hovered {
                    this.pill_hovered = *hovered;
                    cx.notify();
                }
            }))
            .when(hover_tip, |zone| zone.child(self.rest_tip(cx)))
            .child(pill);

        div()
            .font_family(font_family)
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .size_full()
            .child(zone)
            .into_any_element()
    }

    /// What the pill says in states that carry words. `None` keeps it wordless.
    fn pill_message(&self, ready: bool) -> Option<Vec<PillPart>> {
        let details = || PillPart::chip("Details");
        Some(match &self.status {
            HudStatus::Idle if !ready => vec![
                PillPart::label("Download a speech model", text()),
                PillPart::chip("Set up"),
            ],
            HudStatus::Success {
                text: said,
                auto_pasted,
                latency_ms,
                ..
            } => {
                let mut parts = vec![
                    PillPart::label(if *auto_pasted { "Pasted" } else { "Copied" }, text()),
                    PillPart::snippet(said, soft(), SNIPPET_MAX_W),
                    PillPart::meta(format_latency_ms(*latency_ms)),
                ];
                if self.recovery_message.is_some() {
                    parts.push(details());
                }
                parts
            }
            HudStatus::Error { message, .. } => vec![
                PillPart::snippet(first_sentence(message), danger(), ERROR_MAX_W),
                details(),
            ],
            HudStatus::NoSpeech => vec![PillPart::label("No speech detected", text()), details()],
            _ => return None,
        })
    }

    /// What a click on the pill does now. `None` while it only reports.
    fn pill_action(&self) -> Option<PillAction> {
        match &self.status {
            HudStatus::Idle if self.model_ready() => Some(PillAction::Start),
            HudStatus::Listening { .. } => Some(PillAction::Stop),
            HudStatus::Idle | HudStatus::Error { .. } | HudStatus::NoSpeech => {
                Some(PillAction::Settings)
            }
            HudStatus::Success { .. } if self.recovery_message.is_some() => {
                Some(PillAction::Settings)
            }
            _ => None,
        }
    }

    fn pill_clicked(&mut self, cx: &mut Context<Self>) {
        match self.pill_action() {
            Some(PillAction::Start) => self.click_dictation = true,
            Some(PillAction::Stop) => self.stop_requested = true,
            Some(PillAction::Settings) => self.open_settings(cx),
            None => {}
        }
        cx.notify();
    }

    /// The live pill: a blue orb and a short white wave, as in Aqua Voice.
    fn live_wave(&self, limit_left: Option<u64>) -> AnyElement {
        let bars = live_peaks(&self.wave_peaks).into_iter().map(|peak| {
            div()
                .flex_none()
                .w(px(LIVE_BAR_W))
                .h(px(LIVE_BAR_MIN + (LIVE_BAR_MAX - LIVE_BAR_MIN) * peak))
                .rounded_full()
                .bg(text())
                .opacity(0.7 + 0.3 * peak)
        });
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(img(orb_image()).flex_none().size(px(LIVE_ORB)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(LIVE_BAR_GAP))
                    .h(px(LIVE_BAR_MAX))
                    .children(bars),
            )
            .when_some(limit_left, |row, left| {
                row.child(
                    div()
                        .text_size(px(11.0))
                        .font_family("Consolas")
                        .text_color(danger())
                        .whitespace_nowrap()
                        .child(format!("{left}s")),
                )
            })
            .into_any_element()
    }

    /// The tip over the resting pill: how to start, and a way into Settings.
    fn rest_tip(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.0))
            .h(px(TIP_H))
            .pl(px(12.0))
            .pr(px(4.0))
            .rounded_full()
            .bg(bg())
            .border_1()
            .border_color(edge())
            .child(
                div()
                    .mr(px(4.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text())
                    .whitespace_nowrap()
                    .child("Click to start dictation"),
            )
            .child(controls::shortcut_keys(&self.hotkey_label))
            .child(
                div()
                    .id("open_settings_btn")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .ml(px(2.0))
                    .size(px(22.0))
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(soft())
                    .hover(|button| button.bg(chip()).text_color(text()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_settings(cx);
                            cx.notify();
                        }),
                    )
                    .child(Icon::new(IconName::Settings).size(IconSize::Xs)),
            )
            .into_any_element()
    }

    fn render_stats_settings(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let font = cx.theme().font_family.clone();
        let tab_key = match self.active_tab {
            SettingsTab::Stats => "stats",
            SettingsTab::Settings => "settings",
        };
        let tabs = cx.entity();
        let panel_tabs = SegmentedControl::new("panel_tabs", tab_key)
            .size(ControlSize::Sm)
            .segment("stats", "Stats", None)
            .segment("settings", "Settings", None)
            .on_change(move |key, _, cx| {
                tabs.update(cx, |this, cx| {
                    this.active_tab = if key.as_ref() == "settings" {
                        SettingsTab::Settings
                    } else {
                        SettingsTab::Stats
                    };
                    cx.notify();
                });
            });

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
                            .bg(theme::transparent())
                            .child(panel_tabs),
                    )
                    .child(
                        controls::icon_button("close_settings_btn", IconName::X, "Close").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.close_stats_settings(cx);
                                cx.notify();
                            }),
                        ),
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
            faded_content.overflow_hidden().child(content)
        } else {
            faded_content.overflow_hidden().child(
                ScrollArea::new("settings_scroll")
                    .shadows()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(content),
            )
        };
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
        let panel_body = div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .px(pad())
            .pt(pad())
            .pb(pad())
            .rounded(r_window())
            .bg(bg())
            .border_1()
            .border_color(edge())
            .overflow_hidden()
            .child(header);

        div()
            .id("stats_settings_container")
            .font_family(font)
            .track_focus(&self.panel_focus)
            .flex()
            .w_full()
            .h_full()
            .relative()
            .rounded(r_window())
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" && !this.hotkey_capturing {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
                if event.keystroke.key == "escape" {
                    if this.hotkey_capturing {
                        this.toggle_hotkey_capture();
                    } else if this.clear_history_pending {
                        this.clear_history_pending = false;
                    } else {
                        this.close_stats_settings(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child({
                let panel = panel_body
                    .when_some(self.recovery_message.clone(), |panel, message| {
                        let retry = self.recovery_is_microphone;
                        let hud = cx.entity();
                        let mut alert = Alert::new("recovery", Severity::Warning, "Action needed")
                            .body(message)
                            .on_dismiss({
                                let hud = hud.clone();
                                move |_, cx| {
                                    hud.update(cx, |this, cx| {
                                        this.recovery_message = None;
                                        this.recovery_is_microphone = false;
                                        if matches!(
                                            this.status,
                                            HudStatus::Error { .. } | HudStatus::NoSpeech
                                        ) {
                                            this.status = HudStatus::Idle;
                                        }
                                        cx.notify();
                                    });
                                }
                            });
                        if retry {
                            let hud = hud.clone();
                            alert = alert.action(
                                controls::secondary_button("retry_microphone", "Retry microphone")
                                    .on_click(move |_, _, cx| {
                                        hud.update(cx, |this, cx| {
                                            this.retry_microphone = true;
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        panel.child(div().flex_none().pb(px(GAP_SECTION)).child(alert))
                    })
                    .child(faded_content);
                panel.into_any_element()
            })
            .into_any_element()
    }

    fn render_stats_tab(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
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
                let latest = idx == 0;
                let text_val = item.text.clone();
                let is_copied = self.copied_key.as_deref() == Some(item.timestamp.as_str());
                let btn_text = if is_copied { "Copied" } else { "Copy" };
                let copy_tx = text_val.clone();
                let click_key = item.timestamp.clone();
                let btn_key = click_key.clone();
                let expanded =
                    self.expanded_history_key.as_deref() == Some(item.timestamp.as_str());
                let can_expand = history_text_overflows(&text_val, latest);

                let copy_icon = if is_copied {
                    IconName::Check
                } else {
                    IconName::Copy
                };
                let copy_btn = IconButton::new(
                    ElementId::Name(format!("history_copy:{}", item.timestamp).into()),
                    copy_icon,
                )
                .variant(ButtonVariant::Ghost)
                .size(ControlSize::Sm)
                .tooltip(btn_text)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.copy_history(&btn_key, &copy_tx);
                    cx.notify();
                }));

                history_items.push(
                    div()
                        .id(ElementId::Name(
                            format!("history_row:{}", item.timestamp).into(),
                        ))
                        .flex()
                        .flex_col()
                        .w_full()
                        .gap(px(4.0))
                        .px(px(10.0))
                        .py(px(10.0))
                        .rounded(r_chip())
                        .border_1()
                        .border_color(theme::transparent())
                        .when(can_expand, |row| {
                            row.tab_index(0)
                                .cursor_pointer()
                                .hover(|style| style.bg(theme::hover()))
                                .active(|style| style.bg(theme::pressed()))
                        })
                        .focus(|style| style.border_color(theme::focus_ring()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if can_expand {
                                this.toggle_history_preview(&click_key);
                            }
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
                                        .text_size(px(TYPE_META))
                                        .font_family("Consolas")
                                        .text_color(rgb(MUTED))
                                        .whitespace_nowrap()
                                        .child(format!(
                                            "{}  -  {}ms",
                                            item.timestamp, item.latency_ms
                                        )),
                                )
                                .child(copy_btn),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_size(px(if latest { TYPE_LABEL } else { 12.0 }))
                                .line_height(px(if latest { 18.0 } else { 16.0 }))
                                .text_color(rgb(TEXT))
                                .when(!expanded, |text| {
                                    text.line_clamp(if latest { 4 } else { 2 }).text_ellipsis()
                                })
                                .child(text_val),
                        )
                        .when(can_expand, |row| {
                            row.child(div().text_size(px(TYPE_META)).text_color(rgb(MUTED)).child(
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
        let mut history_header = div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .min_h(px(28.0))
            .child(controls::group_label("Recent"));
        if has_recents && !self.clear_history_pending {
            history_header = history_header.child(
                controls::icon_button("clear_recents_btn", IconName::Trash2, "Clear history")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_history_pending = true;
                        cx.notify();
                    })),
            );
        }
        let confirm = self.clear_history_pending.then(|| {
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(8.0))
                .p(px(12.0))
                .rounded(r_section())
                .bg(well())
                .child(
                    div()
                        .text_size(px(TYPE_LABEL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text())
                        .child("Clear history?"),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.0))
                        .child(controls::ghost_button("cancel_clear", "Cancel").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.clear_history_pending = false;
                                cx.notify();
                            }),
                        ))
                        .child(controls::danger_button("confirm_clear", "Clear").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.clear_recents();
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element()
        });

        let stats_row = div()
            .flex()
            .w_full()
            .gap(px(8.0))
            .child(controls::stat_tile(
                "Dictations",
                self.stats.total_transcriptions.to_string(),
                IconName::Mic,
            ))
            .child(controls::stat_tile(
                "Words",
                self.stats.total_words.to_string(),
                IconName::AlignLeft,
            ))
            .child(controls::stat_tile(
                "Recorded",
                format_time_saved(self.stats.total_seconds),
                IconName::Timer,
            ));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(GAP_SECTION))
            .child(stats_row)
            .child(history_header)
            .when_some(confirm, |section, card| section.child(card))
            .child(
                ScrollArea::new("history_scroller")
                    .shadows()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .gap(px(GAP_TIGHT))
                            .children(history_items),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_tab(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let is_auto_paste = self.auto_paste_enabled;
        let hotkey_str = self.hotkey_label.clone();

        let settings_row = |title: &'static str, subtitle: String, trailing: AnyElement| {
            controls::line_row(title, subtitle, trailing)
        };

        let hud = cx.entity();
        let auto_paste_switch = Switch::new("auto_paste", is_auto_paste).on_change({
            let hud = hud.clone();
            move |on, _, cx| {
                hud.update(cx, |this, cx| {
                    if this.auto_paste_enabled != on {
                        this.toggle_auto_paste();
                    }
                    cx.notify();
                });
            }
        });
        let auto_paste_row = settings_row(
            "Auto-paste",
            if is_auto_paste {
                "Inserts into the app you were using."
            } else {
                "Copies text to the clipboard."
            }
            .to_string(),
            auto_paste_switch.into_any_element(),
        );

        let fillers_switch =
            Switch::new("remove_fillers", self.remove_fillers_enabled).on_change({
                let hud = hud.clone();
                move |on, _, cx| {
                    hud.update(cx, |this, cx| {
                        if this.remove_fillers_enabled != on {
                            this.set_remove_fillers(on);
                        }
                        cx.notify();
                    });
                }
            });
        let fillers_row = settings_row(
            "Remove filler words",
            if self.remove_fillers_enabled {
                "Drops um, uh, and similar sounds."
            } else {
                "Keeps every word as spoken."
            }
            .to_string(),
            fillers_switch.into_any_element(),
        );

        let startup_switch = Switch::new("start_with_windows", self.autostart_enabled).on_change({
            let hud = hud.clone();
            move |on, _, cx| {
                hud.update(cx, |this, cx| {
                    if this.autostart_enabled != on {
                        this.toggle_autostart();
                    }
                    cx.notify();
                });
            }
        });
        let dictionary_row = settings_row(
            "Dictionary",
            "Names and terms TDT should spell your way.".to_string(),
            controls::secondary_button("dictionary_btn", "Edit")
                .on_click(cx.listener(|this, _, _, cx| {
                    let opened = voice_stt_desktop::stt::dictionary::ensure_user_file()
                        .and_then(|path| crate::ui::window_util::open_file(&path));
                    this.dictionary_error = opened.err();
                    cx.notify();
                }))
                .into_any_element(),
        );
        let startup_row = settings_row(
            "Start with Windows",
            "Opens TDT when you sign in.".to_string(),
            startup_switch.into_any_element(),
        );

        let capturing = self.hotkey_capturing;
        let hotkey_sub = if capturing {
            "Press the new shortcut. Esc cancels.".to_string()
        } else {
            "Tap to start and stop. Hold to keep recording.".to_string()
        };
        let hotkey_control = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(if capturing {
                div()
                    .text_size(px(TYPE_META))
                    .text_color(text())
                    .child("Press keys")
                    .into_any_element()
            } else {
                controls::shortcut_keys(&hotkey_str)
            })
            .child(
                controls::secondary_button(
                    "hotkey_bind_btn",
                    if capturing { "Cancel" } else { "Change" },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_hotkey_capture();
                    window.focus(&this.panel_focus, cx);
                    cx.notify();
                })),
            )
            .into_any_element();
        let hotkey_row = settings_row("Shortcut", hotkey_sub, hotkey_control);

        let guard = self.update.lock();
        let phase = &*guard;
        let (update_sub, update_action, update_busy) = match phase {
            UpdatePhase::Idle => (
                format!("Current version v{}", update::current_version()),
                "Check for updates".to_string(),
                false,
            ),
            UpdatePhase::Checking => (
                "Checking for updates...".to_string(),
                "Checking".to_string(),
                true,
            ),
            UpdatePhase::UpToDate => (
                format!("You're up to date - v{}", update::current_version()),
                "Check again".to_string(),
                false,
            ),
            UpdatePhase::Available { version, .. } => (
                format!("v{version} is ready."),
                "Download and restart".to_string(),
                false,
            ),
            UpdatePhase::Downloading { done, total } => {
                let pct = models::percent(*done, *total);
                (
                    format!(
                        "{}% - {} of {}",
                        pct,
                        models::format_mb(*done),
                        models::format_mb(*total)
                    ),
                    format!("{pct}%"),
                    true,
                )
            }
            UpdatePhase::Ready { .. } => (
                "Update downloaded. Install when ready.".to_string(),
                "Install update".to_string(),
                false,
            ),
            UpdatePhase::Restarting => (
                "Restarting into the new version...".to_string(),
                "Restarting".to_string(),
                true,
            ),
            UpdatePhase::Failed(error) => (clip_text(error, 48), "Try again".to_string(), false),
        };

        let update_btn = if matches!(
            *phase,
            UpdatePhase::Available { .. } | UpdatePhase::Ready { .. }
        ) {
            controls::primary_button("check_updates_btn", update_action)
        } else {
            controls::secondary_button("check_updates_btn", update_action)
        };
        let update_btn =
            update_btn
                .disabled(update_busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if matches!(
                        *this.update.lock(),
                        UpdatePhase::Checking
                            | UpdatePhase::Downloading { .. }
                            | UpdatePhase::Restarting
                    ) {
                        return;
                    }
                    this.start_update_install();
                    cx.notify();
                }));
        let update_meter = match phase {
            UpdatePhase::Downloading { done, total } => {
                Some(controls::progress_bar("update_progress", *done, *total))
            }
            _ => None,
        };
        drop(guard);
        let update_row = settings_row("Updates", update_sub, update_btn.into_any_element());

        let spec = models::resolve(&self.selected_model);
        let installed = self.installed_models.iter().any(|id| id == spec.id);
        let model_guard = self.model_phase.lock();
        let model_phase = &*model_guard;
        let downloading = matches!(
            model_phase,
            DownloadPhase::Downloading { id, .. } if id == spec.id
        );
        let model_sub = match model_phase {
            DownloadPhase::Downloading {
                id,
                file,
                file_index,
                file_count,
                ..
            } if id == spec.id => format!(
                "Downloading {} ({file_index} of {file_count}).",
                models::file_label(file)
            ),
            DownloadPhase::Failed { id, message } if id == spec.id => message.clone(),
            _ if self.model_loading.as_deref() == Some(spec.id) => {
                match self
                    .active_model
                    .as_deref()
                    .filter(|active| *active != spec.id)
                {
                    Some(active) => format!(
                        "Loading {}. {} keeps running until it is ready.",
                        spec.label,
                        models::resolve(active).label
                    ),
                    None => format!("Loading {}.", spec.label),
                }
            }
            _ if self.active_model.as_deref() == Some(spec.id) => {
                format!("{} is active.", spec.label)
            }
            _ if installed => "Installed. Not in use yet.".to_string(),
            _ => format!("Not downloaded. {}", spec.size_label),
        };
        let selected_model = self.selected_model.clone();
        let model_chips = ChoiceChips::new(
            "model",
            CATALOG
                .iter()
                .map(|option| Choice::new(option.id, option.label)),
        )
        .selected([selected_model.as_str()])
        .on_change({
            let hud = hud.clone();
            move |values, _, cx| {
                let Some(id) = values.first().cloned() else {
                    return;
                };
                hud.update(cx, |this, cx| {
                    if this.selected_model != id.as_ref() {
                        this.select_model(&id);
                    }
                    cx.notify();
                });
            }
        });
        let download_action = match model_phase {
            DownloadPhase::Downloading {
                id, done, total, ..
            } if id == spec.id => {
                format!("{}%", models::percent(*done, *total))
            }
            DownloadPhase::Failed { id, .. } if id == spec.id => "Try again".to_string(),
            _ => "Download".to_string(),
        };
        let show_download = !installed
            || matches!(
                model_phase,
                DownloadPhase::Failed { id, .. } if id == spec.id
            );
        let model_meter = match model_phase {
            DownloadPhase::Downloading {
                id, done, total, ..
            } if id == spec.id => Some(controls::progress_bar("model_progress", *done, *total)),
            _ => None,
        };
        drop(model_guard);
        let running_note = if self.model_loading.as_deref() != Some(spec.id) {
            self.active_model
                .as_ref()
                .filter(|active| active.as_str() != self.selected_model)
                .map(|active| {
                    if installed {
                        format!("{} is still running.", models::resolve(active).label)
                    } else {
                        format!(
                            "{} keeps running until you download this model.",
                            models::resolve(active).label
                        )
                    }
                })
        } else {
            None
        };
        let show_activate = installed
            && self.active_model.as_deref() != Some(spec.id)
            && self.model_loading.is_none();

        let model_actions = div()
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .when(show_activate, |row| {
                row.child(
                    controls::secondary_button("activate_model", "Use this model").on_click(
                        cx.listener(|this, _, _, cx| {
                            let id = this.selected_model.clone();
                            this.install_engine(&id);
                            cx.notify();
                        }),
                    ),
                )
            })
            .when(show_download, |row| {
                row.child(
                    controls::secondary_button("download_model_btn", download_action)
                        .loading(downloading)
                        .disabled(downloading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_model_download();
                            cx.notify();
                        })),
                )
            });

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(GAP_SECTION))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(GAP_TIGHT))
                    .child(controls::group_label("Dictation"))
                    .child(hotkey_row)
                    .child(auto_paste_row)
                    .child(fillers_row),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(GAP_TIGHT))
                    .child(controls::group_label("Recognition"))
                    .child(controls::labeled("Model", model_sub))
                    .child(model_chips)
                    .when_some(running_note, |column, note| {
                        column.child(InlineMessage::new(Severity::Info, note))
                    })
                    .when_some(model_meter, |column, meter| column.child(meter))
                    .when(show_activate || show_download, |column| {
                        column.child(model_actions)
                    })
                    .child(dictionary_row)
                    .when_some(self.dictionary_error.clone(), |column, error| {
                        column.child(InlineMessage::new(Severity::Danger, error))
                    }),
            )
            .when_some(self.model_error.clone(), |column, error| {
                column.child(InlineMessage::new(Severity::Danger, error))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .gap(px(GAP_TIGHT))
                    .child(controls::group_label("App"))
                    .child(startup_row)
                    .child(update_row)
                    .when_some(update_meter, |column, meter| column.child(meter)),
            )
            .when_some(self.autostart_error.clone(), |column, error| {
                column.child(InlineMessage::new(Severity::Danger, error))
            })
            .into_any_element()
    }
}

/// A finished dictation returns to Ready. A notice stays long enough to read, then leaves.
pub fn success_returns_to_idle(elapsed: Duration, has_notice: bool) -> bool {
    let hold = if has_notice {
        Duration::from_secs(4)
    } else {
        Duration::from_millis(1800)
    };
    elapsed > hold
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

fn history_text_overflows(text: &str, latest: bool) -> bool {
    let limit = if latest {
        LATEST_EXPAND_CHARS
    } else {
        HISTORY_EXPAND_CHARS
    };
    text.chars().count() > limit
}

// Compile-time invariant: the painted settings surface must stay inset within
// the pill so its highlight never collides with the border and 14px corners.
const _: () = assert!(SETTINGS_SURFACE_HEIGHT >= 24.0);
