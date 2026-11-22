//! One take: the audio from the start of the recording to its text.
//!
//! The model reads the take while the user still talks, so the wait after
//! the stop is short. This is the take path of TDT desktop
//! (`desktop/src/stt/engine.rs`, 0.9.10): the same windows, the same read in
//! the quiet after the speech, and the same audio for the final text. Keep
//! the two in step.

use crate::engine::Engine;
use crate::segment;
use parking_lot::Mutex;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use trit::{Cancel, Trit};

/// The longest take, 120 s, as on TDT desktop. Memory stays bounded.
const MAX_CAPTURED_SAMPLES: usize = 120 * 16_000;
/// The first second of a take runs through the model once while the user
/// still talks, and its text is dropped. After a long idle the first run
/// pays for weights the system paged out and cores that slept; this moves
/// that cost out of the wait after the stop.
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

/// The result of `call`, or the error for the user when the engine panics.
/// A panic costs one take, not the app.
fn engine_call<T>(call: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(call))
        .map_err(|_| "The speech engine failed on this take. Try again.".to_string())
}

/// TDT is English only: the decoder never writes other languages.
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

/// The text of `samples`, the end of a take, with `end_quiet` samples of
/// digital quiet after them (`Engine::end_quiet`).
fn read_end(
    engine: &Trit,
    samples: &[f32],
    end_quiet: usize,
    cancel: Option<&Cancel>,
) -> Result<String, String> {
    if end_quiet == 0 {
        return transcribe_samples(engine, samples, cancel);
    }
    let mut padded = Vec::with_capacity(samples.len() + end_quiet);
    padded.extend_from_slice(samples);
    padded.resize(samples.len() + end_quiet, 0.0);
    transcribe_samples(engine, &padded, cancel)
}

/// `text` with `part` added, the text of the window that starts at
/// `start`, or `None` when the two do not join. `start` is 0 for the first
/// window, which has nothing to join.
fn join(text: &str, part: String, start: usize) -> Option<String> {
    if start == 0 {
        Some(part)
    } else {
        segment::merge(text, &part)
    }
}

