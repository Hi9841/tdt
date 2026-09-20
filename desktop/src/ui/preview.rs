//! Frozen UI states for visual capture. Active only when `TDT_PREVIEW_STATE` is set.

use crate::config::{AppStats, HistoryItem};
use crate::update::UpdatePhase;
use std::path::PathBuf;

pub fn is_active() -> bool {
    requested().is_some()
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
    PanelSettingsLang(&'static str),
    PanelSettingsHotkeyCapture,
    PanelSettingsUpdateChecking,
    PanelSettingsUpdateUpToDate,
    PanelSettingsUpdateAvailable,
    PanelSettingsUpdateDownloading,
    PanelSettingsUpdateReady,
    PanelSettingsUpdateFailed,
}

/// Every named state the capture catalog launches.
#[allow(dead_code)]
pub const CATALOG: &[&str] = &[
    "bubble.idle",
    "bubble.listening.quiet",
    "bubble.listening.loud",
    "bubble.transcribing",
    "bubble.success.copied",
    "bubble.success.pasted",
    "bubble.error",
    "bubble.setup",
    "bubble.no-speech",
    "bubble.copy-failed",
    "bubble.paste-fallback",
    "bubble.limit",
    "panel.setup",
    "panel.model.failed",
    "panel.recovery",
    "panel.history.clear",
    "panel.stats.empty",
    "panel.stats.history",
    "panel.stats.overflow",
    "panel.stats.expanded",
    "panel.stats.copied",
    "panel.settings.defaults",
    "panel.settings.autopaste.off",
    "panel.settings.startup.on",
    "panel.settings.startup.error",
    "panel.settings.lang.en",
    "panel.settings.lang.zh",
    "panel.settings.lang.ja",
    "panel.settings.lang.ko",
    "panel.settings.lang.yue",
    "panel.settings.hotkey.capture",
    "panel.settings.update.checking",
    "panel.settings.update.uptodate",
    "panel.settings.update.available",
    "panel.settings.update.downloading",
    "panel.settings.update.ready",
    "panel.settings.update.failed",
];

pub fn parse() -> Option<Spec> {
    parse_name(requested()?.as_str())
}

pub fn parse_name(name: &str) -> Option<Spec> {
    Some(match name {
        "bubble.idle" => Spec::BubbleIdle,
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
        "panel.settings.lang.en" => Spec::PanelSettingsLang("en"),
        "panel.settings.lang.zh" => Spec::PanelSettingsLang("zh"),
        "panel.settings.lang.ja" => Spec::PanelSettingsLang("ja"),
        "panel.settings.lang.ko" => Spec::PanelSettingsLang("ko"),
        "panel.settings.lang.yue" => Spec::PanelSettingsLang("yue"),
        "panel.settings.hotkey.capture" => Spec::PanelSettingsHotkeyCapture,
        "panel.settings.update.checking" => Spec::PanelSettingsUpdateChecking,
        "panel.settings.update.uptodate" => Spec::PanelSettingsUpdateUpToDate,
        "panel.settings.update.available" => Spec::PanelSettingsUpdateAvailable,
        "panel.settings.update.downloading" => Spec::PanelSettingsUpdateDownloading,
        "panel.settings.update.ready" => Spec::PanelSettingsUpdateReady,
        "panel.settings.update.failed" => Spec::PanelSettingsUpdateFailed,
        _ => return None,
    })
}

pub fn fixture_stats(count: usize) -> AppStats {
    let long = "This is a longer transcript that wraps onto a second and third line so expand versus collapse is visible in the stats list.";
    let history = (0..count)
        .map(|index| HistoryItem {
            text: if index == 0 {
                long.to_string()
            } else {
                format!("Short transcript {index}: ready when you are")
            },
            latency_ms: 90 + (index as u64) * 15,
            timestamp: format!("12:{:02}:{:02}", index, 10 + index),
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
        Spec::PanelSettingsUpdateAvailable => UpdatePhase::Available {
            version: "0.2.0".into(),
            asset_url: "https://example.invalid/TDT.exe".into(),
            asset_name: "TDT.exe".into(),
            sums_url: None,
        },
        Spec::PanelSettingsUpdateDownloading => UpdatePhase::Downloading {
            done: 12_000_000,
            total: 28_000_000,
        },
        Spec::PanelSettingsUpdateReady => UpdatePhase::Ready {
            installer: PathBuf::from(r"C:\TDT-Setup.exe"),
        },
        Spec::PanelSettingsUpdateFailed => UpdatePhase::Failed("Could not reach GitHub.".into()),
        Spec::PanelSettingsDefaults
        | Spec::PanelSettingsAutoPasteOff
        | Spec::PanelSettingsStartupOn
        | Spec::PanelSettingsStartupError
        | Spec::PanelSettingsLang(_)
        | Spec::PanelSettingsHotkeyCapture => UpdatePhase::Idle,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_name, CATALOG};

    #[test]
    fn catalog_names_parse() {
        for name in CATALOG {
            assert!(parse_name(name).is_some(), "{name}");
        }
    }

    #[test]
    fn unknown_name_is_none() {
        assert!(parse_name("not.a.state").is_none());
    }
}
