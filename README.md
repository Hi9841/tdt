# TDT (Talk Don't Type)


https://github.com/user-attachments/assets/8bb3021b-f878-4d41-9b1d-32040b7e1897


TDT is an offline speech-to-text app for Windows. Audio is
processed locally with Sherpa-ONNX and recognized text is copied to
the system clipboard. The default model is Parakeet INT8. Settings can
switch between Parakeet INT8 and Parakeet Q8. Both are FluidAudio's English
Parakeet Unified 0.6B, using the same 8-bit streaming weights. Extra models
download on demand and stay on disk. The selected model loads and warms up in the
background when TDT starts, then stays in memory so recordings skip model load time.

License: Apache-2.0. Source: https://github.com/Hi9841/tdt

## Install (Windows)

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1
.\dist\TDT-Setup.exe
```

That writes:

- `dist/TDT.exe` slim app binary used by in-app updates
- `dist/TDT-Setup.exe` per-user installer (app only, no speech model)
- `dist/TDT-0.2.12-windows-x64.zip` portable copy (app only, no speech model)
- `dist/SHA256SUMS.txt`

The installer copies TDT and the license files into `%LOCALAPPDATA%\TDT`,
adds a Start Menu shortcut, and opens the app. Speech models are not bundled.
Open Settings and download Parakeet INT8 on first run, or keep a model you already
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

The floating bubble stays on screen while TDT runs and shows recording state
directly. The tray icon mirrors that state, so hovering it gives status or the
latest transcript, and History holds the full text. Windows' animation setting
is respected.

Click the tray icon to open settings. Its menu offers **Stop recording**,
**Open settings**, and **Quit TDT**. If Windows puts TDT under the tray arrow,
drag its icon onto the visible taskbar area. Windows controls which tray icons
remain visible.

In Settings, pick Parakeet INT8 or Parakeet Q8. Missing models show Download model.
Both chips use the same FluidAudio Unified package, about 632 MB. To fetch it:

```powershell
powershell -ExecutionPolicy Bypass -File .\models\download-models.ps1 -Model parakeet-unified-en-0.6b-int8
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

## Repository layout

- `desktop/`: Rust + GPUI Windows app with global `Ctrl + ;` tap or hold-to-talk.
- `models/`: downloader for the FluidAudio Parakeet Unified INT8/Q8 package.
- `packaging/`: Windows installer stub and build script.
- `THIRD_PARTY_NOTICES.md`: upstream license and attribution information.

## Privacy and limits

Recording and transcription stay on-device. The optional updater talks to
GitHub only. Text is copied to the clipboard by design. Recordings are
capped at 120 seconds to prevent unbounded memory growth.

## License

The application source is Apache-2.0. Model weights and bundled dependencies
have their own terms; read `THIRD_PARTY_NOTICES.md` before redistributing them.
