//! Log-mel features as the Parakeet feature extractor computes them:
//! preemphasis, centered STFT with a Hann window, power spectrum, Slaney mel
//! filters, log, and per-bin normalization over the take.

const N_FFT: usize = 512;
const WIN_LENGTH: usize = 400;
const HOP: usize = 160;
const SAMPLE_RATE: f64 = 16_000.0;
const PREEMPHASIS: f32 = 0.97;
const LOG_GUARD: f32 = 5.960_464_5e-8; // 2^-24
const NORM_EPSILON: f32 = 1e-5;
const BINS: usize = N_FFT / 2 + 1;

pub struct MelFrontend {
    /// Per mel bin, the first FFT bin and the weights of its nonzero span.
    filters: Vec<(usize, Vec<f32>)>,
    /// Hann window of WIN_LENGTH, centered inside N_FFT.
    window: Vec<f64>,
    fft: Fft,
}

impl MelFrontend {
    pub fn new(mels: usize) -> Self {
        let mut window = vec![0.0; N_FFT];
        let offset = (N_FFT - WIN_LENGTH) / 2;
        for n in 0..WIN_LENGTH {
            let phase = 2.0 * std::f64::consts::PI * n as f64 / (WIN_LENGTH - 1) as f64;
            window[offset + n] = 0.5 - 0.5 * phase.cos();
        }
        Self {
            filters: spans(&slaney_filters(mels)),
            window,
            fft: Fft::new(N_FFT),
        }
    }

    /// `frames x mels`, frames = samples / 160. Empty for less than one hop.
    pub fn compute(&self, audio: &[f32]) -> (Vec<f32>, usize) {
        let frames = audio.len() / HOP;
        if frames < 2 {
            return (Vec::new(), 0);
        }
        let mut emphasized = Vec::with_capacity(audio.len() + N_FFT);
        emphasized.resize(N_FFT / 2, 0.0f32);
        emphasized.push(audio[0]);
        for pair in audio.windows(2) {
            emphasized.push(pair[1] - PREEMPHASIS * pair[0]);
        }
        emphasized.resize(emphasized.len() + N_FFT / 2, 0.0);

        let mels = self.filters.len();
        let mut out = vec![0.0f32; frames * mels];
        let mut re = vec![0.0f64; N_FFT];
        let mut im = vec![0.0f64; N_FFT];
        let mut power = vec![0.0f32; BINS];
        for frame in 0..frames {
            let start = frame * HOP;
            for n in 0..N_FFT {
                re[n] = f64::from(emphasized[start + n]) * self.window[n];
                im[n] = 0.0;
            }
            self.fft.run(&mut re, &mut im);
            for bin in 0..BINS {
                power[bin] = (re[bin] * re[bin] + im[bin] * im[bin]) as f32;
            }
            let row = &mut out[frame * mels..(frame + 1) * mels];
            for (value, (first, weights)) in row.iter_mut().zip(&self.filters) {
                let energy: f32 = weights
                    .iter()
                    .zip(&power[*first..])
                    .map(|(w, p)| w * p)
                    .sum();
                *value = (energy + LOG_GUARD).ln();
            }
        }
        normalize(&mut out, frames, mels);
        (out, frames)
    }
}

/// Mean and sample standard deviation per mel bin over all frames.
fn normalize(features: &mut [f32], frames: usize, mels: usize) {
    for mel in 0..mels {
        let mean = (0..frames)
            .map(|f| f64::from(features[f * mels + mel]))
            .sum::<f64>()
            / frames as f64;
        let variance = (0..frames)
            .map(|f| (f64::from(features[f * mels + mel]) - mean).powi(2))
            .sum::<f64>()
            / (frames - 1) as f64;
        let std = variance.sqrt() as f32;
        for f in 0..frames {
            let value = &mut features[f * mels + mel];
            *value = (*value - mean as f32) / (std + NORM_EPSILON);
        }
    }
}

