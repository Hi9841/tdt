use super::dictionary;
use super::segment;
use super::takes;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trit::{BiasWeights, Cancel, LoadOptions, Trit};

const MAX_CAPTURED_SAMPLES: usize = 120 * 16_000;
/// The first second of a take runs through the model once while the user
/// still talks, and its text is dropped. After a long idle the first run
/// pays for weights the system paged out and cores that slept; this moves
/// that cost out of the wait after the release.
const PRIME_SAMPLES: usize = 16_000;

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

/// One dictation, from its start to its end. A release that waits from the
/// dictation before does not run during this one. Each end starts the wait
/// for the next release, also the end with an error: the engine holds the
/// buffers of a take that failed as it holds those of a take with text.
struct Dictation;

impl Dictation {
    fn start() -> Self {
        DICTATIONS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        release_when_idle();
    }
}

/// Engine calls that run now; each has a `catch_unwind` around it.
static ENGINE_CALLS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread is inside an engine call.
    static IN_ENGINE_CALL: Cell<bool> = const { Cell::new(false) };
}

/// True for a panic that an engine call catches: on the thread of the call,
/// or on a Trit team thread while a call runs (the team gives its panic to
/// the call). Such a panic costs one take, not the app. A panic of a team
/// thread during a model load has no such call around it.
pub fn panic_in_engine_call() -> bool {
    let team_thread = || {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or_default();
        name.starts_with("trit-speech-") && ENGINE_CALLS.load(Ordering::Relaxed) > 0
    };
    IN_ENGINE_CALL.get() || team_thread()
}

/// The result of `call`, or the error for the pill when the engine panics.
fn engine_call<T>(call: impl FnOnce() -> T) -> Result<T, String> {
    IN_ENGINE_CALL.set(true);
    ENGINE_CALLS.fetch_add(1, Ordering::Relaxed);
    let result = catch_unwind(AssertUnwindSafe(call));
    ENGINE_CALLS.fetch_sub(1, Ordering::Relaxed);
    IN_ENGINE_CALL.set(false);
    result.map_err(|_| "The speech engine failed on this take. Try again.".to_string())
}

/// TDT is English only (0.3.1): the decoder never writes other languages.
/// `cancel` stops the read; the text is then empty.
fn read_options(cancel: Option<&Cancel>) -> trit::Options {
    trit::Options {
        cancel: cancel.cloned(),
        ..trit::Options::language("en")
    }
}

fn transcribe_samples(
    engine: &Trit,
    samples: &[f32],
    cancel: Option<&Cancel>,
) -> Result<String, String> {
    let options = read_options(cancel);
    engine_call(|| engine.transcribe_with(samples, &options))
}

/// `text` with the window `samples` added, or `None` when the two do not
/// join. `start` is 0 for the first window, which has nothing to join.
fn add_window(
    engine: &Trit,
    text: &str,
    samples: &[f32],
    start: usize,
    cancel: Option<&Cancel>,
) -> Result<Option<String>, String> {
    let part = transcribe_samples(engine, samples, cancel)?;
    Ok(if start == 0 {
        Some(part)
    } else {
        segment::merge(text, &part)
    })
}

/// The text of the whole take, from the windows so far and the speech
/// from `window` on, and the samples that the model read for it.
fn read_rest(
    engine: &Trit,
    text: &Option<String>,
    captured: &[f32],
    window: usize,
    cancel: Option<&Cancel>,
) -> Result<(String, usize), String> {
    let joined = match text {
        Some(done) => add_window(engine, done, &captured[window..], window, cancel)?,
        None => None,
    };
    Ok(match joined {
        Some(text) => (text, captured.len() - window),
        None => (
            transcribe_samples(engine, captured, cancel)?,
            captured.len(),
        ),
    })
}

/// A stopped read ends at the next encoder block, in milliseconds. This limit
/// is for a read that does not end: the take must not wait for it.
const QUIET_STOP_WAIT: Duration = Duration::from_secs(2);

/// A quiet read runs now. This stays set until the thread of the read ends,
/// also when the take that started the read has stopped waiting for it.
static QUIET_READ_RUNS: AtomicBool = AtomicBool::new(false);

/// The right to run the one quiet read. Dropping it gives the right back.
struct QuietSlot;

