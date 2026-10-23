//! Keeps the microphone open after the shortcut comes up only while the
//! last word is still ending: a user who was already quiet stops at once.
//! The shortcut itself is unchanged.
//!
//! Quiet is decided from 10 ms frames. Room noise is the lower of the 15th
//! percentile of every frame so far and the median of the first 80 ms. The
//! threshold sits 9 dB above that floor, drops toward 18 dB below the peak so
//! a soft ending still counts as speech, and never comes closer than 4 dB to
//! the floor. A flat loud recording has no floor to measure, so it is not
//! treated as quiet.

use std::time::Duration;

const SAMPLE_RATE: usize = 16_000;
const FRAME_SAMPLES: usize = 160;
const FRAME_MS: usize = 10;
const OPENING_FRAMES: usize = 8;
const DB_FLOOR: f32 = -100.0;
/// Steady speech with no quieter frames still has to run out the cap.
const FLAT_SPEECH_DB: f32 = -42.0;

/// Quiet that ends a take. 60 ms lost no word on 60 LibriSpeech clips at
/// any release point; a longer hold only made the user wait.
pub const QUIET_HOLD: Duration = Duration::from_millis(60);
pub const MAX_TAIL: Duration = Duration::from_millis(600);

#[derive(Debug, Default)]
pub struct SpeechTail {
    frames: Vec<f32>,
    carry: Vec<f32>,
    samples: usize,
    release_sample: Option<usize>,
    release_at: Option<std::time::Instant>,
    noise_cache: Option<(usize, f32)>,
}

impl SpeechTail {
    pub fn push_samples(&mut self, samples: &[f32]) {
        self.samples += samples.len();
        self.carry.extend_from_slice(samples);
        while self.carry.len() >= FRAME_SAMPLES {
            let frame: Vec<f32> = self.carry.drain(..FRAME_SAMPLES).collect();
            self.frames.push(frame_db(&frame));
        }
    }

    pub fn mark_release(&mut self) {
        if self.release_sample.is_none() {
            self.release_sample = Some(self.samples);
            self.release_at = Some(std::time::Instant::now());
        }
    }

    /// When the shortcut came up.
    pub fn released_at(&self) -> Option<std::time::Instant> {
        self.release_at
    }

    /// Audio captured since the shortcut came up.
    pub fn since_release(&self) -> Duration {
        let Some(origin) = self.release_sample else {
            return Duration::ZERO;
        };
        let samples = self.samples.saturating_sub(origin);
        Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64)
    }

    pub fn should_stop(&mut self) -> bool {
        let elapsed = self.since_release();
        if elapsed.is_zero() && self.release_sample.is_none() {
            return false;
        }
        if elapsed >= MAX_TAIL {
            return true;
        }
        // A stalled audio callback must not hold the tail open forever.
        if let Some(release_at) = self.release_at {
            if release_at.elapsed() >= MAX_TAIL {
                return true;
            }
        }
        self.quiet_for() >= QUIET_HOLD
    }

    /// The quiet at the end of the take, counted across the release: a
    /// user who stopped talking before letting go does not wait.
    fn quiet_for(&mut self) -> Duration {
        let Some(noise) = self.room_noise() else {
            return Duration::ZERO;
        };
        let peak = self.frames.iter().copied().fold(DB_FLOOR, f32::max);
        let mut quiet_frames = 0usize;
        for db in self.frames.iter().rev() {
            if !frame_is_quiet(*db, noise, peak) {
                break;
            }
            quiet_frames += 1;
        }
        Duration::from_millis((quiet_frames * FRAME_MS) as u64)
    }

    fn room_noise(&mut self) -> Option<f32> {
        if self.frames.is_empty() {
            return None;
        }
        if let Some((len, value)) = self.noise_cache {
            if len == self.frames.len() {
                return Some(value);
            }
        }
        let opening = &self.frames[..self.frames.len().min(OPENING_FRAMES)];
        let value = percentile_15(&self.frames).min(median(opening));
        self.noise_cache = Some((self.frames.len(), value));
        Some(value)
    }
}

fn frame_is_quiet(db: f32, noise: f32, peak: f32) -> bool {
    let separation = peak - noise;
    if separation < 9.0 {
        return peak < FLAT_SPEECH_DB;
    }
    let threshold = (noise + 9.0).min(peak - 18.0).max(noise + 4.0);
    db < threshold
}

fn frame_db(samples: &[f32]) -> f32 {
    let mut sum = 0.0f64;
    for sample in samples {
        let sample = if sample.is_finite() {
            f64::from(*sample)
        } else {
            0.0
        };
        sum += sample * sample;
    }
    let rms = (sum / samples.len() as f64).sqrt();
    if rms <= 1e-5 {
        DB_FLOOR
    } else {
        (20.0 * rms.log10()) as f32
    }
}

fn percentile_15(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let rank = ((sorted.len() as f32) * 0.15).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) * 0.5
    } else {
        sorted[mid]
    }
}
