use super::envelope::{LiveWave, SpeechEnvelope};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use voice_stt_desktop::resample::Resampler;
use voice_stt_desktop::stt::MAX_TAKE_SAMPLES;

/// A live WASAPI stream delivers buffers even in silence. Longer than this
/// with no callback means the endpoint went away.
const STALE_CALLBACK_MS: u64 = 1_500;
/// First buffer should arrive quickly. Longer means the open did not stick.
const STALE_OPEN_MS: u64 = 800;
/// During a take, this long with no buffer means the stream stopped. The
/// limit is long because a Bluetooth microphone can start slowly.
const STALLED_TAKE_MS: u64 = 3_000;
/// The time between two reads of the Windows default microphone.
const DEFAULT_INPUT_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// The Windows default microphone, as the watch thread last read it. The
/// outer `None` means no read yet. The read is a COM call that can wait on
/// the Windows audio service, so a press of the shortcut does not make it.
static DEFAULT_INPUT: Mutex<Option<Option<String>>> = Mutex::new(None);

/// Starts the thread that keeps `DEFAULT_INPUT` current. A microphone that
/// becomes the default is used from the next take, 2 seconds later at most.
pub fn watch_default_input() {
    std::thread::spawn(|| loop {
        let name = default_input_name(&cpal::default_host());
        *DEFAULT_INPUT.lock() = Some(name);
        std::thread::sleep(DEFAULT_INPUT_POLL);
    });
}

/// The state that the capture callback, the recorder, and the pump of a
/// take share.
struct Capture {
    is_recording: AtomicBool,
    at_limit: AtomicBool,
    /// Captured 16 kHz samples that the pump has not taken yet.
    buffer: Mutex<Vec<f32>>,
    samples_ready: Condvar,
    /// The samples of this take so far, also those the pump took.
    captured: AtomicUsize,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    /// Set by `start`. The capture callback then drops the resampler state of
    /// the previous take.
    new_take: AtomicBool,
    last_callback_ms: AtomicU64,
    last_error: Mutex<Option<String>>,
}

pub struct AudioRecorder {
    capture: Arc<Capture>,
    device_name: String,
    /// The Windows default input when this stream opened.
    default_name: Option<String>,
    /// Device rate, channels, and sample format, for the log.
    format: String,
    opened_ms: u64,
    paused: AtomicBool,
    stream: cpal::Stream,
}

/// The view of the capture that the pump thread of a take uses.
#[derive(Clone)]
pub struct CaptureHandle {
    capture: Arc<Capture>,
    /// Set by `wake`: `wait_for_audio` returns with no audio.
    woken: Arc<AtomicBool>,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let default_name = default_input_name(&host);
        let names = ordered_input_names(&host, default_name.as_deref())?;
        let mut last_error = "No microphone found. Connect a microphone and try again.".to_string();
        for name in names {
            match open_named(&host, &name, default_name.clone()) {
                Ok(recorder) => {
                    // This read is newer than the last one of the watch thread.
                    *DEFAULT_INPUT.lock() = Some(default_name);
                    return Ok(recorder);
                }
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }
}

/// Device audio to 16 kHz mono. The capture callback owns it, so the
/// resampler keeps its state from one packet to the next.
struct Converter {
    channels: usize,
    sample_rate: u32,
    resampler: Option<Resampler>,
    mono: Vec<f32>,
    output: Vec<f32>,
}

impl Converter {
    fn new(channels: usize, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
            resampler: Resampler::new(sample_rate),
            mono: Vec::new(),
            output: Vec::new(),
        }
    }
}

