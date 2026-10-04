use super::models::ModelSpec;
use super::photon::PhotonParakeetEngine;
use crossbeam_channel::Receiver;
use std::path::Path;
use std::time::Instant;

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

pub struct SttEngine {
    inner: PhotonParakeetEngine,
}

impl SttEngine {
    pub fn new(spec: &ModelSpec, model_dir: &Path, language: &str) -> Result<Self, String> {
        let inner = PhotonParakeetEngine::new(spec, model_dir, language)?;
        Ok(Self { inner })
    }

    pub fn prepare(&self) -> Result<(), String> {
        self.inner.prepare()
    }

    pub fn transcribe_live_reporting(
        &self,
        rx: &Receiver<LiveAudio>,
        partials: Option<&parking_lot::Mutex<String>>,
    ) -> Result<LiveTranscript, String> {
        self.inner.transcribe_live_reporting(rx, partials)
    }

    #[allow(dead_code)]
    pub fn release(&self) {
        self.inner.release();
    }

    pub fn set_language(&self, language: &str) -> Result<(), String> {
        self.inner.set_language(language)
    }

    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.inner.is_loaded()
    }
}

/// 25 ms at 16 kHz. Matches the wave's frame.
const SPEECH_FRAME_SAMPLES: usize = 400;
/// A syllable at this level ticks the wave and still comes back empty.
const VISIBLE_FRAME_RMS: f32 = 0.0015;
const BOOST_TARGET_RMS: f32 = 0.05;
const BOOST_MAX_GAIN: f32 = 24.0;

/// Amplify a take whose bars are visible but whose level is too low for Parakeet.
/// `None` for silence, a short blip, or audio that is already loud enough.
pub(crate) fn boost_quiet_speech(samples: &[f32]) -> Option<Vec<f32>> {
    let mut audible = Vec::new();
    for frame in samples.chunks(SPEECH_FRAME_SAMPLES) {
        if frame.len() < SPEECH_FRAME_SAMPLES / 2 {
            continue;
        }
        let level = frame_rms(frame);
        if level >= VISIBLE_FRAME_RMS {
            audible.push(level);
        }
    }
    // 200 ms of visible frames. A single click is not a phrase.
    if audible.len() < 8 {
        return None;
    }
    audible.sort_by(|left, right| left.total_cmp(right));
    let level = audible[(audible.len() * 3) / 4];
    if !level.is_finite() || level >= BOOST_TARGET_RMS {
        return None;
    }
    let gain = (BOOST_TARGET_RMS / level).min(BOOST_MAX_GAIN);
    if gain < 1.25 {
        return None;
    }
    Some(
        samples
            .iter()
            .map(|sample| (sample * gain).clamp(-1.0, 1.0))
            .collect(),
    )
}

fn frame_rms(frame: &[f32]) -> f32 {
    let sum = frame.iter().map(|sample| sample * sample).sum::<f32>();
    (sum / frame.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::SttEngine;
    use crate::stt::models::DEFAULT;
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be valid")
            .as_nanos();
        std::env::temp_dir().join(format!("voice-stt-lazy-model-{tag}-{unique}"))
    }

    #[test]
    fn installed_default_model_decodes_speech() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = std::path::PathBuf::from(local)
            .join("TDT")
            .join("models")
            .join(DEFAULT.dir_name);
        if !DEFAULT.is_installed_in(&dir) {
            return;
        }
        let engine = SttEngine::new(DEFAULT, &dir, "en").expect("engine");
        engine.prepare().expect("load");
        assert!(engine.is_loaded());
    }

    #[test]
    fn constructing_engine_does_not_load_model() {
        let model_dir = unique_dir("redux-idle");
        fs::create_dir_all(&model_dir).expect("temporary model directory should be created");
        for file in DEFAULT.files {
            fs::write(model_dir.join(file.name), b"placeholder")
                .expect("placeholder model should be written");
        }

        let engine = SttEngine::new(DEFAULT, &model_dir, "auto")
            .expect("construction should validate paths without loading model");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
    }

    #[test]
    fn speech_after_a_pause_is_not_an_empty_transcript() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = std::path::PathBuf::from(local)
            .join("TDT")
            .join("models")
            .join(DEFAULT.dir_name);
        if !DEFAULT.is_installed_in(&dir) {
            return;
        }
        let Some(speech) = crate::stt::photon::pcm16_wav(std::path::Path::new(
            r"C:\Users\hi\AppData\Local\Temp\tdt-speech.wav",
        )) else {
            return;
        };
        let speech = trim_audible(&speech);
        let quiet: Vec<f32> = speech.iter().map(|sample| sample * 0.01).collect();
        let text = decode_samples(&dir, &quiet);
        assert!(
            text.to_ascii_lowercase().contains("fox"),
            "the wave shows this level, transcript was {text:?}"
        );
    }

    #[test]
    fn quiet_visible_speech_is_boosted_and_silence_is_not() {
        assert!(super::boost_quiet_speech(&vec![0.0; 16_000]).is_none());
        let whisper: Vec<f32> = (0..16_000)
            .map(|index| 0.001 * (index as f32 * 0.15).sin())
            .collect();
        assert!(
            super::boost_quiet_speech(&whisper).is_none(),
            "a flat wave is not speech"
        );
        let audible: Vec<f32> = (0..16_000)
            .map(|index| 0.012 * (index as f32 * 0.2).sin())
            .collect();
        let boosted = super::boost_quiet_speech(&audible).expect("visible sine");
        let before = super::frame_rms(&audible[..400]);
        let after = super::frame_rms(&boosted[..400]);
        assert!(after > before * 2.0);
        assert!(after <= super::BOOST_TARGET_RMS + 0.001);
        let loud: Vec<f32> = audible.iter().map(|sample| sample * 20.0).collect();
        assert!(super::boost_quiet_speech(&loud).is_none());
    }

    fn decode_samples(dir: &std::path::Path, samples: &[f32]) -> String {
        let engine = SttEngine::new(DEFAULT, dir, "en").expect("engine");
        engine.prepare().expect("load");
        let partials = parking_lot::Mutex::new(String::new());
        let (tx, rx) = crossbeam_channel::unbounded();
        for chunk in samples.chunks(1_600) {
            tx.send(super::LiveAudio::Chunk(chunk.to_vec()))
                .expect("chunk");
        }
        tx.send(super::LiveAudio::Finish(Instant::now()))
            .expect("finish");
        drop(tx);
        engine
            .transcribe_live_reporting(&rx, Some(&partials))
            .expect("decode")
            .text
    }

    fn trim_audible(samples: &[f32]) -> Vec<f32> {
        let start = samples
            .iter()
            .position(|sample| sample.abs() > 0.02)
            .unwrap_or(0);
        let end = samples
            .iter()
            .rposition(|sample| sample.abs() > 0.02)
            .map(|index| index + 1)
            .unwrap_or(samples.len());
        samples[start..end].to_vec()
    }
}
