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

## Session 2026-11-01 (ultra)

Branch `feat/parakeet-ultra` from `origin/main` (0.5.0, Trit tag `v0.4.0`). Goal: TDT is
the Trit CLI with a GUI. RAM is the model, the time of a take is the CLI time, and the
new model (Parakeet Ultra) runs well on Trit. Machine: Ryzen 9 7950X3D, RTX 4090,
Windows 11. Scripts (not in the repo): `%TEMP%\ultra-scripts\` (`cli_bench.py`,
`ab_engine.py`, `tdt-mem.ps1`, `floor\`, the engine-only probe).

### Journeys

| ID | Journey | Harness |
|---|---|---|
| J1 | Engine time of a take: TDT against the CLI | `ab_engine.py 10 <takes> <redux> cli=cli:trit.exe thin=tdt:<bench-asr> fat=tdt:<bench-asr>` (same whole clips, CPU, en) |
| J2 | Words of the default model | `bench/accuracy.py <bench-asr> clean other tech` (budget 81/172/34) |
| J3 | Idle RAM of the app with a loaded model | `tdt-mem.ps1` (full app from `%TEMP%`, 30 s idle) against `floor.exe <model> <clip> cpu` (engine only) |

### Floor

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --all-targets --release -- -D warnings` | clean, before and after |
| J2 ratchet | 81/172/34, before and after |

### Parakeet Ultra on Trit (CLI, CPU, 456 clips, no dictionary, whole clips)

| Model | Word errors | Median ms (clean) | Peak RAM (clean set) | Disk |
|---|---:|---:|---:|---:|
| parakeet-redux | 356 | 32.5 | 549 MB | 178 MB |
| parakeet-tdt-0.6b-v3 | 262 | 36.4 | 1044 MB | 2.5 GB |
| parakeet-ultra | 260 | 36.8 | 1043 MB | 1.26 GB |

Ultra loads with no engine change (its `vad_head` is the speech head Redux has too).
On the GPU it is slower than on the CPU (101 against 59 ms a take) and used three
times the memory, so TDT keeps it on the CPU (`ModelSpec::gpu`).

### J3 RAM: app against engine only (idle, model loaded, CPU)

| Model | Engine only (private) | TDT (private) | TDT (private working set) | Extra of the app |
|---|---:|---:|---:|---:|
| Redux | 496 MB | 505 MB | 335 to 343 MB | +9 MB |
| Ultra | 900 MB | 909 MB | 815 MB | +9 MB |

The GUI, tray, and microphone cost about 9 MB private. A fresh TDT loads only
`d3d11.dll` and `dcomp.dll`. The running TDT of the owner had 697 MB private and
`vulkan-1.dll`, `d3d12.dll`: Use GPU had been on in that session. The engine-only
probe shows the cause (load on GPU, drop, release, load on CPU): 1177 MB private
against 496 MB. Fixed in Trit (`gpu::trim` in `release_memory`, `MemoryHints::MemoryUsage`,
flushed weight uploads): after the drop 390 MB, Redux on the GPU 929 MB (was 1277),
Ultra GPU load peak 1071 MB working set (was 2143). The rest is the wgpu device and
the driver; it stays until the process ends.

### U-003 fat LTO, as the Trit CLI is built

- Hypothesis: TDT builds the engine with `lto = "thin"`, the CLI with `lto = "fat"`.
  Thin LTO costs a few percent of a take.
- Result (J1, 12 interleaved rounds on a quiet machine, 46 takes): CLI 3107 ms,
  thin 3129 ms (1.007, p = 0.0002), fat 3129 ms (1.007, p = 0.0005). No speed change.
  `TDT.exe` 25,352,704 to 23,920,128 bytes (-5.6 %), deterministic.
- Decision: KEEP for the size. Same profile as Trit.

### J1 text and time against the CLI

- Same text: on the 46 takes, 15 texts differed between the CLI and `bench-asr --native
  --whole`. Cause (proved with the Python binding on one engine): `bench-asr` reads PCM16
  as `/ 32767`, `trit::read_wav` as `/ 32768`. With the same samples, 0 of 46 differ.
  The app itself gets f32 from the microphone, so this is a harness difference only.
  `bench-asr` keeps `/ 32767` (as `bench/runtimes.py` of Trit): with `/ 32768` the
  ratchet moves to 81/177/33 by noise alone (other over its budget of 172).
- Engine sensitivity: a gain change of 0.003 % changes 15 of 46 Redux texts and 5 errors
  on `other`. Redux quantizes activations to i8, so the rounding grid moves with the gain.
