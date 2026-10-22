//! Frozen UI states for visual capture. Active only when `TDT_PREVIEW_STATE` is set.

use crate::config::{AppStats, HistoryItem};
use crate::update::{Release, UpdatePhase};

pub fn is_active() -> bool {
    requested().is_some()
}

/// Native tray verification still uses isolated config, audio, and hotkeys.
pub fn wants_tray() -> bool {
    is_active() && std::env::var_os("TDT_PREVIEW_TRAY").is_some()
}

pub fn requested() -> Option<String> {
    std::env::var("TDT_PREVIEW_STATE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn wants_panel() -> bool {
    requested().is_some_and(|value| value.starts_with("panel."))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    BubbleIdle,
    BubbleHover,
    BubbleListening { loud: bool },
    BubbleTranscribing,
    BubbleSuccess { pasted: bool },
    BubbleError,
    BubbleSetup,
    BubbleNoSpeech,
    BubbleCopyFailed,
    BubblePasteFallback,
    BubbleLimit,
    PanelSetup,
    PanelModelFailed,
    PanelRecovery,
    PanelClearHistory,
    PanelStatsEmpty,
    PanelStatsHistory,
    PanelStatsOverflow,
    PanelStatsExpanded,
    PanelStatsCopied,
    PanelSettingsDefaults,
    PanelSettingsAutoPasteOff,
    PanelSettingsStartupOn,
    PanelSettingsStartupError,
    PanelSettingsHotkeyCapture,
    PanelSettingsUpdateChecking,
    PanelSettingsUpdateUpToDate,
    PanelSettingsUpdateAvailable,
    PanelSettingsUpdateDownloading,
    PanelSettingsUpdateInstalling,
    PanelSettingsUpdateFailed,
}

pub fn parse() -> Option<Spec> {
    parse_name(requested()?.as_str())
}

pub fn parse_name(name: &str) -> Option<Spec> {
    Some(match name {
        "bubble.idle" => Spec::BubbleIdle,
        "bubble.hover" => Spec::BubbleHover,
        "bubble.listening.quiet" => Spec::BubbleListening { loud: false },
        "bubble.listening.loud" => Spec::BubbleListening { loud: true },
        "bubble.transcribing" => Spec::BubbleTranscribing,
        "bubble.success.copied" => Spec::BubbleSuccess { pasted: false },
        "bubble.success.pasted" => Spec::BubbleSuccess { pasted: true },
        "bubble.error" => Spec::BubbleError,
        "bubble.setup" => Spec::BubbleSetup,
        "bubble.no-speech" => Spec::BubbleNoSpeech,
        "bubble.copy-failed" => Spec::BubbleCopyFailed,
        "bubble.paste-fallback" => Spec::BubblePasteFallback,
        "bubble.limit" => Spec::BubbleLimit,
        "panel.setup" => Spec::PanelSetup,
        "panel.model.failed" => Spec::PanelModelFailed,
        "panel.recovery" => Spec::PanelRecovery,
        "panel.history.clear" => Spec::PanelClearHistory,
        "panel.stats.empty" => Spec::PanelStatsEmpty,
        "panel.stats.history" => Spec::PanelStatsHistory,
        "panel.stats.overflow" => Spec::PanelStatsOverflow,
        "panel.stats.expanded" => Spec::PanelStatsExpanded,
        "panel.stats.copied" => Spec::PanelStatsCopied,
        "panel.settings.defaults" => Spec::PanelSettingsDefaults,
        "panel.settings.autopaste.off" => Spec::PanelSettingsAutoPasteOff,
        "panel.settings.startup.on" => Spec::PanelSettingsStartupOn,
        "panel.settings.startup.error" => Spec::PanelSettingsStartupError,
        "panel.settings.hotkey.capture" => Spec::PanelSettingsHotkeyCapture,
        "panel.settings.update.checking" => Spec::PanelSettingsUpdateChecking,
        "panel.settings.update.uptodate" => Spec::PanelSettingsUpdateUpToDate,
        "panel.settings.update.available" => Spec::PanelSettingsUpdateAvailable,
        "panel.settings.update.downloading" => Spec::PanelSettingsUpdateDownloading,
        "panel.settings.update.installing" => Spec::PanelSettingsUpdateInstalling,
        "panel.settings.update.failed" => Spec::PanelSettingsUpdateFailed,
        _ => return None,
    })
}

pub fn fixture_stats(count: usize) -> AppStats {
    // Real dictations, so captures of the panel show TDT in use. The first
    // runs past the four lines the latest row shows, for the expand control.
    const TEXTS: [&str; 8] = [
        "Hi Sam, thanks for the notes from today. I moved the design review to Thursday at 3 PM so the whole team can join. Can you send the updated mockups before then? I also put the open questions about the onboarding flow in the shared doc, so we can go through them first.",
        "Restart the Kubernetes pod, then check the PostgreSQL logs.",
        "Remind me to book the flight to Berlin before Friday.",
        "The build passes now. I will merge the pull request after lunch.",
        "Order more coffee for the office, the dark roast.",
        "Move the standup to 9:30 tomorrow.",
        "Send the invoice to Dana and copy the finance team.",
        "Call the dentist on Monday morning.",
    ];
    let history = (0..count)
        .map(|index| HistoryItem {
            text: TEXTS[index % TEXTS.len()].to_string(),
            latency_ms: 90 + (index as u64) * 15,
            timestamp: format!(
                "2026-10-06 12:{:02}:{:02}",
                (index * 15) / 60,
                (index * 15) % 60
            ),
        })
        .collect();
    AppStats {
        total_words: 128.max(count * 6),
        total_seconds: 42.5,
        total_transcriptions: count.max(1),
        last_latency_ms: 180,
        history,
    }
}

pub fn update_phase(spec: &Spec) -> Option<UpdatePhase> {
    Some(match spec {
        Spec::PanelSettingsUpdateChecking => UpdatePhase::Checking,
        Spec::PanelSettingsUpdateUpToDate => UpdatePhase::UpToDate,
        Spec::PanelSettingsUpdateAvailable => UpdatePhase::Available(Release::preview("0.4.1")),
        Spec::PanelSettingsUpdateDownloading => UpdatePhase::Downloading {
            done: 12_000_000,
            total: 28_000_000,
        },
        Spec::PanelSettingsUpdateInstalling => UpdatePhase::Installing,
        Spec::PanelSettingsUpdateFailed => UpdatePhase::Failed("Could not reach GitHub.".into()),
        Spec::PanelSettingsDefaults
        | Spec::PanelSettingsAutoPasteOff
        | Spec::PanelSettingsStartupOn
        | Spec::PanelSettingsStartupError
        | Spec::PanelSettingsHotkeyCapture => UpdatePhase::Idle,
        _ => return None,
    })
}
