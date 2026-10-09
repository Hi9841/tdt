pub mod engine;
pub mod fillers;
pub mod models;
pub mod photon;

pub use engine::{LiveAudio, LiveTranscript, SttEngine};
pub use fillers::remove_fillers;
pub use models::{DownloadPhase, CATALOG, DEFAULT_MODEL_ID};
// Used by the bench-asr bin through the library target; the app binary
// declares its own module tree and does not touch it.
#[allow(unused_imports)]
pub use photon::pcm16_wav;

use parking_lot::Mutex;
use std::sync::Arc;

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
