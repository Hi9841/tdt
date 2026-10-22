//! Display-only speech energy. Never modifies the recorded samples.
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;

pub const VIS_BARS: usize = 26;
const FRAME_SECONDS: f32 = 0.025;

pub struct SpeechEnvelope {
    bars: [f32; VIS_BARS],
    /// The bars before the last step, and when that step happened.
    previous: [f32; VIS_BARS],
    stepped_at: Option<Instant>,
    sum_squares: f64,
    samples: usize,
    level: f32,
}

impl Default for SpeechEnvelope {
    fn default() -> Self {
        Self {
            bars: [0.0; VIS_BARS],
            previous: [0.0; VIS_BARS],
            stepped_at: None,
            sum_squares: 0.0,
            samples: 0,
            level: 0.0,
        }
    }
}

impl SpeechEnvelope {
    pub fn push(&mut self, mono: &[f32], sample_rate: u32) {
        debug_assert!(sample_rate > 0, "envelope needs a real sample rate");
        let frame_samples = (sample_rate as f32 * FRAME_SECONDS).round().max(1.0) as usize;
        let before = self.bars;
        let mut stepped = false;
        for &sample in mono {
            let sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            self.sum_squares += f64::from(sample).powi(2);
            self.samples += 1;
            if self.samples == frame_samples {
                let rms = (self.sum_squares / self.samples as f64).sqrt() as f32;
                // Fixed display range: -54 dBFS is quiet, -12 dBFS is full scale.
                // Do not normalize each frame: pauses must remain pauses.
                let target = ((20.0 * rms.max(0.000001).log10() + 54.0) / 42.0).clamp(0.0, 1.0);
                let tau = if target > self.level { 0.015 } else { 0.070 };
                self.level += (target - self.level) * (1.0 - (-FRAME_SECONDS / tau).exp());
                if self.level < 0.005 {
                    self.level = 0.0;
                }
                self.bars.rotate_left(1);
                self.bars[VIS_BARS - 1] = self.level;
                self.sum_squares = 0.0;
                self.samples = 0;
                stepped = true;
            }
        }
        if stepped {
            self.previous = before;
            self.stepped_at = Some(Instant::now());
        }
    }

    /// The bars as drawn at `now`. A step is 25 ms, so a display that
    /// redraws at 120 Hz or more blends each bar from its previous value
    /// over one step instead of showing the same frame three times.
    pub fn bars_at(&self, now: Instant) -> [f32; VIS_BARS] {
        let Some(stepped_at) = self.stepped_at else {
            return self.bars;
        };
        let t = (now.saturating_duration_since(stepped_at).as_secs_f32() / FRAME_SECONDS).min(1.0);
        std::array::from_fn(|i| self.previous[i] + (self.bars[i] - self.previous[i]) * t)
    }
}

/// The live envelope of the open microphone, for the UI to sample on each frame.
#[derive(Clone)]
pub struct LiveWave(pub(super) Arc<Mutex<SpeechEnvelope>>);

impl LiveWave {
    pub fn bars_at(&self, now: Instant) -> [f32; VIS_BARS] {
        self.0.lock().bars_at(now)
    }
}
