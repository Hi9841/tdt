use super::dictionary;
use super::models::ModelSpec;
use super::segment;
use super::MAX_TAKE_SAMPLES;
use crate::SAMPLE_RATE;
use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError};
use parking_lot::{Condvar, Mutex};
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use trit::{BiasWeights, Cancel, LoadOptions, Trit};

/// Idle time after a dictation before its memory goes back to the system.
/// Long enough that normal pauses between takes keep the warm buffer pool.
const IDLE_RELEASE: Duration = Duration::from_secs(60);

struct IdleState {
    active: usize,
    deadline: Option<Instant>,
}

struct IdleReleaser {
    state: Mutex<IdleState>,
    wake: Condvar,
}

impl IdleReleaser {
    fn global() -> &'static Self {
        static RELEASER: OnceLock<IdleReleaser> = OnceLock::new();
        RELEASER.get_or_init(|| {
            let releaser = IdleReleaser {
                state: Mutex::new(IdleState {
                    active: 0,
                    deadline: None,
                }),
                wake: Condvar::new(),
            };
            std::thread::Builder::new()
                .name("tdt-idle-release".to_string())
                .spawn(|| Self::run_loop(Self::global()))
                .ok();
            releaser
        })
    }

    fn run_loop(this: &'static Self) {
        let mut guard = this.state.lock();
        loop {
            match guard.deadline {
                None => this.wake.wait(&mut guard),
                Some(deadline) => {
                    let now = Instant::now();
                    if now < deadline {
                        this.wake.wait_for(&mut guard, deadline - now);
                        continue;
                    }
                    if guard.active == 0 {
                        guard.deadline = None;
                        trit::release_memory();
                    } else {
                        guard.deadline = None;
                    }
                }
            }
        }
    }

    fn start_dictation(&self) {
        let mut guard = self.state.lock();
        guard.active += 1;
        guard.deadline = None;
    }

    fn end_dictation(&self) {
        let mut guard = self.state.lock();
        guard.active -= 1;
        if guard.active == 0 {
            guard.deadline = Some(Instant::now() + IDLE_RELEASE);
            self.wake.notify_one();
        }
    }
}

/// One dictation, from its start to its end. A release that waits from the
/// dictation before does not run during this one. Each end schedules the
/// single idle-release worker, also the end with an error: the engine holds
/// the buffers of a take that failed as it holds those of a take with text.
struct Dictation;

