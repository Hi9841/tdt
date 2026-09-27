# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Sherpa-ONNX: Apache-2.0. The Windows build uses the `sherpa-onnx` crate.
  Source and releases: https://github.com/k2-fsa/sherpa-onnx
- Parakeet Unified EN 0.6B: English transducer weights from
  https://huggingface.co/nvidia/parakeet-unified-en-0.6b (CC-BY-4.0).
  FluidAudio ships this model on Apple. On Windows, TDT uses the sherpa-onnx
  INT8 streaming export
  `csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-1120ms`
  (`encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`, `tokens.txt`).
  Settings labels that package Parakeet INT8 and Parakeet Q8. Both chips
  download the same files into
  `%LOCALAPPDATA%\TDT\models\parakeet-unified-en-0.6b-q8\`.
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

Older optional downloads (Parakeet v3, Moonshine, SenseVoice, Whisper) are no
longer offered in Settings. Files already on disk are left in place.
