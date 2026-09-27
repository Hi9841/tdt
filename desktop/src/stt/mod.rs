pub mod engine;
pub mod models;

pub use engine::{LiveAudio, LiveTranscript, SttEngine};
pub use models::{DownloadPhase, CATALOG, DEFAULT_MODEL_ID};

use parking_lot::Mutex;
use std::sync::Arc;

pub type SharedEngine = Arc<Mutex<Option<Arc<SttEngine>>>>;
