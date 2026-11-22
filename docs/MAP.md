# TDT Android: map of unknowns

Result of the grill session `tdt-android-k4m9` (2026-10-15, rounds 1-3). The user stopped the walk after round 3 and started the build. Items without a user answer are marked.

## Known knowns

- Same model as TDT desktop: Parakeet Redux (`moondream/parakeet-redux`), 1.58-bit ternary, 178 MB, CC-BY-4.0. Four files pinned by SHA-256 in `TDT/desktop/src/stt/models.rs:61-80`.
- Desktop runs the model with Moondream Photon in a Python worker (`TDT/desktop/src/stt/photon_worker.py`). Photon does not run on Android: `kestrel-kernels` 0.7.5 is proprietary (distribution needs a written agreement with M87 Labs) and ships only Windows, macOS and Linux glibc wheels.
- parakeetogo (Apache-2.0 C++, C API `pk_load` / `pk_transcribe` / `pk_free`) reads the unmodified Redux files. ARM NEON needs ARMv8.2 dot product. Not yet shown on Android.
- Reference UX: Aqua Voice Android (2026-10-06): floating button beside any keyboard through an Accessibility service. Cloud only.
- Fluid Functionalism is React web only. Android uses its motion rules as Compose spring tokens.

## Decision ledger

| Decision | Answer | Source |
|---|---|---|
| Runtime | Trit (TDT's engine) through NDK + JNI behind one `SpeechEngine` interface; was parakeetogo until 2026-10-29 (ADR 0002) | user, 2026-10-29 |
| Take path | The take path of TDT desktop, in Rust (`engine/`): the model reads a take while the user talks, and the phone runs each model as the desktop does (ADR 0003) | user asked for more speed and for the desktop's quality (2026-11-10); design by agent |
| Second model | Voz (Desert Ant Labs SDK), picked in the app; only on phones with about 6 GB of memory or more | user (2026-10-29); memory floor by agent from measurement |
| Dictionary | The user's own terms, one per line; Redux only; TDT ships none | agent (from Aqua's custom dictionary and TDT's no-word-lists rule) |
| Surface | Floating bubble beside the user's keyboard (Accessibility service) | user |
| UI stack | Kotlin + Jetpack Compose; Fluid Functionalism motion as spring tokens | user |
| Network | Strictly offline; network only for model download and update checks | user |
| Model delivery | Download on first run, SHA-256 checked; Wi-Fi by default, ask before mobile data | user |
| Distribution | Signed APK on GitHub Releases, private repo first; Play later | user |
| v1 scope | Bubble, tap or hold, insert with clipboard fallback, filler removal, history, model screen | user |
| Repo | Separate TDTmobile repo | user |
| Speed gate | Under 1.5 s for 10 s of speech, warm model | user |
| Test device | Emulator only; speed gate stays OPEN (not measurable on x86_64 images) | user, round 3 |
| Insert | Accessibility set-text spliced at the selection, then paste, then clipboard | user |
| Gestures | Tap toggle, hold to talk, drag to an edge (remembered), drag down hides 10 min; no shake | user |
| Bubble look | TDT icon; bars show voice level; dashed pulse ring while processing; check after insert | user |
| Bubble size | 48 dp | user |
| Feedback | Haptic tick on start and stop, no sound | user |
| Model in memory | Trit models: load when the bubble service starts, stay in memory, weights as a mapped file (0.6.1; was: load on focus, unload after 5 min idle, which Voz keeps) | agent, 2026-11-11, from the user's report of a 1,100 ms take and the desktop rule (model always loaded); the user can reverse it |
| Theme | Follow system light or dark (needs a light palette; desktop has only dark) | user |
| Audience | The user first, public later | user |
| Application ID | `io.github.hi9841.tdt` | user |
| Min Android | Android 10 (API 29) | agent on the user's behalf (copy Aqua) |
| Password fields | Bubble never shows | agent (copy Aqua) |
| Recording cap | 120 s | agent (desktop parity) |
| History | Last 10 transcripts, on the phone | agent (desktop `config.rs:140`) |
| Filler removal | On by default, setting to turn off, desktop word list | agent (desktop parity) |
| Language | No setting; the model detects it | agent (desktop commit 4200cea) |
| Hidden bubble | Returns from the notification button or after 10 min | agent (from no-shake) |
| ABIs | Debug: arm64-v8a + x86_64. Release: arm64-v8a | agent (from emulator answer) |
| Moondream contact | Agent drafts; user sends | agent (outward action) |

## Unknown knowns

- The user tests on the emulator only and builds for personal daily use first. Onboarding can be plain in v1.

## Unknown unknowns (landmines, found after round 3, not yet shown in a round)

1. **Microphone from an overlay on Android 14+.** RESOLVED on the Android 17 emulator (2026-10-15): the Accessibility service records with `AudioRecord` while another app is in front; `dumpsys audio` reports `silenced:false`, with no foreground service. Recheck on a real phone. Original concern: A microphone foreground service is a while-in-use type. The official exemptions for while-in-use do not list Accessibility services or overlays ([restrictions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start)). Plan: the spike tests direct `AudioRecord` in the Accessibility service and a microphone foreground service on API 35 and 37 emulators. Fallback: a transparent trampoline activity on tap. This is testable on the emulator.
2. **Restricted settings for sideloaded APKs (Android 13+).** An APK installed from a browser or file manager cannot turn on its Accessibility service until the user opens App info, then the menu, then "Allow restricted settings". Onboarding must show this step.
3. **Device floor.** RESOLVED with Trit (2026-10-29): Trit uses `sdot` where the CPU has the Armv8.2 dot product and plain NEON elsewhere, so every arm64 phone runs Redux. Voz needs about 6 GB of memory.
4. **parakeetogo build.** Gone with parakeetogo. The Trit build needs Rust with the Android targets, and mimalloc's `local_dynamic_tls` feature (Trit sets it for Android): bionic refuses initial-exec TLS in a library loaded with `dlopen`.
5. **Attribution.** CC-BY-4.0 needs credit for the Redux weights in the app; the Voz license and Trit's PolyForm Noncommercial license are in `THIRD_PARTY_NOTICES.md`. The app screen credits the model in use.
6. **OEM battery managers** can stop Accessibility services. A sharp edge; document it in onboarding.

## Spike results

- 2026-10-15, emulator `TDT_API_35` (x86_64, 4 cores, AVX2): parakeetogo `pk` built with NDK 27.1 (API 29) transcribed a 12.3 s TTS clip word for word. Cold run with model load: 1.18 s. Three clips in one warm process: 0.78 s total. This is x86 AVX2 speed, not phone speed.
- 2026-10-15, new reference emulator `TDT_Pixel_10_API_37` (Pixel 10 profile, Android 17, 16 KB pages, 4 GB RAM): the same clip, word for word, 2.41 s cold. A build without 16 KB alignment fails to link ("empty/missing DT_HASH/DT_GNU_HASH"). Native code must build with `ANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON` on NDK 27.
- `pk_load` and the ternary loader call `exit()` on a missing file or a CPU without dot product (`src/st.cc`, `src/tern.cc:36`). The app must check files and the CPU before it calls the engine.

- 2026-10-29, Trit replaces parakeetogo. Emulator `TDT_API_35` (x86_64, 6 GB): Redux on Trit transcribed the 12.3 s clip word for word, 0.24 s load, 0.13 s warm, 0.56 GB peak resident memory (whole test process). Voz: same text, 115 s download (1.25 GB), 10 s load, 0.51 s warm, 3.9 GB peak. On `TDT_Pixel_10_API_37` (4 GB) Android killed Voz for low memory.
- 2026-10-29, app flow on both emulators: first-run Redux download through the app, takes from the app button and from the bubble (tap, and hold-to-talk) in Contacts, model switch (Voz frees 3.8 GB on the switch back), dictionary applied by Trit, release APK (arm64 only, signed, 16 KB aligned) run through the ARM translation. Found and fixed: the Voz SDK's `isDownloaded()` verifies 1.25 GB of files (3.6 s), and calling it on the main thread gave an ANR; it now runs on the IO pool, once per process.
- 2026-10-29, Trit's arm64 text under QEMU (`-cpu max`, with the dot product): `sdot`, NEON and scalar paths equal x86 AVX2 and scalar on 12 clips; `sdot` equals x86 AVX2 on 39 of 40 (the 40th differs on every arm64 path, scalar included). Android core selection checked with fake `cpuinfo_max_freq` values on the rooted emulator: 2 of 4 cores, both team threads pinned to them.
- 2026-10-29, Trit's NEON paths under the emulator's ARM translation (no dot product there, so the plain NEON path): 20.9 s clip in 7.5 s against 25.1 s for Trit's scalar path. Translation speed, not phone speed; it shows only the ratio.
- 2026-11-02, TDT 0.2.0 on Trit 0.7.0 (`origin/main` with the arm64 kernels applied, branch `android` in the Trit clone, not pushed). `trit-ffi` has a default `gpu` feature now; the app builds it with `--no-default-features`. Emulator `TDT_Pixel_10_API_37` (Android 17, 16 KB pages): the 12.3 s clip word for word on x86_64 (0.43 s load, 0.14 s warm, 0.56 GB peak) and on arm64 through the ARM translation (1.6 s load, 4.2 s warm; translation speed). Release APK: v2 signed with the 0.1.0 key, 16 KB aligned (zip and ELF LOAD 0x4000), minSdk 29, installs on the emulator. The setup screen now asks for the microphone; before, the bubble only opened the app.
- 2026-11-02, first real phone: OnePlus 13R (CPH2645, Snapdragon 8 Gen 3 SM8650, 12 GB, Android 16, `asimddp` and `i8mm`; fast cores 2-7 by `cpu_capacity`). Installed with adb over wireless debugging: Play Protect's enhanced fraud protection blocks the bubble build from a browser or Files ("App blocked to protect your device"), and turning off Play Protect scanning did not help; the USB cable link failed (adb read and write errors on the B650 chipset port while MTP worked). Engine test inside the release app (`-Ptdt.testBuildType=release`): the 12.3 s clip word for word, 0.41-0.44 s load, 0.55-0.64 s warm, 0.51-0.53 GB peak for the test process. The phone log showed bubble touches inside OnePlus's back-gesture strip (84 px each side of 1264 px).
- 0.3.0 (same day): bubble clear of the gesture strip (system gesture insets plus exclusion rects); a hold under 0.6 s after the 300 ms hold delay counts as a tap; screen kept on (`FLAG_KEEP_SCREEN_ON` on the overlay) and a partial wake lock (150 s cap) from start of a take to its text; "No speech heard" toast; a hint reported as field text ("Message") counts as empty; orb logo (adaptive and themed launcher icon, bubble, app); setup checklist screen with the Trit version; `TDT` log tag with take times and field facts (never text).
- 2026-11-07, TDT 0.5.3 on Trit 0.9.0 (export of the `v0.9.0` tag in `third_party/trit`; the C header did not change). Emulator `TDT_Pixel_10_API_37`: the 12.3 s clip word for word on x86_64 and on arm64 through the ARM translation (1.8 s load, 4.2 s warm, the same as 0.7.0). Interleaved A/B on x86_64, 3 runs each: Trit 0.7.0 warm 143-199 ms, Trit 0.9.0 warm 140-230 ms, peak 0.55 GB for both, so no change. Not measured on a phone.

- 2026-11-10, TDT 0.6.0 (versionCode 12): the take path of TDT desktop (`engine/`, ADR 0003) on Trit 0.11.0 with its second kernel set, and Parakeet Unified English as int8. Not run on a phone. Checks, all on this PC unless a line says other:
  - Text against TDT desktop: `engine/examples/takes.rs` against `bench-asr --whole --chunk-ms 20` of TDT 0.9.6, on the owner's 274 saved takes, with the desktop dictionary. 0 texts differ, for Redux and for Unified.
  - PCM16 scale: the desktop reads a PCM16 value as value / 32767. With value / 32768 (as the app did up to 0.5.3, a change of 0.003 %), Redux wrote other text for 51 of 271 takes and Unified for 4. The take now uses the desktop's scale (`samples_of` in `engine/src/take.rs`).
  - Chunk length: 0 texts differ between 10, 20 and 100 ms chunks (276 takes, both models). Speaking pace against all audio at once: 0 of 60 differ.
  - Kernel sets on x86 with AVX-512: 0 texts differ between the first and the second set (276 takes, both models).
  - arm64: the arm64 Linux build under QEMU 8.2 (`-cpu max`: dot product and i8mm) against the x86-64 Linux build, 60 takes: 0 texts differ, for both models. With the first kernel set, arm64 wrote another word in 1 take of 1 (the only take run with it).
  - The operating system changes the text, the CPU does not: x86-64 Linux against x86-64 Windows, same code and kernels, 276 takes: Redux 41 texts differ (33 in words), Unified 11 (9 in words). The cause is in Trit (most probably the math functions of the system library) and is not found. So the phone does not write the Windows text for each take. Android has a third math library; not measured.
  - Unified as int8 against the stored Q8_0, one read of each whole take, 271 takes: mean 53 ms against 76 ms; 20 texts differ. The int8 load takes 0.4 to 0.8 s here against 0.2 s.
  - Wait after the stop, 60 saved takes at speaking pace, Unified, quiet PC: mean 27 ms against 83 ms for one read after the stop (the path up to 0.5.3), median 0 against 68, longest 163 against 332. For the 35 takes that end in 200 ms of quiet or more, the text was ready: 0 ms (5 ms at most). For the 25 takes that stop in speech: 64 ms against 82. The Redux run had other load on the PC (one read took 2 s) and gives no clean number; its 35 takes with quiet at the end had a median wait of 0 ms.
  - Emulator `TDT_Pixel_10_API_37`, x86_64: the 12.3 s clip word for word through takes; all audio at once 195 to 340 ms, at speaking pace 1 ms. One take with the app button and one with the bubble in Contacts (the emulator microphone is silent: "No speech heard", `take audio=3200ms wait=108ms (last word 0ms, model 108ms)`); the bubble service binds, no crash. The arm64 library through the ARM translation: the same text, 4.6 to 5.3 s at once (0.5.3: 4.2 s for one read; a take also reads its first second once).
  - Release APKs: signed with the 0.1.0 key, 16 KB aligned, arm64 only, versionCode 12. The bubble build and the sideload build hold the same `libtdt_engine.so`.
  - `agent-device` (its snapshot helper) turns off other Accessibility services while it runs: stop `com.callstack.agentdevice.snapshothelper` before a bubble check, and use `adb shell input tap` for the bubble.
  - The ARM translation keeps its own environment: `Os.setenv` in `EngineTest` (`isa`, `kernels`) does not reach the arm64 library there. It works for x86_64.

- 2026-11-11, TDT 0.6.1 (versionCode 13). The owner installed 0.6.0 and reported a take of 1,100 ms with Unified on the phone. No log: the phone was not on adb, so the cause is not known. Changes, each for a cost that 0.6.0 added or kept without a phone measurement:
  - Trit models load when the bubble service starts and stay in memory; weights from the mapped cache file. In 0.6.0 the Unified load as int8 took 2 to 3 times as long as before on the PC (0.4 to 0.8 s against 0.2 s), and a take that started during a load waited for it.
  - Back to Trit's first kernel set and to Unified as stored Q8_0: the settings of the 2026-11-02 phone measurement (5.5 s of speech in 240 ms). `EngineTest` takes `kernels`, `precision` and `threads`, for a comparison on the phone.
  - PC: Redux, 274 saved takes, 0 texts differ from `bench-asr` of TDT 0.9.6. Unified Q8_0: 0 differ between 10, 20 and 100 ms chunks (285 takes); 14 of 274 differ from the desktop's int8 text. First load with the cache 0.8 s (Redux) and 1.3 s (Unified), later loads 0.2 s and 0.45 s. Cache files: 307 MB and 835 MB.
  - Emulator `TDT_Pixel_10_API_37`, x86_64: first load 1.7 s, later 0.27 s; the clip word for word, 1 to 2 ms at speaking pace. With Redux in memory and idle: 406 MB PSS, of which 300 MB are clean pages of the mapped file and 11 MB are native heap. The bubble service loads the model when it starts, also after `adb install -r` (0.86 s). One bubble take, no crash.
  - Seen on the emulator: after a cold boot the bubble was off in Accessibility settings (cause not found; `am instrument` or the emulator's stop without a saved state). Check on the phone that the bubble stays on after an update.

- 2026-11-17, TDT 0.6.2 (versionCode 14). The owner: short takes wait about 1 s, long takes a couple of seconds. The phone was not on adb; the owner chose to go on without it. Ledger: `C:\Users\hi\tdt-mobile-oploop\LEDGER.md`. Changes:
  - The stop cue stays out of the take. The bubble played the stop sound and vibrated at the stop tap, with the microphone still open. As speech, the cue made the read in the quiet after the speech old, and the model read the whole take after the stop. Proxy (`engine/examples/takes.rs --cue`, 20 ms of the cue after the stop, 490 saved takes, Unified): text ready at the stop in 0 takes against 293 without the cue, median 5.2 s of audio read after the stop against 0, and 214 texts changed. Now the take closes before the vibration (`BubbleService.stop`), the stop sound plays when the microphone has closed (at once for a user who was quiet; after the last word otherwise), and a take takes no audio after its tail ended (`Feed::closed` in `engine/src/take.rs`). With the cue: 293 ready, 0 texts changed except 2 takes that end in speech.
  - Trit's Android team fits the cpuset (`third_party/trit/src/cpu/team.rs`: `fit_to`, `Shared::active`). Trit started one spinning thread per fast core from sysfs (6 on the 13R) and never looked at the cpuset; a service (the bubble) gets fewer cores than the app on screen. Only as many threads as the cpuset has team cores take tasks now, and parked workers pin themselves again after Android moves the app to another cpuset. Rooted emulator, process in a 2-core cpuset: 289-331 ms against 519-625 ms for the 12.3 s clip (one thread alone: 514-568); all cores free: no change. Not yet in the Trit repo.
  - The About line shows the last take: length, wait, microphone and model parts, and the cores of the cpuset (`Cpus_allowed_list`), so the owner can read the numbers without adb.
  - Gates: 0 of 490 texts differ from 0.6.1 (no cue); rustfmt and clippy clean for Windows and aarch64-linux-android; `EngineTest` word for word on the emulator; one bubble take on the emulator, no crash.
  - Open: 197 of the 490 takes end in speech, so the model reads them whole after the stop. Their wait is the phone's read speed; measure it on the phone (and an ADPF hint session for CPU frequency).

- 2026-11-17, TDT 0.6.3 (versionCode 15). Owner on 0.6.2: about 500 ms. Measured on the OnePlus 13R over adb (harness `engine/examples/takes.rs` for aarch64 in `/data/local/tmp/tdt`; `simpleperf` is blocked for the shell there):
  - The bubble process is in the `foreground` cpuset, which holds all 8 cores on this phone; the 0.6.2 thread fix changed nothing here.
  - A read takes 45 ms per second of audio; the encoder is 92% of it. The read in the quiet starts 200 ms after the last word and reads the whole take, so long takes wait even when that read is used.
  - Unified now runs as `q8_0`: the GGUF's f16 convolution weights become Q8_0 at load. Reads 19-23% faster on the phone; 1,426 word errors against 1,429 on 632 reference clips (PC).
  - Trit's arm64 `smmla` kernel pairs the frames once per product: the same values (1,778 products, 0 differ), 22% faster on one Cortex-A720. Patch in `third_party/trit/src/cpu/int8.rs`, not yet in the Trit repo.
  - The engine deletes weight caches of other precisions after a load (the `auto` cache is about 800 MB).
  - Open: `int8` (TDT desktop's setting; 1,405 errors on the same clips) is not timed on the phone.

- 2026-11-17, TDT 0.6.4 (versionCode 16). The owner found 0.6.3 slower (about 800 ms). On the cool phone with pauses between runs, the paired-frame kernel gave no gain on 6 threads, so 0.6.4 is 0.6.3 without it (Trit's `int8.rs` as exported). `q8_0` stays: 5 takes in 1,905-2,018 ms against 2,508-3,003 ms with `auto`. In the app's process: 12.3 s clip at once 516-592 ms, 46 ms after the stop at speaking pace.

- 2026-11-17, TDT 0.6.5 (versionCode 17): Unified as `int8`, as on TDT desktop. On the 13R, 6 cores: the int8 kernel at 439 GMAC/s against 290 with `q8_0` (the 6 fast cores reach 623 GMAC/s of `smmla` together: power is the cap); 5 takes read in 1,426-1,533 ms against 1,586-2,086. Load 1.1 s against 0.55 s. Word errors on 632 reference clips: 1,405 against 1,426. OnePlus killed the process twice at 0.93 GB (`o-kill`, exit-info reason 13); TDT is not exempt from battery optimization.

- 2026-11-17, TDT 0.6.6 (versionCode 18): faster reads and load, same text. All changes are in vendored Trit (`third_party/trit/src/cpu/int8.rs`, `activation.rs`, `gemm.rs`, `models/parts/transducer.rs`); each gives the same bits as 0.6.5 (output hashes equal; all 81 phone takes give the same text through the take path and as whole reads with the dictionary). Not yet in the Trit repo.
  - `smmla` kernel with frames paired once per product, for int8's 128-input groups: one block's products 438-458 -> 527-563 GMAC/s (6 threads). Session 2 saw no gain with Q8_0's 32-input groups, where the zips were a smaller share.
  - Exp, SiLU, sigmoid and tanh four vectors at a time: SiLU 2.07 times faster on an A720 core.
  - The joint layer keeps its weights as f32 from the load (each decoder step widened them again); one-frame products (decoder steps) group row blocks per task: the decoder of a 21 s take 39.7 -> 15.3 ms.
  - Load: the int8 quantizer at load is NEON (it divided one weight at a time) and Q8_0 dequantization is parallel: load with cache 812-841 -> 571-584 ms (shell harness).
  - 5 takes (53.5 s of audio), screen on, interleaved: 1,171-1,298 -> 928-1,075 ms. In the app: 12.3 s clip at once 242-278 ms, load 607-755 ms. The first load after the update rewrites the weight cache once (about 3 s).
  - Measure with the screen on: in doze the shell benchmark stalls for about 200 ms at random, and the products run at about 70% speed.
  - Open: `EngineTest` expects Unified's text without the final period, but the take path writes "rest." (0.6.5 too); the test fails on its expectation, not on the engine.

- 2026-11-17, TDT 0.6.7 (versionCode 19): the 13R's microphone sends 40-50 ms of near-silence (-100 to -83 dB) after it opens. The tail and the speech end take the room noise from the first 80 ms, so the floor read -87 dB against a room at -50 dB: every take kept the microphone open for the full 600 ms tail, and the read in the quiet never ran (0.6.6: waits 772-833 ms on 2.5-3.8 s takes, 600 ms of it tail). `Take::push` now drops chunks under -80 dBFS at the start of a take (160 ms at most); a stop before any audio closes the take. On 150 desktop takes with that start added: read in the quiet 63 -> 82 of 150, and the text equals the clean audio's in all 150 (106 differed before). The detection code (`tail.rs`, `segment.rs`) stays as on the desktop. Reviewed by GPT-6.2 Sol (high); its four fixes are in.
  - On the phone (owner, 6 takes): 1.4-3.4 s takes wait 129-231 ms (tail 0-1 ms; 0.6.6: 772-833 ms), a stop inside the last word 433 ms (tail 240), an 18.4 s take 493 ms. The model still reads the whole take after the stop when the user stops within 200 ms of the last word.

- 2026-11-18, TDT 0.6.8 (versionCode 20): `third_party/trit` is an export of Trit `v0.15.0` with no changes. The changes of 0.6.2 to 0.6.6 are in Trit itself (0.14.0 and 0.15.0), with upstream's 0.12.0 and 0.13.0 work (the team parks at once after a call, the buffer pool, Whisper on AVX-512). Trit's license from 0.13.0 is AGPL-3.0 or a commercial licence (`THIRD_PARTY_NOTICES.md`). On the PC, 150 takes: the same text as 0.6.7, through the take path and as whole reads with the dictionary.
- 2026-11-18, TDT 0.6.9 (versionCode 21): the end quiet of TDT desktop 0.9.10. Parakeet Unified English writes each word about 0.3 to 0.4 s after the word ends, and the tail ends a take 60 ms after the last word: a fast stop lost the last word, and a one-word take could give no text. The reads that end a take now get 480 ms of digital quiet after the audio, for Unified English only (`ModelPack.endQuietMillis`, through `tdt_engine_new`). PC gate (`takes.rs`, 20 ms chunks): 309 short LibriSpeech phrases that end 60 ms after the last word, 52 to 27 errors and 7 to 0 empty; clean 57 to 57, other 103 to 99, tech 19 to 17; Redux 0 of 618 texts differ. On the OnePlus 13R: the same 52 to 27 errors on the phrases; without the quiet, 0 of 81 phone takes differ from 0.6.7 (take path, and whole reads with the dictionary), so Trit 0.15.0 keeps the phone text. Installed on the phone 2026-11-18: bubble bound, no crash, model loaded.

- The bubble can cover an app's own button next to the text field (for example the Messages voice-note button). The user can drag it away.

- Speed gate on a real arm64 phone, with Trit's `sdot` path (the emulator's ARM translation has no dot product).
- 0.6.1 on a real phone: the wait after the stop with its parts (`adb logcat -s TDT:I`), memory with the model always loaded, and battery use of the reads during a take. Then the comparisons: `am instrument -e kernels v2`, `-e precision int8`, `-e threads N` against the defaults.
- Trit's text depends on the operating system (see 2026-11-10). A fix belongs in Trit: one math implementation for every system. It changes the desktop's text too, so the owner decides.
- Moondream reply about an Android Photon license. Draft: `docs/moondream-request.md` (not sent).
- Light palette for the system theme.
- Verify the 0.5.0 screens on a device: the emulator's agent-device session was held by another T3 session (t3-e1a6...) all day.
- APK size: the Voz SDK is 56 MB of native code for each ABI (release APK 91 MB; the engine for Redux is 2.4 MB).
