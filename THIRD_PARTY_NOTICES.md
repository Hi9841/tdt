# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Moondream Parakeet Redux: 1.58-bit ternary transducer weights from
  https://huggingface.co/moondream/parakeet-redux (CC-BY-4.0).
  Downloaded on demand into `%LOCALAPPDATA%\TDT\models\parakeet-redux\`.
- TDT speech engine: TDT's own Rust implementation of the Parakeet TDT
  network. Its structure follows the Hugging Face Transformers Parakeet
  implementation (Apache-2.0, https://github.com/huggingface/transformers).
- Photon / Moondream (fallback only): the `moondream` Python package
  (Apache-2.0) installs `kestrel-kernels`, which has a proprietary license
  from M87 Labs. TDT does not ship either package. uv installs them only when
  TDT falls back to Photon.
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

Older components (Sherpa-ONNX, Parakeet Unified 0.6B, Moonshine, SenseVoice, Whisper)
are no longer used or offered in Settings. Files already on disk are left in place.
