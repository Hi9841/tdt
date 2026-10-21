//! Speech benchmark: latency after release and word error rate.
//!
//! `cargo run --release --bin bench-asr -- [--dir PATH] [--realtime] [--repeat N] [--native]`
//!
//! Each `clip.wav` may have a `clip.txt` reference transcript next to it.
//! `--realtime` feeds audio at speaking pace, so latency is the wait a user
//! has after the key is released. Without it, all audio arrives at once.
//! `--native` runs TDT's Rust engine on each whole clip instead of the app path.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use voice_stt_desktop::stt::models::{find_dir, ModelSpec, CATALOG};
use voice_stt_desktop::stt::native::{BiasWeights, NativeParakeet};
use voice_stt_desktop::stt::{LiveAudio, SttEngine};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// 100 ms at 16 kHz, the size of one capture chunk.
const CHUNK_SAMPLES: usize = 1600;
/// The app stops recording after 150 ms of quiet, so a take never ends in
/// more silence than this. Speech synthesis leaves about 800 ms.
const KEPT_TRAILING_QUIET: usize = 150 * 16;
/// Frames under this level count as the quiet after the last word.
const TRAILING_QUIET_DB: f32 = -45.0;

struct Clip {
    name: String,
    samples: Vec<f32>,
    duration_secs: f32,
    reference: Option<String>,
}

struct Options {
    dir: PathBuf,
    realtime: bool,
    native: bool,
    repeat: usize,
}

fn parse_options() -> Options {
    let mut options = Options {
        dir: std::env::var_os("LOCALAPPDATA").map_or_else(
            || std::env::temp_dir().join("tdt-bench-audio"),
            |l| PathBuf::from(l).join("Temp").join("tdt-bench-audio"),
        ),
        realtime: false,
        native: false,
        repeat: 1,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => options.dir = args.next().map(PathBuf::from).unwrap_or(options.dir),
            "--realtime" => options.realtime = true,
            "--native" => options.native = true,
            "--repeat" => {
                options.repeat = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1)
                    .max(1);
            }
            other => eprintln!("ignored argument {other}"),
        }
    }
    options
}

fn load_clips(dir: &Path) -> Vec<Clip> {
    // Recursive: nested bench folders hold grouped clips and must not be
    // silently skipped.
    let mut clips = Vec::new();
    load_clips_into(dir, &mut clips);
    clips.sort_by(|a, b| a.name.cmp(&b.name));
    clips
}

fn load_clips_into(dir: &Path, clips: &mut Vec<Clip>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_clips_into(&path, clips);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("wav") {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some(mut samples) = pcm16_wav(&path) {
            trim_trailing_quiet(&mut samples);
            let duration_secs = samples.len() as f32 / 16000.0;
            let reference = std::fs::read_to_string(path.with_extension("txt")).ok();
            clips.push(Clip {
                name,
                samples,
                duration_secs,
                reference,
            });
        }
    }
}

/// Cut the silence after the last word down to what the app's tail keeps.
fn trim_trailing_quiet(samples: &mut Vec<f32>) {
    let frame = 160;
    let mut end = samples.len();
    while end >= frame {
        let window = &samples[end - frame..end];
        let power = window.iter().map(|s| s * s).sum::<f32>() / frame as f32;
        if 10.0 * (power + 1e-12).log10() >= TRAILING_QUIET_DB {
            break;
        }
        end -= frame;
    }
    samples.truncate((end + KEPT_TRAILING_QUIET).min(samples.len()));
}

/// Lowercase words with punctuation removed, so WER counts words, not style.
fn normalize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Words as written, with case and punctuation, to catch formatting damage.
fn raw_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

