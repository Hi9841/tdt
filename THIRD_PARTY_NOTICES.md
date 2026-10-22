# Third-party notices

TDT bundles or downloads components maintained by other projects. Their
licenses remain applicable to those components.

- Moondream Parakeet Redux: 1.58-bit ternary transducer weights from
  https://huggingface.co/moondream/parakeet-redux (CC-BY-4.0).
  Downloaded on demand into `%LOCALAPPDATA%\TDT\models\parakeet-redux\`.
- TDT speech engine: TDT's own Rust implementation of the Parakeet TDT
  network. Its structure follows the Hugging Face Transformers Parakeet
  implementation (Apache-2.0, https://github.com/huggingface/transformers).
- SCOWL (Spell Checker Oriented Word Lists), size 70, as packaged by Debian
  in `wamerican-large`: the English word list in
  `trit/src/text/english-words.txt`. Copyright 2000-2011 by Kevin Atkinson.
  Permission to use, copy, modify, distribute and sell these word lists, the
  associated scripts, the output created from the scripts, and its
  documentation for any purpose is hereby granted without fee, provided that
  the above copyright notice appears in all copies and that both that
  copyright notice and this permission notice appear in supporting
  documentation. Kevin Atkinson makes no representations about the
  suitability of this array for any purpose. It is provided "as is" without
  express or implied warranty. http://wordlist.aspell.net/
- wordfreq (https://github.com/rspeer/wordfreq): its word frequencies chose
  the tokenizer pieces in `trit/src/text/foreign-pieces.txt`. The file holds
  pieces of the Parakeet v3 tokenizer, not wordfreq data.
- GPUI, CPAL, and Rust crates: each crate's license is recorded in Cargo's
  lockfile and upstream package metadata. See `desktop/Cargo.toml` and
  https://crates.io/

Older components (Photon, Sherpa-ONNX, Parakeet Unified 0.6B, Moonshine, SenseVoice, Whisper)
are no longer used or offered in Settings. Files already on disk are left in place.
