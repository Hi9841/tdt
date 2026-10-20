use super::models::ModelSpec;
use super::native::NativeParakeet;
use super::photon::PhotonParakeetEngine;
use super::segment;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_CAPTURED_SAMPLES: usize = 120 * 16_000;
/// After a cut, a remainder shorter than 200 ms is the pause itself, not speech.
const MIN_REMAINDER_SAMPLES: usize = 3_200;

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

/// Speech to text for one installed model. TDT's own Rust engine runs it;
/// the Python Photon worker remains for `TDT_ENGINE=photon` and for a model
/// the Rust engine cannot load.
pub struct SttEngine {
    model_dir: PathBuf,
    native: Mutex<Option<Arc<NativeParakeet>>>,
    photon: PhotonParakeetEngine,
    use_photon: AtomicBool,
}

impl SttEngine {
    pub fn new(spec: &ModelSpec, model_dir: &Path) -> Result<Self, String> {
        let photon = PhotonParakeetEngine::new(spec, model_dir)?;
        let forced = std::env::var("TDT_ENGINE").is_ok_and(|v| v.eq_ignore_ascii_case("photon"));
        Ok(Self {
            model_dir: model_dir.to_path_buf(),
            native: Mutex::new(None),
            photon,
            use_photon: AtomicBool::new(forced),
        })
    }

    pub fn prepare(&self) -> Result<(), String> {
        if !self.use_photon.load(Ordering::SeqCst) {
            let mut native = self.native.lock();
            if native.is_some() {
                return Ok(());
            }
            match NativeParakeet::load(&self.model_dir) {
                Ok(engine) => {
                    *native = Some(Arc::new(engine));
                    return Ok(());
                }
                Err(error) => {
                    eprintln!(
                        "TDT speech engine could not load this model ({error}); using Photon."
                    );
                    self.use_photon.store(true, Ordering::SeqCst);
                }
            }
        }
        self.photon.prepare()
    }

    fn transcribe_samples(&self, samples: &[f32]) -> Result<String, String> {
        if !self.use_photon.load(Ordering::SeqCst) {
            let native = self.native.lock().clone();
            if let Some(engine) = native {
                return Ok(engine.transcribe(samples));
            }
        }
        self.photon.transcribe_samples(samples)
    }

    pub fn transcribe_live_reporting(
        &self,
        rx: &Receiver<LiveAudio>,
    ) -> Result<LiveTranscript, String> {
        self.prepare()?;
        let mut stopped_at = None;
        let mut captured: Vec<f32> = Vec::new();
        // Speech before `committed` is already text in `parts`.
        let mut committed = 0;
        let mut parts: Vec<String> = Vec::new();
        let mut last_any = Instant::now();

        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(LiveAudio::Chunk(samples)) => {
                    last_any = Instant::now();
                    if captured.len() < MAX_CAPTURED_SAMPLES {
                        captured.extend_from_slice(&samples);
                        // Transcribe finished sentences while the user still
                        // talks. New chunks wait in the channel meanwhile.
                        if let Some(cut) = segment::pause_cut(&captured, committed) {
                            let text = self.transcribe_samples(&captured[committed..cut])?;
                            if !text.is_empty() {
                                parts.push(text);
                            }
                            committed = cut;
                        }
                    }
                }
                Ok(LiveAudio::Finish(at)) => {
                    stopped_at = Some(at);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // A stalled producer must not hang transcription forever.
                    if last_any.elapsed() > Duration::from_secs(120) {
                        return Err("Audio capture stalled; transcription aborted.".to_string());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        let stopped_at = stopped_at.unwrap_or_else(Instant::now);
        // Duration actually fed to the model, not the full take when the
        // capture cap truncated it.
        let duration_secs = captured.len() as f32 / 16000.0;

        if captured.is_empty() {
            return Ok(LiveTranscript {
                text: String::new(),
                latency_ms: stopped_at.elapsed().as_millis() as u64,
                duration_secs,
            });
        }

        let remainder = &captured[committed..];
        if parts.is_empty() || remainder.len() >= MIN_REMAINDER_SAMPLES {
            let text = self.transcribe_samples(remainder)?;
            if !text.is_empty() {
                parts.push(text);
            }
        }
        let mut text = segment::join_parts(&parts);

        if text.is_empty() {
            if let Some(boosted) = boost_quiet_speech(&captured) {
                if let Ok(boosted_text) = self.transcribe_samples(&boosted) {
                    text = boosted_text;
                }
            }
        }

        Ok(LiveTranscript {
            text,
            latency_ms: stopped_at.elapsed().as_millis() as u64,
            duration_secs,
        })
    }

    pub fn release(&self) {
        *self.native.lock() = None;
        self.photon.release();
    }

    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.native.lock().is_some() || self.photon.is_loaded()
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
    // 200 ms of audible frames. A single click is not a phrase.
    const MIN_AUDIBLE_FRAMES: usize = 8;
    if audible.len() < MIN_AUDIBLE_FRAMES {
        return None;
    }
    audible.sort_by(|left, right| left.total_cmp(right));
    let level = audible[(audible.len() * 3) / 4];
    if !level.is_finite() || level >= BOOST_TARGET_RMS {
        return None;
    }
    const MIN_USEFUL_GAIN: f32 = 1.25;
    let gain = (BOOST_TARGET_RMS / level).min(BOOST_MAX_GAIN);
    if gain < MIN_USEFUL_GAIN {
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
    use std::time::{SystemTime, UNIX_EPOCH};

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
        let engine = SttEngine::new(DEFAULT, &dir).expect("engine");
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

        let engine = SttEngine::new(DEFAULT, &model_dir)
            .expect("construction should validate paths without loading model");

        assert!(!engine.is_loaded(), "model must stay unloaded while idle");
        fs::remove_dir_all(model_dir).expect("temporary model directory should be removed");
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
}