/// Word-level edit distance: substitutions, insertions, and deletions.
fn word_errors(reference: &[String], hypothesis: &[String]) -> usize {
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, ref_word) in reference.iter().enumerate() {
        let mut current = vec![i + 1; hypothesis.len() + 1];
        for (j, hyp_word) in hypothesis.iter().enumerate() {
            let substitution = previous[j] + usize::from(ref_word != hyp_word);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[hypothesis.len()]
}

fn feed(clip: &Clip, realtime: bool) -> crossbeam_channel::Receiver<LiveAudio> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let samples = clip.samples.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        for (index, chunk) in samples.chunks(CHUNK_SAMPLES).enumerate() {
            if realtime {
                let due = started + Duration::from_millis(100 * (index as u64 + 1));
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
            let _ = tx.send(LiveAudio::Chunk(chunk.to_vec()));
        }
        let _ = tx.send(LiveAudio::Finish(Instant::now()));
    });
    rx
}

fn bench_model(spec: &ModelSpec, clips: &[Clip], options: &Options) {
    println!("\nModel: {} ({})", spec.label, spec.id);
    let Some(model_dir) = find_dir(spec, None) else {
        println!("Skipped: model directory not found.");
        return;
    };

    let t0 = Instant::now();
    let engine = SttEngine::new(&model_dir);
    if let Err(e) = engine.prepare() {
        println!("Failed to prepare engine: {e}");
        return;
    }
    println!("Init: {} ms", t0.elapsed().as_millis());

    for run in 1..=options.repeat {
        let mut latencies = Vec::new();
        let mut errors = 0;
        let mut format_errors = 0;
        let mut reference_words = 0;
        for clip in clips {
            let rx = feed(clip, options.realtime);
            match engine.transcribe_live_reporting(&rx) {
                Ok(res) => {
                    latencies.push(res.latency_ms);
                    let mut wer_note = String::new();
                    if let Some(reference) = &clip.reference {
                        format_errors += word_errors(&raw_words(reference), &raw_words(&res.text));
                        let reference = normalize_words(reference);
                        let clip_errors = word_errors(&reference, &normalize_words(&res.text));
                        errors += clip_errors;
                        reference_words += reference.len();
                        wer_note = format!(" {clip_errors}/{} errors", reference.len());
                    }
                    println!(
                        "[{:.2}s] {} -> {} ms{}: {:?}",
                        clip.duration_secs,
                        clip.name,
                        res.latency_ms,
                        wer_note,
                        res.text.trim()
                    );
                }
                Err(e) => println!("[{:.2}s] {} -> Error: {e}", clip.duration_secs, clip.name),
            }
        }
        latencies.sort_unstable();
        let p50 = latencies.get(latencies.len() / 2).copied().unwrap_or(0);
        let total: u64 = latencies.iter().sum();
        let wer = if reference_words == 0 {
            f64::NAN
        } else {
            100.0 * errors as f64 / reference_words as f64
        };
        println!(
            "SUMMARY run={run} realtime={} clips={} latency_p50_ms={p50} latency_total_ms={total} wer_pct={wer:.2} errors={errors} format_errors={format_errors} words={reference_words}",
            options.realtime,
            latencies.len()
        );
    }

    engine.release();
}

/// Scores one transcript; returns (word errors, format errors, reference words).
fn score(reference: &str, text: &str) -> (usize, usize, usize) {
    let format = word_errors(&raw_words(reference), &raw_words(text));
    let reference = normalize_words(reference);
    (
        word_errors(&reference, &normalize_words(text)),
        format,
        reference.len(),
    )
}

