use super::models::ModelSpec;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use sherpa_onnx::{
    OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig,
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

enum Recognizer {
    Offline(OfflineRecognizer),
    Online(OnlineRecognizer),
}

enum UtteranceKind {
    Buffered(Vec<f32>),
    Streaming(OnlineStream),
}

pub struct Utterance {
    kind: UtteranceKind,
}

/// Audio captured while the user is still holding the hotkey.
pub enum LiveAudio {
    Chunk(Vec<f32>),
    /// `Instant` is the moment recording stopped. Latency is measured from here.
    Finish(Instant),
}

pub struct LiveTranscript {
    pub text: String,
    pub latency_ms: u64,
    pub duration_secs: f32,
}

struct ModelPaths {
    encoder: PathBuf,
    decoder: PathBuf,
    joiner: PathBuf,
}

pub struct SttEngine {
    recognizer: Arc<Mutex<Option<Recognizer>>>,
    paths: ModelPaths,
    tokens_path: PathBuf,
    label: String,
    current_language: Arc<Mutex<String>>,
}

impl SttEngine {
    pub fn new(spec: &ModelSpec, model_dir: &Path, language: &str) -> Result<Self, String> {
        spec.require_installed(model_dir)?;
        let tokens_path = spec.tokens_path(model_dir);
        let paths = ModelPaths {
            encoder: spec.transducer_part(model_dir, "encoder"),
            decoder: spec.transducer_part(model_dir, "decoder"),
            joiner: spec.transducer_part(model_dir, "joiner"),
        };

        let lang_str = if language.is_empty() {
            "auto".to_string()
        } else {
            language.to_string()
        };

        Ok(Self {
            // Construction only validates paths. The app prepares the selected
            // model in the background, then keeps its weights resident.
            recognizer: Arc::new(Mutex::new(None)),
            paths,
            tokens_path,
            label: spec.label.to_string(),
            current_language: Arc::new(Mutex::new(lang_str)),
        })
    }

    fn create_recognizer(&self) -> Result<Recognizer, String> {
        if let Some(online) = self.create_online() {
            return Ok(Recognizer::Online(online));
        }
        Ok(Recognizer::Offline(self.create_offline()?))
    }

    fn create_online(&self) -> Option<OnlineRecognizer> {
        let ModelPaths {
            encoder,
            decoder,
            joiner,
        } = &self.paths;
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: Some(encoder.to_string_lossy().to_string()),
                    decoder: Some(decoder.to_string_lossy().to_string()),
                    joiner: Some(joiner.to_string_lossy().to_string()),
                },
                tokens: Some(self.tokens_path.to_string_lossy().to_string()),
                num_threads: inference_threads(),
                provider: Some("cpu".to_string()),
                debug: false,
                model_type: Some("nemo_transducer".to_string()),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".to_string()),
            max_active_paths: 1,
            enable_endpoint: false,
            ..Default::default()
        };
        OnlineRecognizer::create(&config)
    }

    fn create_offline(&self) -> Result<OfflineRecognizer, String> {
        let tokens = self.tokens_path.to_string_lossy().to_string();

        // Keep the existing budget for offline models. The larger thread pool
        // is measured against the streaming Parakeet encoder only.
        let threads = inference_threads().min(4);
        let ModelPaths {
            encoder,
            decoder,
            joiner,
        } = &self.paths;
        let model_config = OfflineModelConfig {
            transducer: OfflineTransducerModelConfig {
                encoder: Some(encoder.to_string_lossy().to_string()),
                decoder: Some(decoder.to_string_lossy().to_string()),
                joiner: Some(joiner.to_string_lossy().to_string()),
            },
            tokens: Some(tokens),
            num_threads: threads,
            debug: false,
            provider: Some("cpu".to_string()),
            model_type: Some("nemo_transducer".to_string()),
            ..Default::default()
        };

        let config = OfflineRecognizerConfig {
            model_config,
            // Greedy search is the fastest transducer decode. Beam search
            // spends the extra time on paths we do not show.
            decoding_method: Some("greedy_search".to_string()),
            max_active_paths: 1,
            ..Default::default()
        };

        OfflineRecognizer::create(&config)
            .ok_or_else(|| format!("Failed to initialize Sherpa-ONNX {} recognizer", self.label))
    }

    pub fn prepare(&self) -> Result<(), String> {
        let mut guard = self.recognizer.lock();
        if guard.is_none() {
            let recognizer = self.create_recognizer()?;
            // First ONNX run pays kernel init. Do it on a throwaway stream
            // while the user is still talking, then keep the session resident.
            warm_session(&recognizer);
            *guard = Some(recognizer);
        }
        Ok(())
    }

    pub fn start_utterance(&self) -> Result<Utterance, String> {
        self.prepare()?;
        let guard = self.recognizer.lock();
        match guard.as_ref() {
            Some(Recognizer::Online(recognizer)) => Ok(Utterance {
                kind: UtteranceKind::Streaming(recognizer.create_stream()),
            }),
            Some(Recognizer::Offline(_)) => Ok(Utterance {
                kind: UtteranceKind::Buffered(Vec::new()),
            }),
            None => Err("STT recognizer not initialized".to_string()),
        }
    }

    pub fn push_audio(&self, utterance: &mut Utterance, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        match &mut utterance.kind {
            UtteranceKind::Buffered(buffered) => buffered.extend_from_slice(samples),
            UtteranceKind::Streaming(stream) => {
                let guard = self.recognizer.lock();
                if let Some(Recognizer::Online(recognizer)) = guard.as_ref() {
                    decode_ready(recognizer, stream, samples, false);
                }
            }
        }
    }

    pub fn finish_utterance(&self, utterance: Utterance) -> Result<String, String> {
        match utterance.kind {
            UtteranceKind::Buffered(samples) => self.transcribe_offline(&samples),
            UtteranceKind::Streaming(stream) => {
                let guard = self.recognizer.lock();
                let Some(Recognizer::Online(recognizer)) = guard.as_ref() else {
                    return Err("STT recognizer not initialized".to_string());
                };
                decode_ready(recognizer, &stream, &[], true);
                Ok(recognizer
                    .get_result(&stream)
                    .map(|result| result.text.trim().to_string())
                    .unwrap_or_default())
            }
        }
    }

    /// Decode Parakeet audio as it arrives. `Finish` starts the response-time clock.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn transcribe_live(&self, rx: &Receiver<LiveAudio>) -> Result<LiveTranscript, String> {
        self.transcribe_live_reporting(rx, None)
    }

    /// Same as [`Self::transcribe_live`]. When `partials` is set, each non-empty
    /// streaming chunk stores the current hypothesis there.
    pub fn transcribe_live_reporting(
        &self,
        rx: &Receiver<LiveAudio>,
        partials: Option<&parking_lot::Mutex<String>>,
    ) -> Result<LiveTranscript, String> {
        let mut utterance = self.start_utterance()?;
        let mut samples_seen = 0usize;
        let mut stopped_at = None;
        // Kept only so a too-short utterance can be retried offline. Bounded by
        // the same ceiling the fallback accepts, so a long recording costs
        // 128 KB rather than a second copy of the whole take.
        let mut captured: Vec<f32> = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(LiveAudio::Chunk(samples)) => {
                    samples_seen += samples.len();
                    if captured.len() < SHORT_FALLBACK_MAX_SAMPLES {
                        captured.extend_from_slice(&samples);
                    }
                    self.push_audio(&mut utterance, &samples);
                    if !samples.is_empty() {
                        self.publish_partial(&utterance, partials);
                    }
                }
                Ok(LiveAudio::Finish(at)) => {
                    stopped_at = Some(at);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let stopped_at = stopped_at.unwrap_or_else(Instant::now);
        let streaming = self.is_streaming_recognizer();
        let text = self.finish_utterance(utterance)?;
        // An empty transcript from the streaming path means the utterance was
        // too brief to fill the encoder's context, not that the room was quiet.
        let text = if streaming && text.trim().is_empty() {
            self.retry_short_utterance(&captured).unwrap_or(text)
        } else {
            text
        };
        Ok(LiveTranscript {
            text,
            latency_ms: stopped_at.elapsed().as_millis() as u64,
            duration_secs: samples_seen as f32 / 16000.0,
        })
    }

    /// Copy a non-empty streaming hypothesis into `partials`.
    ///
    /// The recognizer lock is released before `partials` is taken. The UI
    /// thread locks only `partials`.
    fn publish_partial(&self, utterance: &Utterance, partials: Option<&Mutex<String>>) {
        let Some(partials) = partials else {
            return;
        };
        let UtteranceKind::Streaming(stream) = &utterance.kind else {
            return;
        };
        let text = {
            let guard = self.recognizer.lock();
            let Some(Recognizer::Online(recognizer)) = guard.as_ref() else {
                return;
            };
            recognizer
                .get_result(stream)
                .map(|result| result.text.trim().to_string())
                .unwrap_or_default()
        };
        if text.is_empty() {
            return;
        }
        let mut current = partials.lock();
        if *current != text {
            *current = text;
        }
    }

    fn is_streaming_recognizer(&self) -> bool {
        matches!(*self.recognizer.lock(), Some(Recognizer::Online(_)))
    }

    /// Retry a brief utterance on the resident streaming recognizer, then offline.
    ///
    /// The streaming transducer only encodes a window once it holds enough
    /// context, so a short take decodes zero times and `get_result` returns
    /// nothing. Padding to one 1120 ms chunk lets that resident model finish
    /// the take. The offline recognizer is loaded only when that still returns
    /// nothing.
    ///
    /// Only attempted inside the band where streaming is known to give up.
    /// Longer audio always streams, and near-silence is not worth loading a
    /// second recognizer to learn nothing.
    fn retry_short_utterance(&self, samples: &[f32]) -> Option<String> {
        if !(SHORT_FALLBACK_MIN_SAMPLES..SHORT_FALLBACK_MAX_SAMPLES).contains(&samples.len()) {
            return None;
        }
        if let Some(text) = self.decode_padded_streaming(samples) {
            return Some(text);
        }
        let recognizer = self.create_offline().ok()?;
        let stream = recognizer.create_stream();
        stream.accept_waveform(16000, samples);
        recognizer.decode(&stream);
        let text = stream
            .get_result()
            .map(|result| result.text.trim().to_string())
            .unwrap_or_default();
        (!text.is_empty()).then_some(text)
    }

    /// Re-decode on the resident online recognizer. `None` if it is not online
    /// or the trimmed hypothesis is empty, so the caller can still load offline.
    fn decode_padded_streaming(&self, samples: &[f32]) -> Option<String> {
        let guard = self.recognizer.lock();
        let Recognizer::Online(recognizer) = guard.as_ref()? else {
            return None;
        };
        let stream = recognizer.create_stream();
        stream.accept_waveform(16000, samples);
        if let Some(pad) = streaming_pad_len(samples.len()).filter(|pad| *pad > 0) {
            let zeros = vec![0.0f32; pad];
            stream.accept_waveform(16000, &zeros);
        }
        decode_ready(recognizer, &stream, &[], true);
        let text = recognizer
            .get_result(&stream)
            .map(|result| result.text.trim().to_string())
            .unwrap_or_default();
        (!text.is_empty()).then_some(text)
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

    fn transcribe_offline(&self, samples: &[f32]) -> Result<String, String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        self.prepare()?;
        let guard = self.recognizer.lock();
        let Some(Recognizer::Offline(recognizer)) = guard.as_ref() else {
            return Err("STT recognizer not initialized".to_string());
        };
        let stream = recognizer.create_stream();
        stream.accept_waveform(16000, samples);
        recognizer.decode(&stream);
        Ok(stream
            .get_result()
            .map(|result| result.text.trim().to_string())
            .unwrap_or_default())
    }

    #[cfg(test)]
    fn is_loaded(&self) -> bool {
        self.recognizer.lock().is_some()
    }

    #[cfg(test)]
    fn is_streaming(&self) -> bool {
        matches!(self.recognizer.lock().as_ref(), Some(Recognizer::Online(_)))
    }
}

