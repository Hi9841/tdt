//! One take: the pump thread that moves the captured audio to the engine,
//! the engine thread that makes the text, and the end of the take.

use crate::audio::recorder::CaptureHandle;
use crate::audio::{SpeechTail, MAX_TAIL};
use crate::hotkey::{current_binding, input_appeared, input_held, input_mask, note_user_typed};
use crate::priority;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use voice_stt_desktop::stt::{LiveAudio, LiveTranscript, StopTimes, SttEngine};

/// A take from its start until its pump has sent the last audio.
pub struct LiveTake {
    tail: Arc<Mutex<SpeechTail>>,
    release: Arc<AtomicBool>,
    halt: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    hit_limit: Arc<AtomicBool>,
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
        on_result: impl FnOnce(Result<LiveTranscript, String>) + Send + 'static,
    ) -> Self {
        let take = Self {
            tail: Arc::new(Mutex::new(SpeechTail::default())),
            release: Arc::new(AtomicBool::new(false)),
            halt: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
            hit_limit: Arc::new(AtomicBool::new(false)),
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
        let hit_limit = Arc::clone(&take.hit_limit);
        std::thread::spawn(move || {
            priority::raise_thread();
            loop {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                if handle.limit_reached() {
                    hit_limit.store(true, Ordering::SeqCst);
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

    /// The pump stopped at the take limit.
    pub fn hit_limit(&self) -> bool {
        self.hit_limit.load(Ordering::SeqCst)
    }
}
