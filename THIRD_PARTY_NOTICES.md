# Third-party notices

## Parakeet Redux (speech model)

- Source: https://huggingface.co/moondream/parakeet-redux
- Authors: Moondream (M87 Labs), based on NVIDIA Parakeet TDT 0.6B v3.
- License: CC-BY-4.0. TDT downloads the unmodified weights on first run and does not redistribute them.

## Trit (speech engine)

- Source: https://github.com/Hi9841/trit
- License: AGPL-3.0-only, or a paid commercial licence (from Trit 0.13.0; see Trit's `LICENSING.md`). Built from source into `libtdt_engine.so`, with mimalloc (MIT) and the Rust crates that Trit's `THIRD_PARTY_NOTICES.md` lists.

## Voz (speech model and SDK)

- Source: https://github.com/Desert-Ant-Labs/desert-ant-core (`ai.desertant:voz`), weights at https://huggingface.co/desert-ant-labs/voz
- Authors: Desert Ant Labs.
- License: Desert Ant Labs Source-Available License (https://license.desertant.com/1.0). Free for most apps; at scale it needs a commercial license (licensing@desertant.com). The SDK downloads the weights when the user picks Voz.
- The SDK brings LiteRT (Apache-2.0) as `libLiteRt.so` and `libLiteRtClGlAccelerator.so`.