/// Streaming Parakeet benefits from eight threads on larger CPUs. Cap the
/// pool there: twelve threads were slower in the real-time speech replay.
pub(crate) fn inference_thread_count(parallelism: usize) -> i32 {
    parallelism.clamp(1, 8) as i32
}

fn inference_threads() -> i32 {
    #[cfg(test)]
    if let Ok(threads) = std::env::var("TDT_BENCH_THREADS") {
        return threads.parse().expect("benchmark thread count");
    }
    inference_thread_count(
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(2),
    )
}

/// Bounds for retrying a too-short utterance offline, in samples at 16 kHz.
/// 0.15 s is below anything a person would say on purpose; 2 s is above the
/// point where the streaming path reliably produces a transcript.
const SHORT_FALLBACK_MIN_SAMPLES: usize = 2_400;
const SHORT_FALLBACK_MAX_SAMPLES: usize = 32_000;

/// Trailing zeros so a short take fills one 1120 ms streaming chunk.
/// None outside the short-utterance band (same bounds as the offline retry).
fn streaming_pad_len(samples: usize) -> Option<usize> {
    if !(SHORT_FALLBACK_MIN_SAMPLES..SHORT_FALLBACK_MAX_SAMPLES).contains(&samples) {
        return None;
    }
    Some(17_920usize.saturating_sub(samples))
}