impl Dictation {
    fn start() -> Self {
        IdleReleaser::global().start_dictation();
        Self
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        IdleReleaser::global().end_dictation();
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

/// The text of `samples`, the end of a take, with `end_quiet` samples of
/// digital quiet after them (`ModelSpec::end_quiet_samples`).
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
/// from `window` on, the samples that the model read for it, and whether
/// a multi-window take fell back to reading the whole audio from 0.
fn read_rest(
    engine: &Trit,
    text: &Option<String>,
    captured: &[f32],
    window: usize,
    end_quiet: usize,
    cancel: Option<&Cancel>,
) -> Result<(String, usize, bool), String> {
    if cancel.is_some_and(Cancel::is_cancelled) {
        return Ok((String::new(), 0, false));
    }
    let joined = match text {
        Some(done) => {
            let part = read_end(engine, &captured[window..], end_quiet, cancel)?;
            if cancel.is_some_and(Cancel::is_cancelled) {
                return Ok((String::new(), 0, false));
            }
            join(done, part, window)
        }
        None => None,
    };
    if cancel.is_some_and(Cancel::is_cancelled) {
        return Ok((String::new(), 0, false));
    }
    Ok(match joined {
        Some(text) => (text, captured.len() - window, false),
        None => (
            read_end(engine, captured, end_quiet, cancel)?,
            captured.len(),
            window > 0,
        ),
    })
}

/// Upper bound when waiting on a quiet read after the user stops talking.
/// A stalled background read must never leave the pill stuck on Transcribing.
const QUIET_FINISH_WAIT: Duration = Duration::from_secs(15);
/// Grace period for a previously timed-out quiet worker to finish exiting
/// before a subsequent take refuses to block behind it on Trit's `submit` lock.
const QUIET_RECOVERY_WAIT: Duration = Duration::from_millis(500);
/// Minimum captured audio before starting rolling background reads while
/// the user is still speaking (400 ms).
const ROLLING_MIN_SAMPLES: usize = 400 * SAMPLE_RATE / 1000;
/// Minimum new audio between rolling background passes (30 ms).
const ROLLING_STEP_SAMPLES: usize = 30 * SAMPLE_RATE / 1000;
/// Maximum staleness of a rolling read used at key-up (120 ms). Older reads
/// fall back to a full decode so stalls do not drop whole phrases.
const ROLLING_MAX_STALE_SAMPLES: usize = 120 * SAMPLE_RATE / 1000;
/// When the last completed rolling pass is within this window (60 ms) and the
/// in-flight pass is not expected to finish inside `ROLLING_FINISH_WAIT`,
/// return the completed pass immediately instead of waiting for a timeout.
const ROLLING_IMMEDIATE_STALE_SAMPLES: usize = 60 * SAMPLE_RATE / 1000;
/// Small post-release wait budget for a fresher in-flight rolling read.
const ROLLING_FINISH_WAIT: Duration = Duration::from_millis(10);
/// Post-release wait budget when no completed rolling pass is fresh enough.
const ROLLING_SOLO_WAIT: Duration = Duration::from_millis(18);

static QUIET_SLOT: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

struct QuietSlot;

impl QuietSlot {
    fn claim() -> Option<Self> {
        let mut running = QUIET_SLOT.0.lock();
        if *running {
            return None;
        }
        *running = true;
        Some(Self)
    }

    fn wait_until_free(timeout: Duration) -> bool {
        let mut running = QUIET_SLOT.0.lock();
        let deadline = Instant::now() + timeout;
        while *running {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            QUIET_SLOT.1.wait_for(&mut running, remaining);
        }
        true
    }
}

impl Drop for QuietSlot {
    fn drop(&mut self) {
        *QUIET_SLOT.0.lock() = false;
        QUIET_SLOT.1.notify_all();
    }
}

fn await_quiet_worker() -> Result<(), TakeError> {
    if QuietSlot::wait_until_free(QUIET_RECOVERY_WAIT) {
        Ok(())
    } else {
        Err("The previous speech job is still finishing. Wait a moment and try again.".into())
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
        let heard = speech.read_samples(end);
        (heard >= windows_end).then_some(Self { end, window, heard })
    }
}

struct CaptureState {
    captured: Vec<f32>,
    /// The windows so far are in `text`, `None` once two did not join; the
    /// next one starts at `window`.
    window: usize,
    /// The end of the last window that the model read.
    windows_end: usize,
    text: Option<String>,
    speech: segment::EndOfSpeech,
}

impl CaptureState {
    fn new(end_quiet: usize) -> Self {
        let speech = if end_quiet > 0 {
            segment::EndOfSpeech::for_padded_end()
        } else {
            segment::EndOfSpeech::new()
        };
        Self {
            captured: Vec::new(),
            window: 0,
            windows_end: 0,
            text: Some(String::new()),
            speech,
        }
    }

    fn ingest(
        &mut self,
        samples: &[f32],
        engine: &Trit,
        active: &mut Option<ActiveRead>,
        latest_rolling: &mut Option<CompletedRolling>,
        last_rolling_heard: &mut usize,
    ) -> Result<(), TakeError> {
        if self.captured.len() >= MAX_TAKE_SAMPLES {
            return Ok(());
        }
        let room = MAX_TAKE_SAMPLES - self.captured.len();
        self.captured
            .extend_from_slice(&samples[..samples.len().min(room)]);
        // Read full windows while the user still talks. New chunks wait in the
        // channel meanwhile.
        while self.captured.len() - self.window >= segment::WINDOW_SAMPLES {
            let Some(done) = &self.text else {
                break;
            };
            // The next window makes any background read old.
            *active = None;
            *latest_rolling = None;
            *last_rolling_heard = 0;
            await_quiet_worker()?;
            let end = self.window + segment::WINDOW_SAMPLES;
            let part = transcribe_samples(engine, &self.captured[self.window..end], None)?;
            self.text = join(done, part, self.window);
            self.window = end - segment::OVERLAP_SAMPLES;
            self.windows_end = end;
        }
        self.speech.update(&self.captured);
        Ok(())
    }
}

enum ActiveRead {
    Quiet(QuietRead),
    Rolling(RollingRead),
}

struct RollingRead {
    window: usize,
    heard: usize,
    started_at: Instant,
    cancel: Cancel,
    result: Receiver<Result<(String, bool), String>>,
}

struct CompletedRolling {
    window: usize,
    heard: usize,
    text: String,
    fallback: bool,
}

impl RollingRead {
    fn start(
        engine: &Arc<Trit>,
        text: &Option<String>,
        heard: &[f32],
        window: usize,
        end_quiet: usize,
    ) -> Option<Self> {
        let slot = QuietSlot::claim()?;
        let (done, result) = crossbeam_channel::bounded(1);
        let cancel = Cancel::new();
        let flag = cancel.clone();
        let heard_len = heard.len();
        let (engine, text, samples) = (Arc::clone(engine), text.clone(), heard.to_vec());
        std::thread::spawn(move || {
            let _active = Dictation::start();
            let read = read_rest(&engine, &text, &samples, window, end_quiet, Some(&flag));
            drop(slot);
            let _ = done.send(read.map(|(text, _, fallback)| (text, fallback)));
        });
        Some(Self {
            window,
            heard: heard_len,
            started_at: Instant::now(),
            cancel,
            result,
        })
    }

    fn wait_for(self, timeout: Duration) -> QuietWait {
        match self.result.recv_timeout(timeout) {
            Ok(result) => QuietWait::Finished(result),
            Err(RecvTimeoutError::Timeout) => {
                self.cancel.cancel();
                QuietWait::TimedOut
            }
            Err(RecvTimeoutError::Disconnected) => QuietWait::Disconnected,
        }
    }
}

impl Drop for RollingRead {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn is_fresh(read_window: usize, heard: usize, window: usize, captured_len: usize) -> bool {
    read_window == window && captured_len.saturating_sub(heard) <= ROLLING_MAX_STALE_SAMPLES
}

/// Resolves a mid-speech release from an in-flight or recently completed
/// rolling read within a small post-release wait budget. Returns `None` when
/// no fresh non-empty rolling transcript is ready, so the caller falls back
/// to a full decode.
fn finish_rolling_read(
    active: Option<ActiveRead>,
    latest: Option<CompletedRolling>,
    last_duration: Duration,
    window: usize,
    captured_len: usize,
    quiet_wait_ms: &mut u64,
) -> Option<(String, bool)> {
    let latest = latest
        .filter(|r| is_fresh(r.window, r.heard, window, captured_len) && !r.text.trim().is_empty());
    let rolling = match active {
        Some(ActiveRead::Rolling(rolling))
            if is_fresh(rolling.window, rolling.heard, window, captured_len) =>
        {
            Some(rolling)
        }
        other => {
            drop(other);
            None
        }
    };
    if let Some(rolling) = rolling {
        if let Ok(Ok((text, fallback))) = rolling.result.try_recv() {
            if !text.trim().is_empty() {
                return Some((text, fallback));
            }
        }
        let skip_wait = latest.as_ref().is_some_and(|ready| {
            let stale = captured_len.saturating_sub(ready.heard);
            let remaining = last_duration.saturating_sub(rolling.started_at.elapsed());
            stale <= ROLLING_IMMEDIATE_STALE_SAMPLES && remaining > ROLLING_FINISH_WAIT
        });
        if skip_wait {
            drop(rolling);
            return latest.map(|ready| (ready.text, ready.fallback));
        }
        let budget = if latest.is_some() {
            ROLLING_FINISH_WAIT
        } else {
            ROLLING_SOLO_WAIT
        };
        let wait_start = Instant::now();
        let outcome = rolling.wait_for(budget);
        *quiet_wait_ms = wait_start.elapsed().as_millis() as u64;
        if let QuietWait::Finished(Ok((text, fallback))) = outcome {
            if !text.trim().is_empty() {
                return Some((text, fallback));
            }
        }
    }
    latest.map(|ready| (ready.text, ready.fallback))
}

/// A read of the take in the quiet after the speech, on its own thread: new
/// audio must reach the capture loop during the read, so the loop can stop a
/// read that new speech made useless. Dropping cancels the read immediately
/// without blocking the capture loop; `QuietSlot` stays held until the
/// cancelled thread exits, preventing overlapping quiet reads.
struct QuietRead {
    point: QuietPoint,
    cancel: Cancel,
    result: Receiver<Result<(String, bool), String>>,
}

enum QuietWait {
    Finished(Result<(String, bool), String>),
    Disconnected,
    TimedOut,
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
        let (done, result) = crossbeam_channel::bounded(1);
        let cancel = Cancel::new();
        let flag = cancel.clone();
        let (engine, text, samples) = (Arc::clone(engine), text.clone(), heard.to_vec());
        std::thread::spawn(move || {
            // Count the quiet worker's full lifetime in `IdleReleaser` so the
            // 60 s idle trim cannot fire while a quiet read is still active.
            let _active = Dictation::start();
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
            let _ = done.send(read.map(|(text, _, fallback)| (text, fallback)));
        });
        Some(Self {
            point,
            cancel,
            result,
        })
    }

    /// Cancels the worker if it cannot finish by the deadline.
    fn wait_for(self, timeout: Duration) -> QuietWait {
        match self.result.recv_timeout(timeout) {
            Ok(result) => QuietWait::Finished(result),
            Err(RecvTimeoutError::Timeout) => {
                self.cancel.cancel();
                crate::log::note(&format!(
                    "A read in the quiet did not finish in {timeout:?}; aborting this take."
                ));
                QuietWait::TimedOut
            }
            Err(RecvTimeoutError::Disconnected) => QuietWait::Disconnected,
        }
    }

    fn wait(self) -> QuietWait {
        self.wait_for(QUIET_FINISH_WAIT)
    }
}

impl Drop for QuietRead {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// What an error is about. A microphone error offers Retry microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Microphone,
    Other,
}

/// Why a take has no text. The text is written for the user.
#[derive(Debug)]
pub struct TakeError {
    pub message: String,
    pub kind: ErrorKind,
}

impl From<String> for TakeError {
    fn from(message: String) -> Self {
        Self {
            message,
            kind: ErrorKind::Other,
        }
    }
}

impl From<&str> for TakeError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl std::fmt::Display for TakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
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
    /// The UI loop dispatched the stop request to the capture pump.
    pub dispatched: Instant,
    /// Capture stopped accepting audio.
    pub closed: Instant,
    /// Audio captured after `dispatched`, in milliseconds of 16 kHz samples.
    pub tail_audio_ms: u64,
    /// Latest CPAL capture-to-callback packet age observed during the take.
    pub packet_age_ms: u64,
}

impl StopTimes {
    /// A stop with no wait for the last word.
    pub fn now() -> Self {
        let now = Instant::now();
        Self {
            asked: now,
            dispatched: now,
            closed: now,
            tail_audio_ms: 0,
            packet_age_ms: 0,
        }
    }
}

/// Per-take timing and fault signals logged whenever a take is unusually slow.
#[derive(Clone, Copy, Debug, Default)]
pub struct TakeDiagnostics {
    /// Time between `asked` (hook event) and UI dispatch (`begin_stop`).
    pub dispatch_ms: u64,
    /// Milliseconds of 16 kHz audio captured after `begin_stop`.
    pub tail_audio_ms: u64,
    /// Latest CPAL capture-to-callback packet age in milliseconds.
    pub packet_age_ms: u64,
    /// Time spent waiting for model preparation before capture processing began.
    pub wait_prepare_ms: u64,
    /// Time spent waiting on an in-flight `QuietRead` after key release.
    pub quiet_wait_ms: u64,
    /// True if a multi-window take fell back to transcribing the whole audio.
    pub whole_fallback: bool,
    /// True if an empty take was retried with amplified quiet speech.
    pub boost_retry: bool,
    /// Process page faults observed during this take (`None` if OS query failed).
    pub page_faults: Option<u32>,
}

impl std::fmt::Display for TakeDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dispatch {} ms, tail_audio {} ms, pkt_age {} ms, prepare {} ms, quiet_wait {} ms, faults ",
            self.dispatch_ms,
            self.tail_audio_ms,
            self.packet_age_ms,
            self.wait_prepare_ms,
            self.quiet_wait_ms,
        )?;
        match self.page_faults {
            Some(faults) => write!(f, "{faults}")?,
            None => f.write_str("n/a")?,
        }
        write!(
            f,
            ", whole_fallback {}, boost_retry {}",
            self.whole_fallback, self.boost_retry
        )
    }
}

