mod envelope;
pub mod recorder;
pub mod sounds;
mod tail;

pub use envelope::{LiveWave, VIS_BARS};
pub use recorder::{watch_default_input, AudioRecorder, MAX_RECORDING_SECONDS};
pub use sounds::{play_sound, prewarm, SoundEffect};
pub use tail::{SpeechTail, MAX_TAIL};
