use super::dictionary;
use super::segment;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trit::{BiasWeights, Trit};

const MAX_CAPTURED_SAMPLES: usize = 120 * 16_000;
/// After a cut, a remainder shorter than 200 ms is the pause itself, not speech.
const MIN_REMAINDER_SAMPLES: usize = 3_200;

/// Dictation starts and ends so far; a release waits for this to stay the same.
static DICTATIONS: AtomicU64 = AtomicU64::new(0);
/// Idle time after a dictation before its memory goes back to the system.
const IDLE_RELEASE: Duration = Duration::from_secs(5);

/// Returns the memory of the last dictation to the system once no new one
/// has started for `IDLE_RELEASE`. During dictations the engine keeps its
/// buffers, so a quick next one does not wait for the system to give them
/// back.
fn release_when_idle() {
    let dictation = DICTATIONS.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(IDLE_RELEASE);
        if DICTATIONS.load(Ordering::Relaxed) == dictation {
            trit::release_memory();
        }
    });
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

/// Speech to text for one installed model, on TDT's own engine. The model
/// loads on first use.
pub struct SttEngine {
    model_dir: PathBuf,
    native: Mutex<Option<Arc<Trit>>>,
    /// The dictionary file as last applied to the engine.
    dictionary: Mutex<dictionary::Watch>,
}

impl SttEngine {
    pub fn new(model_dir: &Path) -> Self {
        Self {
            model_dir: model_dir.to_path_buf(),
            native: Mutex::new(None),
            dictionary: Mutex::new(dictionary::Watch::default()),
        }
    }

    pub fn prepare(&self) -> Result<(), String> {
        let mut native = self.native.lock();
        if native.is_none() {
            *native = Some(Arc::new(Trit::load(&self.model_dir)?));
            // Loading converts weights through buffers that are freed again.
            trit::release_memory();
            // A fresh engine has no dictionary yet.
            *self.dictionary.lock() = dictionary::Watch::default();
        }
        Ok(())
    }

    /// Applies the built-in terms and the user's dictionary file when it changed.
    fn refresh_dictionary(&self) {
        let native = self.native.lock().clone();
        let Some(engine) = native else {
            return;
        };
        let path = dictionary::user_file();
        if let Some(terms) = self.dictionary.lock().changed_terms(path.as_deref()) {
            let skipped = engine.set_dictionary(&terms, BiasWeights::default());
            if !skipped.is_empty() {
                eprintln!("Dictionary terms the model cannot spell: {skipped:?}");
            }
        }
    }

    fn transcribe_samples(&self, samples: &[f32]) -> Result<String, String> {
        let native = self.native.lock().clone();
        let engine = native.ok_or("The speech model is not loaded.")?;
        // TDT is English only (0.3.1): the decoder never writes other languages.
        Ok(engine.transcribe_with(samples, &trit::Options::language("en")))
    }

    pub fn transcribe_live_reporting(
        &self,
        rx: &Receiver<LiveAudio>,
    ) -> Result<LiveTranscript, String> {
        // A release waiting from the last dictation must not run during this one.
        DICTATIONS.fetch_add(1, Ordering::Relaxed);
        self.prepare()?;
        self.refresh_dictionary();
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

        let transcript = LiveTranscript {
            text,
            latency_ms: stopped_at.elapsed().as_millis() as u64,
            duration_secs,
        };
        release_when_idle();
        Ok(transcript)
    }

    pub fn release(&self) {
        *self.native.lock() = None;
    }
}

/// 25 ms at 16 kHz. Matches the wave's frame.
const SPEECH_FRAME_SAMPLES: usize = 400;
/// A syllable at this level ticks the wave and still comes back empty.
const VISIBLE_FRAME_RMS: f32 = 0.0015;
/// Speech frames are 15 dB or more over the noise floor of the same take.
/// Steady noise and hum stay near their floor at any level.
const SPEECH_OVER_FLOOR: f32 = 5.6;
/// 200 ms of speech frames in the take. A single click is not a phrase.
const MIN_SPEECH_FRAMES: usize = 8;
/// 150 ms of speech frames in one run, as in a voiced syllable. Key clicks
/// are shorter, even fast typing.
const MIN_VOICED_RUN: usize = 6;
const BOOST_TARGET_RMS: f32 = 0.05;
const BOOST_MAX_GAIN: f32 = 24.0;

/// Amplify a take whose speech is visible but too quiet for Parakeet.
/// `None` for silence, steady noise or hum, clicks, a short blip, or audio
/// that is already loud enough. Amplified noise makes the model write words
/// that nobody said, so the take must show speech over its own noise floor.
pub(crate) fn boost_quiet_speech(samples: &[f32]) -> Option<Vec<f32>> {
    let levels: Vec<f32> = samples
        .as_chunks::<SPEECH_FRAME_SAMPLES>()
        .0
        .iter()
        .map(|frame| frame_rms(frame))
        .collect();
    if levels.len() < MIN_SPEECH_FRAMES {
        return None;
    }
    // Digital silence, as from a device that starts muted, is not the
    // noise floor: with it the gate would fall back to `VISIBLE_FRAME_RMS`.
    let mut sorted: Vec<f32> = levels.iter().copied().filter(|l| *l > 1e-6).collect();
    if sorted.is_empty() {
        return None;
    }
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let speech_min = (floor * SPEECH_OVER_FLOOR).max(VISIBLE_FRAME_RMS);
    let mut speech = Vec::new();
    let (mut run, mut longest_run) = (0, 0);
    for level in levels {
        if level >= speech_min {
            speech.push(level);
            run += 1;
            longest_run = longest_run.max(run);
        } else {
            run = 0;
        }
    }
    if speech.len() < MIN_SPEECH_FRAMES || longest_run < MIN_VOICED_RUN {
        return None;
    }
    speech.sort_by(f32::total_cmp);
    let level = speech[(speech.len() * 3) / 4];
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
