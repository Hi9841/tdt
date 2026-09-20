//! Display-only speech energy. Never modifies the recorded samples.
pub const VIS_BARS: usize = 26;
const FRAME_SECONDS: f32 = 0.025;

pub struct SpeechEnvelope {
    bars: [f32; VIS_BARS],
    sum_squares: f64,
    samples: usize,
    level: f32,
}

impl Default for SpeechEnvelope {
    fn default() -> Self {
        Self {
            bars: [0.0; VIS_BARS],
            sum_squares: 0.0,
            samples: 0,
            level: 0.0,
        }
    }
}

impl SpeechEnvelope {
    pub fn push(&mut self, mono: &[f32], sample_rate: u32) {
        let frame_samples = (sample_rate as f32 * FRAME_SECONDS).round().max(1.0) as usize;
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
            }
        }
    }

    pub fn bars(&self) -> [f32; VIS_BARS] {
        self.bars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| amplitude * (std::f32::consts::TAU * 200.0 * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn silence_and_low_noise_stay_flat() {
        let mut envelope = SpeechEnvelope::default();
        envelope.push(&vec![0.0; 16000], 16000);
        envelope.push(&tone(0.001, 16000, 1.0), 16000);
        assert_eq!(envelope.bars(), [0.0; VIS_BARS]);
    }

    #[test]
    fn speech_is_visible_and_loudness_is_preserved() {
        let mut quiet = SpeechEnvelope::default();
        let mut loud = SpeechEnvelope::default();
        quiet.push(&tone(0.02, 16000, 0.1), 16000);
        loud.push(&tone(0.2, 16000, 0.1), 16000);
        assert!(quiet.bars()[25] > 0.3);
        assert!(loud.bars()[25] > quiet.bars()[25] + 0.3);
        assert_eq!(quiet.bars()[0], 0.0);
    }

    #[test]
    fn onset_is_fast_and_pause_settles_without_fake_motion() {
        let mut envelope = SpeechEnvelope::default();
        envelope.push(&tone(0.1, 16000, 0.025), 16000);
        let attack = envelope.bars()[25];
        assert!(attack > 0.5);
        envelope.push(&[0.0; 400], 16000);
        assert!(envelope.bars()[25] > 0.0 && envelope.bars()[25] < attack);
        assert_eq!(envelope.bars()[24], attack);
        envelope.push(&[0.0; 16000], 16000);
        assert_eq!(envelope.bars(), [0.0; VIS_BARS]);
    }

    #[test]
    fn callback_chunk_size_does_not_change_history() {
        let signal = tone(0.08, 48000, 0.8);
        let mut whole = SpeechEnvelope::default();
        let mut chunked = SpeechEnvelope::default();
        whole.push(&signal, 48000);
        for chunk in signal.chunks(137) {
            chunked.push(chunk, 48000);
        }
        assert_eq!(whole.bars(), chunked.bars());
    }

    #[test]
    fn sample_rates_have_the_same_display_time_scale() {
        let mut low = SpeechEnvelope::default();
        let mut high = SpeechEnvelope::default();
        low.push(&tone(0.08, 16000, 0.25), 16000);
        high.push(&tone(0.08, 48000, 0.25), 48000);
        for (a, b) in low.bars().into_iter().zip(high.bars()) {
            assert!((a - b).abs() < 0.001);
        }
    }

    #[test]
    fn invalid_samples_cannot_poison_the_display() {
        let mut envelope = SpeechEnvelope::default();
        envelope.push(&[f32::NAN; 400], 16000);
        envelope.push(&[f32::INFINITY; 400], 16000);
        assert_eq!(envelope.bars(), [0.0; VIS_BARS]);
    }
}