fn open_named(
    host: &cpal::Host,
    device_name: &str,
    default_name: Option<String>,
) -> Result<AudioRecorder, String> {
    let device = find_input(host, device_name)
        .ok_or_else(|| format!("Microphone {device_name} disappeared"))?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| format!("{device_name}: {error}"))?;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();

    let capture = Arc::new(Capture {
        is_recording: AtomicBool::new(false),
        at_limit: AtomicBool::new(false),
        buffer: Mutex::new(Vec::new()),
        samples_ready: Condvar::new(),
        captured: AtomicUsize::new(0),
        envelope: Arc::new(Mutex::new(SpeechEnvelope::default())),
        new_take: AtomicBool::new(false),
        last_callback_ms: AtomicU64::new(0),
        last_error: Mutex::new(None),
    });

    let input_sample_rate = config.sample_rate.0;
    let channels = config.channels.max(1) as usize;
    let on_error = {
        let capture = Arc::clone(&capture);
        move |err: cpal::StreamError| {
            voice_stt_desktop::log::note(&format!("Microphone stream error: {err}"));
            *capture.last_error.lock() = Some(format!(
                "Microphone disconnected. Check the input device and try again. ({err})"
            ));
            capture.is_recording.store(false, Ordering::SeqCst);
            *capture.envelope.lock() = SpeechEnvelope::default();
        }
    };

    let stream = build_stream(
        &device,
        &config,
        sample_format,
        Converter::new(channels, input_sample_rate),
        Arc::clone(&capture),
        on_error,
    )?;

    stream
        .play()
        .map_err(|e| format!("Failed to start input stream: {e}"))?;

    let format = format!("{input_sample_rate} Hz, {channels} ch, {sample_format}");

    Ok(AudioRecorder {
        capture,
        device_name: device_name.to_string(),
        default_name,
        format,
        opened_ms: now_ms(),
        paused: AtomicBool::new(false),
        stream,
    })
}

impl AudioRecorder {
    /// The device and its format, and the Windows default when it is another device.
    pub fn describe(&self) -> String {
        let mut text = format!("{} ({})", self.device_name, self.format);
        match &self.default_name {
            Some(default) if *default != self.device_name => {
                text.push_str(&format!(
                    "; the Windows default {default} could not be used"
                ));
            }
            None => text.push_str("; Windows has no default microphone"),
            Some(_) => {}
        }
        text
    }

    /// Why the next take must open the microphone again, or `None` when this
    /// stream is still the right one. Compares with the Windows default that
    /// the watch thread read, so a USB microphone that is plugged in,
    /// unplugged, or chosen in Settings is used from the next take.
    pub fn reopen_reason(&self) -> Option<String> {
        if let Some(error) = self.take_error() {
            return Some(error);
        }
        if self.needs_reopen() {
            return Some("the stream stopped".to_string());
        }
        let default_name = DEFAULT_INPUT.lock().clone()?;
        if default_name != self.default_name {
            return Some(format!(
                "the Windows default changed to {}",
                default_name.as_deref().unwrap_or("none")
            ));
        }
        None
    }

    /// Park the input stream while idle so the microphone (and its Windows
    /// privacy indicator) is only hot around actual recordings.
    pub fn pause_input(&self) {
        if self.paused.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.stream.pause();
    }

    /// Resume a parked stream. Refreshes the callback clock so a long idle
    /// does not look like a dead device. An error means that the stream is
    /// gone: open the microphone again.
    pub fn resume_input(&self) -> Result<(), String> {
        if !self.paused.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        note_callback(&self.capture.last_callback_ms);
        self.stream
            .play()
            .map_err(|error| format!("the stream did not start again: {error}"))
    }

    /// True when a take records and the stream gave no buffer for a long
    /// time. A live stream gives buffers in silence too.
    pub fn stalled(&self) -> bool {
        if self.paused.load(Ordering::SeqCst) {
            return false;
        }
        let last = self.last_callback().unwrap_or(self.opened_ms);
        now_ms().saturating_sub(last) > STALLED_TAKE_MS
    }

    /// The samples of this take so far.
    pub fn captured_samples(&self) -> usize {
        self.capture.captured.load(Ordering::SeqCst)
    }

    /// True when the capture callback has stopped. The next recording should
    /// open the microphone again instead of writing into a dead stream.
    /// A deliberately parked stream is healthy, not stale.
    fn needs_reopen(&self) -> bool {
        if self.paused.load(Ordering::SeqCst) {
            return false;
        }
        let since = |at: u64| now_ms().saturating_sub(at);
        match self.last_callback() {
            Some(last) => since(last) > STALE_CALLBACK_MS,
            None => since(self.opened_ms) > STALE_OPEN_MS,
        }
    }

