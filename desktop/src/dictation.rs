//! One take: the pump thread that moves the captured audio to the engine,
//! the engine thread that makes the text, and the end of the take.

use crate::audio::recorder::CaptureHandle;
use crate::audio::{play_sound, AudioRecorder, LiveWave, SoundEffect, SpeechTail, MAX_TAIL};
use crate::config::{AppStats, Settings};
use crate::hotkey::{
    current_binding, input_appeared, input_held, input_mask, note_user_typed, reset_user_typed,
    user_typed,
};
use crate::paste::{DeliveryOutcome, DeliverySteps, NotTyped, PasteInjector};
use crate::priority;
use crate::ui::window_util;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use voice_stt_desktop::log;
use voice_stt_desktop::stt::{
    remove_fillers, EngineSlot, ErrorKind, Latency, LiveAudio, LiveTranscript, StopTimes,
    SttEngine, TakeError,
};

/// A take from its start until its pump has sent the last audio.
pub struct LiveTake {
    tail: Arc<Mutex<SpeechTail>>,
    release: Arc<AtomicBool>,
    halt: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    /// The keys that were down when the end of the take started. `None`
    /// while the take records.
    baseline: Option<[bool; 256]>,
    released_at: Option<Instant>,
    /// Wakes the pump at the release and at the stop.
    handle: CaptureHandle,
}