#[cfg(windows)]
fn process_page_faults() -> Option<u32> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: `GetCurrentProcess` returns the current process pseudo-handle and
    // `counters` is valid for `size` bytes.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) }.is_ok();
    ok.then_some(counters.PageFaultCount)
}

#[cfg(not(windows))]
fn process_page_faults() -> Option<u32> {
    None
}

fn page_fault_delta(before: Option<u32>, after: Option<u32>) -> Option<u32> {
    Some(after?.saturating_sub(before?))
}

pub struct LiveTranscript {
    pub text: String,
    /// The audio that the model heard, for a test take.
    pub audio: Vec<f32>,
    pub duration_secs: f32,
    /// The speech the model still had to read after the stop. The model part
    /// of the latency follows it.
    pub last_part_secs: f32,
    pub stop: StopTimes,
    /// The model's text was ready.
    pub transcribed: Instant,
    pub diagnostics: TakeDiagnostics,
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
    /// `ModelSpec::end_quiet_samples`.
    end_quiet: usize,
    native: Mutex<Option<Arc<Trit>>>,
    /// Held during a load, so a take that starts during the startup load
    /// waits for it instead of loading a second model.
    loading: Mutex<()>,
    /// The dictionary file as last applied to the engine.
    dictionary: Mutex<dictionary::Watch>,
}

