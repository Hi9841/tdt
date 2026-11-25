//! One take: the pump thread that moves the captured audio to the engine,
//! the engine thread that makes the text, and the end of the take.

use crate::audio::recorder::CaptureHandle;
use crate::audio::{play_sound, AudioRecorder, LiveWave, SoundEffect};
use crate::config::{AppStats, Settings};
use crate::hotkey::{
    current_binding, input_held, note_user_typed, typed_generation, user_typed_since,
};
use crate::paste::{DeliveryOutcome, DeliverySteps, NotTyped, PasteInjector};
use crate::priority;
use crate::ui::window_util;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use voice_stt_desktop::log;
use voice_stt_desktop::stt::{
    remove_fillers, EngineSlot, ErrorKind, Latency, LiveAudio, LiveTranscript, StopTimes,
    SttEngine, TakeError,
};

/// A take from its start until its pump has sent the last audio.
struct LiveTake {
    stop: Arc<Mutex<Option<StopTimes>>>,
    done: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    typing_generation: Arc<AtomicU64>,
    handle: CaptureHandle,
}

impl LiveTake {
    /// Starts the pump of `handle`, and the engine thread when there is an
    /// engine. `on_result` gets the text or the error of the engine, unless
    /// the take was discarded.
    fn start(
        handle: CaptureHandle,
        engine: Option<Arc<SttEngine>>,
        typing_generation: u64,
        on_result: impl FnOnce(Result<LiveTranscript, TakeError>, u64) + Send + 'static,
    ) -> Self {
        let take = Self {
            stop: Arc::new(Mutex::new(None)),
            done: Arc::new(AtomicBool::new(false)),
            discard: Arc::new(AtomicBool::new(false)),
            typing_generation: Arc::new(AtomicU64::new(typing_generation)),
            handle,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        if let Some(engine) = engine {
            let discard = Arc::clone(&take.discard);
            let typing_generation = Arc::clone(&take.typing_generation);
            std::thread::spawn(move || {
                priority::raise_thread();
                let result = engine.transcribe_live_reporting(&rx);
                if !discard.load(Ordering::SeqCst) {
                    on_result(result, typing_generation.load(Ordering::SeqCst));
                }
            });
        } else {
            drop(rx);
        }
        let handle = take.handle.clone();
        let stop = Arc::clone(&take.stop);
        let done = Arc::clone(&take.done);
        std::thread::spawn(move || {
            priority::raise_thread();
            while stop.lock().is_none() && !handle.limit_reached() {
                let chunk = handle.drain();
                if chunk.is_empty() {
                    handle.wait_for_audio(Duration::from_millis(8));
                } else {
                    let _ = tx.send(LiveAudio::Chunk(chunk));
                }
            }
            let rest = handle.stop();
            if !rest.is_empty() {
                let _ = tx.send(LiveAudio::Chunk(rest));
            }
            let stop = stop.lock().unwrap_or_else(|| StopTimes {
                packet_age_ms: handle.packet_age_ms(),
                ..StopTimes::now()
            });
            done.store(true, Ordering::SeqCst);
            let _ = tx.send(LiveAudio::Finish(stop));
        });
        take
    }

    fn stop(&self, asked: Instant) {
        let mut stop = self.stop.lock();
        if stop.is_none() {
            let dispatched = Instant::now();
            self.handle.close();
            *stop = Some(StopTimes {
                asked,
                dispatched,
                closed: Instant::now(),
                tail_audio_ms: 0,
                packet_age_ms: self.handle.packet_age_ms(),
            });
        }
    }

    fn discard(self) {
        self.discard.store(true, Ordering::SeqCst);
        self.stop(Instant::now());
    }

    fn is_held(&self) -> bool {
        self.stop.lock().is_none() && !self.done.load(Ordering::SeqCst)
    }

    /// The pump has sent the last audio.
    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// The take reached the take limit. The pump stops there.
    fn at_limit(&self) -> bool {
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
    PostDelivery {
        clipboard_failed: bool,
    },
    TakeError(TakeError),
    NoSpeech,
    MicrophoneReady,
}

/// Where the dictation is. The view shows it; only `Dictation` changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Capture accepts microphone audio.
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
    Press(u64),
    HoldReleased(Instant),
    Toggled(Instant),
}

/// The dictation of TDT: the microphone, the take, and the state between
/// the shortcut and the text. The UI loop gives it the input and shows the
/// changes it returns.
pub struct Dictation {
    phase: Phase,
    take_id: u64,
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
            take_id: 0,
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
                if cfg!(target_os = "macos") {
                    format!("Could not open the microphone. Check macOS Microphone permission and try again. {error}")
                } else {
                    format!("Could not open the microphone. Check Windows microphone access and try again. {error}")
                },
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
        // Drain any completed take before handling new input so a quick second
        // shortcut press is never dropped behind an already-finished take.
        self.take_results(&mut changes);
        if clicked && !self.is_busy() {
            input.push(Input::Press(typed_generation()));
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
                if let Some(take) = &self.take {
                    take.typing_generation
                        .store(typed_generation(), Ordering::SeqCst);
                }
            }
            self.limit_notice = at_limit;
            stop_now = true;
        }
        if !self.frozen {
            for input in input {
                match input {
                    Input::Press(generation) => self.press(generation, &mut changes, &mut stop_now),
                    Input::HoldReleased(asked) => self.release(asked, true, &mut stop_now),
                    Input::Toggled(asked) => self.release(asked, false, &mut stop_now),
                }
            }
        }
        if self.recording() && self.take.as_ref().is_some_and(LiveTake::is_done) {
            if self.take.as_ref().is_some_and(LiveTake::at_limit) {
                self.limit_notice = true;
            }
            stop_now = true;
        }
        if stop_now && self.recording() {
            if let Some(take) = &self.take {
                take.stop(Instant::now());
            }
            self.phase = Phase::Transcribing;
            play_sound(SoundEffect::StopListening);
            changes.push(ViewChange::Transcribing {
                at_limit: self.limit_notice,
            });
        }
        self.take_results(&mut changes);
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
            let generation = self.take.take().map_or_else(typed_generation, |take| {
                let generation = take.typing_generation.load(Ordering::SeqCst);
                take.discard();
                generation
            });
            self.phase = Phase::Idle;
            self.restart = Restart::Pending;
            input.insert(0, Input::Press(generation));
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

