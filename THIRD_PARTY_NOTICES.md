# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Sherpa-ONNX: Apache-2.0. The Android build uses the `sherpa-onnx` release
  AAR. Source and releases: https://github.com/k2-fsa/sherpa-onnx
- SenseVoice model: model weights and tokenizer are distributed by the
  FunASR/SenseVoice project via the sherpa-onnx export
  `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`. Review the
  model `LICENSE` and `README.md` before redistributing them. The installer
  copies the Small int8 files into `%LOCALAPPDATA%\TDT\models\sensevoice\`.
  Settings can also download the full-precision `model.onnx` into that folder.
  https://huggingface.co/FunAudioLLM/SenseVoiceSmall
- Whisper models: optional Small and Medium weights are OpenAI Whisper exports
  packaged by sherpa-onnx (`csukuangfj/sherpa-onnx-whisper-small` and
  `csukuangfj/sherpa-onnx-whisper-medium`). TDT downloads them into
  `%LOCALAPPDATA%\TDT\models\whisper-small\` or `whisper-medium\` when you
  choose them in Settings. Review the upstream model cards before
  redistributing them.
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

The downloader keeps the SenseVoice license beside the downloaded files at
`models/sensevoice/LICENSE`.
