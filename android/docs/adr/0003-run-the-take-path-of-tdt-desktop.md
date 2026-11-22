# 0003: Run the take path of TDT desktop

Date: 2026-11-10. Status: accepted.

## Context

Up to 0.5.3 the app recorded a take, closed the microphone, and then gave the whole take to Trit in one call. The wait after the stop was the time of the model for the whole take, plus about 100 ms for the microphone to close. On the OnePlus 13R (2026-11-02): 200 to 700 ms for takes of 1 to 14 s, and 1.9 s for a take of 30 s.

The owner reported two problems on 2026-11-10: the app is much slower than TDT desktop, and its text is worse.

TDT desktop does not work this way. Its take path (`desktop/src/stt/engine.rs`, `segment.rs`, `audio/tail.rs`) reads the take while the user talks:

- The first second runs through the model once. Its text is dropped. The cores and the weights are then ready.
- A take longer than 30 s runs in windows of 30 s that share 8 s. The texts join in the shared words. If they do not join, the whole take runs once after the stop.
- When the user is quiet for 200 ms after speech, the model reads the take up to that point. If the user stops with no new speech, that text is the text of the take.
- After the stop, the microphone stays open while the last word still ends (60 ms of quiet, 0.6 s at most).
- A take with speech too quiet for the model runs again with more gain.

The phone app also differed from the desktop in the engine setup:

- Trit 0.9.0. Its kernels add in an order that depends on the CPU, so an arm64 CPU can write other text than the desktop for the same audio. Trit 0.11.0 has a second kernel set that gives the same bits on every CPU.
- Parakeet Unified English ran in the stored Q8_0 form. The desktop runs it in Trit's own int8.

## Decision

The phone runs the take path of TDT desktop, in Rust, in `engine/` (crate `tdt-speech`):

- `engine/src/segment.rs` and `tail.rs` are copies of the desktop files. `take.rs` is the desktop take loop. A change to one side must go to the other side.
- The library depends on the `trit` crate (`third_party/trit`, Trit 0.11.0), not on Trit's C library. It has its own small C ABI (`engine/include/tdt_speech.h`): an engine, and takes that get audio while it comes.
- The library runs Trit's first kernel set, the default. 0.6.0 selected the second set (`TRIT_KERNELS=v2`, the same bits on every CPU); 0.6.1 took that back, because the speed of the second set on a real arm64 CPU is not measured, and the text changes with the operating system in each case.
- A take gets PCM16 audio and reads a value as value / 32767, as the desktop does. Parakeet Redux writes other text for about 1 take in 5 with value / 32768.
- Parakeet Unified English runs in the stored Q8_0 form, the form with a speed that was measured on a phone. 0.6.0 loaded it as int8, as the desktop does; 0.6.1 took that back for the same reason, and because the int8 load is 2 to 3 times as long.
- A Trit model loads when the bubble service starts and stays in memory (0.6.1), as on the desktop, so no take waits for a load. Trit maps the weights from a cache file (`LoadOptions::cache`), so they are file memory that Android can drop and read again. Voz still leaves memory after 5 minutes of idle time.
- The recorder thread opens and closes the microphone. It gives each 20 ms of audio to the take. The text does not wait for the microphone to close.
- Voz has no such path. Its SDK reads a whole take in one call, as before.

A Kotlin port of the take path was the other option. It was rejected: the same Rust source on both sides can run against the same takes on a PC, and a second language is a second set of rounding rules for the speech-end levels.

## Consequences

- For the same PCM16 audio, the phone's speech path writes the text of TDT desktop 0.9.6 on Windows (274 saved takes, 0 differ: both models with the desktop's settings, and Redux with the settings of 0.6.1). On Linux the same code writes other text for some takes (Redux 41 of 276, Unified 11 of 276): Trit's text depends on the operating system. The arm64 build writes the x86-64 Linux text.
- The wait after the stop no longer grows with the take, for a user who is quiet before the stop. See `docs/MAP.md` for the numbers.
- The model works during a take: each pause of 200 ms starts a read, and new speech stops it. A take uses more CPU time and battery than before.
- The cache file takes disk space: 307 MB for Redux and 835 MB for Unified, next to the model.
- `local.properties` `trit.dir` is gone. Trit is always `third_party/trit`.
- `engine/examples/takes.rs` runs WAV files through the phone's speech path on a PC. Use it to compare text with the desktop, and one chunk length with another.
