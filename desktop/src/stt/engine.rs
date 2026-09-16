use parking_lot::Mutex;
use sherpa_onnx::{
    OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct SttEngine {
    recognizer: Arc<Mutex<Option<OfflineRecognizer>>>,
    model_dir: PathBuf,
    current_language: Arc<Mutex<String>>,
}

impl SttEngine {
    pub fn new(model_dir: &Path, language: &str) -> Result<Self, String> {
        let model_path = Self::model_path(model_dir);
        let tokens_path = model_dir.join("tokens.txt");

        if !model_path.exists() {
            return Err(format!(
                "SenseVoice model file not found at: {} (or model.int8.onnx)",
                model_path.display()
            ));
        }
        if !tokens_path.exists() {
            return Err(format!(
                "SenseVoice tokens file not found at: {}",
                tokens_path.display()
            ));
        }

        let lang_str = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };

        Ok(Self {
            // The 228 MB model is loaded when recording starts, then released after
            // transcription so the idle overlay stays lightweight.
            recognizer: Arc::new(Mutex::new(None)),
            model_dir: model_dir.to_path_buf(),
            current_language: Arc::new(Mutex::new(lang_str)),
        })
    }

    fn model_path(model_dir: &Path) -> PathBuf {
        if model_dir.join("model.int8.onnx").exists() {
            model_dir.join("model.int8.onnx")
        } else {
            model_dir.join("model.onnx")
        }
    }

    fn create_recognizer(&self) -> Result<OfflineRecognizer, String> {
        let model_path = Self::model_path(&self.model_dir);
        let tokens_path = self.model_dir.join("tokens.txt");
        let language = self.current_language.lock().clone();

        let sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(model_path.to_string_lossy().to_string()),
            language: Some(language),
            use_itn: true,
        };

        let model_config = OfflineModelConfig {
            sense_voice,
            tokens: Some(tokens_path.to_string_lossy().to_string()),
            num_threads: 2,
            debug: false,
            provider: Some("cpu".to_string()),
            model_type: Some("sense_voice".to_string()),
            ..Default::default()
        };

        let config = OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        };

        OfflineRecognizer::create(&config)
            .ok_or_else(|| "Failed to initialize Sherpa-ONNX SenseVoice recognizer".to_string())
    }

    pub fn prepare(&self) -> Result<(), String> {
        let mut guard = self.recognizer.lock();
        if guard.is_none() {
            *guard = Some(self.create_recognizer()?);
        }
        Ok(())
    }

    pub fn release(&self) {
        *self.recognizer.lock() = None;
    }

    pub fn set_language(&self, language: &str) -> Result<(), String> {
        let language = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };

        let mut current_lang = self.current_language.lock();
        *current_lang = language.clone();
        drop(current_lang);

        // Apply the new language the next time the model is prepared.
        self.release();

        println!("SenseVoice language set to: {}", language);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn current_language(&self) -> String {
        self.current_language.lock().clone()
    }

    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        self.prepare()?;

        let result = {
            let recognizer_guard = self.recognizer.lock();
            let recognizer = recognizer_guard
                .as_ref()
                .ok_or_else(|| "STT recognizer not initialized".to_string())?;

            let stream = recognizer.create_stream();
            stream.accept_waveform(16000, samples);
            recognizer.decode(&stream);

            stream
                .get_result()
                .map(|result| result.text.trim().to_string())
                .unwrap_or_default()
        };

        self.release();

        Ok(result)
    }

    #[cfg(test)]
    fn is_loaded(&self) -> bool {
        self.recognizer.lock().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::SttEngine;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn constructing_engine_does_not_load_model() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        let model_dir = std::env::temp_dir().join(format!("voice-stt-lazy-model-{unique}"));
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        fs::write(model_dir.join("model.int8.onnx"), b"placeholder")
            .expect("placeholder model should be written");
        fs::write(model_dir.join("tokens.txt"), b"placeholder")
            .expect("placeholder tokens should be written");

        let engine = SttEngine::new(&model_dir, "auto")
            .expect("construction should validate paths without loading ONNX");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }
}
