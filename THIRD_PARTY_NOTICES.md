# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Sherpa-ONNX: Apache-2.0. The Android build uses the `sherpa-onnx` release
  AAR. Source and releases: https://github.com/k2-fsa/sherpa-onnx
- SenseVoice model: model weights and tokenizer are distributed by the
  FunASR/SenseVoice project via the sherpa-onnx export
  `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`. Review the
  model `LICENSE` and `README.md` before redistributing them. The installer
  copies those files into `%LOCALAPPDATA%\TDT\models\sensevoice\`.
  https://huggingface.co/FunAudioLLM/SenseVoiceSmall
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

The downloader keeps the model license beside the downloaded files at
`models/sensevoice/LICENSE`.
