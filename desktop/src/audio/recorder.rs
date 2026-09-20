use super::envelope::{SpeechEnvelope, VIS_BARS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const TARGET_SAMPLE_RATE: u32 = 16000;
pub const MAX_RECORDING_SECONDS: usize = 120;

pub struct AudioRecorder {
    is_recording: Arc<AtomicBool>,
    at_limit: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
    buffer: Arc<Mutex<Vec<f32>>>,
    current_rms: Arc<Mutex<f32>>,
    envelope: Arc<Mutex<SpeechEnvelope>>,
    _stream: cpal::Stream,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "No audio input device found".to_string())?;

        let supported_config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get default input config: {}", e))?;
        let sample_format = supported_config.sample_format();
        let config: cpal::StreamConfig = supported_config.into();

        let is_recording = Arc::new(AtomicBool::new(false));
        let at_limit = Arc::new(AtomicBool::new(false));
        let last_error = Arc::new(Mutex::new(None));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let current_rms = Arc::new(Mutex::new(0.0f32));
        let envelope = Arc::new(Mutex::new(SpeechEnvelope::default()));

        let is_rec_clone = Arc::clone(&is_recording);
        let at_limit_clone = Arc::clone(&at_limit);
        let buffer_clone = Arc::clone(&buffer);
        let rms_clone = Arc::clone(&current_rms);
        let envelope_clone = Arc::clone(&envelope);

        let input_sample_rate = config.sample_rate.0;
        let channels = config.channels as usize;

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| {
                    if is_rec_clone.load(Ordering::Relaxed) {
                        if Self::process_samples_f32(
                            data,
                            channels,
                            input_sample_rate,
                            &buffer_clone,
                            &rms_clone,
                            &envelope_clone,
                        ) {
                            at_limit_clone.store(true, Ordering::SeqCst);
                        }
                    } else {
                        *rms_clone.lock() = 0.0;
                    }
                },
                stream_error_callback(Arc::clone(&last_error), Arc::clone(&is_recording)),
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &_| {
                    if is_rec_clone.load(Ordering::Relaxed) {
                        let f32_data: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        if Self::process_samples_f32(
                            &f32_data,
                            channels,
                            input_sample_rate,
                            &buffer_clone,
                            &rms_clone,
                            &envelope_clone,
                        ) {
                            at_limit_clone.store(true, Ordering::SeqCst);
                        }
                    } else {
                        *rms_clone.lock() = 0.0;
                    }
                },
                stream_error_callback(Arc::clone(&last_error), Arc::clone(&is_recording)),
                None,
            ),
            _ => return Err("Unsupported audio sample format".to_string()),
        }
        .map_err(|e| format!("Failed to build input stream: {}", e))?;

        stream
            .play()
            .map_err(|e| format!("Failed to start input stream: {}", e))?;

        Ok(Self {
            is_recording,
            at_limit,
            last_error,
            buffer,
            current_rms,
            envelope,
            _stream: stream,
        })
    }

    fn process_samples_f32(
        data: &[f32],
        channels: usize,
        sample_rate: u32,
        buffer: &Arc<Mutex<Vec<f32>>>,
        rms: &Arc<Mutex<f32>>,
        envelope: &Arc<Mutex<SpeechEnvelope>>,
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
}