impl LiveTake {
    /// Starts the pump of `handle`, and the engine thread when there is an
    /// engine. `on_result` gets the text or the error of the engine, unless
    /// the take was discarded.
    pub fn start(
        handle: CaptureHandle,
        engine: Option<Arc<SttEngine>>,
        on_result: impl FnOnce(Result<LiveTranscript, TakeError>) + Send + 'static,
    ) -> Self {
        let take = Self {
            tail: Arc::new(Mutex::new(SpeechTail::default())),
            release: Arc::new(AtomicBool::new(false)),
            halt: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
            discard: Arc::new(AtomicBool::new(false)),
            baseline: None,
            released_at: None,
            handle,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        if let Some(engine) = engine {
            let discard = Arc::clone(&take.discard);
            std::thread::spawn(move || {
                priority::raise_thread();
                let result = engine.transcribe_live_reporting(&rx);
                if !discard.load(Ordering::SeqCst) {
                    on_result(result);
                }
            });
        } else {
            drop(rx);
        }
        let handle = take.handle.clone();
        let tail = Arc::clone(&take.tail);
        let release = Arc::clone(&take.release);
        let halt = Arc::clone(&take.halt);
        let done = Arc::clone(&take.done);
        std::thread::spawn(move || {
            priority::raise_thread();
            loop {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                if handle.limit_reached() {
                    break;
                }
                let chunk = handle.drain();
                let empty = chunk.is_empty();
                if !empty {
                    tail.lock().push_samples(&chunk);
                    let _ = tx.send(LiveAudio::Chunk(chunk));
                }
                if release.load(Ordering::SeqCst) && tail.lock().should_stop() {
                    break;
                }
                if empty {
                    handle.wait_for_audio(Duration::from_millis(8));
                }
            }
            let rest = handle.stop();
            if !rest.is_empty() {
                let _ = tx.send(LiveAudio::Chunk(rest));
            }
            // Latency counts from the stop request, the wait the user feels.
            let closed = Instant::now();
            let asked = tail.lock().asked_at().unwrap_or(closed);
            let _ = tx.send(LiveAudio::Finish(StopTimes { asked, closed }));
            done.store(true, Ordering::SeqCst);
        });
        take
    }

    /// Starts the end of a take: the microphone stays open only while the
    /// last word ends. `shortcut` is the shortcut key when it is down now (a
    /// stop at the key-down): it is not a key that the user types. Returns
    /// true when the take must stop at once.
    pub fn begin_stop(take: &mut Option<Self>, asked: Instant, shortcut: Option<u16>) -> bool {
        if take.as_ref().is_some_and(|live| live.baseline.is_some()) {
            // The end of the take started before.
            return false;
        }
        let mut baseline = input_mask();
        if let Some(slot) = shortcut.and_then(|vk| baseline.get_mut(usize::from(vk))) {
            *slot = true;
        }
        if let Some(vk) = input_held(current_binding().vk) {
            note_user_typed(vk);
            return true;
        }
        let Some(live) = take.as_mut() else {
            return true;
        };
        live.tail.lock().mark_release(asked);
        live.released_at = Some(Instant::now());
        live.release.store(true, Ordering::SeqCst);
        // A user who was quiet stops now, not at the next audio buffer.
        live.handle.wake();
        live.baseline = Some(baseline);
        false
    }

    /// True when the end of the take must not wait for the tail: a key went
    /// down after the end started, or the tail ran for `MAX_TAIL` of wall
    /// time (the sample time stops when the capture callback stalls).
    pub fn must_stop_now(&self) -> bool {
        if let Some(vk) = self.baseline.as_ref().and_then(input_appeared) {
            note_user_typed(vk);
            return true;
        }
        self.released_at.is_some_and(|at| at.elapsed() >= MAX_TAIL)
    }

    /// Stops the recording now. The audio so far still goes to the engine.
    pub fn halt(&mut self) {
        self.halt.store(true, Ordering::SeqCst);
        self.handle.wake();
        self.baseline = None;
    }

    /// Stops the take, and drops its text and its errors.
    pub fn discard(mut self) {
        self.discard.store(true, Ordering::SeqCst);
        self.halt();
    }

    /// The user still holds or records: the end of the take has not started,
    /// and the pump runs.
    pub fn is_held(&self) -> bool {
        !self.release.load(Ordering::SeqCst) && !self.done.load(Ordering::SeqCst)
    }

    /// The pump has sent the last audio.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// The take reached the take limit. The pump stops there.
    pub fn at_limit(&self) -> bool {
        self.handle.limit_reached()
    }
}

/// A change that the pill must show. `Dictation` gives them in order; the
/// UI loop gives each one to the view.
pub enum ViewChange {
    Error(String, ErrorKind),
    /// No speech model: say so, and open Settings.
    NeedModel,
    /// A take starts: the panel closes.
    CloseSettings,
    Listening(Option<LiveWave>),
    Transcribing {
        at_limit: bool,
    },
    Success {
        text: String,
        auto_pasted: bool,
        latency_ms: u64,
        notice: Option<(String, ErrorKind)>,
    },
    TakeError(TakeError),
    NoSpeech,
    MicrophoneReady,
}

/// Where the dictation is. The view shows it; only `Dictation` changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// The microphone records. The end of the take can have started
    /// (`LiveTake::begin_stop`): the tail of the last word then still records.
    Recording,
    /// The engine makes the text of the take.
    Transcribing,
}

/// A take whose stream gave no audio starts again one time, with the
/// microphone opened again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Restart {
    None,
    /// The silent take is gone; the next press starts it again, with no
    /// start sound.
    Pending,
    /// The take started again. It does not start again a second time, and
    /// its result says that the first words can be missing.
    Reopened,
}

/// The press of a shortcut, a click on the pill, or the release.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Press,
    HoldReleased(Instant),
    Toggled(Instant),
}

/// The dictation of TDT: the microphone, the take, and the state between
/// the shortcut and the text. The UI loop gives it the input and shows the
/// changes it returns.
pub struct Dictation {
    phase: Phase,
    /// The last take. It stays after its end until its pump is done, so a
    /// new take does not start next to it.
    take: Option<LiveTake>,
    recorder: Option<AudioRecorder>,
    /// The take recorded when the shortcut went down. A tap stops only a
    /// take that it did not start.
    recording_before_press: bool,
    /// The take stopped at the take limit.
    limit_notice: bool,
    /// A microphone that failed during the take, for the notice after its text.
    microphone_lost: Option<String>,
    restart: Restart,
    engine: EngineSlot,
    injector: Arc<PasteInjector>,
    settings: Settings,
    /// The window that had the focus when the take started gets its text.
    target: Arc<Mutex<Option<isize>>>,
    events_tx: crossbeam_channel::Sender<InternalEvent>,
    events_rx: crossbeam_channel::Receiver<InternalEvent>,
    /// The preview captures: no input, no microphone.
    frozen: bool,
}

