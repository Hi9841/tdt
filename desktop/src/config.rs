use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn config_dir() -> Option<PathBuf> {
    let new = directories::ProjectDirs::from("com", "tdt", "TDT")
        .map(|dirs| dirs.config_dir().to_path_buf());
    if let Some(new_dir) = &new {
        if !new_dir.join("config.json").exists() && !new_dir.join("stats.json").exists() {
            if let Some(old_dir) = directories::ProjectDirs::from("com", "voicestt", "VoiceSTT")
                .map(|dirs| dirs.config_dir().to_path_buf())
            {
                for name in ["config.json", "stats.json"] {
                    let old = old_dir.join(name);
                    let newer = new_dir.join(name);
                    if old.exists() && !newer.exists() {
                        let _ = fs::create_dir_all(new_dir);
                        let _ = fs::copy(&old, &newer);
                    }
                }
            }
        }
    }
    new
}

fn load_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<Option<T>, ()> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|_| ())?;
    match serde_json::from_str(&content) {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            let backup = path.with_extension("json.bak");
            let _ = fs::copy(path, &backup);
            eprintln!(
                "TDT: could not parse {} ({error}); backed up to {}",
                path.display(),
                backup.display()
            );
            Err(())
        }
    }
}

fn write_json_atomic<T: Serialize>(
    path: &std::path::Path,
    value: &T,
) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub auto_paste: bool,
    pub hotkey: String,
    pub model_dir: Option<PathBuf>,
    pub model_id: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            auto_paste: true,
            hotkey: "Ctrl+;".to_string(),
            model_dir: None,
            model_id: voice_stt_desktop::stt::DEFAULT_MODEL_ID.to_string(),
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let base = config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        match load_json(&path) {
            Ok(Some(config)) => {
                let mut config: AppConfig = config;
                config.sanitize();
                config
            }
            Ok(None) => Self::default(),
            Err(()) => Self::default(),
        }
    }

    fn sanitize(&mut self) {
        if self.hotkey.trim().is_empty() {
            self.hotkey = AppConfig::default().hotkey;
        }
        if self.model_id.trim().is_empty() {
            self.model_id = AppConfig::default().model_id;
        }
        if let Some(dir) = &self.model_dir {
            if !dir.is_dir() {
                self.model_dir = None;
            }
        }
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        write_json_atomic(&Self::config_path(), self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub text: String,
    pub latency_ms: u64,
    pub timestamp: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppStats {
    pub total_words: usize,
    pub total_seconds: f32,
    pub total_transcriptions: usize,
    pub last_latency_ms: u64,
    pub history: Vec<HistoryItem>,
}

/// How many recent transcripts stay visible in the stats tab.
pub const MAX_HISTORY: usize = 10;

impl AppStats {
    pub fn stats_path() -> PathBuf {
        let base = config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("stats.json")
    }

    pub fn load() -> Self {
        let path = Self::stats_path();
        match load_json(&path) {
            Ok(Some(stats)) => stats,
            _ => Self::default(),
        }
    }

    /// Update the running totals in memory. Callers decide when to persist, so
    /// this stays pure and testable.
    pub fn record(&mut self, text: &str, duration_secs: f32, latency_ms: u64) {
        let words = text.split_whitespace().count();
        self.total_words += words;
        self.total_seconds += duration_secs;
        self.total_transcriptions += 1;
        self.last_latency_ms = latency_ms;

        let time_str = local_hms();

        self.history.insert(
            0,
            HistoryItem {
                text: text.to_string(),
                latency_ms,
                timestamp: time_str,
            },
        );

        if self.history.len() > MAX_HISTORY {
            self.history.truncate(MAX_HISTORY);
        }
    }

    /// Record a transcript and persist the result.
    pub fn record_and_save(&mut self, text: &str, duration_secs: f32, latency_ms: u64) {
        self.record(text, duration_secs, latency_ms);
        let _ = self.save();
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        let _ = self.save();
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        write_json_atomic(&Self::stats_path(), self)
    }
}

pub(crate) fn local_hms() -> String {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            millisecond: u16,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }

        let mut time = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            millisecond: 0,
        };
        unsafe {
            GetLocalTime(&mut time);
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            time.year, time.month, time.day, time.hour, time.minute, time.second
        )
    }

    #[cfg(not(windows))]
    {
        let now = std::time::SystemTime::now();
        let duration = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();
        format!(
            "{:02}:{:02}:{:02}",
            (secs / 3600) % 24,
            (secs / 60) % 60,
            secs % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, AppStats, MAX_HISTORY};

    #[test]
    fn record_counts_words_and_sessions() {
        let mut stats = AppStats::default();
        stats.record("hello there world", 2.5, 120);
        stats.record("bye", 0.5, 80);

        assert_eq!(stats.total_words, 4);
        assert_eq!(stats.total_transcriptions, 2);
        assert_eq!(stats.last_latency_ms, 80);
        assert!((stats.total_seconds - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn newest_transcript_is_first_and_history_is_bounded() {
        let mut stats = AppStats::default();
        for index in 0..MAX_HISTORY + 5 {
            stats.record(&format!("entry {index}"), 0.1, 50);
        }

        assert_eq!(stats.history.len(), MAX_HISTORY);
        assert_eq!(stats.history[0].text, format!("entry {}", MAX_HISTORY + 4));
    }

    #[test]
    fn partial_config_json_keeps_defaults_for_missing_fields() {
        let config: AppConfig = serde_json::from_str(r#"{"auto_paste": false}"#).unwrap();
        assert!(!config.auto_paste);
        assert_eq!(config.hotkey, AppConfig::default().hotkey);
        assert_eq!(config.model_id, voice_stt_desktop::stt::DEFAULT_MODEL_ID);
    }

    #[test]
    fn stats_round_trip_keeps_history() {
        let mut stats = AppStats::default();
        stats.record("first", 1.0, 10);
        stats.record("second", 2.0, 20);

        let json = serde_json::to_string(&stats).unwrap();
        let restored: AppStats = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.history.len(), 2);
        assert_eq!(restored.history[0].text, "second");
        assert_eq!(restored.total_words, 2);
    }
}
