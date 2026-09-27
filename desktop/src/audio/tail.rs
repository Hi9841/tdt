//! Keeps the microphone open after the shortcut comes up until the last word
//! has actually ended. The shortcut itself is unchanged.
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

pub const MIN_AFTER_RELEASE: Duration = Duration::from_millis(40);
pub const QUIET_HOLD: Duration = Duration::from_millis(150);
pub const MAX_TAIL: Duration = Duration::from_millis(600);

#[derive(Debug, Default)]
pub struct SpeechTail {
    frames: Vec<f32>,
    carry: Vec<f32>,
    samples: usize,
    release_sample: Option<usize>,
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
        }
    }

    /// Audio captured since the shortcut came up.
    pub fn since_release(&self) -> Duration {
        let Some(origin) = self.release_sample else {
            return Duration::ZERO;
        };
        let samples = self.samples.saturating_sub(origin);
        Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64)
    }

    pub fn should_stop(&self) -> bool {
        let elapsed = self.since_release();
        if elapsed.is_zero() && self.release_sample.is_none() {
            return false;
        }
        if elapsed >= MAX_TAIL {
            return true;
        }
        elapsed >= MIN_AFTER_RELEASE && self.quiet_for() >= QUIET_HOLD
    }

    fn quiet_for(&self) -> Duration {
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

    fn room_noise(&self) -> Option<f32> {
        if self.frames.is_empty() {
            return None;
        }
        let opening = &self.frames[..self.frames.len().min(OPENING_FRAMES)];
        Some(percentile_15(&self.frames).min(median(opening)))
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
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((sorted.len() as f32) * 0.15).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) * 0.5
    } else {
        sorted[mid]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0; SAMPLE_RATE * ms / 1000]
    }

    fn tone(ms: usize, amplitude: f32) -> Vec<f32> {
        vec![amplitude; SAMPLE_RATE * ms / 1000]
    }

    #[test]
    fn already_quiet_stops_40ms_after_release() {
        let mut tail = SpeechTail::default();
        tail.push_samples(&silence(200));
        tail.mark_release();
        tail.push_samples(&silence(30));
        assert!(!tail.should_stop());
        tail.push_samples(&silence(10));
        assert!(tail.should_stop());
        assert!(tail.since_release() >= MIN_AFTER_RELEASE);
        assert!(tail.since_release() < QUIET_HOLD);
    }

    #[test]
    fn speech_past_release_waits_for_150ms_of_quiet() {
        let mut tail = SpeechTail::default();
        tail.push_samples(&silence(80));
        tail.push_samples(&tone(300, 0.1));
        tail.mark_release();
        tail.push_samples(&tone(40, 0.1));
        tail.push_samples(&silence(140));
        assert!(!tail.should_stop());
        tail.push_samples(&silence(10));
        assert!(tail.should_stop());
        assert!(tail.since_release() >= Duration::from_millis(190));
    }

    #[test]
    fn steady_speech_runs_to_the_600ms_cap() {
        let mut tail = SpeechTail::default();
        tail.push_samples(&tone(200, 0.1));
        tail.mark_release();
        tail.push_samples(&tone(590, 0.1));
        assert!(!tail.should_stop());
        tail.push_samples(&tone(10, 0.1));
        assert!(tail.should_stop());
    }

    #[test]
    fn soft_room_still_hears_a_quiet_ending() {
        // Noise near -40 dB, one louder peak. The threshold has to stay above
        // the floor, or the room never counts as quiet.
        let mut tail = SpeechTail::default();
        tail.push_samples(&tone(200, 0.01));
        tail.push_samples(&tone(20, 0.1));
        tail.mark_release();
        tail.push_samples(&tone(200, 0.01));
        assert!(tail.should_stop());
    }

    #[test]
    fn release_without_audio_does_not_stop() {
        let mut tail = SpeechTail::default();
        tail.mark_release();
        assert!(!tail.should_stop());
    }
}
