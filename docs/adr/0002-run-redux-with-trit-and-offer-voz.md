# 0002: Run Redux with Trit, and offer Voz as a second model

Supersedes [0001](0001-run-redux-with-parakeetogo.md).

TDT Android runs Parakeet Redux on Trit, the same engine as TDT desktop, built from source into `libtdt_engine.so` (Rust static library behind a small C++ JNI bridge). The user can pick Voz by Desert Ant Labs instead, through its Android SDK (`ai.desertant:voz`).

## Why Trit, not parakeetogo

- One engine for desktop and phone: a fix or a speed-up in Trit reaches both, and both give the same text.
- Trit now has arm64 paths (2026-10-29): the ternary product with `sdot` (Armv8.2 dot product) or plain NEON on older cores, NEON f32 tiles, NEON activations, and `fcvtl` for the f16 decoder. parakeetogo needed the dot product and called `exit()` on other CPUs; Trit runs on every arm64 phone.
- On Android, Trit's team runs only on the fast cores (at least half the capacity of the fastest core) and pins to them.
- Trit reads Redux's `config.json`, `tokenizer.json` and `model.safetensors` unmodified, and takes the user's dictionary.

## Why Voz is an option, not the default

- Its Android files are 1.25 GB (`encoder.tflite` alone is 1,207.5 MB); the 337 MB in its docs is the Core ML build for Apple devices.
- On the 6 GB emulator, Voz peaked at 3.9 GB resident memory and loaded in 10 s; on the 4 GB emulator Android killed the app for low memory. Redux on Trit peaked at 0.56 GB and loaded in 0.3 s.
- Voz speaks 25 languages, but its Kotlin SDK takes no language and no vocabulary, so it cannot be held to English and does not use the dictionary.
- The app offers Voz only on phones with about 6 GB of memory or more.

## Costs

- Building the app needs Rust (stable) with the `aarch64-linux-android` and `x86_64-linux-android` targets, and a Trit checkout.
- Trit is PolyForm Noncommercial and Voz is source-available with a commercial license at scale. Check both before a commercial release.
- Phone speed is not measured yet: the emulator runs x86. The arm64 text was checked under QEMU (`-cpu max`, which has the dot product) with a static Linux build of Trit: on 12 clips the `sdot`, NEON and scalar arm64 paths gave the same text as Trit's x86 AVX2 and scalar paths; on 40 clips the `sdot` path matched x86 AVX2 on 39. The 40th is a clip the model hears as Italian, where every arm64 path, scalar included, rounds to other words than every x86 path.