- Time: TDT 0.5 to 0.6 % slower than the CLI with the same texts (about 0.45 ms a take).
  H2 (threads on the other CCD of the 7950X3D): rejected, both pinned to CCD0 gave
  1.006 (p < 0.0001). H3 (`cc` 1.6.0 against 1.9.0 for mimalloc): 1.005, no change,
  reverted. Stopped after three tries: cause not found, below a user's perception.
- App path (realtime, pause cuts), 46 takes: Redux 128 errors, p50 64 ms after release;
  Ultra 79 errors, p50 60 ms. Tech: Redux 44, Ultra 28.

### Not changed (proposals)

- `config.rs` `load_json`: a `config.json` with a byte order mark fails to parse, and
  TDT runs with defaults (seen when a PowerShell 5.1 script wrote it). The dictionary
  had the same defect (fixed in 0.4.18).
- Trit keeps the CPU copy of weights that are on the GPU (short takes, the decoder,
  and the GPU fallback read it): about 80 MB working set above the CPU on Redux.
- Parakeet Ultra is not on the Trit website yet (owner: investigate first). Trit int8
  200 errors, Trit f16/f32 198, Transformers fp32 197 (`bench/runtimes.py`, 3 rounds),
  so `wer_rule.py` would fail. Raw results in Trit `target/runtimes/results.jsonl`.

### U-004 no GPU in TDT (owner's decision)

- The Use GPU option, its config field, and the GPU load path are removed; Trit is
  built without `gpu`. `TDT.exe` 23,920,128 -> 18,489,856 bytes, `bench-asr.exe`
  7,556,096 -> 2,063,872. Ratchet on 0.6.0 (Trit v0.4.1): 81/172/34.

## Session 2026-11-04 to 05 (stable latency, quality first)

Branch `opt/stability` from `origin/main` (0.8.0). Owner report: the wait changes by 5
times between takes, and by 4 times under load. Owner rule: quality is the hard gate, and
speed gives way, on every CPU.

- Cause, take length: on 18 takes matched to the log, the model part is 6 to 8 ms per
  second of speech. The pause cut fired on 2 of 143 real takes: it asked for a pause 25 dB
  under speech, and the owner's microphone has a median of 22.8 dB to the room noise.
  Same audio on a quiet PC stays within about 20 %, and 15 s of idle adds no clear cost.
- Gate: 400 long takes (`long-clean`, `long-other`: LibriSpeech utterances joined in reading
  order, 20 to 89 s, 5.6 h, true text), scored with the normalizer of `accuracy.py`, paired
  per take with a 95 % bootstrap range. The 400 clips of `clean` and `other` hid every loss
  below: they have few joins. `bench-asr` errors count "gray/grey" and "mr/mister".
- Rejected, against one run of the whole take (451 + 777 errors): noise-aware pause rule
  (59 of 143 takes changed, soft word endings look like pauses); 16 s windows with 4 s
  shared +50 / +56; windows sized by CPU speed (text would depend on the CPU, 2 threads
  +13 % on the old set); the old pause cut +31 / +88 (on Trit 0.6.0, so not a clean compare);
  a rescue of empty decodes (see the Trit ledger).
- Kept: 30 s windows with 8 s shared, the same on every CPU, +12 / +3, inside the noise.
  `merge` lines up the end of a text with the start of the next, and joins them in the middle
  of the shared words. Under 8 shared words, or one window with no text, the take runs once
  whole after the release. At 36 s that path fired on the 2 takes that lost 40+ words: +4,
  and no run lost. `pause_cut`, `join_parts`, and the bridge are gone.
- Text, final build: `clean` 66, `other` 117, the same as no windows; `tech` and `tts16` the
  same; 7 of 143 owner takes changed, 321 errors against their saved text (323 no windows).
  2 threads give the same totals as 8. Redux long set: old 678 + 1301, new 663 + 1155.
  50 no-speech clips stay empty on both models. Ratchet 81/172/34.
- Wait, p50 ms, 2 / 8 / 20 / 46 s takes, final build, 3 rounds. Quiet 22 / 62 / 132 / 162, CCD0
  fully loaded 20 / 61 / 145 / 213, 4 threads 25 / 94 / 225 / 251, 2 threads 47 / 165 / 374 / 525,
  4 threads on AVX2 61 / 212 / 513 / 580. Before (4 rounds): quiet 21 / 62 / 157 / 371, loaded
  26 / 94 / 215 / 589. A take up to 30 s waits as long as its length needs: that is the cost
  of the quality rule.
- Also: the log line gives the take length, the last part, the characters, and the delivery
  split. The hook, pump, and engine threads run above normal priority (not measured).
