# TDT (Talk Don't Type)

TDT is an offline speech-to-text app for Windows and Android. Audio is
processed locally with Sherpa-ONNX and recognized text is copied to
the system clipboard. The default model is SenseVoice Small. Settings can
switch to SenseVoice Full, Whisper Small, or Whisper Medium. Extra models
download on demand and stay on disk. The selected model is loaded while you
talk and released after each transcription, so idle memory stays low.

License: Apache-2.0. Source: https://github.com/Hi9841/tdt

## Install (Windows)

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1
.\dist\TDT-Setup.exe
```

That writes:

- `dist/TDT.exe` slim app binary used by in-app updates
- `dist/TDT-Setup.exe` per-user installer (app only, no speech model)
- `dist/TDT-0.1.7-windows-x64.zip` portable copy (app only, no speech model)
- `dist/SHA256SUMS.txt`

The installer copies TDT and the license files into `%LOCALAPPDATA%\TDT`,
adds a Start Menu shortcut, and opens the app. SenseVoice is not bundled.
Open Settings and download Small on first run, or keep a model you already
have. No admin rights. Uninstall from Settings > Apps, or:

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\TDT\uninstall-tdt.ps1" -Uninstall
```

## Desktop (dev)

```powershell
powershell -ExecutionPolicy Bypass -File .\models\download-models.ps1
cargo build --release --manifest-path .\desktop\Cargo.toml
.\run-desktop.bat
```

The first short hotkey tap starts recording and the second stops it. Holding
the hotkey records until release. Default shortcut is `Ctrl+;`. Change it in
Settings by clicking the shortcut chip, then press the new combo. The tray
menu controls auto-paste. Auto-paste writes the result to the clipboard and
injects Unicode text directly into the previously focused application.

In Settings, pick Small, Full, Whisper, or Medium. Missing models show
Download model. Whisper Medium is about 902 MB. To fetch one from a
terminal:

```powershell
powershell -ExecutionPolicy Bypass -File .\models\download-models.ps1 -Model whisper-medium
```

```powershell
cargo fmt --manifest-path .\desktop\Cargo.toml -- --check
cargo test --manifest-path .\desktop\Cargo.toml
cargo clippy --manifest-path .\desktop\Cargo.toml --all-targets --all-features -- -D warnings
```

## Updates

GitHub Releases is the update source (`Hi9841/tdt` by default, or
`TDT_GITHUB_REPO`). Checking happens in the background after launch, never as
part of recording. Installing an update downloads `TDT.exe` (the app only)
and replaces the running binary, the same way Prism updates. The speech
model is not re-downloaded. Older releases fall back to `TDT-Setup.exe`.
Set `TDT_DISABLE_UPDATES=1` to turn that off.

Publish a release tagged `v0.1.0` (or later) so the in-app checker can find
it. The `Release` workflow publishes `TDT.exe`, `TDT-Setup.exe`, and the
portable zip on tag `v*`.

## Android

The Android build expects JDK 21, Android SDK 35, and the Sherpa-ONNX AAR in
`mobile/app/libs/`. Download SenseVoice first so the Gradle asset task can copy
`models/sensevoice/model.int8.onnx` and `tokens.txt` into the APK:

```powershell
powershell -ExecutionPolicy Bypass -File .\models\download-models.ps1
cd .\mobile
.\gradlew.bat :app:assembleDebug
```

Install `app/build/outputs/apk/debug/app-debug.apk` with `adb install -r`.
Grant microphone, notification, and overlay permissions when prompted.

## Repository layout

- `desktop/`: Rust + GPUI Windows app with global `Ctrl + ;` tap or hold-to-talk.
- `mobile/`: Kotlin Multiplatform Android app with push-to-talk and an optional
  floating overlay bubble.
- `models/`: downloader for SenseVoice and Whisper models used by desktop.
  Android still bundles SenseVoice Small.
- `packaging/`: Windows installer stub and build script.
- `THIRD_PARTY_NOTICES.md`: upstream license and attribution information.

## Privacy and limits

Recording and transcription stay on-device. The optional updater talks to
GitHub only. Text is copied to the clipboard by design. Desktop and Android
recordings are capped at 120 seconds to prevent unbounded memory growth.

## License

The application source is Apache-2.0. Model weights and bundled dependencies
have their own terms; read `THIRD_PARTY_NOTICES.md` before redistributing them.
