use super::envelope::{SpeechEnvelope, VIS_BARS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    current_rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    device_name: String,
    opened_ms: u64,
    last_callback_ms: Arc<AtomicU64>,
    _stream: cpal::Stream,
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

fn open_named(host: &cpal::Host, device_name: &str) -> Result<AudioRecorder, String> {
    let device = find_input(host, device_name)
        .ok_or_else(|| format!("Microphone {device_name} disappeared"))?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| format!("{device_name}: {error}"))?;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();

    let is_recording = Arc::new(AtomicBool::new(false));
    let at_limit = Arc::new(AtomicBool::new(false));
    let last_error = Arc::new(Mutex::new(None));
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let current_rms = Arc::new(Mutex::new(0.0f32));
    let envelope = Arc::new(Mutex::new(SpeechEnvelope::default()));
    let last_callback_ms = Arc::new(AtomicU64::new(0));

    let is_rec_clone = Arc::clone(&is_recording);
    let at_limit_clone = Arc::clone(&at_limit);
    let buffer_clone = Arc::clone(&buffer);
    let rms_clone = Arc::clone(&current_rms);
    let envelope_clone = Arc::clone(&envelope);
    let callback_stamp = Arc::clone(&last_callback_ms);

    let input_sample_rate = config.sample_rate.0;
    let channels = config.channels.max(1) as usize;
    let error_cb = stream_error_callback(Arc::clone(&last_error), Arc::clone(&is_recording));

    let stream = build_stream(
        &device,
        &config,
        sample_format,
        channels,
        input_sample_rate,
        is_rec_clone,
        at_limit_clone,
        buffer_clone,
        rms_clone,
        envelope_clone,
        callback_stamp,
        error_cb,
    )?;

    stream
        .play()
        .map_err(|e| format!("Failed to start input stream: {}", e))?;

    eprintln!("Microphone open: {device_name}");

    Ok(AudioRecorder {
        is_recording,
        at_limit,
        last_error,
        buffer,
        current_rms,
        envelope,
        device_name: device_name.to_string(),
        opened_ms: now_ms(),
        last_callback_ms,
        _stream: stream,
    })
}

impl AudioRecorder {
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// True when the capture callback has stopped. The next recording should
    /// open the microphone again instead of writing into a dead stream.
    pub fn needs_reopen(&self) -> bool {
        stream_is_stale(
            self.opened_ms,
            nonzero(self.last_callback_ms.load(Ordering::Relaxed)),
            now_ms(),
        )
    }

    fn process_samples_f32(
        data: &[f32],
        channels: usize,
        sample_rate: u32,
        buffer: &Mutex<Vec<f32>>,
        rms: &Mutex<f32>,
        envelope: &Mutex<SpeechEnvelope>,
        is_recording: &AtomicBool,
    ) -> bool {
        if data.is_empty() {
            return false;
        }

        // Downmix to mono
        let mut mono: Vec<f32> = Vec::with_capacity(data.len() / channels);
        for chunk in data.chunks_exact(channels) {
            let sum: f32 = chunk.iter().sum();
            mono.push(sum / channels as f32);
        }

        // Calculate RMS audio level (0.0 to 1.0)
        let sum_squares: f32 = mono.iter().map(|&s| s * s).sum();
        let level = (sum_squares / mono.len() as f32).sqrt().min(1.0);
        *rms.lock() = level;
        envelope.lock().push(&mono, sample_rate);

        // Resample to 16,000 Hz if necessary
        let resampled = if sample_rate != TARGET_SAMPLE_RATE && sample_rate > 0 {
            Self::resample_linear(&mono, sample_rate, TARGET_SAMPLE_RATE)
        } else {
            mono
        };

        let mut recorded = buffer.lock();
        if !is_recording.load(Ordering::SeqCst) {
            return false;
        }
        append_bounded(&mut recorded, &resampled, max_recording_samples())
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
        self.buffer.lock().clear();
        *self.current_rms.lock() = 0.0;
        *self.envelope.lock() = SpeechEnvelope::default();
        clear_session_signals(&self.last_error, &self.at_limit);
        self.is_recording.store(true, Ordering::SeqCst);
    }

    /// Samples captured since the last drain. Empty once recording has stopped.
    pub fn drain(&self) -> Vec<f32> {
        if !self.is_recording.load(Ordering::SeqCst) {
            return Vec::new();
        }
        std::mem::take(&mut *self.buffer.lock())
    }