impl Dictation {
    pub fn new(
        recorder: Option<AudioRecorder>,
        engine: EngineSlot,
        settings: Settings,
        frozen: bool,
    ) -> Self {
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        Self {
            phase: Phase::Idle,
            take: None,
            recorder,
            recording_before_press: false,
            limit_notice: false,
            microphone_lost: None,
            restart: Restart::None,
            engine,
            injector: Arc::new(PasteInjector::new()),
            settings,
            target: Arc::new(Mutex::new(None)),
            events_tx,
            events_rx,
            frozen,
        }
    }

    /// A take records or transcribes: the UI loop polls fast.
    pub fn is_busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    fn recording(&self) -> bool {
        self.phase == Phase::Recording
    }

    /// The user chose Retry microphone. Opening the microphone makes COM
    /// calls that can run a message loop: call this between view updates.
    pub fn retry_microphone(&mut self) -> Vec<ViewChange> {
        if self.frozen || self.is_busy() {
            return Vec::new();
        }
        match AudioRecorder::new() {
            Ok(recorder) => {
                log::note(&format!("Microphone: {}", recorder.describe()));
                self.recorder = Some(recorder);
                vec![ViewChange::MicrophoneReady]
            }
            Err(error) => vec![ViewChange::Error(
                format!("Could not open the microphone. Check Windows microphone access and try again. {error}"),
                ErrorKind::Microphone,
            )],
        }
    }

    /// TDT quits: the take ends with no text.
    pub fn quit(&mut self) {
        if let Some(take) = self.take.take() {
            take.discard();
        }
    }

    /// One step of the UI loop. `clicked`: a click on the resting pill, a
    /// tap of the shortcut. `stop_requested` reads (and clears) a stop of
    /// the pill or the tray; it is read only while a take records.
    pub fn step(
        &mut self,
        mut input: Vec<Input>,
        clicked: bool,
        stop_requested: impl FnOnce() -> bool,
    ) -> Vec<ViewChange> {
        let mut changes = Vec::new();
        if clicked && !self.is_busy() {
            // The hook clears this when the shortcut goes down. A click must
            // clear it too, or a key typed before the click stops the paste.
            reset_user_typed();
            input.push(Input::Press);
            input.push(Input::Toggled(Instant::now()));
        }
        // The take must stop in this step.
        let mut stop_now = self.check_stream(&mut input, &mut changes);
        let at_limit = self.recording() && self.take.as_ref().is_some_and(LiveTake::at_limit);
        let stop_requested = self.recording() && stop_requested();
        if self.recording() && (at_limit || stop_requested) {
            if stop_requested {
                // As a stop press of the shortcut does: only keys typed after
                // the stop keep the text off the app.
                reset_user_typed();
            }
            self.limit_notice = at_limit;
            stop_now = true;
        }
        if !self.frozen {
            for input in input {
                match input {
                    Input::Press => self.press(&mut changes, &mut stop_now),
                    Input::HoldReleased(asked) => self.release(asked, true, &mut stop_now),
                    Input::Toggled(asked) => self.release(asked, false, &mut stop_now),
                }
            }
        }
        if self.recording() && self.take.as_ref().is_some_and(LiveTake::must_stop_now) {
            stop_now = true;
        }
        if self.recording() && self.take.as_ref().is_some_and(LiveTake::is_done) {
            if self.take.as_ref().is_some_and(LiveTake::at_limit) {
                self.limit_notice = true;
            }
            stop_now = true;
        }
        if stop_now && self.recording() {
            if let Some(take) = self.take.as_mut() {
                take.halt();
            }
            self.phase = Phase::Transcribing;
            play_sound(SoundEffect::StopListening);
            changes.push(ViewChange::Transcribing {
                at_limit: self.limit_notice,
            });
        }
        self.take_results(&mut changes);
        // Fully idle: park the microphone until the next press.
        if !self.is_busy() {
            if let Some(recorder) = &self.recorder {
                recorder.pause_input();
            }
        }
        changes
    }