- `bench-asr --gap` waits before each clip; its summary counts `whole_runs`.
- Slow CPUs are stand-ins: fewer threads and `TRIT_ISA=avx2` on the 7950X3D. No real laptop,
  hybrid CPU, or all-core load was measured. The app itself did not run.

## Session 2026-11-05 (OpLoop: the wait after the stop, quality gate first)

Baseline `8e5274d`. Journey: model part of the wait on 36 of the owner's real takes (every
4th by length, 0.6 to 45 s, real endings), `bench-asr --dir <takes> --whole --realtime`.
An A/A run repeats within 0.1 ms on the mean and 1 ms at p90.
Gate: the long set of the session above, `clean`, `other`, `tech`, `tts16`, the 143 takes, 50
no-speech clips, the ratchet.

### H1 read the take in the quiet before the release: KEPT (`f465b84`)

- Hypothesis: the median take ends in 375 ms of quiet before the key comes up, and the model
  starts only after it. Reading after 200 ms of quiet should make the wait about 0 for most
  takes. If speech resumes, the read is dropped.
- First try lost words: "[hello hello hello hello hello no] -> []". In a short take with no
  pause the 5th percentile is soft speech. The room noise now also takes the first 80 ms.
- Result, quiet: mean 52 to 16 ms, p50 35 to 14 ms, p90 127 to 66 ms, max 158 to 90 ms. Loaded:
  mean 50 to 17 ms. Gate: long set -1 / +3 against the build before, takes 317.
- Cost: 2.7 reads per take, 3.8 times the speech of one run, while the user talks. A limit on
  reads (a quarter of the take apart) lost too many hits (0.45 to 0.32): not added.

### H2 read takes from 0.6 s, not 1.5 s: KEPT (`610fb1b`)

- Result: p50 13 to 0 ms, mean 14.9 to 13.5 ms, 17 to 20 of 36 takes ready. Gate unchanged.

### H3 read after 150 ms of quiet, not 200 ms: REVERTED

- Result: 21 of 36 takes ready, mean inside the noise, takes 317 to 321 errors, one take at
  125 ms. No gain, and a worse text.

### Final, `610fb1b`

| Setup | Before: mean, p90, max | Now: mean, p90, max |
|---|---|---|
| Quiet | 53, 136, 166 | 16, 66, 103 |
| CCD0 fully loaded | 50, 124, 163 | 23, 97, 132 |
| 2 threads | 160, 423, 524 | 77, 237, 336 |
| 4 threads, AVX2 | 200, 510, 700 | 107, 338, 464 |

- Text: long set +11 / +6 against one whole run (inside the noise), the same 3 runs of 4 or
  more words as before the loop. 2 threads give the same totals. 36 takes at speaking pace:
  0 texts differ under load and on 2 threads, 1 differs on the AVX2 path.
- Redux long set 661 + 1156 (old 678 + 1301). No-speech clips stay empty. Ratchet 81/172/34.
- The loaded run overlapped about 1 minute of a scoring script. A real take can end sooner
  than its saved audio: the app was not run.

### Next hypotheses

1. Delivery: 20 to 30 % of the wait at p50 on long texts. The new log line gives copy, focus,
   and typing per take; typing after a copy off the critical path needs that data first.
2. Reads of mid-take pauses are 3.8 times the work of one run. Skipping a read when the
   last read finished less than a fixed amount of speech ago could cut it without a lost hit.
3. Trit: greedy decoding that can return nothing (Trit ledger) needs a fix that does not
   invent words from noise.

## Session 2026-11-05 (review fixes, TDT 0.9.0)

Baseline `30cbc23` (Trit `2352bce`). The change: Trit 0.9.0 (`178b4cc`), the read in the quiet
on its own thread with a `trit::Cancel`, and an engine panic that fails one take.

- Quiet read: the capture loop reads new audio during the read. New speech or a full window
  stops a read that is old. Each end of speech gets one read at the same audio as before, so
  the text does not depend on the CPU.
- Engine panic: each engine call has a `catch_unwind`. The panic hook writes the log and
  returns for a panic on the thread of the call or on a `trit-speech-` thread during a call.
  The take ends with an error on the pill, and the app stays open.

Gate, `bench-asr` of the baseline against the change, app path, the owner's dictionary on,
through `safe.cmd`:

- Text, all audio at once: the 145 saved takes 0 differ, `clean` 0 of 200, `other` 0 of 200,
  `tech` 0 of 40, long-clean 0 of 200, long-other 0 of 200.