impl QuietSlot {
    /// `None` while another read runs.
    fn claim() -> Option<Self> {
        (!QUIET_READ_RUNS.swap(true, Ordering::AcqRel)).then_some(Self)
    }
}

impl Drop for QuietSlot {
    fn drop(&mut self) {
        QUIET_READ_RUNS.store(false, Ordering::Release);
    }
}

/// The audio that gives the text of a take while the user stays quiet after
/// the speech: the take up to the quiet after the speech end. Each value
/// comes from the audio only. So a take has the same point, and the same
/// text, on each computer, under each load, and with each chunk length of
/// the capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuietPoint {
    /// The frame where the speech ended.
    end: usize,
    /// The start of the audio that no window has read.
    window: usize,
    /// The samples of the take that the read gets.
    heard: usize,
}

impl QuietPoint {
    /// The point of the take so far. `None` while the user talks, and when
    /// the last window ended after the point (`windows_end`): that window
    /// made the read of the point old. The time of the look does not change
    /// the result. With one chunk for each packet, the read starts before
    /// the end of the window and the window then stops it. With one long
    /// chunk, the window is read first.
    fn of(speech: &segment::EndOfSpeech, window: usize, windows_end: usize) -> Option<Self> {
        let end = speech.quiet_after()?;
        let heard = segment::EndOfSpeech::read_samples(end);
        (heard >= windows_end).then_some(Self { end, window, heard })
    }
}

/// A read of the take in the quiet after the speech, on its own thread: new
/// audio must reach the capture loop during the read, so the loop can stop a
/// read that new speech made useless. Dropping the read stops it and waits
/// for its end, for `QUIET_STOP_WAIT` at most, so the next read does not run
/// next to it. A read that does not end keeps its `QuietSlot`: no other quiet
/// read starts, and the take reads the same audio after the release.
struct QuietRead {
    point: QuietPoint,
    cancel: Cancel,
    result: Receiver<Result<String, String>>,
}

impl QuietRead {
    /// `heard` is the take up to `point.heard`. `None` when a stopped read
    /// has not ended yet.
    fn start(
        engine: &Arc<Trit>,
        text: &Option<String>,
        heard: &[f32],
        point: QuietPoint,
    ) -> Option<Self> {
        let slot = QuietSlot::claim()?;
        let (done, result) = crossbeam_channel::bounded(1);
        let cancel = Cancel::new();
        let flag = cancel.clone();
        let (engine, text, samples) = (Arc::clone(engine), text.clone(), heard.to_vec());
        std::thread::spawn(move || {
            let read = read_rest(&engine, &text, &samples, point.window, Some(&flag));
            // First the slot: a wait that ends at the send can start a read.
            drop(slot);
            let _ = done.send(read.map(|(text, _)| text));
        });
        Some(Self {
            point,
            cancel,
            result,
        })
    }

    /// Waits for the text; `None` when the read died.
    fn wait(self) -> Option<Result<String, String>> {
        self.result.recv().ok()
    }
}

impl Drop for QuietRead {
    fn drop(&mut self) {
        self.cancel.cancel();
        // After `wait` the channel is empty and closes at once.
        if let Err(RecvTimeoutError::Timeout) = self.result.recv_timeout(QUIET_STOP_WAIT) {
            crate::log::note(&format!(
                "A stopped read in the quiet did not end in {QUIET_STOP_WAIT:?}."
            ));
        }
    }
}

/// Audio captured while the user is still holding the hotkey.
pub enum LiveAudio {
    Chunk(Vec<f32>),
    Finish(StopTimes),
}

/// The end of a take's recording.
#[derive(Clone, Copy, Debug)]
pub struct StopTimes {
    /// The user asked the take to stop: a hold came up, a stopping tap went
    /// down, or TDT stopped the take itself. The latency starts here.
    pub asked: Instant,
    /// The microphone closed, after the last word ended.
    pub closed: Instant,
}

impl StopTimes {
    /// A stop with no wait for the last word.
    pub fn now() -> Self {
        let now = Instant::now();
        Self {
            asked: now,
            closed: now,
        }
    }
}

pub struct LiveTranscript {
    pub text: String,
    pub duration_secs: f32,
    /// The speech the model still had to read after the stop. The model part
    /// of the latency follows it.
    pub last_part_secs: f32,
    pub stop: StopTimes,
    /// The model's text was ready.
    pub transcribed: Instant,
}

