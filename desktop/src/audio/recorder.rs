use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const TARGET_SAMPLE_RATE: u32 = 16000;
pub const MAX_RECORDING_SECONDS: usize = 120;
pub const VIS_BARS: usize = 26;
const VIS_WINDOW: usize = VIS_BARS * 40;

pub struct AudioRecorder {
    is_recording: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<f32>>>,
    current_rms: Arc<Mutex<f32>>,
    vis_ring: Arc<Mutex<VecDeque<f32>>>,
    vis_peaks: Arc<Mutex<[f32; VIS_BARS]>>,
    _stream: cpal::Stream,
}

pub fn bin_peaks(samples: &[f32], bars: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; bars];
    if samples.is_empty() || bars == 0 {
        return out;
    }
    let chunk = (samples.len() / bars).max(1);
    for (i, slot) in out.iter_mut().enumerate() {
        let start = (i * chunk).min(samples.len());
        let end = ((i + 1) * chunk).min(samples.len());
        if start >= end {
            break;
        }
        let mut peak = 0.0f32;
        for &sample in &samples[start..end] {
            peak = peak.max(sample.abs());
        }
        *slot = peak.min(1.0);
    }
    out
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
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let current_rms = Arc::new(Mutex::new(0.0f32));
        let vis_ring = Arc::new(Mutex::new(VecDeque::with_capacity(VIS_WINDOW)));
        let vis_peaks = Arc::new(Mutex::new([0.0f32; VIS_BARS]));

        let is_rec_clone = Arc::clone(&is_recording);
        let buffer_clone = Arc::clone(&buffer);
        let rms_clone = Arc::clone(&current_rms);
        let vis_ring_clone = Arc::clone(&vis_ring);
        let vis_peaks_clone = Arc::clone(&vis_peaks);

        let input_sample_rate = config.sample_rate.0;
        let channels = config.channels as usize;

        let err_fn = |err| eprintln!("Audio stream error: {}", err);

        let stream = match sample_format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| {
                    if is_rec_clone.load(Ordering::Relaxed) {
                        Self::process_samples_f32(
                            data,
                            channels,
                            input_sample_rate,
                            &buffer_clone,
                            &rms_clone,
                            &vis_ring_clone,
                            &vis_peaks_clone,
                        );
                    } else {
                        *rms_clone.lock() = 0.0;
                        decay_vis(&vis_peaks_clone);
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &_| {
                    if is_rec_clone.load(Ordering::Relaxed) {
                        let f32_data: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        Self::process_samples_f32(
                            &f32_data,
                            channels,
                            input_sample_rate,
                            &buffer_clone,
                            &rms_clone,
                            &vis_ring_clone,
                            &vis_peaks_clone,
                        );
                    } else {
                        *rms_clone.lock() = 0.0;
                        decay_vis(&vis_peaks_clone);
                    }
                },
                err_fn,
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
            buffer,
            current_rms,
            vis_ring,
            vis_peaks,
            _stream: stream,
        })
    }

    fn process_samples_f32(
        data: &[f32],
        channels: usize,
        sample_rate: u32,
        buffer: &Arc<Mutex<Vec<f32>>>,
        rms: &Arc<Mutex<f32>>,
        vis_ring: &Arc<Mutex<VecDeque<f32>>>,
        vis_peaks: &Arc<Mutex<[f32; VIS_BARS]>>,
    ) {
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
        let sum_squares: f32 = mono.iter().map(|&s| s * s).sum();
        let level = (sum_squares / mono.len() as f32).sqrt().min(1.0);
        *rms.lock() = level;

        // Resample to 16,000 Hz if necessary
        let resampled = if sample_rate != TARGET_SAMPLE_RATE && sample_rate > 0 {
            Self::resample_linear(&mono, sample_rate, TARGET_SAMPLE_RATE)
        } else {
            mono
        };

        {
            let mut ring = vis_ring.lock();
            ring.extend(resampled.iter().copied());
            while ring.len() > VIS_WINDOW {
                ring.pop_front();
            }
            let snapshot: Vec<f32> = ring.iter().copied().collect();
            let binned = bin_peaks(&snapshot, VIS_BARS);
            let mut peaks = vis_peaks.lock();
            for (slot, value) in peaks.iter_mut().zip(binned) {
                if value > *slot {
                    *slot = *slot * 0.25 + value * 0.75;
                } else {
                    *slot = *slot * 0.80 + value * 0.20;
                }
            }
        }

        let mut recorded = buffer.lock();
        let room = MAX_RECORDING_SECONDS * TARGET_SAMPLE_RATE as usize;
        if recorded.len() >= room {
            return;
        }
        let take = (room - recorded.len()).min(resampled.len());
        recorded.extend_from_slice(&resampled[..take]);
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
        self.vis_ring.lock().clear();
        *self.vis_peaks.lock() = [0.0; VIS_BARS];
        self.is_recording.store(true, Ordering::SeqCst);
    }

    pub fn stop(&self) -> Vec<f32> {
        self.is_recording.store(false, Ordering::SeqCst);
        *self.current_rms.lock() = 0.0;
        self.vis_ring.lock().clear();
        *self.vis_peaks.lock() = [0.0; VIS_BARS];
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
        *self.vis_peaks.lock()
    }
}

fn decay_vis(vis_peaks: &Arc<Mutex<[f32; VIS_BARS]>>) {
    let mut peaks = vis_peaks.lock();
    for peak in peaks.iter_mut() {
        *peak *= 0.55;
        if *peak < 0.002 {
            *peak = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_peaks_silence_is_flat() {
        let peaks = bin_peaks(&[0.0; 260], VIS_BARS);
        assert_eq!(peaks.len(), VIS_BARS);
        assert!(peaks.iter().all(|&p| p == 0.0));
    }

    #[test]
    fn bin_peaks_spike_lands_in_last_bar() {
        let mut samples = vec![0.0f32; VIS_BARS * 4];
        let last_start = (VIS_BARS - 1) * 4;
        for sample in &mut samples[last_start..] {
            *sample = 0.8;
        }
        let peaks = bin_peaks(&samples, VIS_BARS);
        assert!(peaks[VIS_BARS - 1] > 0.7);
        assert!(peaks[0] < 0.01);
    }
}
