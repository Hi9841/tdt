mod envelope;
pub mod recorder;
pub mod sounds;
mod tail;

pub use envelope::VIS_BARS;
#[allow(unused_imports)]
pub use recorder::CaptureHandle;
pub use recorder::{AudioRecorder, MAX_RECORDING_SECONDS};
pub use sounds::{play_sound, SoundEffect};
pub use tail::{SpeechTail, MAX_TAIL};
