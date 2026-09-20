mod envelope;
pub mod recorder;
pub mod sounds;

pub use envelope::VIS_BARS;
pub use recorder::{AudioRecorder, MAX_RECORDING_SECONDS};
pub use sounds::{play_sound, SoundEffect};