    /// A stream error, or a stream that stopped during the take. Returns
    /// true when the take must stop.
    fn check_stream(&mut self, input: &mut Vec<Input>, changes: &mut Vec<ViewChange>) -> bool {
        let recording = self.recording();
        let Some(error) = self.recorder.as_ref().and_then(|recorder| {
            recorder.take_error().or_else(|| {
                (recording && recorder.stalled()).then(|| {
                    "The microphone stopped during the recording. Check the input device and try again.".to_string()
                })
            })
        }) else {
            return false;
        };
        let silent = self
            .recorder
            .as_ref()
            .is_some_and(|recorder| recorder.captured_samples() == 0);
        let held = self.take.as_ref().is_some_and(LiveTake::is_held);
        // The next press opens the microphone again.
        self.recorder = None;
        if recording && silent && held && self.restart == Restart::None {
            // The stream gave no audio in this take, as it can after the PC
            // wakes. The user still talks, so open the microphone again in
            // this press.
            log::note(&format!(
                "The microphone gave no audio. TDT opens it again in the same take. {error}"
            ));
            if let Some(take) = self.take.take() {
                take.discard();
            }
            self.phase = Phase::Idle;
            self.restart = Restart::Pending;
            input.insert(0, Input::Press);
            false
        } else if self.is_busy() {
            // The audio before the failure is already with the engine:
            // finish the take, then say why it ended.
            self.microphone_lost = Some(error);
            recording
        } else {
            changes.push(ViewChange::Error(error, ErrorKind::Microphone));
            false
        }
    }

    fn press(&mut self, changes: &mut Vec<ViewChange>, stop_now: &mut bool) {
        // Snapshot the before-press state; it is cleared on every release,
        // so a swallowed press can never stop a take it did not start.
        self.recording_before_press = self.recording();
        // A press during a take can only be the stop of a take that a tap
        // started. The stop starts when the key goes down. At the key-up,
        // the click of the key is in the audio, and the microphone stays open
        // until that sound ends (about 300 ms).
        if self.recording() && !*stop_now {
            *stop_now =
                LiveTake::begin_stop(&mut self.take, Instant::now(), Some(current_binding().vk));
        }
        // One take at a time: the recorder and the engine have one session.
        let pump_busy = self.take.as_ref().is_some_and(|take| !take.is_done());
        if self.is_busy() || pump_busy {
            return;
        }
        let restarted = self.restart == Restart::Pending;
        self.restart = if restarted {
            Restart::Reopened
        } else {
            Restart::None
        };
        if self.engine.get().is_none() {
            changes.push(ViewChange::NeedModel);
            return;
        }
        // A stream that does not start again is opened again too.
        let reopen = self.recorder.as_ref().and_then(|recorder| {
            recorder
                .reopen_reason()
                .or_else(|| recorder.resume_input().err())
        });
        if self.recorder.is_none() || reopen.is_some() {
            self.recorder = None;
            match AudioRecorder::new() {
                Ok(recorder) => {
                    let reason = reopen
                        .map(|reason| format!(", opened again: {reason}"))
                        .unwrap_or_default();
                    log::note(&format!("Microphone: {}{reason}", recorder.describe()));
                    self.recorder = Some(recorder);
                }
                Err(error) => {
                    changes.push(ViewChange::Error(
                        format!("Could not open the microphone. Connect a microphone and retry. {error}"),
                        ErrorKind::Microphone,
                    ));
                    return;
                }
            }
        }
        let Some(recorder) = &self.recorder else {
            return;
        };
        // Capture first: the buffer holds the audio until the pump drains it.
        recorder.start();
        *self.target.lock() = window_util::dictation_target_window();
        // The pill shows the take on the screen where the text goes.
        window_util::move_pill_to_screen_of(*self.target.lock());
        changes.push(ViewChange::CloseSettings);
        let handle = recorder.handle();
        let wave = recorder.live_wave();
        self.limit_notice = false;
        let events = self.events_tx.clone();
        let injector = Arc::clone(&self.injector);
        let settings = self.settings.clone();
        let target = Arc::clone(&self.target);
        self.take = Some(LiveTake::start(handle, self.engine.get(), move |result| {
            match result {
                Ok(result) => {
                    let target = *target.lock();
                    deliver_transcript(result, injector, events, &settings, target, user_typed());
                }
                // The engine's errors are written for the user.
                Err(error) => {
                    let _ = events.send(InternalEvent::TranscribeError(error));
                }
            }
        }));
        if !restarted {
            play_sound(SoundEffect::StartListening);
        }
        self.phase = Phase::Recording;
        changes.push(ViewChange::Listening(Some(wave)));
    }