/// The text of the whole take, from the windows so far and the speech
/// from `window` on, and the samples that the model read for it.
fn read_rest(
    engine: &Trit,
    text: &Option<String>,
    captured: &[f32],
    window: usize,
    end_quiet: usize,
    cancel: Option<&Cancel>,
) -> Result<(String, usize), String> {
    let joined = match text {
        Some(done) => join(
            done,
            read_end(engine, &captured[window..], end_quiet, cancel)?,
            window,
        ),
        None => None,
    };
    Ok(match joined {
        Some(text) => (text, captured.len() - window),
        None => (
            read_end(engine, captured, end_quiet, cancel)?,
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
/// text, on each phone and computer, under each load, and with each chunk
/// length of the capture.
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
    /// the result.
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
/// read starts, and the take reads the same audio after the stop.
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
        end_quiet: usize,
    ) -> Option<Self> {
        let slot = QuietSlot::claim()?;
        let (done, result) = mpsc::sync_channel(1);
        let cancel = Cancel::new();
        let flag = cancel.clone();
        let (engine, text, samples) = (Arc::clone(engine), text.clone(), heard.to_vec());
        std::thread::spawn(move || {
            let read = read_rest(
                &engine,
                &text,
                &samples,
                point.window,
                end_quiet,
                Some(&flag),
            );
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

/// Audio from the microphone while the take records.
enum Live {
    Chunk(Vec<f32>),
    Finish,
}

/// The text of a take, and how the model got to it.
#[derive(Debug)]
pub struct Heard {
    pub text: String,
    /// The audio of the take, in samples.
    pub samples: usize,
    /// The audio that the model still had to read after the stop. The wait
    /// follows it. 0: the text was ready, or in work, when the user stopped.
    pub last_part: usize,
}

/// Reads the take from `audio` until its `Finish`.
fn read_take(engine: &Engine, audio: &Receiver<Live>) -> Result<Heard, String> {
    // A release waiting from the last dictation must not run during this one.
    DICTATIONS.fetch_add(1, Ordering::Relaxed);
    let end_quiet = engine.end_quiet();
    let engine = &{
        let model = engine
            .load()
            .map_err(|error| format!("Could not load the speech model. {error}"))?;
        engine.refresh_dictionary(&model);
        model
    };
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

    loop {
        match audio.recv_timeout(Duration::from_millis(20)) {
            Ok(Live::Chunk(samples)) => {
                last_any = Instant::now();
                if captured.len() < MAX_CAPTURED_SAMPLES {
                    let room = MAX_CAPTURED_SAMPLES - captured.len();
                    captured.extend_from_slice(&samples[..samples.len().min(room)]);
                    if !primed && captured.len() >= PRIME_SAMPLES {
                        primed = true;
                        transcribe_samples(engine, &captured[..PRIME_SAMPLES], None)?;
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
                        let part = transcribe_samples(engine, &captured[window..end], None)?;
                        text = join(done, part, window);
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
                            quiet = QuietRead::start(engine, &text, heard, point, end_quiet);
                        }
                    }
                }
            }
            Ok(Live::Finish) => break,
            Err(RecvTimeoutError::Timeout) => {
                // A stalled recorder must not hang the take forever.
                if last_any.elapsed() > Duration::from_secs(120) {
                    return Err(
                        "The microphone stopped sending audio. Check the microphone and try again."
                            .to_string(),
                    );
                }
            }
            // The user dropped the take: there is no text to write.
            Err(RecvTimeoutError::Disconnected) => {
                return Err("The take was cancelled.".to_string());
            }
        }
    }

    if captured.is_empty() {
        return Ok(Heard {
            text: String::new(),
            samples: 0,
            last_part: 0,
        });
    }

    // While a quiet point is current, the text of the take is the text of
    // the audio up to that point, with or without a read that ran before
    // the stop. The read only moves the work to the time before the stop.
    // A read that new speech or a window made old has stopped.
    let (mut text, last_part) = match point {
        Some(point) => match quiet.and_then(QuietRead::wait) {
            Some(read) => (read?, 0),
            None => read_rest(
                engine,
                &text,
                &captured[..point.heard],
                window,
                end_quiet,
                None,
            )?,
        },
        None => read_rest(engine, &text, &captured, window, end_quiet, None)?,
    };

    if text.is_empty() {
        if let Some(boosted) = boost_quiet_speech(&captured) {
            if let Ok(boosted_text) = read_end(engine, &boosted, end_quiet, None) {
                text = boosted_text;
            }
        }
    }

    release_when_idle();
    Ok(Heard {
        text,
        samples: captured.len(),
        last_part,
    })
}

/// 25 ms at 16 kHz.
const SPEECH_FRAME_SAMPLES: usize = 400;
/// A syllable at this level shows in the wave and still comes back empty.
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
fn boost_quiet_speech(samples: &[f32]) -> Option<Vec<f32>> {
    let levels: Vec<f32> = samples
        .as_chunks::<SPEECH_FRAME_SAMPLES>()
        .0
        .iter()
        .map(|frame| frame_rms(frame))
        .collect();
    if levels.len() < MIN_SPEECH_FRAMES {
        return None;
    }
    // Digital silence, as from a microphone that starts muted, is not the
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

/// PCM16 as the samples that the model reads: value / 32767, as TDT desktop
/// reads PCM16 (`desktop/src/audio/recorder.rs`, `bench-asr`). Keep this the
/// same on both sides: with value / 32768, a change of 0.003 %, Parakeet
/// Redux writes other text for about 1 take in 5.
pub fn samples_of(pcm: &[i16]) -> Vec<f32> {
    pcm.iter()
        .map(|&value| f32::from(value) / f32::from(i16::MAX))
        .collect()
}

/// A microphone that has just opened can send near-silence before the room
/// noise comes through: on the OnePlus 13R 40 to 50 ms at -100 to -83 dB,
/// against a room at about -50 dB. The speech end and the tail take the
/// room noise from the first 80 ms of the take, so that start put the floor
/// at -87 dB: nothing after the speech counted as quiet, every tail ran to
/// its cap, and the read in the quiet never started (every take, 2026-11-17).
/// Chunks this quiet at the start are not audio of the room; the take drops
/// them. Speech and room noise are far above it.
const STARTUP_SILENCE_RMS: f32 = 1e-4;
/// The most audio the take drops as start-up silence: a microphone that
/// stays this quiet is the room.
const MAX_STARTUP_SILENCE_SAMPLES: usize = 160 * 16;

/// The side of a take that the recorder feeds.
struct Feed {
    audio: Sender<Live>,
    samples: usize,
    /// Start-up silence dropped so far; `None` once the take has audio.
    startup: Option<usize>,
    /// Reject later packets after the stop or recording limit.
    closed: bool,
}

impl Feed {
    fn stop(&mut self) {
        if !self.closed {
            self.closed = true;
            let _ = self.audio.send(Live::Finish);
        }
    }
}

/// One take in work. The recorder gives it the audio as it comes
/// (`push`), the user asks it to stop (`ask_stop`), and `finish` gives the
/// text. A thread of its own reads the audio, so `push` never waits for the
/// model.
pub struct Take {
    feed: Mutex<Feed>,
    reader: JoinHandle<Result<Heard, String>>,
}

impl Take {
    /// Starts a take. The model loads on the take's thread when it is not
    /// in memory; the audio waits for it.
    pub fn start(engine: &Arc<Engine>) -> Self {
        let (audio, heard) = mpsc::channel();
        let engine = Arc::clone(engine);
        let reader = std::thread::Builder::new()
            .name("tdt-take".into())
            .spawn(move || read_take(&engine, &heard))
            .expect("the system starts a thread for the take");
        Self {
            feed: Mutex::new(Feed {
                audio,
                samples: 0,
                startup: Some(0),
                closed: false,
            }),
            reader,
        }
    }

    /// Audio from the microphone: 16 kHz mono PCM16. Returns false when the
    /// take reached its recording limit or the user stopped it.
    pub fn push(&self, pcm: &[i16]) -> bool {
        let mut feed = self.feed.lock();
        if feed.closed {
            return false;
        }
        let samples = samples_of(pcm);
        if let Some(dropped) = feed.startup {
            let total = dropped + samples.len();
            if total <= MAX_STARTUP_SILENCE_SAMPLES && frame_rms(&samples) < STARTUP_SILENCE_RMS {
                feed.startup = Some(total);
                return true;
            }
            feed.startup = None;
        }
        feed.samples += samples.len();
        // The reader ends before `finish` only on an error; `finish` gives it.
        let _ = feed.audio.send(Live::Chunk(samples));
        if feed.samples >= MAX_CAPTURED_SAMPLES {
            feed.stop();
        }
        !feed.closed
    }

    /// Closes the take to input. The recorder can stop immediately.
    pub fn ask_stop(&self) -> bool {
        self.feed.lock().stop();
        false
    }

    /// Ends the take and waits for its text.
    pub fn finish(self) -> Result<Heard, String> {
        self.feed.lock().stop();
        let heard = self
            .reader
            .join()
            .unwrap_or_else(|_| Err("The speech engine failed on this take. Try again.".into()))?;
        // Times and counts only, never the words.
        crate::log::note(&format!(
            "take audio={} ms, model read {} ms of it after the stop",
            heard.samples / 16,
            heard.last_part / 16
        ));
        Ok(heard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_rejects_late_packet_while_user_is_speaking() {
        let (audio, input) = mpsc::channel();
        let (finished, completion) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut samples = 0;
            while let Ok(event) = input.recv() {
                match event {
                    Live::Chunk(audio) => samples += audio.len(),
                    Live::Finish => break,
                }
            }
            finished.send(samples).expect("report finished input");
            Ok(Heard {
                text: String::new(),
                samples,
                last_part: 0,
            })
        });
        let take = Take {
            feed: Mutex::new(Feed {
                audio,
                samples: 0,
                startup: Some(0),
                closed: false,
            }),
            reader,
        };
        assert!(take.push(&[10_000; 320]));
        assert!(!take.ask_stop());
        assert!(!take.push(&[10_000; 320]));
        assert_eq!(
            completion
                .recv_timeout(Duration::from_secs(1))
                .expect("release ends model input"),
            320
        );
        assert_eq!(take.finish().expect("take finishes").samples, 320);
    }
}