    pub fn stop(&self) -> Vec<f32> {
        self.is_recording.store(false, Ordering::SeqCst);
        *self.current_rms.lock() = 0.0;
        *self.envelope.lock() = SpeechEnvelope::default();
        let mut buf = self.buffer.lock();
        std::mem::take(&mut *buf)
    }

    #[allow(dead_code)]
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::Relaxed)
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

fn max_recording_samples() -> usize {
    MAX_RECORDING_SECONDS * TARGET_SAMPLE_RATE as usize
}

/// Appends samples until `capacity`. Returns true when the buffer is full.
fn append_bounded(recorded: &mut Vec<f32>, samples: &[f32], capacity: usize) -> bool {
    if recorded.len() >= capacity {
        return true;
    }
    let take = (capacity - recorded.len()).min(samples.len());
    recorded.extend_from_slice(&samples[..take]);
    recorded.len() >= capacity
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
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        eprintln!("Audio stream error: {}", err);
        apply_stream_error(&last_error, &is_recording, err);
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

fn ingest(
    samples: &[f32],
    channels: usize,
    sample_rate: u32,
    is_recording: &AtomicBool,
    at_limit: &AtomicBool,
    buffer: &Mutex<Vec<f32>>,
    rms: &Mutex<f32>,
    envelope: &Mutex<SpeechEnvelope>,
    last_callback_ms: &AtomicU64,
) {
    note_callback(last_callback_ms);
    if is_recording.load(Ordering::Relaxed) {
        if AudioRecorder::process_samples_f32(
            samples,
            channels,
            sample_rate,
            buffer,
            rms,
            envelope,
            is_recording,
        ) {
            at_limit.store(true, Ordering::SeqCst);
        }
    } else {
        *rms.lock() = 0.0;
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    channels: usize,
    sample_rate: u32,
    is_recording: Arc<AtomicBool>,
    at_limit: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<f32>>>,
    rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    last_callback_ms: Arc<AtomicU64>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String> {
    let unsupported = || format!("Unsupported audio sample format: {sample_format}");
    match sample_format {
        cpal::SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| {
                    ingest(
                        data,
                        channels,
                        sample_rate,
                        &is_recording,
                        &at_limit,
                        &buffer,
                        &rms,
                        &envelope,
                        &last_callback_ms,
                    );
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
                    ingest(
                        &samples,
                        channels,
                        sample_rate,
                        &is_recording,
                        &at_limit,
                        &buffer,
                        &rms,
                        &envelope,
                        &last_callback_ms,
                    );
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
                    ingest(
                        &samples,
                        channels,
                        sample_rate,
                        &is_recording,
                        &at_limit,
                        &buffer,
                        &rms,
                        &envelope,
                        &last_callback_ms,
                    );
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
                    ingest(
                        &samples,
                        channels,
                        sample_rate,
                        &is_recording,
                        &at_limit,
                        &buffer,
                        &rms,
                        &envelope,
                        &last_callback_ms,
                    );
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
    fn max_recording_samples_matches_120s_at_target_rate() {
        assert_eq!(
            max_recording_samples(),
            MAX_RECORDING_SECONDS * TARGET_SAMPLE_RATE as usize
        );
        assert_eq!(max_recording_samples(), 120 * 16_000);
    }

    #[test]
    fn append_bounded_signals_limit_and_drops_overflow() {
        let mut recorded = Vec::new();
        let capacity = 8;
        assert!(!append_bounded(&mut recorded, &[0.1, 0.2, 0.3], capacity));
        assert_eq!(recorded, vec![0.1, 0.2, 0.3]);

        assert!(append_bounded(
            &mut recorded,
            &[0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
            capacity
        ));
        assert_eq!(recorded, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);

        assert!(append_bounded(&mut recorded, &[1.0, 1.0], capacity));
        assert_eq!(recorded.len(), capacity);
        assert_eq!(recorded.last().copied(), Some(0.8));
    }

    #[test]
    fn append_bounded_full_buffer_matches_120s_capacity() {
        let capacity = max_recording_samples();
        let mut recorded = vec![0.0f32; capacity - 4];
        assert!(append_bounded(&mut recorded, &[0.5; 16], capacity));
        assert_eq!(recorded.len(), capacity);
        assert!(append_bounded(&mut recorded, &[0.9; 32], capacity));
        assert_eq!(recorded.len(), capacity);
    }

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
