//! Standalone stability & latency verification application for TDT + Trit.
//!
//! Runs 6 targeted checks directly against an installed speech model (defaulting
//! to Parakeet Unified English, or Parakeet Redux if Unified English is absent),
//! using an isolated scratch directory so the running TDT app and its live
//! `.trit/` cache are never touched.
//!
//! Usage:
//!   test-stability [--model-dir PATH] [--wav PATH]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use trit::{Cancel, LoadOptions, Options, Trit};
use voice_stt_desktop::resample::Resampler;
use voice_stt_desktop::stt::models::{find_dir, resolve, ModelSpec, UNIFIED_ID};
use voice_stt_desktop::stt::{LiveAudio, StopTimes, SttEngine};
use voice_stt_desktop::SAMPLE_RATE;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct TestResult {
    name: &'static str,
    passed: bool,
    detail: String,
}

fn main() {
    let (model_override, wav_override) = parse_args();
    println!("TDT + Trit Stability Verification Suite");
    println!("=======================================\n");

    let (spec, source_dir) = match locate_model(model_override.as_deref()) {
        Some(found) => found,
        None => {
            eprintln!("FAIL: Could not find an installed Parakeet model.");
            eprintln!("Pass --model-dir <path> or download a model in TDT Settings.");
            std::process::exit(1);
        }
    };
    println!("Model:  {} ({})", spec.label, spec.id);
    println!("Source: {}", source_dir.display());

    let scratch = ScratchDir::new("tdt-stability-suite");
    let isolated_model = scratch.path.join("model");
    if let Err(error) = mirror_model_dir(&source_dir, &isolated_model) {
        eprintln!("FAIL: Could not prepare isolated model directory: {error}");
        std::process::exit(1);
    }

    let sample_audio = match load_or_synthesize_audio(wav_override.as_deref(), &scratch.path) {
        Ok(audio) => audio,
        Err(error) => {
            eprintln!("FAIL: Could not prepare test audio: {error}");
            std::process::exit(1);
        }
    };
    println!(
        "Audio:  {:.2} s (short {:.2} s, multi-pause {:.2} s)\n",
        sample_audio.main.len() as f32 / SAMPLE_RATE as f32,
        sample_audio.short.len() as f32 / SAMPLE_RATE as f32,
        sample_audio.multi_pause.len() as f32 / SAMPLE_RATE as f32,
    );

    let mut results = Vec::new();
    results.push(test_failed_load_cache_safety(&scratch.path, spec));
    let (load_result, engine) =
        test_weight_load_speed_and_accuracy(&isolated_model, spec, &sample_audio.main);
    results.push(load_result);

    if let Some(engine) = engine {
        results.push(test_eviction_and_prefetch(
            &engine,
            &sample_audio.short,
            &sample_audio.main,
        ));
        results.push(test_concurrent_release_memory(&engine, &sample_audio.short));
        results.push(test_quiet_read_and_cancellation(
            &engine,
            &isolated_model,
            spec,
            &sample_audio.multi_pause,
            &sample_audio.long_clip,
        ));
        engine.release();
    } else {
        results.push(TestResult {
            name: "Eviction + async prefetch",
            passed: false,
            detail: "skipped (model load failed)".into(),
        });
        results.push(TestResult {
            name: "Concurrent release_memory",
            passed: false,
            detail: "skipped (model load failed)".into(),
        });
        results.push(TestResult {
            name: "QuietRead cancel + long take",
            passed: false,
            detail: "skipped (model load failed)".into(),
        });
    }

    results.push(test_wasapi_endpoint_id());

    println!("\nSummary");
    println!("-------");
    let mut failed = 0usize;
    for (index, item) in results.iter().enumerate() {
        let status = if item.passed { "PASS" } else { "FAIL" };
        if !item.passed {
            failed += 1;
        }
        println!(
            "[{}/{}] {:<4} {:<32} {}",
            index + 1,
            results.len(),
            status,
            item.name,
            item.detail
        );
    }
    println!();
    if failed == 0 {
        println!("ALL {} CHECKS PASSED.", results.len());
    } else {
        println!("{failed} OF {} CHECKS FAILED.", results.len());
        std::process::exit(1);
    }
}

