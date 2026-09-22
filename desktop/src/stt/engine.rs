use super::models::{whisper_language, ModelFamily, ModelSpec};
use parking_lot::Mutex;
use sherpa_onnx::{
    OfflineModelConfig, OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSenseVoiceModelConfig, OfflineTransducerModelConfig, OfflineWhisperModelConfig,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

enum ModelPaths {
    SenseVoice {
        model: PathBuf,
    },
    Whisper {
        encoder: PathBuf,
        decoder: PathBuf,
    },
    ParakeetTransducer {
        encoder: PathBuf,
        decoder: PathBuf,
        joiner: PathBuf,
    },
    Moonshine {
        preprocessor: PathBuf,
        encoder: PathBuf,
        uncached_decoder: PathBuf,
        cached_decoder: PathBuf,
    },
}

pub struct SttEngine {
    recognizer: Arc<Mutex<Option<OfflineRecognizer>>>,
    paths: ModelPaths,
    tokens_path: PathBuf,
    label: String,
    current_language: Arc<Mutex<String>>,
}

impl SttEngine {
    pub fn new(spec: &ModelSpec, model_dir: &Path, language: &str) -> Result<Self, String> {
        spec.require_installed(model_dir)?;
        let tokens_path = spec.tokens_path(model_dir);
        let paths = match spec.family {
            ModelFamily::SenseVoice => ModelPaths::SenseVoice {
                model: spec.sense_voice_model(model_dir),
            },
            ModelFamily::Whisper => ModelPaths::Whisper {
                encoder: spec.whisper_encoder(model_dir),
                decoder: spec.whisper_decoder(model_dir),
            },
            ModelFamily::ParakeetTransducer => ModelPaths::ParakeetTransducer {
                encoder: spec.transducer_part(model_dir, "encoder"),
                decoder: spec.transducer_part(model_dir, "decoder"),
                joiner: spec.transducer_part(model_dir, "joiner"),
            },
            ModelFamily::Moonshine => ModelPaths::Moonshine {
                preprocessor: spec.moonshine_preprocessor(model_dir),
                encoder: spec.moonshine_encoder(model_dir),
                uncached_decoder: spec.moonshine_uncached_decoder(model_dir),
                cached_decoder: spec.moonshine_cached_decoder(model_dir),
            },
        };

        let lang_str = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };

        Ok(Self {
            // Weights load when recording starts and drop after transcription
            // so the idle overlay stays lightweight.
            recognizer: Arc::new(Mutex::new(None)),
            paths,
            tokens_path,
            label: spec.label.to_string(),
            current_language: Arc::new(Mutex::new(lang_str)),
        })
    }

    fn create_recognizer(&self) -> Result<OfflineRecognizer, String> {
        let language = self.current_language.lock().clone();
        let tokens = self.tokens_path.to_string_lossy().to_string();

        let model_config = match &self.paths {
            ModelPaths::SenseVoice { model } => OfflineModelConfig {
                sense_voice: OfflineSenseVoiceModelConfig {
                    model: Some(model.to_string_lossy().to_string()),
                    language: Some(language),
                    use_itn: true,
                },
                tokens: Some(tokens),
                num_threads: 2,
                debug: false,
                provider: Some("cpu".to_string()),
                model_type: Some("sense_voice".to_string()),
                ..Default::default()
            },
            ModelPaths::Whisper { encoder, decoder } => OfflineModelConfig {
                whisper: OfflineWhisperModelConfig {
                    encoder: Some(encoder.to_string_lossy().to_string()),
                    decoder: Some(decoder.to_string_lossy().to_string()),
                    language: whisper_language(&language),
                    task: Some("transcribe".to_string()),
                    ..Default::default()
                },
                tokens: Some(tokens),
                num_threads: 2,
                debug: false,
                provider: Some("cpu".to_string()),
                model_type: Some("whisper".to_string()),
                ..Default::default()
            },
            ModelPaths::ParakeetTransducer {
                encoder,
                decoder,
                joiner,
            } => OfflineModelConfig {
                transducer: OfflineTransducerModelConfig {
                    encoder: Some(encoder.to_string_lossy().to_string()),
                    decoder: Some(decoder.to_string_lossy().to_string()),
                    joiner: Some(joiner.to_string_lossy().to_string()),
                },
                tokens: Some(tokens),
                num_threads: 2,
                debug: false,
                provider: Some("cpu".to_string()),
                ..Default::default()
            },
            ModelPaths::Moonshine {
                preprocessor,
                encoder,
                uncached_decoder,
                cached_decoder,
            } => OfflineModelConfig {
                moonshine: OfflineMoonshineModelConfig {
                    preprocessor: Some(preprocessor.to_string_lossy().to_string()),
                    encoder: Some(encoder.to_string_lossy().to_string()),
                    uncached_decoder: Some(uncached_decoder.to_string_lossy().to_string()),
                    cached_decoder: Some(cached_decoder.to_string_lossy().to_string()),
                    ..Default::default()
                },
                tokens: Some(tokens),
                num_threads: 2,
                debug: false,
                provider: Some("cpu".to_string()),
                ..Default::default()
            },
        };

        let config = OfflineRecognizerConfig {
            model_config,
            ..Default::default()
        };

        OfflineRecognizer::create(&config)
            .ok_or_else(|| format!("Failed to initialize Sherpa-ONNX {} recognizer", self.label))
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

        println!("{} language set to: {}", self.label, language);
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
    use crate::stt::models::{by_id, DEFAULT};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("voice-stt-lazy-model-{tag}-{unique}"))
    }

    #[test]
    fn constructing_engine_does_not_load_model() {
        let model_dir = unique_dir("pq8");
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        for file in DEFAULT.files {
            fs::write(model_dir.join(file.name), b"placeholder")
                .expect("placeholder model should be written");
        }

        let engine = SttEngine::new(DEFAULT, &model_dir, "auto")
            .expect("construction should validate paths without loading ONNX");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }

    #[test]
    fn constructing_moonshine_engine_does_not_load_model() {
        let spec = by_id("moonshine-medium-streaming").expect("moonshine medium is in the catalog");
        let model_dir = unique_dir("mm");
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        for file in spec.files {
            fs::write(model_dir.join(file.name), b"placeholder")
                .expect("placeholder model should be written");
        }

        let engine = SttEngine::new(spec, &model_dir, "en")
            .expect("construction should validate moonshine paths without loading ONNX");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }

    #[test]
    fn constructing_whisper_engine_does_not_load_model() {
        let spec = by_id("whisper-medium").expect("whisper-medium is in the catalog");
        let model_dir = unique_dir("w");
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        for file in spec.files {
            fs::write(model_dir.join(file.name), b"placeholder")
                .expect("placeholder model should be written");
        }

        let engine = SttEngine::new(spec, &model_dir, "en")
            .expect("construction should validate whisper paths without loading ONNX");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }
}