fn hz_to_mel(hz: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if hz >= min_log_hz {
        min_log_mel + (hz / min_log_hz).ln() / logstep
    } else {
        hz / f_sp
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if mel >= min_log_mel {
        min_log_hz * (logstep * (mel - min_log_mel)).exp()
    } else {
        f_sp * mel
    }
}

/// librosa.filters.mel(sr=16000, n_fft=512, fmin=0, fmax=8000, htk=False, norm="slaney").
fn slaney_filters(mels: usize) -> Vec<f32> {
    let fft_freqs: Vec<f64> = (0..BINS)
        .map(|bin| bin as f64 * SAMPLE_RATE / N_FFT as f64)
        .collect();
    let max_mel = hz_to_mel(SAMPLE_RATE / 2.0);
    let points: Vec<f64> = (0..mels + 2)
        .map(|i| mel_to_hz(max_mel * i as f64 / (mels + 1) as f64))
        .collect();
    let mut filters = vec![0.0f32; mels * BINS];
    for mel in 0..mels {
        let lower_width = points[mel + 1] - points[mel];
        let upper_width = points[mel + 2] - points[mel + 1];
        let norm = 2.0 / (points[mel + 2] - points[mel]);
        for (bin, freq) in fft_freqs.iter().enumerate() {
            let lower = (freq - points[mel]) / lower_width;
            let upper = (points[mel + 2] - freq) / upper_width;
            filters[mel * BINS + bin] = (lower.min(upper).max(0.0) * norm) as f32;
        }
    }
    filters
}

/// The nonzero span of each `BINS`-wide filter row. A filter covers a few
/// bins, so the product skips the zeros; the sum stays the same, in the same order.
fn spans(filters: &[f32]) -> Vec<(usize, Vec<f32>)> {
    filters
        .as_chunks::<BINS>()
        .0
        .iter()
        .map(|row| {
            let first = row.iter().position(|w| *w != 0.0).unwrap_or(0);
            let last = row.iter().rposition(|w| *w != 0.0).unwrap_or(0);
            (first, row[first..=last.max(first)].to_vec())
        })
        .collect()
}

/// Iterative radix-2 complex FFT for one power-of-two size.
struct Fft {
    size: usize,
    cos: Vec<f64>,
    sin: Vec<f64>,
    reversed: Vec<usize>,
}

impl Fft {
    fn new(size: usize) -> Self {
        let bits = size.trailing_zeros();
        let reversed = (0..size)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let angles = (0..size / 2).map(|k| -2.0 * std::f64::consts::PI * k as f64 / size as f64);
        Self {
            size,
            cos: angles.clone().map(f64::cos).collect(),
            sin: angles.map(f64::sin).collect(),
            reversed,
        }
    }

    fn run(&self, re: &mut [f64], im: &mut [f64]) {
        for i in 0..self.size {
            let j = self.reversed[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= self.size {
            let step = self.size / len;
            for start in (0..self.size).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + len / 2);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hz_to_mel, mel_to_hz, slaney_filters, spans, Fft, MelFrontend, BINS};

    #[test]
    fn the_fft_matches_a_direct_transform() {
        let fft = Fft::new(16);
        let signal: Vec<f64> = (0..16).map(|i| ((i * 7 % 5) as f64) - 2.0).collect();
        let (mut re, mut im) = (signal.clone(), vec![0.0; 16]);
        fft.run(&mut re, &mut im);
        for k in 0..16 {
            let (mut dr, mut di) = (0.0, 0.0);
            for (n, x) in signal.iter().enumerate() {
                let angle = -2.0 * std::f64::consts::PI * (k * n) as f64 / 16.0;
                dr += x * angle.cos();
                di += x * angle.sin();
            }
            assert!((re[k] - dr).abs() < 1e-9 && (im[k] - di).abs() < 1e-9);
        }
    }

    #[test]
    fn the_slaney_scale_round_trips_and_filters_cover_the_band() {
        for hz in [0.0, 300.0, 1000.0, 4000.0, 8000.0] {
            assert!((mel_to_hz(hz_to_mel(hz)) - hz).abs() < 1e-6);
        }
        let filters = slaney_filters(128);
        for mel in 0..128 {
            assert!(filters[mel * BINS..(mel + 1) * BINS]
                .iter()
                .any(|w| *w > 0.0));
        }
    }

    #[test]
    fn filter_spans_expand_back_to_the_full_filters() {
        let filters = slaney_filters(128);
        for (mel, (first, weights)) in spans(&filters).iter().enumerate() {
            let mut row = vec![0.0f32; BINS];
            row[*first..first + weights.len()].copy_from_slice(weights);
            assert_eq!(row, filters[mel * BINS..(mel + 1) * BINS], "mel {mel}");
        }
    }

    #[test]
    fn features_are_normalized_per_bin() {
        let audio: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 * 0.05).sin() * 0.3 + (i as f32 * 0.31).sin() * 0.1)
            .collect();
        let (features, frames) = MelFrontend::new(128).compute(&audio);
        assert_eq!(frames, 100);
        let mean: f32 = (0..frames).map(|f| features[f * 128 + 10]).sum::<f32>() / frames as f32;
        assert!(mean.abs() < 1e-4);
    }
}