fn decode_ready(
    recognizer: &OnlineRecognizer,
    stream: &OnlineStream,
    samples: &[f32],
    finished: bool,
) {
    if !samples.is_empty() {
        stream.accept_waveform(16000, samples);
    }
    if finished {
        stream.input_finished();
    }
    while recognizer.is_ready(stream) {
        recognizer.decode(stream);
    }
}

fn warm_session(recognizer: &Recognizer) {
    let silence = [0.0f32; 3200];
    match recognizer {
        Recognizer::Offline(recognizer) => {
            let stream = recognizer.create_stream();
            stream.accept_waveform(16000, &silence);
            recognizer.decode(&stream);
        }
        Recognizer::Online(recognizer) => {
            let stream = recognizer.create_stream();
            decode_ready(recognizer, &stream, &silence, true);
        }
    }
}

#[cfg(test)]
#[path = "latency_tests.rs"]
mod latency_tests;

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
    fn installed_parakeet_q8_decodes_while_audio_arrives() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = std::path::PathBuf::from(local)
            .join("TDT")
            .join("models")
            .join("parakeet-unified-en-0.6b-q8");
        if !dir.join("encoder.int8.onnx").is_file() {
            return;
        }
        let engine = SttEngine::new(DEFAULT, &dir, "en").expect("engine");
        engine.prepare().expect("load");
        assert!(
            engine.is_streaming(),
            "Parakeet INT8 should decode with the streaming recognizer"
        );
        let mut utterance = engine.start_utterance().expect("utterance");
        let chunk = vec![0.0f32; 16_000];
        for _ in 0..3 {
            engine.push_audio(&mut utterance, &chunk);
        }
        let flush_ms = {
            let started = std::time::Instant::now();
            engine.finish_utterance(utterance).expect("finish");
            started.elapsed().as_millis()
        };
        assert!(
            flush_ms < 1_000,
            "flush after a streamed utterance took {flush_ms} ms"
        );
    }

    #[test]
    fn short_utterance_fallback_declines_audio_outside_its_band() {
        // Both of these must return before a second recognizer is loaded: a long
        // take always streams, and a stray tap is not an utterance. Guards the
        // model-load cost of the fallback.
        let model_dir = unique_dir("short-fallback");
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        for file in DEFAULT.files {
            fs::write(model_dir.join(file.name), b"placeholder")
                .expect("placeholder model should be written");
        }
        let engine = SttEngine::new(DEFAULT, &model_dir, "en").expect("engine");

        assert!(engine.retry_short_utterance(&[]).is_none(), "no audio");
        assert!(
            engine.retry_short_utterance(&vec![0.1; 100]).is_none(),
            "below the band"
        );
        assert!(
            engine.retry_short_utterance(&vec![0.1; 40_000]).is_none(),
            "above the band, streaming already handled it"
        );

        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }

    #[test]
    fn streaming_pad_len_fills_one_chunk_inside_the_short_band() {
        assert_eq!(super::streaming_pad_len(0), None);
        assert_eq!(super::streaming_pad_len(100), None);
        assert_eq!(super::streaming_pad_len(40_000), None);
        assert_eq!(super::streaming_pad_len(2_400), Some(17_920 - 2_400));
    }

    #[test]
    fn inference_threads_scale_with_cores_and_cap_at_eight() {
        assert_eq!(super::inference_thread_count(0), 1);
        assert_eq!(super::inference_thread_count(1), 1);
        assert_eq!(super::inference_thread_count(2), 2);
        assert_eq!(super::inference_thread_count(4), 4);
        assert_eq!(super::inference_thread_count(8), 8);
        assert_eq!(super::inference_thread_count(16), 8);
        assert_eq!(super::inference_thread_count(usize::MAX), 8);
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
    fn constructing_parakeet_q8_engine_does_not_load_model() {
        let spec = by_id("parakeet-unified-en-0.6b-q8").expect("parakeet q8 is in the catalog");
        let model_dir = unique_dir("pq8");
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        for file in spec.files {
            fs::write(model_dir.join(file.name), b"placeholder")
                .expect("placeholder model should be written");
        }

        let engine = SttEngine::new(spec, &model_dir, "en")
            .expect("construction should validate parakeet paths without loading ONNX");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }
}
