use std::path::{Path, PathBuf};
use std::time::Instant;
use voice_stt_desktop::stt::models::{find_dir, ModelSpec, CATALOG};
use voice_stt_desktop::stt::{pcm16_wav, LiveAudio, SttEngine};

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[link(name = "psapi")]
extern "system" {
    fn K32GetProcessMemoryInfo(
        process: *mut std::ffi::c_void,
        counters: *mut ProcessMemoryCounters,
        cb: u32,
    ) -> i32;
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
}

fn get_working_set_mb() -> f64 {
    unsafe {
        let mut pmc = std::mem::zeroed::<ProcessMemoryCounters>();
        pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) != 0 {
            pmc.working_set_size as f64 / (1024.0 * 1024.0)
        } else {
            0.0
        }
    }
}

struct BenchClip {
    #[allow(dead_code)]
    name: String,
    bucket: &'static str,
    samples: Vec<f32>,
    duration_secs: f32,
}

fn load_bench_clips(dir: &Path) -> Vec<BenchClip> {
    let mut clips = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return clips,
    };

    let mut sorted_entries = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("wav") {
            sorted_entries.push(path);
        }
    }
    sorted_entries.sort();

    for path in sorted_entries {
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some(samples) = pcm16_wav(&path) {
            let duration_secs = samples.len() as f32 / 16000.0;
            let bucket = match duration_secs {
                d if d < 1.0 => "0.5-1s",
                d if d < 2.0 => "1-2s",
                d if d < 4.0 => "2-4s",
                d if d < 8.0 => "4-8s",
                d if d < 15.0 => "8-15s",
                d if d < 30.0 => "15-30s",
                _ => "30+s",
            };
            clips.push(BenchClip {
                name: filename,
                bucket,
                samples,
                duration_secs,
            });
        }
    }
    clips
}

fn run_engine_benchmark(spec: &ModelSpec, clips: &[BenchClip]) {
    println!("\n=======================================================");
    println!("Benchmarking Model: {} ({})", spec.label, spec.id);
    println!("Backend: {:?}", spec.backend);
    println!("=======================================================");

    let model_dir = match find_dir(spec, None) {
        Some(dir) => dir,
        None => {
            println!(
                "SKIPPED: Model directory not found for {}. Run download-models.ps1 first.",
                spec.id
            );
            return;
        }
    };

    // Measure Model Load / Init Time
    let t_load_start = Instant::now();
    let engine = match SttEngine::new(spec, &model_dir, "en") {
        Ok(e) => e,
        Err(err) => {
            println!("ERROR: Failed to create engine: {err}");
            return;
        }
    };

    if let Err(e) = engine.prepare() {
        println!("ERROR: Failed to prepare engine: {e}");
        return;
    }
    let load_time_ms = t_load_start.elapsed().as_millis();
    let ram_after_load = get_working_set_mb();
    println!(
        "Model load time: {} ms | Process RAM: {:.1} MB",
        load_time_ms, ram_after_load
    );

    println!("\n| Bucket | Audio (s) | Latency (ms) | RTF (× realtime) | Output |");
    println!("|---|---|---|---|---|");

    for clip in clips {
        // Run inference
        let (tx, rx) = crossbeam_channel::unbounded();
        for chunk in clip.samples.chunks(1600) {
            let _ = tx.send(LiveAudio::Chunk(chunk.to_vec()));
        }
        let stop_at = Instant::now();
        let _ = tx.send(LiveAudio::Finish(stop_at));
        drop(tx);

        let t_infer_start = Instant::now();
        match engine.transcribe_live_reporting(&rx, None) {
            Ok(transcript) => {
                let infer_time = t_infer_start.elapsed();
                let latency_ms = infer_time.as_millis().max(1) as f32;
                let rtf = (clip.duration_secs * 1000.0) / latency_ms;
                let preview = if transcript.text.len() > 40 {
                    format!("{}...", &transcript.text[..37])
                } else {
                    transcript.text.clone()
                };

                println!(
                    "| {:<6} | {:>7.2}s | {:>10}ms | {:>14.1}× | {:<40} |",
                    clip.bucket,
                    clip.duration_secs,
                    infer_time.as_millis(),
                    rtf,
                    preview
                );
            }
            Err(e) => {
                println!(
                    "| {:<6} | {:>7.2}s | ERROR: {} |",
                    clip.bucket, clip.duration_secs, e
                );
            }
        }
    }

    engine.release();
}

fn main() {
    let bench_dir = std::env::var_os("LOCALAPPDATA")
        .map(|l| PathBuf::from(l).join("Temp").join("tdt-bench-audio"))
        .unwrap_or_else(|| std::env::temp_dir().join("tdt-bench-audio"));

    println!("TDT ASR Diagnostic Benchmark Harness");
    println!("Reading bench audio clips from: {}", bench_dir.display());

    let clips = load_bench_clips(&bench_dir);
    if clips.is_empty() {
        println!(
            "No benchmark audio clips found in {}. Generating audio first...",
            bench_dir.display()
        );
        return;
    }

    println!(
        "Loaded {} test audio clips covering all duration brackets:",
        clips.len()
    );
    for clip in &clips {
        println!(
            " - {:<6} [{:.2}s]: {}",
            clip.bucket, clip.duration_secs, clip.name
        );
    }

    for spec in CATALOG {
        run_engine_benchmark(spec, &clips);
    }

    println!("\nBenchmark completed.");
}
