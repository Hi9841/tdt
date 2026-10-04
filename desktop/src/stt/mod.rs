pub mod engine;
pub mod models;
pub mod photon;

pub use engine::{LiveAudio, LiveTranscript, SttEngine};
pub use models::{DownloadPhase, CATALOG, DEFAULT_MODEL_ID};
#[allow(unused_imports)]
pub use photon::pcm16_wav;

use parking_lot::Mutex;
use std::sync::Arc;

pub type SharedEngine = Arc<Mutex<Option<Arc<SttEngine>>>>;