impl SttEngine {
    pub fn new(model_dir: &Path, spec: &ModelSpec) -> Self {
        Self {
            model_dir: model_dir.to_path_buf(),
            precision: spec.precision,
            end_quiet: spec.end_quiet_samples,
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
        let started = Instant::now();
        let options = LoadOptions {
            cache: true,
            precision: self.precision,
            ..LoadOptions::default()
        };
        let engine = Trit::load_reporting(&self.model_dir, &options, progress)?;
        let cache_state = if engine.loaded_from_cache() {
            "hit"
        } else {
            "rebuilt"
        };
        crate::log::note(&format!(
            "Speech model ready in {} ms (cache {cache_state}).",
            started.elapsed().as_millis()
        ));
        let engine = Arc::new(engine);
        *self.native.lock() = Some(Arc::clone(&engine));
        // Loading converts weights through buffers that are freed again.
        trit::release_memory();
        // Fault mapped weight pages into the working set in the background so
        // startup stays fast and the first take does not stall on PTE faults.
        std::thread::spawn(move || engine.prefetch_weights());
        // A fresh engine has no dictionary yet.
        *self.dictionary.lock() = dictionary::Watch::default();
        Ok(())
    }

    /// True when the loaded engine mapped an existing weight cache directly.
    pub fn loaded_from_cache(&self) -> bool {
        self.native
            .lock()
            .as_ref()
            .is_some_and(|engine| engine.loaded_from_cache())
    }

    /// Hints to the OS to fault mapped weight pages into RAM sequentially.
    pub fn prefetch_weights(&self) {
        if let Some(engine) = self.native.lock().clone() {
            engine.prefetch_weights();
        }
    }

    /// True when this model appends digital quiet context after the final
    /// audio (used to select the guarded post-release tail stop).
    pub fn has_end_quiet(&self) -> bool {
        self.end_quiet > 0
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
    ) -> Result<LiveTranscript, TakeError> {
        let _dictation = Dictation::start();
        await_quiet_worker()?;
        let faults_before = process_page_faults();
        let prepare_start = Instant::now();
        let was_loaded = self.native.lock().is_some();
        self.prepare().map_err(|error| {
            format!("Could not load the speech model. Choose another model in Settings. {error}")
        })?;
        let wait_prepare_ms = if was_loaded {
            0
        } else {
            prepare_start.elapsed().as_millis() as u64
        };
        self.refresh_dictionary();
        let native = self.native.lock().clone();
        let engine = native.ok_or("The speech model is not loaded.")?;
        // Prefetch mapped weights sequentially at t = 0 on a helper thread so
        // evicted weight pages return to RAM while the user speaks, covering
        // takes shorter than 1 s without blocking `rx`.
        {
            let prefetch_engine = Arc::clone(&engine);
            std::thread::spawn(move || prefetch_engine.prefetch_weights());
        }
        let mut state = CaptureState::new(self.end_quiet);
        let mut last_any = Instant::now();
        // The audio of the final text while the user stays quiet, and the
        // active background read (either a lossless quiet read or a rolling
        // speculative read while speaking).
        let mut point: Option<QuietPoint> = None;
        let mut active: Option<ActiveRead> = None;
        // Each quiet point gets one read.
        let mut read_point = None;
        let mut latest_rolling: Option<CompletedRolling> = None;
        let mut last_rolling_heard = 0usize;
        let mut last_rolling_duration = Duration::ZERO;

        let poll_rolling = |active: &mut Option<ActiveRead>,
                            latest: &mut Option<CompletedRolling>,
                            last_dur: &mut Duration| {
            let finished = match active {
                Some(ActiveRead::Rolling(rolling)) => match rolling.result.try_recv() {
                    Ok(res) => Some((rolling.window, rolling.heard, rolling.started_at, res)),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => {
                        *active = None;
                        None
                    }
                },
                _ => None,
            };
            if let Some((window, heard, started_at, res)) = finished {
                *last_dur = started_at.elapsed();
                if let Ok((text, fallback)) = res {
                    if !text.trim().is_empty() {
                        *latest = Some(CompletedRolling {
                            window,
                            heard,
                            text,
                            fallback,
                        });
                    }
                }
                *active = None;
            }
        };

        let stop = loop {
            let mut finished = None;
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(LiveAudio::Chunk(samples)) => {
                    last_any = Instant::now();
                    state.ingest(
                        &samples,
                        &engine,
                        &mut active,
                        &mut latest_rolling,
                        &mut last_rolling_heard,
                    )?;
                    // Drain any chunks or Finish already waiting in the channel
                    // before starting a background read so stale pauses do not
                    // spawn workers that must be cancelled on the next chunk.
                    loop {
                        match rx.try_recv() {
                            Ok(LiveAudio::Chunk(more)) => {
                                last_any = Instant::now();
                                state.ingest(
                                    &more,
                                    &engine,
                                    &mut active,
                                    &mut latest_rolling,
                                    &mut last_rolling_heard,
                                )?;
                            }
                            Ok(LiveAudio::Finish(times)) => {
                                finished = Some(times);
                                break;
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                return Err("The recording stopped too early. Try again.".into());
                            }
                        }
                    }
                }
                Ok(LiveAudio::Finish(times)) => break times,
                Err(RecvTimeoutError::Timeout) => {
                    poll_rolling(&mut active, &mut latest_rolling, &mut last_rolling_duration);
                    // A stalled producer must not hang transcription forever.
                    if last_any.elapsed() > Duration::from_secs(120) {
                        return Err(TakeError {
                            message: "The microphone stopped sending audio. Check the input device and try again."
                                .to_string(),
                            kind: ErrorKind::Microphone,
                        });
                    }
                    continue;
                }
                // The pump always ends a take with `Finish`, so audio may be
                // missing: text from a part of the take must not look whole.
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("The recording stopped too early. Try again.".into());
                }
            }

            poll_rolling(&mut active, &mut latest_rolling, &mut last_rolling_duration);
            point = QuietPoint::of(&state.speech, state.window, state.windows_end);
            match point {
                Some(point) => {
                    // A lossless quiet read supersedes any rolling or stale read.
                    if !matches!(&active, Some(ActiveRead::Quiet(read)) if read.point == point) {
                        active = None;
                    }
                    if let Some(times) = finished {
                        break times;
                    }
                    if read_point != Some(point) {
                        let heard = &state.captured[..point.heard];
                        if let Some(started) =
                            QuietRead::start(&engine, &state.text, heard, point, self.end_quiet)
                        {
                            // Record `read_point` only once the slot is claimed, so a
                            // busy slot retries on the next chunk instead of skipping
                            // pre-transcription for this pause permanently.
                            read_point = Some(point);
                            active = Some(ActiveRead::Quiet(started));
                        }
                    }
                }
                None => {
                    // New speech made any prior quiet read old.
                    if matches!(active, Some(ActiveRead::Quiet(_))) {
                        active = None;
                    }
                    if let Some(times) = finished {
                        break times;
                    }
                    if active.is_none()
                        && state.captured.len() >= ROLLING_MIN_SAMPLES
                        && state.captured.len().saturating_sub(last_rolling_heard)
                            >= ROLLING_STEP_SAMPLES
                    {
                        if let Some(started) = RollingRead::start(
                            &engine,
                            &state.text,
                            &state.captured,
                            state.window,
                            self.end_quiet,
                        ) {
                            last_rolling_heard = state.captured.len();
                            active = Some(ActiveRead::Rolling(started));
                        }
                    }
                }
            }
        };

