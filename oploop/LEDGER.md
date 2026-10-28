# OpLoop ledger: TDT app dictation path

The engine ledger is in the Trit repository (`oploop/LEDGER.md`). This file keeps
the iterations on the app side (`desktop/`).

## Session 2026-10-28 (ux)

Branch `opt/ux` from `origin/main` (0.4.16, Trit tag `v0.3.1`). The Trit line in
`desktop/Cargo.toml` did not change, so `bench-asr` measures the same engine code on
both sides. The changes are in the TDT binary (`main.rs`, `hotkey/hook.rs`), which
`bench-asr` does not run. Their numbers come from the code: the poll intervals and
the order of the calls. The GUI was not started.

### Correctness floor

| Check | Command (from `desktop/`) | Result at `fa3b13b` |
|---|---|---|
| Lint | `cargo clippy --all-targets --release -- -D warnings`, `cargo fmt --check` | clean |
| TDT ratchet | `uv run bench/accuracy.py <bench-asr.exe> clean other tech tts16 --engine native` | 81/172/34/6 (budget 81/172/34), also at `00c519b` |

### Audit of the dictation path (code reading)

| Point | Finding |
|---|---|
| Press to capture | The UI loop read the hotkey channel only at its poll: 75 ms idle, 25 ms while recording. A press waited 0 to 75 ms (mean 37 ms) before `resume_input` and `start`. The microphone is parked while idle, so no audio exists before `start`; the wait is lost speech at the first word. |
| Release to tail | The release waited 0 to 25 ms (mean 12 ms) for the poll before `mark_release`. `latency_ms` starts at `mark_release`, so this wait was in the paste time but not in the number. |
| Tail | The pump drains until 60 ms of quiet across the release, or 600 ms. `stop` takes the rest of the buffer, so a halt loses no captured audio. |
| Pause cuts | A cut is the middle of the first 500 ms quiet run after 3 s. Parts join at the same sample, so no audio is lost between parts. The remainder after a cut is at least 250 ms by construction, so `MIN_REMAINDER_SAMPLES` (200 ms) never applies. |
| 120 s limit | The recorder stops at 120 s, the pump breaks, `stop` sends the rest, and the take is transcribed with a notice. No loss. |
| Microphone failure | A stream error during a take, or while it was transcribing, discarded the whole take. |
| Idle release | `release_memory` 5 s after a take costs about 1 ms on the next take (Trit P-4: 12.8 against 11.5 ms on 1.6 s). Kept: it returns the take's memory. |
| Text | `join_parts` and `remove_fillers` were read on their edge cases; no defect found. English is `Options::language("en")` in every engine call, the quiet-speech boost included. |

### U-001 wake the UI loop on a shortcut press or release

- Hypothesis: the hotkey poll costs 0 to 75 ms (mean 37 ms) of audio at the start of
  each take and 0 to 25 ms (mean 12 ms) of paste time at each release. An async hotkey
  channel that the loop awaits, with the poll interval as the timeout, makes both
  close to 0. The press also starts the capture before it reads the target window
  and closes the panel.
- Test: the loop still wakes at the poll interval for the tray, the pill and the
  engine events; a failed hook or the preview has no channel and waits on the timer
  as before. `async-channel` 2.5 was already in `Cargo.lock` (through GPUI), so no new
  crate is downloaded.
- Result: not measured with the GUI. `bench-asr.exe` built at `fa3b13b` and at
  `00c519b` is the same file (SHA-256 `22333f69377fb22c...`), so the engine path,
  its texts, and its realtime latency are unchanged by construction.
- Decision: KEEP (commit `9860f31`).

### U-002 keep the take when the microphone fails during it

- Hypothesis: a stream error (a USB or Bluetooth microphone that goes away) during a
  take loses every word of the take, because the UI loop set `discard`. The pump has
  already sent the audio before the error to the engine, and `stop` takes the rest of
  the buffer. Stopping the take as at a release keeps that text.
- Change: on a stream error the recorder is dropped (the next press opens the
  microphone again, as before). A take that records stops; a take that transcribes
  goes on. The text is delivered, and the error then shows as the notice of the pill
  (as an error when the take had no speech).
- Result: code reading only; a device failure cannot be made without the GUI.
- Decision: KEEP (commit `00c519b`).

### Not changed (proposals)

- `stt/engine.rs` `takes::save` runs before `latency_ms` and before delivery when
  "save takes" is on: about 0.7 ms for a 10 s take (file write, measured in Python).
- The pump end and the engine result still wait for the 25 ms poll: the stop chime,
  the Transcribing pill, and the success chime come up to 25 ms late. The paste is
  not late.
- A press while the last take still transcribes is dropped with no signal.
- Two dictations into one field touch ("first.Second"): no space is typed between
  them. A change alters the typed text, so it needs the owner's decision.
- A short hangover of the open microphone after a take (a few seconds before
  `pause_input`) would remove the device start from a quick next press. It keeps
  the Windows microphone indicator on longer: the owner's decision.