fn bench_native(clips: &[Clip], options: &Options) {
    let Some(model_dir) = find_dir(&CATALOG[0], None) else {
        println!("Skipped: model directory not found.");
        return;
    };
    let started = Instant::now();
    let engine = match NativeParakeet::load(&model_dir) {
        Ok(engine) => engine,
        Err(e) => {
            println!("Failed to load the native engine: {e}");
            return;
        }
    };
    println!(
        "\nNative engine. Init: {} ms",
        started.elapsed().as_millis()
    );
    // TDT_DICTIONARY: one term per line. TDT_BIAS: "start,cont,margin[,min_start,min_prefix]".
    if let Some(path) = std::env::var_os("TDT_DICTIONARY") {
        let terms: Vec<String> = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let mut weights = BiasWeights::default();
        if let Ok(spec) = std::env::var("TDT_BIAS") {
            let values: Vec<f32> = spec
                .split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect();
            if let [start, cont, margin, rest @ ..] = values.as_slice() {
                weights = BiasWeights {
                    start: *start,
                    cont: *cont,
                    margin: *margin,
                    min_start: rest.first().map_or(weights.min_start, |v| *v as usize),
                    min_prefix: rest.get(1).map_or(weights.min_prefix, |v| *v as usize),
                };
            }
        }
        let skipped = engine.set_dictionary(&terms, weights);
        println!(
            "Dictionary: {} terms, {weights:?}, skipped {skipped:?}",
            terms.len() - skipped.len()
        );
    }
    whole_clips(clips, options, "native", |samples| {
        engine.transcribe(samples)
    });
}

/// Transcribes each whole clip in one call and reports latency and WER.
fn whole_clips(
    clips: &[Clip],
    options: &Options,
    label: &str,
    mut transcribe: impl FnMut(&[f32]) -> String,
) {
    for run in 1..=options.repeat {
        let (mut errors, mut format_errors, mut words) = (0, 0, 0);
        let mut latencies = Vec::new();
        for clip in clips {
            let started = Instant::now();
            let text = transcribe(&clip.samples);
            let elapsed = started.elapsed();
            latencies.push(elapsed.as_millis() as u64);
            let mut note = String::new();
            if let Some(reference) = &clip.reference {
                let (e, f, w) = score(reference, &text);
                errors += e;
                format_errors += f;
                words += w;
                note = format!(" {e}/{w} errors");
            }
            println!(
                "[{:.2}s] {} -> {:.1} ms{note}: {text:?}",
                clip.duration_secs,
                clip.name,
                elapsed.as_secs_f64() * 1e3
            );
        }
        latencies.sort_unstable();
        let total: u64 = latencies.iter().sum();
        println!(
            "SUMMARY run={run} engine={label} clips={} latency_p50_ms={} latency_total_ms={total} wer_pct={:.2} errors={errors} format_errors={format_errors} words={words}",
            latencies.len(),
            latencies[latencies.len() / 2],
            100.0 * errors as f64 / words.max(1) as f64
        );
    }
}

fn main() {
    let options = parse_options();
    let clips = load_clips(&options.dir);
    if clips.is_empty() {
        println!("No wav clips found in {}", options.dir.display());
        return;
    }

    println!(
        "Benchmarking {} clips from {}",
        clips.len(),
        options.dir.display()
    );
    if options.native {
        bench_native(&clips, &options);
        return;
    }
    for spec in CATALOG {
        bench_model(spec, &clips, &options);
    }
}

/// 16 kHz PCM16 WAV as mono f32; None for any other format.
fn pcm16_wav(path: &Path) -> Option<Vec<f32>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut channels = 1u16;
    let mut rate = 0u32;
    let mut bits = 16u16;
    let mut data: Option<&[u8]> = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let body = offset + 8;
        if body + size > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().ok()?);
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().ok()?);
        } else if id == b"data" {
            data = Some(&bytes[body..body + size]);
        }
        offset = body + size + (size % 2);
    }
    let data = data?;
    if bits != 16 || channels == 0 || rate != 16_000 {
        return None;
    }
    let width = channels as usize;
    let mut mono = Vec::with_capacity(data.len() / (2 * width));
    for frame in data.chunks_exact(2 * width) {
        let mut sum = 0.0f32;
        for channel in 0..width {
            let sample = i16::from_le_bytes([frame[channel * 2], frame[channel * 2 + 1]]);
            sum += f32::from(sample) / f32::from(i16::MAX);
        }
        mono.push(sum / width as f32);
    }
    Some(mono)
}
