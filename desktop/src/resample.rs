//! Converts microphone audio to the 16 kHz mono that the speech model reads.
//!
//! A USB microphone gives 44.1 or 48 kHz. Taking every third sample folds
//! everything above 8 kHz (hiss, the top of "s" and "sh") back into the
//! speech band, so a low-pass filter runs before the rate drops. The state
//! carries over between capture packets: a packet size that does not divide
//! the rate ratio does not drop samples or restart the filter.

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Zero crossings of the kernel on each side of its center, at the output rate.
const ZERO_CROSSINGS: f64 = 16.0;
/// Kaiser window shape. With 16 zero crossings: flat to 6.5 kHz, -85 dB from 9 kHz.
const KAISER_BETA: f64 = 8.0;
/// Cutoff as a fraction of the lower Nyquist rate.
const ROLLOFF: f64 = 0.92;
/// Kernel table points for each input sample. Between points, interpolate.
const TABLE_STEPS: usize = 64;

/// Stream resampler from one device rate to 16 kHz.
pub struct Resampler {
    /// Input samples for each output sample.
    step: f64,
    /// Kernel half-width in input samples.
    half: usize,
    /// Kernel at `1 / TABLE_STEPS` input samples from `-half` to `+half`.
    table: Vec<f32>,
    /// Input that later outputs still need.
    history: Vec<f32>,
    /// Position of the next output in `history`.
    next: f64,
}

impl Resampler {
    /// `None` when the device rate is already 16 kHz.
    pub fn new(from_rate: u32) -> Option<Self> {
        if from_rate == TARGET_SAMPLE_RATE || from_rate == 0 {
            return None;
        }
        let step = f64::from(from_rate) / f64::from(TARGET_SAMPLE_RATE);
        // Cycles per input sample.
        let cutoff = 0.5 * ROLLOFF / step.max(1.0);
        let half = (ZERO_CROSSINGS / (2.0 * cutoff)).ceil() as usize;
        let points = 2 * half * TABLE_STEPS + 1;
        let table = (0..points)
            .map(|index| {
                let x = index as f64 / TABLE_STEPS as f64 - half as f64;
                kernel(x, cutoff, half as f64) as f32
            })
            .collect();
        let mut resampler = Self {
            step,
            half,
            table,
            history: Vec::new(),
            next: 0.0,
        };
        resampler.reset();
        Some(resampler)
    }

    /// Forget the previous take. The first output is centered on the first
    /// new input sample.
    pub fn reset(&mut self) {
        self.history.clear();
        self.history.resize(self.half, 0.0);
        self.next = self.half as f64;
    }

    /// Appends the 16 kHz samples for `input` to `output`.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        self.history.extend_from_slice(input);
        let half = self.half as f64;
        while self.next + half < self.history.len() as f64 {
            let center = self.next.floor() as usize;
            let first = center + 1 - self.half;
            let mut sum = 0.0f32;
            for (offset, sample) in self.history[first..=center + self.half].iter().enumerate() {
                let distance = self.next - (first + offset) as f64;
                sum += sample * self.weight(distance);
            }
            output.push(sum);
            self.next += self.step;
        }
        let used = (self.next.floor() as usize + 1).saturating_sub(self.half);
        let used = used.min(self.history.len());
        self.history.drain(..used);
        self.next -= used as f64;
    }

    fn weight(&self, distance: f64) -> f32 {
        let position = (distance + self.half as f64) * TABLE_STEPS as f64;
        let index = position.floor() as usize;
        let Some(&after) = self.table.get(index + 1) else {
            return 0.0;
        };
        let before = self.table[index];
        before + (after - before) * (position - index as f64) as f32
    }
}

/// Kaiser-windowed sinc low-pass at `cutoff` cycles per sample.
fn kernel(x: f64, cutoff: f64, half: f64) -> f64 {
    let ratio = x / half;
    if ratio.abs() >= 1.0 {
        return 0.0;
    }
    let arg = 2.0 * cutoff * x;
    let sinc = if arg == 0.0 {
        1.0
    } else {
        (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg)
    };
    let window = bessel_i0(KAISER_BETA * (1.0 - ratio * ratio).sqrt()) / bessel_i0(KAISER_BETA);
    2.0 * cutoff * sinc * window
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let mut k = 1.0;
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}
