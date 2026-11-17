/// The speech model reads 16 kHz mono audio. The capture, the takes, and the
/// bench all give it audio at this rate.
pub const SAMPLE_RATE: usize = 16_000;

pub mod log;
pub mod resample;
pub mod stt;
