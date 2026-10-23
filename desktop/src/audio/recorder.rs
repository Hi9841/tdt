use super::envelope::{LiveWave, SpeechEnvelope};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use voice_stt_desktop::resample::{Resampler, TARGET_SAMPLE_RATE};

pub const MAX_RECORDING_SECONDS: usize = 120;
/// A live WASAPI stream delivers buffers even in silence. Longer than this
/// with no callback means the endpoint went away.
const STALE_CALLBACK_MS: u64 = 1_500;
/// First buffer should arrive quickly. Longer means the open did not stick.
const STALE_OPEN_MS: u64 = 800;

pub struct AudioRecorder {
    is_recording: Arc<AtomicBool>,
    at_limit: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
    buffer: Arc<Mutex<Vec<f32>>>,
    samples_ready: Arc<Condvar>,
    captured: Arc<AtomicUsize>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    /// Set by `start`. The capture callback then drops the resampler state of
    /// the previous take.
    new_take: Arc<AtomicBool>,
    device_name: String,
    /// The Windows default input when this stream opened.
    default_name: Option<String>,
    /// Device rate, channels, and sample format, for the log.
    format: String,
    opened_ms: u64,
    last_callback_ms: Arc<AtomicU64>,
    paused: AtomicBool,
    _stream: cpal::Stream,
}

#[derive(Clone)]
pub struct CaptureHandle {
    is_recording: Arc<AtomicBool>,
    at_limit: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<f32>>>,
    samples_ready: Arc<Condvar>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    captured: Arc<AtomicUsize>,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let default_name = default_input_name(&host);
        let names = ordered_input_names(&host, default_name.as_deref())?;
        let mut last_error = "No microphone found. Connect a microphone and try again.".to_string();
        for name in names {
            match open_named(&host, &name, default_name.clone()) {
                Ok(recorder) => return Ok(recorder),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }
}

#[derive(Clone)]
struct CaptureParts {
    is_recording: Arc<AtomicBool>,
    at_limit: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<f32>>>,
    samples_ready: Arc<Condvar>,
    captured: Arc<AtomicUsize>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    new_take: Arc<AtomicBool>,
    last_callback_ms: Arc<AtomicU64>,
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

    let last_error = Arc::new(Mutex::new(None));
    let parts = CaptureParts {
        is_recording: Arc::new(AtomicBool::new(false)),
        at_limit: Arc::new(AtomicBool::new(false)),
        buffer: Arc::new(Mutex::new(Vec::new())),
        samples_ready: Arc::new(Condvar::new()),
        captured: Arc::new(AtomicUsize::new(0)),
        envelope: Arc::new(Mutex::new(SpeechEnvelope::default())),
        new_take: Arc::new(AtomicBool::new(false)),
        last_callback_ms: Arc::new(AtomicU64::new(0)),
    };

    let input_sample_rate = config.sample_rate.0;
    let channels = config.channels.max(1) as usize;
    let error_cb = stream_error_callback(
        Arc::clone(&last_error),
        Arc::clone(&parts.is_recording),
        Arc::clone(&parts.envelope),
    );

    let stream = build_stream(
        &device,
        &config,
        sample_format,
        Converter::new(channels, input_sample_rate),
        parts.clone(),
        error_cb,
    )?;

    stream
        .play()
        .map_err(|e| format!("Failed to start input stream: {e}"))?;

    let format = format!("{input_sample_rate} Hz, {channels} ch, {sample_format}");
    eprintln!("Microphone open: {device_name} ({format})");

