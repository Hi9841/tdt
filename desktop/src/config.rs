use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub auto_paste: bool,
    pub hotkey: String,
    pub model_dir: Option<PathBuf>,
    pub language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            auto_paste: true,
            hotkey: "Ctrl+;".to_string(),
            model_dir: None,
            language: "auto".to_string(),
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let base = directories::ProjectDirs::from("com", "voicestt", "VoiceSTT")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn find_model_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.model_dir {
            if dir.join("model.onnx").exists() || dir.join("model.int8.onnx").exists() {
                return Some(dir.clone());
            }
        }

        let mut candidates = Vec::new();

        if let Some(dir) = std::env::var_os("VOICE_STT_MODEL_DIR") {
            candidates.push(PathBuf::from(dir));
        }

        if let Ok(current_dir) = std::env::current_dir() {
            candidates.push(current_dir.join("models/sensevoice"));
            candidates.push(current_dir.join("../models/sensevoice"));
            candidates.push(current_dir.join("sensevoice"));
        }

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                candidates.push(exe_dir.join("models/sensevoice"));
                candidates.push(exe_dir.join("../models/sensevoice"));
                candidates.push(exe_dir.join("../../models/sensevoice"));
                candidates.push(exe_dir.join("../../../models/sensevoice"));
            }
        }

        for candidate in &candidates {
            if candidate.join("model.onnx").exists() || candidate.join("model.int8.onnx").exists() {
                if let Ok(canon) = candidate.canonicalize() {
                    return Some(canon);
                }
                return Some(candidate.clone());
            }
        }

        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub text: String,
    pub latency_ms: u64,
    pub timestamp: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppStats {
    pub total_words: usize,
    pub total_seconds: f32,
    pub total_transcriptions: usize,
    pub last_latency_ms: u64,
    pub history: Vec<HistoryItem>,
}

impl AppStats {
    pub fn stats_path() -> PathBuf {
        let base = directories::ProjectDirs::from("com", "voicestt", "VoiceSTT")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("stats.json")
    }

    pub fn load() -> Self {
        let path = Self::stats_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(stats) = serde_json::from_str(&content) {
                    return stats;
                }
            }
        }
        Self::default()
    }

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

        if self.history.len() > 10 {
            self.history.truncate(10);
        }

        let _ = self.save();
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::stats_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, json)?;
        Ok(())
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
        format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second)
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