impl LiveTranscript {
    /// The wait from the stop request until `delivered`, in its parts.
    pub fn latency(&self, delivered: Instant) -> Latency {
        Latency {
            last_word: self.stop.closed.saturating_duration_since(self.stop.asked),
            model: self.transcribed.saturating_duration_since(self.stop.closed),
            delivery: delivered.saturating_duration_since(self.transcribed),
        }
    }
}

/// The wait the user feels after a take, from the stop request until the
/// text is in the app.
#[derive(Clone, Copy, Debug)]
pub struct Latency {
    /// The microphone stayed open for the end of the last word.
    pub last_word: Duration,
    /// The model finished the speech not yet transcribed.
    pub model: Duration,
    /// Filler removal, the clipboard, focus, and typing.
    pub delivery: Duration,
}

impl Latency {
    pub fn total_ms(&self) -> u64 {
        (self.last_word + self.model + self.delivery).as_millis() as u64
    }
}

impl std::fmt::Display for Latency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ms (last word {}, model {}, delivery {})",
            self.total_ms(),
            self.last_word.as_millis(),
            self.model.as_millis(),
            self.delivery.as_millis()
        )
    }
}

/// Speech to text for one installed model, on Trit and the CPU. The model
/// loads on first use.
pub struct SttEngine {
    model_dir: PathBuf,
    /// How Trit holds this model's weights (`ModelSpec::precision`).
    precision: trit::Precision,
    native: Mutex<Option<Arc<Trit>>>,
    /// Held during a load, so a take that starts during the startup load
    /// waits for it instead of loading a second model.
    loading: Mutex<()>,
    /// The dictionary file as last applied to the engine.
    dictionary: Mutex<dictionary::Watch>,
}

impl SttEngine {
    pub fn new(model_dir: &Path, precision: trit::Precision) -> Self {
        Self {
            model_dir: model_dir.to_path_buf(),
            precision,
            native: Mutex::new(None),
            loading: Mutex::new(()),
            dictionary: Mutex::new(dictionary::Watch::default()),
        }
    }

    pub fn prepare(&self) -> Result<(), String> {
        self.prepare_reporting(None)
    }

    /// `prepare`; `progress` gets the fraction loaded, from 0 to 1.
    pub fn prepare_reporting(
        &self,
        progress: Option<Arc<dyn Fn(f32) + Send + Sync>>,
    ) -> Result<(), String> {
        let _loading = self.loading.lock();
        if self.native.lock().is_some() {
            return Ok(());
        }
        // The cache maps the converted weights from a file next to the
        // model, so they count as file memory, not TDT's private memory.
        let options = LoadOptions {
            cache: true,
            precision: self.precision,
            ..LoadOptions::default()
        };
        let engine = Trit::load_reporting(&self.model_dir, &options, progress)?;
        *self.native.lock() = Some(Arc::new(engine));
        // Loading converts weights through buffers that are freed again.
        trit::release_memory();
        // A fresh engine has no dictionary yet.
        *self.dictionary.lock() = dictionary::Watch::default();
        Ok(())
    }

    /// Applies the user's dictionary file when it changed.
    fn refresh_dictionary(&self) {
        let native = self.native.lock().clone();
        let Some(engine) = native else {
            return;
        };
        let path = dictionary::user_file();
        if let Some(terms) = self.dictionary.lock().changed_terms(path.as_deref()) {
            let skipped = engine.set_dictionary(&terms, BiasWeights::default());
            if !skipped.is_empty() {
                // The count only: the log holds no text of the user.
                crate::log::note(&format!(
                    "The model cannot spell {} of the {} dictionary terms. TDT does not use them.",
                    skipped.len(),
                    terms.len()
                ));
            }
        }
    }