    Ok(AudioRecorder {
        is_recording: parts.is_recording,
        at_limit: parts.at_limit,
        last_error,
        buffer: parts.buffer,
        samples_ready: parts.samples_ready,
        captured: parts.captured,
        envelope: parts.envelope,
        new_take: parts.new_take,
        device_name: device_name.to_string(),
        default_name,
        format,
        opened_ms: now_ms(),
        last_callback_ms: parts.last_callback_ms,
        paused: AtomicBool::new(false),
        _stream: stream,
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
    /// stream is still the right one. Reads the Windows default each time, so
    /// a USB microphone that is plugged in, unplugged, or chosen in Settings
    /// is used from the next take.
    pub fn reopen_reason(&self) -> Option<String> {
        if let Some(error) = self.take_error() {
            return Some(error);
        }
        if self.needs_reopen() {
            return Some("the stream stopped".to_string());
        }
        let default_name = default_input_name(&cpal::default_host());
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
        let _ = self._stream.pause();
    }

    /// Resume a parked stream. Refreshes the callback clock so a long idle
    /// does not look like a dead device.
    pub fn resume_input(&self) {
        if !self.paused.swap(false, Ordering::SeqCst) {
            return;
        }
        let _ = self._stream.play();
        note_callback(&self.last_callback_ms);
    }

    /// True when the capture callback has stopped. The next recording should
    /// open the microphone again instead of writing into a dead stream.
    /// A deliberately parked stream is healthy, not stale.
    fn needs_reopen(&self) -> bool {
        if self.paused.load(Ordering::SeqCst) {
            return false;
        }
        stream_is_stale(
            self.opened_ms,
            nonzero(self.last_callback_ms.load(Ordering::Relaxed)),
            now_ms(),
        )
    }

    fn process_samples_f32(data: &[f32], converter: &mut Converter, parts: &CaptureParts) {
        let channels = converter.channels;
        converter.mono.clear();
        converter.mono.extend(
            data.chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        if converter.mono.is_empty() {
            return;
        }
        parts
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

        let mut recorded = parts.buffer.lock();
        if !parts.is_recording.load(Ordering::SeqCst) {
            return;
        }
        let capacity = max_recording_samples();
        let already = parts.captured.load(Ordering::SeqCst);
        let take = take_count(already, resampled.len(), capacity);
        if take == 0 {
            if already >= capacity {
                parts.at_limit.store(true, Ordering::SeqCst);
            }
            return;
        }
        recorded.extend_from_slice(&resampled[..take]);
        let stored = parts.captured.fetch_add(take, Ordering::SeqCst) + take;
        if stored >= capacity {
            parts.at_limit.store(true, Ordering::SeqCst);
        }
        parts.samples_ready.notify_one();
    }

    pub fn start(&self) {
        {
            let mut recorded = self.buffer.lock();
            recorded.clear();
            self.captured.store(0, Ordering::SeqCst);
        }
        *self.envelope.lock() = SpeechEnvelope::default();
        // A pending stream error stays: when the resume just before this
        // failed, the UI loop must see it and open the microphone again.
        self.at_limit.store(false, Ordering::SeqCst);
        self.new_take.store(true, Ordering::SeqCst);
        self.is_recording.store(true, Ordering::SeqCst);
    }

    /// Clone of the capture state. Safe to drain from a non-UI thread.
    /// The cpal stream stays inside AudioRecorder.
    pub fn handle(&self) -> CaptureHandle {
        CaptureHandle {
            is_recording: Arc::clone(&self.is_recording),
            at_limit: Arc::clone(&self.at_limit),
            buffer: Arc::clone(&self.buffer),
            samples_ready: Arc::clone(&self.samples_ready),
            envelope: Arc::clone(&self.envelope),
            captured: Arc::clone(&self.captured),
        }
    }

    pub fn live_wave(&self) -> LiveWave {
        LiveWave(Arc::clone(&self.envelope))
    }

    pub fn take_error(&self) -> Option<String> {
        take_pending_error(&self.last_error)
    }

    pub fn limit_reached(&self) -> bool {
        self.at_limit.load(Ordering::Relaxed)
    }
}

impl CaptureHandle {
    pub fn drain(&self) -> Vec<f32> {
        drain_samples(&self.is_recording, &self.buffer)
    }

    /// Same end state as AudioRecorder::stop: is_recording false,
    /// envelope reset, buffer taken.
    pub fn stop(&self) -> Vec<f32> {
        stop_samples(
            &self.is_recording,
            &self.envelope,
            &self.buffer,
            &self.captured,
        )
    }

    pub fn limit_reached(&self) -> bool {
        self.at_limit.load(Ordering::Relaxed)
    }

    /// Block until new samples arrive or `timeout`. Must not miss a wakeup:
    /// the condvar waits on the same mutex as the sample buffer.
    pub fn wait_for_audio(&self, timeout: std::time::Duration) {
        let mut guard = self.buffer.lock();
        let start = std::time::Instant::now();
        while guard.is_empty() {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            self.samples_ready.wait_for(&mut guard, remaining);
        }
    }
}

/// Takes the samples so far. `captured` keeps counting the whole take, so
/// the 2-minute limit still fires while the pump drains.
fn drain_samples(is_recording: &AtomicBool, buffer: &Mutex<Vec<f32>>) -> Vec<f32> {
    let mut recorded = buffer.lock();
    if !is_recording.load(Ordering::SeqCst) {
        return Vec::new();
    }
    std::mem::take(&mut *recorded)
}

fn stop_samples(
    is_recording: &AtomicBool,
    envelope: &Mutex<SpeechEnvelope>,
    buffer: &Mutex<Vec<f32>>,
    captured: &AtomicUsize,
) -> Vec<f32> {
    is_recording.store(false, Ordering::SeqCst);
    *envelope.lock() = SpeechEnvelope::default();
    captured.store(0, Ordering::SeqCst);
    std::mem::take(&mut *buffer.lock())
}

fn max_recording_samples() -> usize {
    MAX_RECORDING_SECONDS * TARGET_SAMPLE_RATE as usize
}

fn take_count(already: usize, incoming: usize, capacity: usize) -> usize {
    capacity.saturating_sub(already).min(incoming)
}

fn stream_error_message(err: impl std::fmt::Display) -> String {
    format!("Microphone disconnected. Check the input device and try again. ({err})")
}

fn apply_stream_error(
    last_error: &Mutex<Option<String>>,
    is_recording: &AtomicBool,
    err: impl std::fmt::Display,
) {
    *last_error.lock() = Some(stream_error_message(err));
    is_recording.store(false, Ordering::SeqCst);
}

fn take_pending_error(last_error: &Mutex<Option<String>>) -> Option<String> {
    last_error.lock().take()
}

fn stream_error_callback(
    last_error: Arc<Mutex<Option<String>>>,
    is_recording: Arc<AtomicBool>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        eprintln!("Audio stream error: {err}");
        apply_stream_error(&last_error, &is_recording, err);
        *envelope.lock() = SpeechEnvelope::default();
    }
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
pub fn preferred_input_names(default_name: Option<&str>, available: &[String]) -> Vec<String> {
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

fn stream_is_stale(opened_ms: u64, last_callback_ms: Option<u64>, now_ms: u64) -> bool {
    match last_callback_ms {
        Some(last) => now_ms.saturating_sub(last) > STALE_CALLBACK_MS,
        None => now_ms.saturating_sub(opened_ms) > STALE_OPEN_MS,
    }
}

fn nonzero(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn note_callback(last_callback_ms: &AtomicU64) {
    last_callback_ms.store(now_ms().max(1), Ordering::Relaxed);
}

fn ingest(samples: &[f32], converter: &mut Converter, parts: &CaptureParts) {
    note_callback(&parts.last_callback_ms);
    if parts.is_recording.load(Ordering::Relaxed) {
        if parts.new_take.swap(false, Ordering::SeqCst) {
            if let Some(resampler) = converter.resampler.as_mut() {
                resampler.reset();
            }
        }
        AudioRecorder::process_samples_f32(samples, converter, parts);
    } else {
        *parts.envelope.lock() = SpeechEnvelope::default();
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    mut converter: Converter,
    parts: CaptureParts,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String> {
    let unsupported = || format!("Unsupported audio sample format: {sample_format}");
    match sample_format {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    ingest(data, &mut converter, &parts);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        cpal::SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let samples = data
                        .iter()
                        .map(|sample| *sample as f32 / i16::MAX as f32)
                        .collect::<Vec<_>>();
                    ingest(&samples, &mut converter, &parts);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        cpal::SampleFormat::I32 => device
            .build_input_stream(
                config,
                move |data: &[i32], _| {
                    let samples = data
                        .iter()
                        .map(|sample| *sample as f32 / i32::MAX as f32)
                        .collect::<Vec<_>>();
                    ingest(&samples, &mut converter, &parts);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        cpal::SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    let samples = data
                        .iter()
                        .map(|sample| (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect::<Vec<_>>();
                    ingest(&samples, &mut converter, &parts);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        _ => Err(unsupported()),
    }
}
