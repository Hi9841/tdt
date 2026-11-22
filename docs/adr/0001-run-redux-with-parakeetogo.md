# 0001: Run Parakeet Redux with parakeetogo, not Photon

TDT Android runs the same Parakeet Redux weights as TDT desktop, but through parakeetogo (Apache-2.0 C++) with the NDK and JNI, not through Moondream Photon.

Desktop uses Photon in a Python worker. On Android, Photon needs a Python runtime, PyTorch, and the proprietary `kestrel-kernels` package. That package ships only Windows, macOS and Linux glibc wheels, and an APK that contains it distributes it, which needs a written agreement with M87 Labs. parakeetogo loads the unmodified `model.safetensors` and `tokenizer.json`, has a small C API, and has an ARM NEON kernel.

The engine sits behind one Kotlin `SpeechEngine` interface. If Moondream later licenses an Android Photon build, it can replace parakeetogo without UI changes. The cost: parakeetogo is untuned and unproven on Android, and its transcripts differ from Photon's by about 0.23% of words.