    /// A hold always stops; a tap only stops a take that it did not start.
    fn release(&mut self, asked: Instant, hold: bool, stop_now: &mut bool) {
        let should_stop = hold || self.recording_before_press;
        self.recording_before_press = false;
        if should_stop && self.recording() && !*stop_now {
            *stop_now = LiveTake::begin_stop(&mut self.take, asked, None);
        }
    }

    /// The results of the engine thread.
    fn take_results(&mut self, changes: &mut Vec<ViewChange>) {
        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                InternalEvent::TranscribeSuccess {
                    text,
                    auto_pasted,
                    notice,
                    latency_ms,
                } => {
                    let late_start = self.end_take();
                    play_sound(SoundEffect::Success);
                    let notice = notice
                        .map(|notice| (notice, ErrorKind::Other))
                        .or(self
                            .microphone_lost
                            .take()
                            .map(|lost| (lost, ErrorKind::Microphone)))
                        .or_else(|| {
                            late_start.then(|| {
                                (
                                    "The microphone started late, so the first words can be missing."
                                        .to_string(),
                                    ErrorKind::Other,
                                )
                            })
                        });
                    changes.push(ViewChange::Success {
                        text,
                        auto_pasted,
                        latency_ms,
                        notice,
                    });
                }
                InternalEvent::TranscribeError(error) => {
                    self.microphone_lost = None;
                    self.end_take();
                    play_sound(SoundEffect::Error);
                    changes.push(ViewChange::TakeError(error));
                }
                InternalEvent::NoSpeech => {
                    self.end_take();
                    if let Some(error) = self.microphone_lost.take() {
                        changes.push(ViewChange::Error(error, ErrorKind::Microphone));
                        continue;
                    }
                    changes.push(ViewChange::NoSpeech);
                }
            }
        }
    }

    /// The engine gave the result of the take, so the take is over, also
    /// when the loop has not seen the end of its pump yet. Returns true when
    /// the take started again late (`Restart::Reopened`).
    fn end_take(&mut self) -> bool {
        if self.recording() {
            if let Some(take) = self.take.as_mut() {
                take.halt();
            }
        }
        self.phase = Phase::Idle;
        let late_start = self.restart == Restart::Reopened;
        if late_start {
            self.restart = Restart::None;
        }
        late_start
    }
}

/// What the engine thread of a take tells the UI loop.
pub enum InternalEvent {
    TranscribeSuccess {
        text: String,
        auto_pasted: bool,
        notice: Option<String>,
        latency_ms: u64,
    },
    TranscribeError(TakeError),
    NoSpeech,
}

