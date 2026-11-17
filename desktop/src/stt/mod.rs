pub mod dictionary;
pub mod engine;
pub mod fillers;
pub mod models;
pub mod segment;
pub mod takes;

pub use engine::{
    panic_in_engine_call, ErrorKind, Latency, LiveAudio, LiveTranscript, StopTimes, SttEngine,
    TakeError,
};
pub use fillers::remove_fillers;
pub use models::{DownloadPhase, CATALOG, DEFAULT_MODEL_ID};

use crate::SAMPLE_RATE;
use parking_lot::Mutex;
use std::sync::Arc;

/// The longest take. The recorder stops here, and the engine reads no more.
pub const MAX_TAKE_SECONDS: usize = 120;
pub const MAX_TAKE_SAMPLES: usize = MAX_TAKE_SECONDS * SAMPLE_RATE;

type SharedEngine = Arc<Mutex<Option<Arc<SttEngine>>>>;

/// The single STT engine slot shared by the hotkey loop and the settings UI.
/// Hides the lock so callers cannot nest it or forget the clone.
#[derive(Clone, Default)]
pub struct EngineSlot(SharedEngine);

impl EngineSlot {
    pub fn get(&self) -> Option<Arc<SttEngine>> {
        self.0.lock().clone()
    }

    pub fn set(&self, engine: Option<Arc<SttEngine>>) {
        *self.0.lock() = engine;
    }

    pub fn is_some(&self) -> bool {
        self.0.lock().is_some()
    }
}
