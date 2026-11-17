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

/// The file as a value, or `None` when there is no file. A file that TDT
/// cannot read is an error: the log has the cause, and a copy of a damaged
/// file is at the path that the error gives.
fn load_json<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Result<Option<T>, std::path::PathBuf> {
    if !path.exists() {
        return Ok(None);
    }
    let backup = path.with_extension("json.bak");
    let content = fs::read_to_string(path).map_err(|error| {
        voice_stt_desktop::log::note(&format!("Could not read {}: {error}", path.display()));
        backup.clone()
    })?;
    match serde_json::from_str(&content) {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            let _ = fs::copy(path, &backup);
            voice_stt_desktop::log::note(&format!(
                "Could not parse {} ({error}). A copy is at {}.",
                path.display(),
                backup.display()
            ));
            Err(backup)
        }
    }
}

/// Set when `config.json` was damaged and the settings went back to the
/// defaults. The pill shows it one time.
static RESET_NOTICE: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

/// The notice of a settings reset, one time.
pub fn take_reset_notice() -> Option<String> {
    RESET_NOTICE.lock().take()
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
    /// Drop "um", "uh", and similar sounds from transcripts.
    pub remove_fillers: bool,
    /// Look for a new release at start, every hour, and when the panel opens.
    pub check_updates: bool,
    pub hotkey: String,
    pub model_dir: Option<PathBuf>,
    pub model_id: String,
    /// Keep each take's audio and text in `%LOCALAPPDATA%\TDT\takes` for testing.
    pub save_takes: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            auto_paste: true,
            remove_fillers: true,
            check_updates: true,
            hotkey: crate::hotkey::HotkeyBinding::DEFAULT.display(),
            model_dir: None,
            model_id: voice_stt_desktop::stt::DEFAULT_MODEL_ID.to_string(),
            save_takes: false,
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
            Err(backup) => {
                *RESET_NOTICE.lock() = Some(format!(
                    "TDT could not read its settings file and uses the default settings. \
                     A copy of the old file is at {}.",
                    backup.display()
                ));
                Self::default()
            }
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

/// The settings of this run. The UI changes them, and the threads of a take
/// read them. Each change goes to `config.json` first, so the app never runs
/// with a setting that the file does not hold.
#[derive(Clone)]
pub struct Settings {
    live: std::sync::Arc<parking_lot::Mutex<AppConfig>>,
    /// False for the preview captures: they never write the file.
    persist: bool,
}

impl Settings {
    pub fn new(config: AppConfig, persist: bool) -> Self {
        Self {
            live: std::sync::Arc::new(parking_lot::Mutex::new(config)),
            persist,
        }
    }

    pub fn get<T>(&self, read: impl FnOnce(&AppConfig) -> T) -> T {
        read(&self.live.lock())
    }

    /// Applies `change` to the file, and then to this run. The file is read
    /// again first, so a change keeps the other values in the file. When the
    /// file cannot be written, this run keeps its settings.
    pub fn update(&self, change: impl Fn(&mut AppConfig)) -> Result<(), std::io::Error> {
        if self.persist {
            let mut file = AppConfig::load();
            change(&mut file);
            file.save()?;
        }
        change(&mut self.live.lock());
        Ok(())
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

    /// Loads stats.json, applies `change`, and saves it, as one step. The
    /// transcribe thread records and the UI thread clears History: without
    /// the lock, one save could undo the other.
    pub fn update(change: impl FnOnce(&mut Self)) -> Self {
        static FILE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        let _file = FILE.lock();
        let mut stats = Self::load();
        change(&mut stats);
        let _ = stats.save();
        stats
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        write_json_atomic(&Self::stats_path(), self)
    }
}

pub(crate) fn local_hms() -> String {
    // SAFETY: GetLocalTime takes no pointer and returns the time by value.
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}