pub fn deliver_transcript(
    mut result: LiveTranscript,
    injector: Arc<PasteInjector>,
    tx: crossbeam_channel::Sender<InternalEvent>,
    settings: &Settings,
    target: Option<isize>,
    typed_key: Option<u16>,
) {
    let (should_paste, clean_fillers, save_take) =
        settings.get(|config| (config.auto_paste, config.remove_fillers, config.save_takes));
    // The model's own text, before the filler removal. The disk write must
    // not delay the text.
    if save_take {
        let (audio, text) = (std::mem::take(&mut result.audio), result.text.clone());
        std::thread::spawn(move || {
            if let Err(error) = voice_stt_desktop::stt::takes::save(&audio, &text) {
                log::note(&format!("Could not save the test take: {error}"));
            }
        });
    }
    if clean_fillers {
        result.text = remove_fillers(&result.text);
    }
    let mut steps = DeliverySteps::default();
    let delivered = injector.deliver(
        &result.text,
        should_paste,
        target,
        typed_key.is_some(),
        &mut steps,
    );
    // The wait of the user ends when the text is in the app. The copy to the
    // clipboard comes after that.
    let latency = result.latency(steps.typed_at.unwrap_or_else(Instant::now));
    log_delivery(&delivered, target, typed_key, latency, &result, steps);
    // Text reaches the user first; the history write is not on the latency path.
    // A failed copy still lands in History so the transcript is not lost.
    if !result.text.trim().is_empty() {
        AppStats::update(|stats| {
            stats.record(&result.text, result.duration_secs, latency.total_ms())
        });
    }
    let (auto_pasted, notice) = match delivered {
        Ok(DeliveryOutcome::NoSpeech) => {
            let _ = tx.send(InternalEvent::NoSpeech);
            return;
        }
        Ok(DeliveryOutcome::Copied) => (false, None),
        Ok(DeliveryOutcome::Pasted) => (true, None),
        Ok(DeliveryOutcome::PastedNotCopied(_)) => (
            true,
            Some("TDT typed the text, but could not copy it to the clipboard.".to_string()),
        ),
        Ok(DeliveryOutcome::CopiedFallback(reason)) => (false, Some(reason.notice().to_string())),
        Err(error) => {
            let _ = tx.send(InternalEvent::TranscribeError(
                format!("Could not copy text. Open History to copy the saved transcript. {error}")
                    .into(),
            ));
            return;
        }
    };
    let _ = tx.send(InternalEvent::TranscribeSuccess {
        text: result.text,
        auto_pasted,
        notice,
        latency_ms: latency.total_ms(),
    });
}

/// One line per dictation, so a paste that did not arrive or a slow take can
/// be explained from the log. It names the app, never the text.
fn log_delivery(
    delivered: &Result<DeliveryOutcome, String>,
    target: Option<isize>,
    typed_key: Option<u16>,
    latency: Latency,
    take: &LiveTranscript,
    steps: DeliverySteps,
) {
    let result = match delivered {
        Ok(DeliveryOutcome::NoSpeech) => return,
        Ok(DeliveryOutcome::Pasted) => "typed".to_string(),
        Ok(DeliveryOutcome::PastedNotCopied(error)) => format!("typed, not copied: {error}"),
        Ok(DeliveryOutcome::Copied) => "copied (auto-paste is off)".to_string(),
        Ok(DeliveryOutcome::CopiedFallback(NotTyped::UserTyped)) => format!(
            "copied, not typed: key 0x{:02X} went down while TDT was finishing",
            typed_key.unwrap_or(0)
        ),
        Ok(DeliveryOutcome::CopiedFallback(reason)) => format!("copied, not typed: {reason}"),
        Err(error) => format!("not copied: {error}"),
    };
    let app = target.map_or_else(
        || "no window".to_string(),
        |raw| crate::target::Target::of(raw).to_string(),
    );
    log::note(&format!(
        "Delivery: {result}. App: {app}. Latency: {latency}. Take {:.1} s, last part {:.1} s, {} characters ({steps}).",
        take.duration_secs,
        take.last_part_secs,
        take.text.chars().count()
    ));
}