    pub fn transcribe_live_reporting(
        &self,
        rx: &Receiver<LiveAudio>,
    ) -> Result<LiveTranscript, String> {
        let _dictation = Dictation::start();
        self.prepare().map_err(|error| {
            format!("Could not load the speech model. Choose another model in Settings. {error}")
        })?;
        self.refresh_dictionary();
        let native = self.native.lock().clone();
        let engine = native.ok_or("The speech model is not loaded.")?;
        let mut captured: Vec<f32> = Vec::new();
        // The windows so far are in `text`, `None` once two did not join; the
        // next one starts at `window`.
        let mut window = 0;
        // The end of the last window that the model read.
        let mut windows_end = 0;
        let mut text = Some(String::new());
        let mut last_any = Instant::now();
        let mut primed = false;
        let mut speech = segment::EndOfSpeech::default();
        // The audio of the final text while the user stays quiet, and the
        // read of that audio.
        let mut point: Option<QuietPoint> = None;
        let mut quiet: Option<QuietRead> = None;
        // Each point gets one read.
        let mut read_point = None;

        let stop = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(LiveAudio::Chunk(samples)) => {
                    last_any = Instant::now();
                    if captured.len() < MAX_CAPTURED_SAMPLES {
                        let room = MAX_CAPTURED_SAMPLES - captured.len();
                        captured.extend_from_slice(&samples[..samples.len().min(room)]);
                        if !primed && captured.len() >= PRIME_SAMPLES {
                            primed = true;
                            transcribe_samples(&engine, &captured[..PRIME_SAMPLES], None)?;
                        }
                        // Read full windows while the user still talks. New
                        // chunks wait in the channel meanwhile.
                        while captured.len() - window >= segment::WINDOW_SAMPLES {
                            let Some(done) = &text else {
                                break;
                            };
                            // The next window makes the read old.
                            quiet = None;
                            let end = window + segment::WINDOW_SAMPLES;
                            text = add_window(&engine, done, &captured[window..end], window, None)?;
                            window = end - segment::OVERLAP_SAMPLES;
                            windows_end = end;
                        }
                        speech.update(&captured);
                        point = QuietPoint::of(&speech, window, windows_end);
                        // New speech or a window made the read old. It ends
                        // before the next one starts.
                        if quiet.as_ref().is_some_and(|read| Some(read.point) != point) {
                            quiet = None;
                        }
                        if let Some(point) = point {
                            if read_point != Some(point) {
                                read_point = Some(point);
                                let heard = &captured[..point.heard];
                                quiet = QuietRead::start(&engine, &text, heard, point);
                            }
                        }
                    }
                }
                Ok(LiveAudio::Finish(times)) => break times,
                Err(RecvTimeoutError::Timeout) => {
                    // A stalled producer must not hang transcription forever.
                    if last_any.elapsed() > Duration::from_secs(120) {
                        return Err(
                            "The microphone stopped sending audio. Check the input device and try again."
                                .to_string(),
                        );
                    }
                }
                // The pump always ends a take with `Finish`, so audio may be
                // missing: text from a part of the take must not look whole.
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("The recording stopped too early. Try again.".to_string());
                }
            }
        };

        // Duration actually fed to the model, not the full take when the
        // capture cap truncated it.
        let duration_secs = captured.len() as f32 / 16000.0;

        if captured.is_empty() {
            return Ok(LiveTranscript {
                text: String::new(),
                duration_secs,
                last_part_secs: 0.0,
                stop,
                transcribed: Instant::now(),
            });
        }

        // While a quiet point is current, the text of the take is the text of
        // the audio up to that point, with or without a read that ran before
        // the release. The read only moves the work to the time before the
        // release. A read that new speech or a window made old has stopped.
        let (mut text, last_part) = match point {
            Some(point) => match quiet.and_then(QuietRead::wait) {
                Some(read) => (read?, 0),
                None => read_rest(&engine, &text, &captured[..point.heard], window, None)?,
            },
            None => read_rest(&engine, &text, &captured, window, None)?,
        };
        let last_part_secs = last_part as f32 / 16000.0;

        if text.is_empty() {
            if let Some(boosted) = boost_quiet_speech(&captured) {
                if let Ok(boosted_text) = transcribe_samples(&engine, &boosted, None) {
                    text = boosted_text;
                }
            }
        }

        let transcribed = Instant::now();
        // The disk write must not delay the text.
        if takes::saving() {
            let saved_text = text.clone();
            std::thread::spawn(move || {
                if let Err(error) = takes::save(&captured, &saved_text) {
                    crate::log::note(&format!("Could not save the test take: {error}"));
                }
            });
        }
        Ok(LiveTranscript {
            text,
            duration_secs,
            last_part_secs,
            stop,
            transcribed,
        })
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
