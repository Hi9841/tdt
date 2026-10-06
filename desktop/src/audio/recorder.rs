use super::envelope::{SpeechEnvelope, VIS_BARS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TARGET_SAMPLE_RATE: u32 = 16000;
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
    #[allow(dead_code)]
    samples_ready: Arc<Condvar>,
    captured: Arc<AtomicUsize>,
    current_rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    device_name: String,
    opened_ms: u64,
    last_callback_ms: Arc<AtomicU64>,
    paused: AtomicBool,
    _stream: cpal::Stream,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct CaptureHandle {
    is_recording: Arc<AtomicBool>,
    at_limit: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<f32>>>,
    samples_ready: Arc<Condvar>,
    current_rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    captured: Arc<AtomicUsize>,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let names = ordered_input_names(&host)?;
        let mut last_error = "No microphone found. Connect a microphone and try again.".to_string();
        for name in names {
            match open_named(&host, &name) {
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
    rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    last_callback_ms: Arc<AtomicU64>,
}

fn open_named(host: &cpal::Host, device_name: &str) -> Result<AudioRecorder, String> {
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
        rms: Arc::new(Mutex::new(0.0f32)),
        envelope: Arc::new(Mutex::new(SpeechEnvelope::default())),
        last_callback_ms: Arc::new(AtomicU64::new(0)),
    };

    let input_sample_rate = config.sample_rate.0;
    let channels = config.channels.max(1) as usize;
    let error_cb = stream_error_callback(
        Arc::clone(&last_error),
        Arc::clone(&parts.is_recording),
        Arc::clone(&parts.rms),
        Arc::clone(&parts.envelope),
    );

    let stream = build_stream(
        &device,
        &config,
        sample_format,
        channels,
        input_sample_rate,
        parts.clone(),
        error_cb,
    )?;

    stream
        .play()
        .map_err(|e| format!("Failed to start input stream: {}", e))?;

    eprintln!("Microphone open: {device_name}");

    Ok(AudioRecorder {
        is_recording: parts.is_recording,
        at_limit: parts.at_limit,
        last_error,
        buffer: parts.buffer,
        samples_ready: parts.samples_ready,
        captured: parts.captured,
        current_rms: parts.rms,
        envelope: parts.envelope,
        device_name: device_name.to_string(),
        opened_ms: now_ms(),
        last_callback_ms: parts.last_callback_ms,
        paused: AtomicBool::new(false),
        _stream: stream,
    })
}

impl AudioRecorder {
    pub fn device_name(&self) -> &str {
        &self.device_name
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
    pub fn needs_reopen(&self) -> bool {
        if self.paused.load(Ordering::SeqCst) {
            return false;
        }
        stream_is_stale(
            self.opened_ms,
            nonzero(self.last_callback_ms.load(Ordering::Relaxed)),
            now_ms(),
        )
    }

    fn process_samples_f32(data: &[f32], channels: usize, sample_rate: u32, parts: &CaptureParts) {
        if data.is_empty() {
            return;
        }

        // Downmix to mono
        let mut mono: Vec<f32> = Vec::with_capacity(data.len() / channels);
        for chunk in data.chunks_exact(channels) {
            let sum: f32 = chunk.iter().sum();
            mono.push(sum / channels as f32);
        }

        // Calculate RMS audio level (0.0 to 1.0)
        if mono.is_empty() {
            return;
        }
        let sum_squares: f32 = mono.iter().map(|&s| s * s).sum();
        let level = (sum_squares / mono.len() as f32).sqrt().min(1.0);
        *parts.rms.lock() = level;
        parts.envelope.lock().push(&mono, sample_rate);

        // Resample to 16,000 Hz if necessary
        let resampled = if sample_rate != TARGET_SAMPLE_RATE && sample_rate > 0 {
            Self::resample_linear(&mono, sample_rate, TARGET_SAMPLE_RATE)
        } else {
            mono
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

    fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let ratio = from_rate as f64 / to_rate as f64;
        let out_len = ((input.len() as f64) / ratio).floor() as usize;
        let mut output = Vec::with_capacity(out_len);

        for i in 0..out_len {
            let src_idx = i as f64 * ratio;
            let idx_floor = src_idx.floor() as usize;
            let frac = (src_idx - idx_floor as f64) as f32;

            let s0 = input[idx_floor.min(input.len() - 1)];
            let s1 = input[(idx_floor + 1).min(input.len() - 1)];

            output.push(s0 + frac * (s1 - s0));
        }

        output
    }

    pub fn start(&self) {
        {
            let mut recorded = self.buffer.lock();
            recorded.clear();
            self.captured.store(0, Ordering::SeqCst);
        }
        *self.current_rms.lock() = 0.0;
        *self.envelope.lock() = SpeechEnvelope::default();
        clear_session_signals(&self.last_error, &self.at_limit);
        self.is_recording.store(true, Ordering::SeqCst);
    }

    /// Clone of the capture state. Safe to drain from a non-UI thread.
    /// The cpal stream stays inside AudioRecorder.
    #[allow(dead_code)]
    pub fn handle(&self) -> CaptureHandle {
        CaptureHandle {
            is_recording: Arc::clone(&self.is_recording),
            at_limit: Arc::clone(&self.at_limit),
            buffer: Arc::clone(&self.buffer),
            samples_ready: Arc::clone(&self.samples_ready),
            current_rms: Arc::clone(&self.current_rms),
            envelope: Arc::clone(&self.envelope),
            captured: Arc::clone(&self.captured),
        }
    }

    pub fn audio_level(&self) -> f32 {
        *self.current_rms.lock()
    }

    pub fn vis_peaks(&self) -> [f32; VIS_BARS] {
        self.envelope.lock().bars()
    }

    pub fn take_error(&self) -> Option<String> {
        take_pending_error(&self.last_error)
    }

    pub fn limit_reached(&self) -> bool {
        self.at_limit.load(Ordering::Relaxed)
    }
}

#[allow(dead_code)]
impl CaptureHandle {
    pub fn drain(&self) -> Vec<f32> {
        drain_samples(&self.is_recording, &self.buffer, &self.captured)
    }

    /// Same end state as AudioRecorder::stop: is_recording false, rms 0,
    /// envelope reset, buffer taken.
    pub fn stop(&self) -> Vec<f32> {
        stop_samples(
            &self.is_recording,
            &self.current_rms,
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

fn drain_samples(
    is_recording: &AtomicBool,
    buffer: &Mutex<Vec<f32>>,
    captured: &AtomicUsize,
) -> Vec<f32> {
    let mut recorded = buffer.lock();
    if !is_recording.load(Ordering::SeqCst) {
        return Vec::new();
    }
    captured.store(0, Ordering::SeqCst);
    std::mem::take(&mut *recorded)
}

fn stop_samples(
    is_recording: &AtomicBool,
    current_rms: &Mutex<f32>,
    envelope: &Mutex<SpeechEnvelope>,
    buffer: &Mutex<Vec<f32>>,
    captured: &AtomicUsize,
) -> Vec<f32> {
    is_recording.store(false, Ordering::SeqCst);
    *current_rms.lock() = 0.0;
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

fn clear_session_signals(last_error: &Mutex<Option<String>>, at_limit: &AtomicBool) {
    *last_error.lock() = None;
    at_limit.store(false, Ordering::SeqCst);
}

fn stream_error_callback(
    last_error: Arc<Mutex<Option<String>>>,
    is_recording: Arc<AtomicBool>,
    rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        eprintln!("Audio stream error: {}", err);
        apply_stream_error(&last_error, &is_recording, err);
        *rms.lock() = 0.0;
        *envelope.lock() = SpeechEnvelope::default();
    }
}

fn ordered_input_names(host: &cpal::Host) -> Result<Vec<String>, String> {
    let available = host
        .input_devices()
        .map_err(|error| format!("Could not list microphones: {error}"))?
        .filter_map(|device| device.name().ok())
        .collect::<Vec<_>>();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let names = preferred_input_names(default_name.as_deref(), &available);
    if names.is_empty() {
        Err("No microphone found. Connect a microphone and try again.".to_string())
    } else {
        Ok(names)
    }
}

fn find_input(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    host.input_devices().ok()?.find(|device| {
        device
            .name()
            .ok()
            .is_some_and(|device_name| device_name == name)
    })
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
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn note_callback(last_callback_ms: &AtomicU64) {
    last_callback_ms.store(now_ms().max(1), Ordering::Relaxed);
}

fn ingest(samples: &[f32], channels: usize, sample_rate: u32, parts: &CaptureParts) {
    note_callback(&parts.last_callback_ms);
    if parts.is_recording.load(Ordering::Relaxed) {
        AudioRecorder::process_samples_f32(samples, channels, sample_rate, parts);
    } else {
        *parts.rms.lock() = 0.0;
        *parts.envelope.lock() = SpeechEnvelope::default();
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    channels: usize,
    sample_rate: u32,
    parts: CaptureParts,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String> {
    let unsupported = || format!("Unsupported audio sample format: {sample_format}");
    match sample_format {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    ingest(data, channels, sample_rate, &parts);
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
                    ingest(&samples, channels, sample_rate, &parts);
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
                    ingest(&samples, channels, sample_rate, &parts);
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
                    ingest(&samples, channels, sample_rate, &parts);
                },
                on_error,
                None,
            )
            .map_err(|error| format!("Failed to build input stream: {error}")),
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_error_stops_recording_and_is_taken_once() {
        let last_error = Mutex::new(None);
        let is_recording = AtomicBool::new(true);

        apply_stream_error(&last_error, &is_recording, "device unplugged");
        assert!(!is_recording.load(Ordering::SeqCst));

        let message = take_pending_error(&last_error).expect("stream error");
        assert!(message.contains("Microphone disconnected"));
        assert!(message.contains("Check the input device and try again"));
        assert!(message.contains("device unplugged"));
        assert!(take_pending_error(&last_error).is_none());
    }

    #[test]
    fn start_reset_clears_pending_error_and_limit() {
        let last_error = Mutex::new(Some("stale error".into()));
        let at_limit = AtomicBool::new(true);

        clear_session_signals(&last_error, &at_limit);
        assert!(take_pending_error(&last_error).is_none());
        assert!(!at_limit.load(Ordering::SeqCst));
    }

    #[test]
    fn preferred_input_skips_loopback_and_keeps_the_default_mic_first() {
        let available = vec![
            "Stereo Mix (Realtek)".to_string(),
            "Microphone (fifine Microphone)".to_string(),
            "Microphone (AB13X USB Audio)".to_string(),
        ];
        assert_eq!(
            preferred_input_names(Some("Microphone (AB13X USB Audio)"), &available),
            vec![
                "Microphone (AB13X USB Audio)",
                "Microphone (fifine Microphone)",
            ]
        );
        assert_eq!(
            preferred_input_names(None, &available),
            vec![
                "Microphone (fifine Microphone)",
                "Microphone (AB13X USB Audio)",
            ]
        );
        assert!(preferred_input_names(
            Some("Stereo Mix (Realtek)"),
            &["Stereo Mix (Realtek)".into()]
        )
        .is_empty());
    }

    #[test]
    fn stale_stream_is_reopened_when_callbacks_stop() {
        assert!(!stream_is_stale(1_000, None, 1_500));
        assert!(stream_is_stale(1_000, None, 1_000 + STALE_OPEN_MS + 1));
        assert!(!stream_is_stale(
            1_000,
            Some(5_000),
            5_000 + STALE_CALLBACK_MS
        ));
        assert!(stream_is_stale(
            1_000,
            Some(5_000),
            5_000 + STALE_CALLBACK_MS + 1
        ));
    }
}
