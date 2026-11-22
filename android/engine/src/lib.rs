//! The speech path of TDT for Android.
//!
//! It is the take path of TDT desktop (`desktop/src/stt/engine.rs`) on the
//! same engine, Trit:
//!
//! - `engine`: one model on disk, loaded on first use.
//! - `take`: one take. The model reads it while the user talks, so the text
//!   is ready when the user stops.
//! - `segment`: finds pauses and joins overlapping reads.
//! - `ffi`: the C ABI (`include/tdt_speech.h`) that the JNI bridge calls.

mod engine;
mod ffi;
mod log;
mod segment;
mod take;

pub use engine::Engine;
pub use take::{samples_of, Heard, Take};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