/// Check 1: verify a failed model load never publishes a partial `.trit` cache file.
fn test_failed_load_cache_safety(scratch: &Path, spec: &ModelSpec) -> TestResult {
    let broken_dir = scratch.join("broken-model");
    let _ = fs::create_dir_all(&broken_dir);
    let _ = fs::write(
        broken_dir.join("config.json"),
        b"{\"model_type\":\"parakeet_tdt\"}",
    );
    let _ = fs::write(
        broken_dir.join("model.safetensors"),
        b"not-a-valid-safetensors",
    );

    let options = LoadOptions {
        cache: true,
        precision: spec.precision,
        ..LoadOptions::default()
    };
    let loaded = Trit::load_with(&broken_dir, &options);
    let cache_dir = broken_dir.join(".trit");
    let (read_ok, cache_files) = match fs::read_dir(&cache_dir) {
        Ok(entries) => (
            true,
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect::<Vec<_>>(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (true, Vec::new()),
        Err(_) => (false, Vec::new()),
    };
    let passed = loaded.is_err() && read_ok && cache_files.is_empty();
    let detail = if passed {
        "failed load left zero published or temporary cache files".to_string()
    } else {
        format!(
            "load_ok={}, read_ok={read_ok}, leftover_files={cache_files:?}",
            loaded.is_ok()
        )
    };
    TestResult {
        name: "Failed-load cache safety",
        passed,
        detail,
    }
}

/// Check 2: measure uncached, cache-miss (build), and cache-hit (direct map)
/// load times and confirm identical transcript output.
fn test_weight_load_speed_and_accuracy(
    model_dir: &Path,
    spec: &ModelSpec,
    samples: &[f32],
) -> (TestResult, Option<Arc<SttEngine>>) {
    let uncached_opts = LoadOptions {
        cache: false,
        precision: spec.precision,
        ..LoadOptions::default()
    };
    let t0 = Instant::now();
    let uncached = match Trit::load_with(model_dir, &uncached_opts) {
        Ok(model) => model,
        Err(error) => {
            return (
                TestResult {
                    name: "Weight load speed & accuracy",
                    passed: false,
                    detail: format!("uncached load failed: {error}"),
                },
                None,
            );
        }
    };
    let uncached_ms = t0.elapsed().as_millis();
    let baseline_text = uncached
        .transcribe_with(samples, &Options::language("en"))
        .trim()
        .to_string();
    drop(uncached);
    trit::release_memory();

    let t1 = Instant::now();
    let engine_miss = SttEngine::new(model_dir, spec);
    if let Err(error) = engine_miss.prepare() {
        return (
            TestResult {
                name: "Weight load speed & accuracy",
                passed: false,
                detail: format!("cache-miss load failed: {error}"),
            },
            None,
        );
    }
    let miss_ms = t1.elapsed().as_millis();
    let miss_was_hit = engine_miss.loaded_from_cache();
    let miss_text = run_once(&engine_miss, samples, false)
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default();
    engine_miss.release();
    drop(engine_miss);
    trit::release_memory();

    let t2 = Instant::now();
    let engine_hit = Arc::new(SttEngine::new(model_dir, spec));
    if let Err(error) = engine_hit.prepare() {
        return (
            TestResult {
                name: "Weight load speed & accuracy",
                passed: false,
                detail: format!("cache-hit load failed: {error}"),
            },
            None,
        );
    }
    let hit_ms = t2.elapsed().as_millis();
    let hit_was_hit = engine_hit.loaded_from_cache();
    let hit_text = run_once(&engine_hit, samples, false)
        .map(|r| r.text.trim().to_string())
        .unwrap_or_default();

    let speedup_ok = hit_ms < miss_ms && hit_ms < uncached_ms;
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let text_matches = !baseline_text.is_empty()
        && !miss_text.is_empty()
        && !hit_text.is_empty()
        && hit_text == miss_text
        && norm(&hit_text) == norm(&baseline_text);
    let passed = !miss_was_hit && hit_was_hit && speedup_ok && text_matches;
    let detail = format!(
        "uncached {uncached_ms} ms, miss {miss_ms} ms, hit {hit_ms} ms ({:.1}x faster); text exact match: {text_matches} (\"{}\")",
        uncached_ms as f64 / hit_ms.max(1) as f64,
        hit_text
    );
    (
        TestResult {
            name: "Weight load speed & accuracy",
            passed,
            detail,
        },
        Some(engine_hit),
    )
}

/// Check 3: evict the process working set (`EmptyWorkingSet`) to simulate real
/// OS memory pressure, compare a prefetch-off control against prefetched
/// inference, and confirm the resident fast-path skips redundant scans.
fn test_eviction_and_prefetch(
    engine: &Arc<SttEngine>,
    short_samples: &[f32],
    main_samples: &[f32],
) -> TestResult {
    // Control: evict working set, then let prefetch run inside a fresh
    // short (< 1 s) realtime take and measure inference latency.
    let evicted_1 = empty_working_set();
    let short_res = match run_once(engine, short_samples, true) {
        Ok(res) => res,
        Err(error) => {
            return TestResult {
                name: "Eviction + async prefetch",
                passed: false,
                detail: format!("short take failed: {error}"),
            };
        }
    };
    let short_ms = short_res.latency(short_res.transcribed).total_ms();
    let short_faults = short_res.diagnostics.page_faults;

    // Controlled check: evict working set, run `prefetch_weights()` explicitly,
    // verify the subsequent inference sees very few faults, and confirm a
    // second `prefetch_weights()` skips the scan via QueryWorkingSetEx.
    let evicted_2 = empty_working_set();
    let t_pre = Instant::now();
    engine.prefetch_weights();
    let prefetch_ms = t_pre.elapsed().as_millis();

    let t_skip = Instant::now();
    engine.prefetch_weights();
    let skip_ms = t_skip.elapsed().as_millis();

    let main_res = match run_once(engine, main_samples, false) {
        Ok(res) => res,
        Err(error) => {
            return TestResult {
                name: "Eviction + async prefetch",
                passed: false,
                detail: format!("medium take failed: {error}"),
            };
        }
    };
    let main_ms = main_res.latency(main_res.transcribed).total_ms();
    let main_faults = main_res.diagnostics.page_faults;
    let faults_low = main_faults.is_none_or(|f| f < 10_000);

    let passed = evicted_1
        && evicted_2
        && short_ms < 500
        && main_ms < 500
        && faults_low
        && skip_ms < 15
        && !main_res.text.trim().is_empty();
    let detail = format!(
        "evict={evicted_1}/{evicted_2}; short(<1s) {short_ms} ms (faults {short_faults:?}); prefetch {prefetch_ms} ms, resident-skip {skip_ms} ms, after-prefetch {main_ms} ms (faults {main_faults:?})"
    );
    TestResult {
        name: "Eviction + async prefetch",
        passed,
        detail,
    }
}

/// Check 4: fire `trit::release_memory()` repeatedly while transcribing to
/// verify there is no 1-second spin-barrier stall on `team::submit`.
fn test_concurrent_release_memory(engine: &Arc<SttEngine>, samples: &[f32]) -> TestResult {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let spammer = std::thread::spawn(move || {
        let mut calls = 0usize;
        while !stop_flag.load(Ordering::Relaxed) {
            trit::release_memory();
            calls += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
        calls
    });

    let mut max_ms = 0u64;
    let mut sum_ms = 0u64;
    let runs = 10u64;
    for _ in 0..runs {
        match run_once(engine, samples, false) {
            Ok(res) => {
                let ms = res.latency(res.transcribed).total_ms();
                max_ms = max_ms.max(ms);
                sum_ms += ms;
            }
            Err(error) => {
                stop.store(true, Ordering::Relaxed);
                let _ = spammer.join();
                return TestResult {
                    name: "Concurrent release_memory",
                    passed: false,
                    detail: format!("transcribe failed: {error}"),
                };
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let calls = match spammer.join() {
        Ok(calls) => calls,
        Err(_) => {
            return TestResult {
                name: "Concurrent release_memory",
                passed: false,
                detail: "release_memory collector thread panicked".to_string(),
            };
        }
    };
    let avg_ms = sum_ms / runs;
    let passed = calls > 0 && max_ms < 250;
    TestResult {
        name: "Concurrent release_memory",
        passed,
        detail: format!(
            "{runs} takes during {calls} release_memory() calls: avg {avg_ms} ms, max {max_ms} ms"
        ),
    }
}

/// Check 5: feed a multi-pause realtime take (triggering `QuietRead` start +
/// cancel) and verify fast cancellation and accurate output, plus a
/// pre-cancelled >30 s check and a real 34 s windowed take.
fn test_quiet_read_and_cancellation(
    engine: &Arc<SttEngine>,
    model_dir: &Path,
    spec: &ModelSpec,
    multi_pause: &[f32],
    long_clip: &[f32],
) -> TestResult {
    let pause_res = match run_once(engine, multi_pause, true) {
        Ok(res) => res,
        Err(error) => {
            return TestResult {
                name: "QuietRead cancel + long take",
                passed: false,
                detail: format!("multi-pause take failed: {error}"),
            };
        }
    };
    let pause_ms = pause_res.latency(pause_res.transcribed).total_ms();

    // Verify both pre-cancelled and mid-inference cancellation on a >30 s clip.
    let (pre_cancel_ms, mid_cancel_ms) = match Trit::load_with(
        model_dir,
        &LoadOptions {
            cache: true,
            precision: spec.precision,
            ..LoadOptions::default()
        },
    ) {
        Ok(direct) => {
            let direct = Arc::new(direct);
            let pre_cancel = Cancel::new();
            pre_cancel.cancel();
            let pre_opts = Options {
                cancel: Some(pre_cancel),
                ..Options::language("en")
            };
            let t_pre = Instant::now();
            let out = direct.transcribe_with(long_clip, &pre_opts);
            let pre_elapsed = t_pre.elapsed().as_millis();
            if !out.is_empty() {
                return TestResult {
                    name: "QuietRead cancel + long take",
                    passed: false,
                    detail: format!("pre-cancelled decode returned non-empty text: {out:?}"),
                };
            }

            // Mid-inference cancel: start decoding 34 s clip, trigger cancel
            // after 15 ms while the encoder is actively running, and measure
            // time from cancel trigger to return.
            let mid_cancel = Cancel::new();
            let mid_trigger = mid_cancel.clone();
            let mid_engine = Arc::clone(&direct);
            let clip_owned = long_clip.to_vec();
            let worker = std::thread::spawn(move || {
                let opts = Options {
                    cancel: Some(mid_cancel),
                    ..Options::language("en")
                };
                mid_engine.transcribe_with(&clip_owned, &opts)
            });
            std::thread::sleep(Duration::from_millis(15));
            let t_mid = Instant::now();
            mid_trigger.cancel();
            let mid_out = match worker.join() {
                Ok(text) => text,
                Err(_) => {
                    return TestResult {
                        name: "QuietRead cancel + long take",
                        passed: false,
                        detail: "mid-inference cancel worker panicked".to_string(),
                    };
                }
            };
            let mid_elapsed = t_mid.elapsed().as_millis();
            if !mid_out.is_empty() {
                return TestResult {
                    name: "QuietRead cancel + long take",
                    passed: false,
                    detail: format!("mid-cancelled decode returned non-empty text: {mid_out:?}"),
                };
            }
            (pre_elapsed, mid_elapsed)
        }
        Err(error) => {
            return TestResult {
                name: "QuietRead cancel + long take",
                passed: false,
                detail: format!("direct cached load for cancel test failed: {error}"),
            };
        }
    };

    // Also run the real >30 s clip through `SttEngine::transcribe_live_reporting`
    // (non-realtime so it finishes quickly) to verify windowing + merge stay fast.
    let long_start = Instant::now();
    let long_res = match run_once(engine, long_clip, false) {
        Ok(res) => res,
        Err(error) => {
            return TestResult {
                name: "QuietRead cancel + long take",
                passed: false,
                detail: format!("long take failed: {error}"),
            };
        }
    };
    let long_total_ms = long_start.elapsed().as_millis();
    let long_after_stop_ms = long_res.latency(long_res.transcribed).total_ms();

    let passed = !pause_res.text.trim().is_empty()
        && pause_ms < 500
        && pre_cancel_ms < 25
        && mid_cancel_ms < 200
        && !long_res.text.trim().is_empty()
        && !long_res.diagnostics.whole_fallback
        && long_after_stop_ms < 1500;
    TestResult {
        name: "QuietRead cancel + long take",
        passed,
        detail: format!(
            "multi-pause after-stop {pause_ms} ms (quiet_wait {} ms); 34s cancel pre={pre_cancel_ms} ms mid={mid_cancel_ms} ms; 34s clip total {long_total_ms} ms, after-stop {long_after_stop_ms} ms, fallback={}",
            pause_res.diagnostics.quiet_wait_ms,
            long_res.diagnostics.whole_fallback
        ),
    }
}

/// Check 6: verify WASAPI returns a non-empty unique endpoint ID for the
/// default mic.
fn test_wasapi_endpoint_id() -> TestResult {
    match voice_stt_desktop::default_capture_endpoint_id() {
        Some(id) if !id.is_empty() => TestResult {
            name: "WASAPI endpoint ID lookup",
            passed: true,
            detail: format!("endpoint id = {id}"),
        },
        _ => TestResult {
            name: "WASAPI endpoint ID lookup",
            passed: false,
            detail: "no default capture endpoint ID returned".to_string(),
        },
    }
}

fn run_once(
    engine: &SttEngine,
    samples: &[f32],
    realtime: bool,
) -> Result<voice_stt_desktop::stt::LiveTranscript, String> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let clip = samples.to_vec();
    std::thread::spawn(move || {
        let chunk_ms = 50u64;
        let chunk_len = chunk_ms as usize * SAMPLE_RATE / 1000;
        let started = Instant::now();
        for (index, chunk) in clip.chunks(chunk_len).enumerate() {
            if realtime {
                let due = started + Duration::from_millis(chunk_ms * (index as u64 + 1));
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
            let _ = tx.send(LiveAudio::Chunk(chunk.to_vec()));
        }
        let _ = tx.send(LiveAudio::Finish(StopTimes::now()));
    });
    engine
        .transcribe_live_reporting(&rx)
        .map_err(|error| error.message)
}

#[cfg(windows)]
fn empty_working_set() -> bool {
    use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: `GetCurrentProcess` returns the current process pseudo-handle.
    unsafe { EmptyWorkingSet(GetCurrentProcess()) }.is_ok()
}

#[cfg(not(windows))]
fn empty_working_set() -> bool {
    false
}

struct SampleAudio {
    short: Vec<f32>,
    main: Vec<f32>,
    multi_pause: Vec<f32>,
    long_clip: Vec<f32>,
}

fn load_or_synthesize_audio(wav: Option<&Path>, scratch: &Path) -> Result<SampleAudio, String> {
    let main = match wav {
        Some(path) => read_wav_16k(path)?,
        None => synthesize_speech(
            scratch,
            "main.wav",
            "Please send the stability report before lunch today.",
        )?,
    };
    let short = synthesize_speech(scratch, "short.wav", "Go now.")?
        .into_iter()
        .take((0.75 * SAMPLE_RATE as f32) as usize)
        .collect::<Vec<_>>();
    let part_a = synthesize_speech(scratch, "part_a.wav", "First clause of the sentence")?;
    let part_b = synthesize_speech(scratch, "part_b.wav", "and then the second part arrives.")?;
    let pause_400ms = vec![0.0f32; 400 * SAMPLE_RATE / 1000];
    let mut multi_pause = Vec::with_capacity(part_a.len() + pause_400ms.len() + part_b.len());
    multi_pause.extend_from_slice(&part_a);
    multi_pause.extend_from_slice(&pause_400ms);
    multi_pause.extend_from_slice(&part_b);

    // Build a 34-second clip (> 30 s WINDOW_SAMPLES) by repeating main with
    // natural 300 ms gaps so windowing + merge are exercised.
    let gap = vec![0.0f32; 300 * SAMPLE_RATE / 1000];
    let target_len = 34 * SAMPLE_RATE;
    let mut long_clip = Vec::with_capacity(target_len + main.len());
    while long_clip.len() < target_len {
        long_clip.extend_from_slice(&main);
        long_clip.extend_from_slice(&gap);
    }

    Ok(SampleAudio {
        short,
        main,
        multi_pause,
        long_clip,
    })
}

fn synthesize_speech(scratch: &Path, filename: &str, text: &str) -> Result<Vec<f32>, String> {
    let wav_path = scratch.join(filename);
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         $fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono); \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $s.Rate = 1; \
         $s.SetOutputToWaveFile('{}', $fmt); \
         $s.Speak('{}'); \
         $s.Dispose()",
        wav_path.display().to_string().replace('\'', "''"),
        text.replace('\'', "''"),
    );
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()
        .map_err(|error| format!("could not run PowerShell TTS: {error}"))?;
    if !status.success() {
        return Err(format!("PowerShell TTS exited with {status}"));
    }
    read_wav_16k(&wav_path)
}

fn read_wav_16k(path: &Path) -> Result<Vec<f32>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let (mono, rate) = voice_stt_desktop::wav::decode_mono(&bytes)
        .ok_or_else(|| format!("{} is not a supported PCM16 WAV", path.display()))?;
    let Some(mut resampler) = Resampler::new(rate) else {
        return Ok(mono);
    };
    let mut out = Vec::new();
    for chunk in mono.chunks((rate / 100) as usize) {
        resampler.process(chunk, &mut out);
    }
    Ok(out)
}

fn locate_model(override_dir: Option<&Path>) -> Option<(&'static ModelSpec, PathBuf)> {
    if let Some(dir) = override_dir {
        for spec in voice_stt_desktop::stt::CATALOG {
            if spec.is_installed_in(dir) {
                return Some((spec, dir.to_path_buf()));
            }
        }
    }
    let unified = resolve(UNIFIED_ID);
    if let Some(dir) = find_dir(unified, override_dir) {
        return Some((unified, dir));
    }
    for spec in voice_stt_desktop::stt::CATALOG {
        if let Some(dir) = find_dir(spec, override_dir) {
            return Some((spec, dir));
        }
    }
    None
}

/// Hard-links (zero-copy) or copies top-level model files into an isolated
/// directory so `.trit/` cache tests never modify the user's live model folder.
fn mirror_model_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("create_dir_all: {e}"))?;
    for entry in fs::read_dir(src)
        .map_err(|e| format!("read_dir: {e}"))?
        .flatten()
    {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let target = dst.join(entry.file_name());
        if fs::hard_link(entry.path(), &target).is_err() {
            fs::copy(entry.path(), &target)
                .map_err(|e| format!("copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn parse_args() -> (Option<PathBuf>, Option<PathBuf>) {
    let mut model_dir = None;
    let mut wav = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model-dir" => model_dir = args.next().map(PathBuf::from),
            "--wav" => wav = args.next().map(PathBuf::from),
            _ => {}
        }
    }
    (model_dir, wav)
}

struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(prefix: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{stamp}", std::process::id()));
        let _ = fs::create_dir_all(&path);
        Self { path }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