        let CaptureState {
            captured,
            window,
            text,
            ..
        } = state;

        // Duration actually fed to the model, not the full take when the
        // capture cap truncated it.
        let duration_secs = captured.len() as f32 / SAMPLE_RATE as f32;

        let dispatch_ms = stop
            .dispatched
            .saturating_duration_since(stop.asked)
            .as_millis() as u64;

        if captured.is_empty() {
            return Ok(LiveTranscript {
                text: String::new(),
                audio: captured,
                duration_secs,
                last_part_secs: 0.0,
                stop,
                transcribed: Instant::now(),
                diagnostics: TakeDiagnostics {
                    dispatch_ms,
                    tail_audio_ms: stop.tail_audio_ms,
                    packet_age_ms: stop.packet_age_ms,
                    wait_prepare_ms,
                    page_faults: page_fault_delta(faults_before, process_page_faults()),
                    ..TakeDiagnostics::default()
                },
            });
        }

        // While a quiet point is current, the text of the take is the text of
        // the audio up to that point, with or without a read that ran before
        // the release. The read only moves the work to the time before the
        // release. A read that new speech or a window made old has stopped.
        let mut quiet_wait_ms = 0u64;
        let (mut text, last_part, whole_fallback) = match point {
            Some(point) => {
                let quiet_result = if let Some(ActiveRead::Quiet(read)) = active {
                    let wait_start = Instant::now();
                    let result = read.wait();
                    quiet_wait_ms = wait_start.elapsed().as_millis() as u64;
                    Some(result)
                } else {
                    drop(active);
                    None
                };
                match quiet_result {
                    Some(QuietWait::Finished(read)) => {
                        let (text, fallback) = read?;
                        (text, 0, fallback)
                    }
                    Some(QuietWait::TimedOut) => {
                        return Err(
                            "The speech engine took too long to finish this take. Try again."
                                .into(),
                        );
                    }
                    Some(QuietWait::Disconnected) | None => {
                        await_quiet_worker()?;
                        read_rest(
                            &engine,
                            &text,
                            &captured[..point.heard],
                            window,
                            self.end_quiet,
                            None,
                        )?
                    }
                }
            }
            None => {
                if let Some((rolling_text, fallback)) = finish_rolling_read(
                    active,
                    latest_rolling,
                    last_rolling_duration,
                    window,
                    captured.len(),
                    &mut quiet_wait_ms,
                ) {
                    (rolling_text, 0, fallback)
                } else {
                    await_quiet_worker()?;
                    read_rest(&engine, &text, &captured, window, self.end_quiet, None)?
                }
            }
        };
        let last_part_secs = last_part as f32 / SAMPLE_RATE as f32;

