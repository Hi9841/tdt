# TDT (Talk Don't Type)


https://github.com/user-attachments/assets/8bb3021b-f878-4d41-9b1d-32040b7e1897


TDT is an offline speech-to-text app for Windows. TDT's own speech engine,
written in Rust, transcribes the audio on your computer, and TDT copies the
recognized text to the clipboard. The default model is Parakeet Redux, a
178 MB model with 1.58-bit ternary weights. The model downloads on demand and
stays on disk. When TDT starts, the model loads in the background in about
0.2 seconds. Then it stays in memory, so a recording does not wait for it.

License: Apache-2.0. Source: https://github.com/Hi9841/tdt

## Install (Windows)

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1
.\dist\TDT-Setup.exe
```

That writes:

- `dist/TDT.exe` slim app binary used by in-app updates
- `dist/TDT-Setup.exe` per-user installer (app only, no speech model)
- `dist/TDT-0.2.26-windows-x64.zip` portable copy (app only, no speech model)
- `dist/SHA256SUMS.txt`

The installer copies TDT and the license files into `%LOCALAPPDATA%\TDT`,
adds a Start Menu shortcut, and opens the app. Speech models are not bundled.
Open Settings and download Parakeet Redux on first run, or keep a model you already
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

TDT removes filler sounds such as "um", "uh", "erm", and "hmm" before it
delivers the text, together with the commas around them. Words that can carry
meaning, such as "like" or "you know", stay. Turn this off in Settings with
**Remove filler words**.

The floating bubble stays on screen while TDT runs and shows recording state
directly. The tray icon mirrors that state, so hovering it gives status or the
latest transcript, and History holds the full text. Windows' animation setting
is respected.

Click the tray icon to open settings. Its menu offers **Stop recording**,
**Open settings**, and **Quit TDT**. If Windows puts TDT under the tray arrow,
drag its icon onto the visible taskbar area. Windows controls which tray icons
remain visible.

In Settings, Parakeet Redux is the default model. If missing, it shows Download model (178 MB).
To fetch the model directly via PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -File .\models\download-models.ps1
```

To run the speech benchmark (WER and speed tables: see [Benchmarks](#benchmarks)):

```powershell
cargo run --release --bin bench-asr --manifest-path .\desktop\Cargo.toml
```

```powershell
cargo fmt --manifest-path .\desktop\Cargo.toml -- --check
cargo test --manifest-path .\desktop\Cargo.toml
cargo clippy --manifest-path .\desktop\Cargo.toml --all-targets --all-features -- -D warnings
```

## Dictionary

TDT spells names and technical terms better when they are in its dictionary.
A list of about 130 developer terms (for example Kubernetes, PostgreSQL, and
GitHub) is always active. To add your own words, open Settings, find
Dictionary, and select Edit. TDT opens `%LOCALAPPDATA%\TDT\dictionary.txt`.
Write one word or name per line, the way you want it spelled, and save the
file. The next dictation uses it.

TDT changes a word only when the speech supports it. On the test sentences
with developer terms, the dictionary removes about a third of the errors. On
400 recordings of ordinary speech (LibriSpeech), it changes no transcript.

## Benchmarks

Measured on 2026-10-20 with the Parakeet Redux model on an AMD Ryzen 9
7950X3D (32 GB RAM, Windows 11). The four engines got the same 456 clips:

- **Photon**: the Python worker. TDT now uses it only as a fallback.
- **v0.2.25**: the first release with TDT's own Rust engine.
- **v0.2.26**: the Rust engine in this release.
- **v0.2.26 + dictionary**: the same engine with the built-in developer terms.
  This is what the app does by default.

**Word error rate.** Lower is better. The error count is in brackets.

| Set | Clips | Photon | v0.2.25 | v0.2.26 | v0.2.26 + dictionary |
|---|---:|---:|---:|---:|---:|
| LibriSpeech test-clean | 200 | 2.07 % (83) | 2.02 % (81) | 2.02 % (81) | 2.02 % (81) |
| LibriSpeech test-other | 200 | 4.64 % (177) | 4.59 % (175) | 4.59 % (175) | 4.59 % (175) |
| Developer terms | 40 | 7.19 % (34) | 7.19 % (34) | 7.19 % (34) | **4.65 % (22)** |
| Everyday dictation | 16 | 1.55 % (6) | 1.55 % (6) | 1.55 % (6) | **1.04 % (4)** |

**Speed.** The number is audio time divided by transcription time, so 90×
means that 1 minute of speech takes less than 1 second. Higher is better. The
median time per clip is in brackets.

| Set | Photon | v0.2.25 | v0.2.26 | v0.2.26 + dictionary |
|---|---:|---:|---:|---:|
| LibriSpeech test-clean | 36× (168 ms) | 86× (68 ms) | 84× (69 ms) | 78× (74 ms) |
| LibriSpeech test-other | 36× (159 ms) | 89× (62 ms) | 96× (56 ms) | 99× (54 ms) |
| Developer terms | 32× (127 ms) | 80× (52 ms) | 93× (43 ms) | 91× (44 ms) |
| Everyday dictation | 38× (142 ms) | 88× (59 ms) | 86× (64 ms) | 98× (54 ms) |

- Each value comes from one run. The speed changes by about 10 % from one run
  to the next, so the three Rust columns have the same speed. The Rust engine
  is about 2.5 times as fast as Photon.
- The developer-term and everyday sets are synthetic speech from the Windows
  voices. LibriSpeech is recorded human speech.
- The bench gives each whole clip to the engine. The app cuts long
  recordings at pauses, so text comes sooner. On LibriSpeech test-other this
  gives 184 errors instead of 175.

The method, the raw output, and the commands that reproduce these tables are
in [desktop/bench](desktop/bench/README.md).

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

Recording and transcription stay on-device. The speech engine is part of
TDT and needs no Python and no network. TDT uses the network only for these
operations:

- The model download that you start in Settings.
- The optional updater. The updater connects only to GitHub.

If TDT's engine cannot load a model, TDT uses the Moondream Photon worker
instead. That worker installs its runtime with uv on first use, and TDT blocks
the runtime's usage reports and Hugging Face lookups.

TDT copies the text to the clipboard by design. The maximum recording length
is 120 seconds. This limit stops unbounded memory growth.

## License

The application source is Apache-2.0. Model weights and bundled dependencies
have their own terms. Read `THIRD_PARTY_NOTICES.md` before you redistribute them.