- At speaking pace (`--whole --realtime`, 46 real takes, one run each): 0 texts differ. Wait:
  mean 25.5 against 26.2 ms, p90 103 against 108 ms, max 166 against 165 ms, 27 takes ready
  at the release in each build. Level.
- Ratchet 81/172/34.
- With all audio at once, the model time falls (takes 28,091 to 8,435 ms, long-clean 207,979
  to 81,602 ms), because an old read stops. This is less work during a take, not a shorter
  wait.

Not verified: the app itself (hook, microphone, paste) did not run, and no panic was forced.

## Session 2026-11-06 (the 16 app findings of the review)

Baseline `efc2b66` (TDT 0.9.0). The review of 2026-11-05 gave 16 findings in the app, from
the source only. This session changes the code for each. No change is in the model path.

Stability:

1. Hooks: Windows removes a low-level hook that answers too slowly, with no notice. The hook
   thread now compares, each 2 s, the time of the last input (`GetLastInputInfo`) with the
   time of the last hook event. Input more than 1 s newer installs both hooks again. An
   administrator window in front is no reason (its input reaches no hook of a normal
   program). For other unseen input (touch, the lock screen) the installs go down to about
   one each minute, with one log line each hour at most.
2. Calls into Explorer and COM left the UI loop: a thread reads the Windows default
   microphone each 2 s (a press compares with that value), and the virtual desktop check has
   the thread that styles the window at the start. The stream is still opened on the UI
   thread, only when the device changed.
3. A press whose key-up did not reach the hook ends when a modifier of the shortcut is up
   (`GetAsyncKeyState`, in the UI loop). A shortcut with no modifier (F9, Mouse 4) has no such
   sign and keeps the 2-minute limit.
4. Keys from other programs can be the shortcut. TDT marks its own key events
   (`dwExtraInfo`), and the hook passes on only those. Typing by the user is still counted
   from keyboard keys only.
5. `SendInput` gets 256 characters for each call, with a pause of 1 ms between calls. A text
   up to 256 characters has no pause.
6. A stream that gives no buffer for 3 s during a take ends the take with a notice. The text
   up to that point is kept.
7. A model download continues its partial file (`Range`), and a try that added bytes does
   not count as a failure. One read waits 60 s at most (it was 2 hours).
8. A damaged `config.json`: a notice on the pill, a line in the log, and the default file
   written again. The copy of the old file is `config.json.bak`.

Speed:

9. The text goes to the app before the copy to the clipboard. The latency in the log and in
   History ends when the text is in the app.
10. A stream that fails at the start of a take, or gives no audio for 3 s, is opened again
    in the same press, one time. The text then has a notice that the first words can be
    missing.
11. The UI loop runs each 250 ms (it was 75 ms) when the pill is at rest and the panel is
    closed. The shortcut, a click on the pill, and a tray click wake it.
12. The release and the stop wake the pump. It waited up to 8 ms for the next buffer.
13. No change: the stop of an old read is in 0.9.0, and a minimum time between reads lost
    hits in the session of 2026-11-05 (0.45 to 0.32).
14. The level list of `EndOfSpeech` stays sorted by insertion. It was sorted again for each
    chunk of audio.
15. The resampler has fixed weights when the device rate is a whole multiple of 16 kHz
    (48 kHz). Other rates use the code of before.
16. The tray tooltip changes only when no take runs.

Checks:

- Items 14 and 15 give the same bits as the code of before, in a scratch program: 16.9 million
  resampler outputs at 8 device rates, and 91,707 updates of `EndOfSpeech` on 300 takes.
- Text, `bench-asr` of 0.9.0 against this build, app path, the owner's dictionary on, through
  `safe.cmd`: `tech` 0 of 40 differ, the 145 saved takes 0, `clean` 0 of 200, `other` 0 of 200,
  long-clean 0 of 200, long-other 0 of 200. Model time on the long sets, one run each: 80,179
  to 75,643 ms and 75,782 to 70,703 ms (item 14).
- At speaking pace (`--whole --realtime`, 46 real takes, one run each): 0 texts differ, mean
  wait 25.8 against 25.6 ms, p90 104 against 106 ms, 27 takes ready at the release in each.
- Ratchet 81/172/34. Clippy, format, and the word-list check of CI pass.
- Item 7 against Hugging Face: a partial file without its last 3 MB ended in 2.0 s with the
  right SHA-256, and a partial file with wrong content started again and ended right.

Not verified: the app itself did not run, so items 1 to 6, 8 to 12, and 16 have no test. No
slow app was tried for item 5, and no sleep and wake for item 10.

Released as TDT 0.9.1.
