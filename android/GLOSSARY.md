# TDT Android

Language for the TDT speech-to-text app on Android.

## Language

### Bubble

The small floating button that slides out beside the user's own keyboard when a text field has focus. The user taps or holds it to dictate. Avoid "keyboard" or "IME": TDT does not replace the user's keyboard.

### Offline promise

Speech audio and transcripts never leave the phone. The network is used only for the model download and update checks.

### Take

One dictation: the audio from the start of the recording to its text. The model reads a take while the user talks, so the text is ready soon after the stop. Avoid "recording" or "clip" for the same thing.

### Model

The speech model the user picked: Parakeet Redux (moondream/parakeet-redux, the default) or Parakeet Unified English, the same weights as TDT desktop, or Voz (Desert Ant Labs). Each model is downloaded once; TDT checks Redux by SHA-256, and the Voz SDK checks its own files. Avoid naming the runtime as the model: Redux runs on Trit, Voz on LiteRT.

### Dictionary

Names and terms the user adds, one per line. Redux and Unified use them. TDT ships no terms of its own.
