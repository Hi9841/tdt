# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Moondream Parakeet Redux: 1.58-bit ternary transducer weights from
  https://huggingface.co/moondream/parakeet-redux (CC-BY-4.0).
  Downloaded on demand into `%LOCALAPPDATA%\TDT\models\parakeet-redux\`.
- Moondream Parakeet Ultra: f16 transducer weights from
  https://huggingface.co/moondream/parakeet-ultra (CC-BY-4.0).
  Downloaded on demand into `%LOCALAPPDATA%\TDT\models\parakeet-ultra\`.
- TDT speech engine: TDT's own Rust implementation of the Parakeet TDT
  network. Its structure follows the Hugging Face Transformers Parakeet
  implementation (Apache-2.0, https://github.com/huggingface/transformers).
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

Older components (Photon, Sherpa-ONNX, Parakeet Unified 0.6B, Parakeet TDT 0.6B v3, Moonshine, SenseVoice, Whisper)
are no longer used or offered in Settings. Files already on disk are left in place.