    /// When the capture callback last ran; `None` before its first run.
    fn last_callback(&self) -> Option<u64> {
        Some(self.capture.last_callback_ms.load(Ordering::Relaxed)).filter(|&ms| ms != 0)
    }

    pub fn start(&self) {
        {
            let mut recorded = self.capture.buffer.lock();
            recorded.clear();
            self.capture.captured.store(0, Ordering::SeqCst);
        }
        *self.capture.envelope.lock() = SpeechEnvelope::default();
        // A pending stream error stays: when the resume just before this
        // failed, the UI loop must see it and open the microphone again.
        self.capture.at_limit.store(false, Ordering::SeqCst);
        self.capture.new_take.store(true, Ordering::SeqCst);
        self.capture.is_recording.store(true, Ordering::SeqCst);
    }

    /// Safe to drain from a non-UI thread. The cpal stream stays inside
    /// AudioRecorder.
    pub fn handle(&self) -> CaptureHandle {
        CaptureHandle {
            capture: Arc::clone(&self.capture),
            woken: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn live_wave(&self) -> LiveWave {
        LiveWave(Arc::clone(&self.capture.envelope))
    }

    pub fn take_error(&self) -> Option<String> {
        self.capture.last_error.lock().take()
    }
}

impl CaptureHandle {
    /// Takes the samples so far. `captured` keeps counting the whole take, so
    /// the 2-minute limit still fires while the pump drains. The copy is made
    /// here and `buffer` keeps its memory: after `mem::take`, the next capture
    /// callback would have to allocate.
    pub fn drain(&self) -> Vec<f32> {
        let mut recorded = self.capture.buffer.lock();
        if !self.capture.is_recording.load(Ordering::SeqCst) {
            return Vec::new();
        }
        let samples = recorded.clone();
        recorded.clear();
        samples
    }

    /// Ends the recording and takes the rest of the buffer.
    pub fn stop(&self) -> Vec<f32> {
        self.capture.is_recording.store(false, Ordering::SeqCst);
        *self.capture.envelope.lock() = SpeechEnvelope::default();
        self.capture.captured.store(0, Ordering::SeqCst);
        std::mem::take(&mut *self.capture.buffer.lock())
    }

    pub fn limit_reached(&self) -> bool {
        self.capture.at_limit.load(Ordering::Relaxed)
    }

    /// Ends a `wait_for_audio` now. The release of the shortcut and the stop
    /// of a take must not wait for the next buffer or for the timeout.
    pub fn wake(&self) {
        let _guard = self.capture.buffer.lock();
        self.woken.store(true, Ordering::SeqCst);
        self.capture.samples_ready.notify_all();
    }

    /// Block until new samples arrive, `wake` is called, or `timeout`. Must
    /// not miss a wakeup: the condvar waits on the same mutex as the sample
    /// buffer, and `wake` sets its flag under that mutex.
    pub fn wait_for_audio(&self, timeout: std::time::Duration) {
        let mut guard = self.capture.buffer.lock();
        let start = std::time::Instant::now();
        while guard.is_empty() && !self.woken.swap(false, Ordering::SeqCst) {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            self.capture.samples_ready.wait_for(&mut guard, remaining);
        }
    }
}

/// Device samples go straight into `converter.mono`, so a packet of any
/// sample format needs no buffer of its own.
fn process_samples<T: Copy>(
    data: &[T],
    to_f32: impl Fn(T) -> f32,
    converter: &mut Converter,
    capture: &Capture,
) {
    let channels = converter.channels;
    converter.mono.clear();
    converter
        .mono
        .extend(data.chunks_exact(channels).map(|frame| {
            frame.iter().map(|sample| to_f32(*sample)).sum::<f32>() / channels as f32
        }));
    if converter.mono.is_empty() {
        return;
    }
    capture
        .envelope
        .lock()
        .push(&converter.mono, converter.sample_rate);

    let resampled = match converter.resampler.as_mut() {
        Some(resampler) => {
            converter.output.clear();
            resampler.process(&converter.mono, &mut converter.output);
            &converter.output
        }
        None => &converter.mono,
    };

    let mut recorded = capture.buffer.lock();
    if !capture.is_recording.load(Ordering::SeqCst) {
        return;
    }
    let already = capture.captured.load(Ordering::SeqCst);
    let take = MAX_TAKE_SAMPLES
        .saturating_sub(already)
        .min(resampled.len());
    if take == 0 {
        if already >= MAX_TAKE_SAMPLES {
            capture.at_limit.store(true, Ordering::SeqCst);
        }
        return;
    }
    recorded.extend_from_slice(&resampled[..take]);
    let stored = capture.captured.fetch_add(take, Ordering::SeqCst) + take;
    if stored >= MAX_TAKE_SAMPLES {
        capture.at_limit.store(true, Ordering::SeqCst);
    }
    capture.samples_ready.notify_one();
}

fn default_input_name(host: &cpal::Host) -> Option<String> {
    host.default_input_device()
        .and_then(|device| device.name().ok())
}

fn ordered_input_names(
    host: &cpal::Host,
    default_name: Option<&str>,
) -> Result<Vec<String>, String> {
    let available = host
        .input_devices()
        .map_err(|error| format!("Could not list microphones: {error}"))?
        .filter_map(|device| device.name().ok())
        .collect::<Vec<_>>();
    let names = preferred_input_names(default_name, &available);
    if names.is_empty() {
        Err("No microphone found. Connect a microphone and try again.".to_string())
    } else {
        Ok(names)
    }
}

fn find_input(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    host.input_devices()
        .ok()?
        .find(|device| device.name().is_ok_and(|device_name| device_name == name))
}

/// Default endpoint first, then other real inputs. Loopback endpoints are omitted.
fn preferred_input_names(default_name: Option<&str>, available: &[String]) -> Vec<String> {
    let mut ranked = Vec::new();
    for (index, name) in available.iter().enumerate() {
        if let Some(rank) = input_priority(name) {
            ranked.push((rank, index, name.as_str()));
        }
    }
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    let mut names: Vec<String> = ranked
        .into_iter()
        .map(|(_, _, name)| name.to_string())
        .collect();
    if let Some(default_name) = default_name {
        if let Some(position) = names.iter().position(|name| name == default_name) {
            let chosen = names.remove(position);
            names.insert(0, chosen);
        }
    }
    names
}

fn input_priority(name: &str) -> Option<u8> {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("stereo mix")
        || normalized.contains("what u hear")
        || normalized.contains("loopback")
        || normalized.contains("wave out")
        || normalized.contains("cable output")
    {
        return None;
    }
    if normalized.contains("mic") || normalized.contains("headset") {
        Some(2)
    } else {
        Some(1)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn note_callback(last_callback_ms: &AtomicU64) {
    last_callback_ms.store(now_ms().max(1), Ordering::Relaxed);
}

fn ingest<T: Copy>(
    samples: &[T],
    to_f32: impl Fn(T) -> f32,
    converter: &mut Converter,
    capture: &Capture,
) {
    note_callback(&capture.last_callback_ms);
    if capture.is_recording.load(Ordering::Relaxed) {
        if capture.new_take.swap(false, Ordering::SeqCst) {
            if let Some(resampler) = converter.resampler.as_mut() {
                resampler.reset();
            }
        }
        process_samples(samples, to_f32, converter, capture);
    } else {
        *capture.envelope.lock() = SpeechEnvelope::default();
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    mut converter: Converter,
    capture: Arc<Capture>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String> {
    let unsupported = || format!("Unsupported audio sample format: {sample_format}");
    match sample_format {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    ingest(data, |sample| sample, &mut converter, &capture);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        cpal::SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let to_f32 = |sample: i16| sample as f32 / i16::MAX as f32;
                    ingest(data, to_f32, &mut converter, &capture);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        cpal::SampleFormat::I32 => device
            .build_input_stream(
                config,
                move |data: &[i32], _| {
                    let to_f32 = |sample: i32| sample as f32 / i32::MAX as f32;
                    ingest(data, to_f32, &mut converter, &capture);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        cpal::SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    let to_f32 = |sample: u16| (sample as f32 / u16::MAX as f32) * 2.0 - 1.0;
                    ingest(data, to_f32, &mut converter, &capture);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        _ => Err(unsupported()),
    }
}
