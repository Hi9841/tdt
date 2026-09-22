# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Sherpa-ONNX: Apache-2.0. The Android build uses the `sherpa-onnx` release
  AAR. Source and releases: https://github.com/k2-fsa/sherpa-onnx
- Parakeet Unified EN 0.6B Q8: high-accuracy English transducer weights are
  distributed via the sherpa-onnx export
  `csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-1120ms`.
  TDT downloads `encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`,
  and `tokens.txt` into `%LOCALAPPDATA%\TDT\models\parakeet-unified-en-0.6b-q8\`
  when you choose Parakeet Q8 in Settings.
- Moonshine Medium Streaming: this lightweight English package uses the
  upstream Moonshine base-en int8 export
  (`csukuangfj/sherpa-onnx-moonshine-base-en-int8`) with its native-streaming
  v1 files (`preprocess.onnx`, `encode.int8.onnx`, `uncached_decode.int8.onnx`,
  `cached_decode.int8.onnx`, `tokens.txt`). TDT downloads them into
  `%LOCALAPPDATA%\TDT\models\moonshine-medium-streaming\` when you choose
  Moonshine Medium in Settings. Review the upstream model cards before
  redistributing them.
- SenseVoice model: model weights and tokenizer are distributed by the
  FunASR/SenseVoice project via the sherpa-onnx export
  `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`. Review the
  model `LICENSE` and `README.md` before redistributing them. The installer
  no longer bundles a model. Settings can download the full-precision
  `model.onnx` for SenseVoice Full into `%LOCALAPPDATA%\TDT\models\sensevoice\`.
  https://huggingface.co/FunAudioLLM/SenseVoiceSmall
- Whisper models: the optional Medium weights are an OpenAI Whisper export
  packaged by sherpa-onnx (`csukuangfj/sherpa-onnx-whisper-medium`). TDT
  downloads them into `%LOCALAPPDATA%\TDT\models\whisper-medium\` when you
  choose them in Settings. Review the upstream model cards before
  redistributing them.
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

The downloader keeps the SenseVoice license beside the downloaded files at
`models/sensevoice/LICENSE`.