        let mut boost_retry = false;
        if text.is_empty() {
            if let Some(boosted) = boost_quiet_speech(&captured) {
                boost_retry = true;
                if let Ok(boosted_text) = read_end(&engine, &boosted, self.end_quiet, None) {
                    text = boosted_text;
                }
            }
        }

        let transcribed = Instant::now();
        let diagnostics = TakeDiagnostics {
            dispatch_ms,
            tail_audio_ms: stop.tail_audio_ms,
            packet_age_ms: stop.packet_age_ms,
            wait_prepare_ms,
            quiet_wait_ms,
            whole_fallback,
            boost_retry,
            page_faults: page_fault_delta(faults_before, process_page_faults()),
        };
        Ok(LiveTranscript {
            text,
            audio: captured,
            duration_secs,
            last_part_secs,
            stop,
            transcribed,
            diagnostics,
        })
    }

    pub fn release(&self) {
        *self.native.lock() = None;
    }
}

/// 25 ms. Matches the wave's frame.
const SPEECH_FRAME_SAMPLES: usize = SAMPLE_RATE / 40;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_read_drop_cancels_without_blocking() {
        let cancel = Cancel::new();
        let (_tx, rx) = crossbeam_channel::bounded::<Result<(String, bool), String>>(1);
        let read = QuietRead {
            point: QuietPoint {
                end: 100,
                window: 0,
                heard: 1600,
            },
            cancel: cancel.clone(),
            result: rx,
        };
        let started = Instant::now();
        drop(read);
        assert!(cancel.is_cancelled(), "dropping QuietRead must cancel it");
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "dropping QuietRead must not block the capture loop: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn quiet_read_wait_timeout_cancels_and_returns_timed_out() {
        let cancel = Cancel::new();
        let (_tx, rx) = crossbeam_channel::bounded::<Result<(String, bool), String>>(1);
        let read = QuietRead {
            point: QuietPoint {
                end: 100,
                window: 0,
                heard: 1600,
            },
            cancel: cancel.clone(),
            result: rx,
        };
        let started = Instant::now();
        let outcome = read.wait_for(Duration::from_millis(20));
        assert!(matches!(outcome, QuietWait::TimedOut));
        assert!(
            cancel.is_cancelled(),
            "timeout must cancel the quiet worker"
        );
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn quiet_slot_waits_for_cancelled_worker_to_release() {
        let slot = QuietSlot::claim().expect("claim slot");
        assert!(QuietSlot::claim().is_none());
        assert!(
            !QuietSlot::wait_until_free(Duration::from_millis(20)),
            "recovery must report false while the worker holds the slot"
        );
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            drop(slot);
        });
        assert!(
            QuietSlot::wait_until_free(Duration::from_millis(250)),
            "recovery must unblock as soon as the worker drops its slot"
        );
        holder.join().expect("holder exits");
    }

    #[test]
    fn rolling_read_returns_fresher_in_flight_within_budget() {
        let cancel = Cancel::new();
        let (tx, rx) = crossbeam_channel::bounded::<Result<(String, bool), String>>(1);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2));
            let _ = tx.send(Ok(("full sentence".to_string(), false)));
        });
        let active = Some(ActiveRead::Rolling(RollingRead {
            window: 0,
            heard: 16_000,
            started_at: Instant::now(),
            cancel,
            result: rx,
        }));
        let latest = Some(CompletedRolling {
            window: 0,
            heard: 15_200,
            text: "full".to_string(),
            fallback: false,
        });
        let mut wait_ms = 0;
        let out = finish_rolling_read(
            active,
            latest,
            Duration::from_millis(5),
            0,
            16_160,
            &mut wait_ms,
        );
        assert_eq!(out, Some(("full sentence".to_string(), false)));
    }

    #[test]
    fn rolling_read_skips_wait_when_completed_is_fresh_and_in_flight_is_slow() {
        let cancel = Cancel::new();
        let (_tx, rx) = crossbeam_channel::bounded::<Result<(String, bool), String>>(1);
        let active = Some(ActiveRead::Rolling(RollingRead {
            window: 0,
            heard: 16_000,
            started_at: Instant::now(),
            cancel: cancel.clone(),
            result: rx,
        }));
        let latest = Some(CompletedRolling {
            window: 0,
            heard: 15_600, // 35 ms stale (<= 60 ms)
            text: "ready text".to_string(),
            fallback: false,
        });
        let mut wait_ms = 0;
        let out = finish_rolling_read(
            active,
            latest,
            Duration::from_millis(40),
            0,
            16_160,
            &mut wait_ms,
        );
        assert_eq!(out, Some(("ready text".to_string(), false)));
        assert_eq!(wait_ms, 0);
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn rolling_read_falls_back_when_stale_or_empty() {
        let latest = Some(CompletedRolling {
            window: 0,
            heard: 10_000, // > 120 ms stale
            text: "old".to_string(),
            fallback: false,
        });
        let mut wait_ms = 0;
        assert_eq!(
            finish_rolling_read(None, latest, Duration::ZERO, 0, 16_000, &mut wait_ms),
            None
        );

        let empty = Some(CompletedRolling {
            window: 0,
            heard: 15_900,
            text: "   ".to_string(),
            fallback: false,
        });
        assert_eq!(
            finish_rolling_read(None, empty, Duration::ZERO, 0, 16_000, &mut wait_ms),
            None
        );
    }
}