    fn press(&mut self, generation: u64, changes: &mut Vec<ViewChange>, stop_now: &mut bool) {
        // Snapshot the before-press state; it is cleared on every release,
        // so a swallowed press can never stop a take it did not start.
        self.recording_before_press = self.recording();
        if self.recording() && !*stop_now {
            if let Some(take) = &self.take {
                take.typing_generation.store(generation, Ordering::SeqCst);
            }
            self.stop_on_shortcut(Instant::now());
            *stop_now = true;
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
        self.take_id = self.take_id.wrapping_add(1);
        let take_id = self.take_id;
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
        self.take = Some(LiveTake::start(
            handle,
            self.engine.get(),
            generation,
            move |result, generation| {
                match result {
                    Ok(result) => {
                        let target = *target.lock();
                        let delivery = DeliveryTarget {
                            window: target,
                            generation,
                            take_id,
                        };
                        deliver_transcript(result, injector, events, &settings, delivery);
                    }
                    // The engine's errors are written for the user.
                    Err(error) => {
                        let _ = events.send(InternalEvent::TranscribeError(error));
                    }
                }
            },
        ));
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
            self.stop_on_shortcut(asked);
            *stop_now = true;
        }
    }

    fn stop_on_shortcut(&self, asked: Instant) {
        if let Some(vk) = input_held(current_binding().vk) {
            note_user_typed(vk);
        }
        if let Some(take) = &self.take {
            take.stop(asked);
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
                InternalEvent::PostDelivery {
                    take_id,
                    clipboard_failed,
                } => {
                    changes.push(ViewChange::PostDelivery {
                        clipboard_failed: clipboard_failed
                            && self.take_id == take_id
                            && !self.is_busy(),
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
            if let Some(take) = &self.take {
                take.stop(Instant::now());
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
enum InternalEvent {
    TranscribeSuccess {
        text: String,
        auto_pasted: bool,
        notice: Option<String>,
        latency_ms: u64,
    },
    PostDelivery {
        take_id: u64,
        clipboard_failed: bool,
    },
    TranscribeError(TakeError),
    NoSpeech,
}

struct DeliveryTarget {
    window: Option<isize>,
    generation: u64,
    take_id: u64,
}

enum CopyPlan {
    Typed(Latency),
    Copy,
    Fallback(NotTyped),
}

struct PostDeliveryJob {
    injector: Arc<PasteInjector>,
    plan: CopyPlan,
    target: DeliveryTarget,
    typed_key: Option<u16>,
    steps: DeliverySteps,
    result: LiveTranscript,
    events: crossbeam_channel::Sender<InternalEvent>,
}

fn enqueue_post_delivery(job: PostDeliveryJob) {
    static SUBMIT: Mutex<()> = Mutex::new(());
    static TX: std::sync::OnceLock<crossbeam_channel::Sender<PostDeliveryJob>> =
        std::sync::OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<PostDeliveryJob>();
        std::thread::spawn(move || {
            for job in rx {
                job.finish();
            }
        });
        tx
    });

    // Keep the queue order even if the UI starts the next take as soon as it sees success.
    let _submit = SUBMIT.lock();
    if let CopyPlan::Typed(latency) = job.plan {
        let _ = job.events.send(InternalEvent::TranscribeSuccess {
            text: job.result.text.clone(),
            auto_pasted: true,
            notice: None,
            latency_ms: latency.total_ms(),
        });
    }
    let _ = tx.send(job);
}

impl PostDeliveryJob {
    fn finish(mut self) {
        let text = &self.result.text;
        let clipboard_text = match &self.plan {
            CopyPlan::Fallback(_) => text.trim(),
            _ => text,
        };
        let copied = self.injector.copy_timed(clipboard_text, &mut self.steps);
        let latency = match &self.plan {
            CopyPlan::Typed(latency) => *latency,
            _ => self.result.latency(Instant::now()),
        };
        let delivered = match self.plan {
            CopyPlan::Typed(_) => Ok(match copied {
                Ok(()) => DeliveryOutcome::Pasted,
                Err(error) => DeliveryOutcome::PastedNotCopied(error),
            }),
            CopyPlan::Copy => copied.map(|()| DeliveryOutcome::Copied),
            CopyPlan::Fallback(reason) => copied.map(|()| DeliveryOutcome::CopiedFallback(reason)),
        };
        log_delivery(
            &delivered,
            self.target.window,
            self.typed_key,
            latency,
            &self.result,
            self.steps,
        );
        AppStats::update(|stats| stats.record(text, self.result.duration_secs, latency.total_ms()));
        let event = match delivered {
            Ok(DeliveryOutcome::Pasted | DeliveryOutcome::PastedNotCopied(_)) => {
                InternalEvent::PostDelivery {
                    take_id: self.target.take_id,
                    clipboard_failed: matches!(delivered, Ok(DeliveryOutcome::PastedNotCopied(_))),
                }
            }
            Ok(outcome) => InternalEvent::TranscribeSuccess {
                text: self.result.text,
                auto_pasted: false,
                notice: match outcome {
                    DeliveryOutcome::CopiedFallback(reason) => Some(reason.notice().to_string()),
                    _ => None,
                },
                latency_ms: latency.total_ms(),
            },
            Err(error) => InternalEvent::TranscribeError(
                format!("Could not copy text. Open History to copy the saved transcript. {error}")
                    .into(),
            ),
        };
        let _ = self.events.send(event);
    }
}

fn deliver_transcript(
    mut result: LiveTranscript,
    injector: Arc<PasteInjector>,
    tx: crossbeam_channel::Sender<InternalEvent>,
    settings: &Settings,
    target: DeliveryTarget,
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
    if result.text.trim().is_empty() {
        let _ = tx.send(InternalEvent::NoSpeech);
        return;
    }

    let mut steps = DeliverySteps::default();
    let user_typed = || user_typed_since(target.generation);
    let plan = if user_typed().is_some() {
        CopyPlan::Fallback(NotTyped::UserTyped)
    } else if should_paste {
        match injector.try_type(
            &result.text,
            target.window,
            &|| user_typed().is_some(),
            &mut steps,
        ) {
            Ok(()) => CopyPlan::Typed(result.latency(steps.typed_at.unwrap_or_else(Instant::now))),
            Err(reason) => CopyPlan::Fallback(reason),
        }
    } else {
        CopyPlan::Copy
    };
    let typed_key = user_typed();
    result.audio = Vec::new();
    enqueue_post_delivery(PostDeliveryJob {
        injector,
        plan,
        target,
        typed_key,
        steps,
        result,
        events: tx,
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
    let diag = take.diagnostics;
    let show_diag = latency.model >= Duration::from_millis(150)
        || latency.last_word >= Duration::from_millis(100)
        || diag.wait_prepare_ms > 0
        || diag.quiet_wait_ms > 0
        || diag.whole_fallback
        || diag.boost_retry
        || diag.page_faults.is_some_and(|faults| faults >= 500);
    if show_diag {
        log::note(&format!(
            "Delivery: {result}. App: {app}. Latency: {latency}. Take {:.1} s, last part {:.1} s, {} characters ({steps}). Details: {diag}.",
            take.duration_secs,
            take.last_part_secs,
            take.text.chars().count()
        ));
    } else {
        log::note(&format!(
            "Delivery: {result}. App: {app}. Latency: {latency}. Take {:.1} s, last part {:.1} s, {} characters ({steps}).",
            take.duration_secs,
            take.last_part_secs,
            take.text.chars().count()
        ));
    }
}
