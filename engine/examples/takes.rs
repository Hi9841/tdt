//! Runs saved takes through the speech path of the phone app, on this
//! computer, and prints the text and the wait of each. Use it to compare
//! the text with TDT desktop, and one chunk length with another: the text
//! of a take must be the same for each.
//!
//! `cargo run --release --example takes -- --model DIR --dir DIR
//!   [--precision NAME] [--end-quiet-ms N] [--dictionary FILE] [--chunk-ms N] [--realtime] [--whole] [--limit N]`
//!
//! - `--model`: a model folder of TDT (Parakeet Redux, or Parakeet Unified
//!   English with `--precision int8`).
//! - `--dir`: a folder of 16 kHz PCM16 WAV files.
//! - `--dictionary`: a file of terms to favor, one per line, as the user's
//!   dictionary in the app. Lines that start with `#` are ignored.
//! - `--chunk-ms`: the length of the chunks that the take gets (20 by
//!   default, as from the phone's recorder).
//! - `--realtime`: feeds the audio at speaking pace, so the wait is the wait
//!   of a user after the stop. Without it, all audio comes at once.
//! - `--whole`: one read of each whole clip after its end, as the app did
//!   before 0.6.0.
//!
//! Each line: the clip, the wait in ms, the audio in ms that the model read
//! after the stop, and the text.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tdt_speech::{Engine, Take};

struct Options {
    model: PathBuf,
    dir: PathBuf,
    precision: trit::Precision,
    /// The app's `ModelPack.endQuietMillis`: 480 for Parakeet Unified English.
    end_quiet_ms: usize,
    dictionary: Option<PathBuf>,
    chunk_ms: u64,
    realtime: bool,
    whole: bool,
    /// Push 20 ms of the stop cue after the stop, as the phone's recorder
    /// can: the chunk it was reading when the user tapped stop.
    cue: bool,
    /// Start each take with the 40 ms of near-silence that the OnePlus 13R's
    /// microphone sends after it opens (10 ms of zeros, 30 ms at 1 LSB).
    warmup: bool,
    limit: usize,
}

fn parse_options() -> Result<Options, String> {
    let mut model = None;
    let mut dir = None;
    let mut options = Options {
        model: PathBuf::new(),
        dir: PathBuf::new(),
        precision: trit::Precision::Auto,
        end_quiet_ms: 0,
        dictionary: None,
        chunk_ms: 20,
        realtime: false,
        whole: false,
        cue: false,
        warmup: false,
        limit: usize::MAX,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--model" => model = Some(PathBuf::from(value()?)),
            "--dir" => dir = Some(PathBuf::from(value()?)),
            "--precision" => options.precision = trit::Precision::parse(&value()?)?,
            "--end-quiet-ms" => {
                options.end_quiet_ms = value()?
                    .parse()
                    .map_err(|_| "--end-quiet-ms needs a number")?;
            }
            "--dictionary" => options.dictionary = Some(PathBuf::from(value()?)),
            "--chunk-ms" => {
                options.chunk_ms = value()?.parse().map_err(|_| "--chunk-ms needs a number")?;
            }
            "--limit" => options.limit = value()?.parse().map_err(|_| "--limit needs a number")?,
            "--realtime" => options.realtime = true,
            "--whole" => options.whole = true,
            "--cue" => options.cue = true,
            "--warmup" => options.warmup = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    options.model = model.ok_or("--model DIR is missing")?;
    options.dir = dir.ok_or("--dir DIR is missing")?;
    options.chunk_ms = options.chunk_ms.max(1);
    Ok(options)
}

/// The text, the wait after the last audio, and the samples read in that wait.
fn run_take(
    engine: &Arc<Engine>,
    pcm: &[i16],
    options: &Options,
) -> Result<(String, Duration, usize), String> {
    let take = Take::start(engine);
    let started = Instant::now();
    // 16 samples for each millisecond at 16 kHz.
    for (index, chunk) in pcm.chunks(options.chunk_ms as usize * 16).enumerate() {
        if options.realtime {
            let due = started + Duration::from_millis(options.chunk_ms * (index as u64 + 1));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        take.push(chunk);
    }
    let stopped = Instant::now();
    take.ask_stop();
    if options.cue {
        take.push(&stop_cue());
    }
    let heard = take.finish()?;
    Ok((heard.text, stopped.elapsed(), heard.last_part))
}

/// 20 ms of the app's stop cue (a 760 Hz tone at 0.09 of full scale) as
/// the microphone hears it next to the speaker.
fn stop_cue() -> Vec<i16> {
    (0..320)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * 760.0 * i as f32 / 16_000.0;
            (0.09 * phase.sin() * 32767.0) as i16
        })
        .collect()
}

fn run_whole(engine: &Arc<Engine>, samples: &[f32]) -> Result<(String, Duration, usize), String> {
    let model = engine.load()?;
    let started = Instant::now();
    let text = model.transcribe_with(samples, &trit::Options::language("en"));
    Ok((text, started.elapsed(), samples.len()))
}

fn main() -> Result<(), String> {
    let options = parse_options()?;
    let mut clips: Vec<PathBuf> = std::fs::read_dir(&options.dir)
        .map_err(|error| format!("cannot read {}: {error}", options.dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
        .collect();
    clips.sort();
    clips.truncate(options.limit);

    let engine = Arc::new(Engine::new(
        &options.model,
        options.precision,
        options.end_quiet_ms * 16,
    ));
    let terms = match &options.dictionary {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?
            .lines()
            .map(str::trim)
            // TDT desktop's dictionary file has comment lines.
            .filter(|term| !term.is_empty() && !term.starts_with('#'))
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    let started = Instant::now();
    let model = engine.load()?;
    if options.whole {
        model.set_dictionary(&terms, trit::BiasWeights::default());
    } else {
        engine.set_dictionary(terms);
    }
    eprintln!(
        "load {} ms, {}",
        started.elapsed().as_millis(),
        trit::cpu_path()
    );

    let mut waits = Vec::new();
    let mut ready = 0;
    for clip in &clips {
        // `read_wav` gives value / 32768, which is exact for PCM16, so this
        // gives the values of the file again, as the phone's recorder has them.
        let mut pcm: Vec<i16> = trit::read_wav(clip)?
            .iter()
            .map(|sample| (sample * 32768.0) as i16)
            .collect();
        if options.warmup {
            let start = (0..640).map(|i| {
                if i < 160 {
                    0
                } else if i % 2 == 0 {
                    1
                } else {
                    -1
                }
            });
            pcm.splice(0..0, start);
        }
        let (text, wait, last_part) = if options.whole {
            run_whole(&engine, &tdt_speech::samples_of(&pcm))?
        } else {
            run_take(&engine, &pcm, &options)?
        };
        let name = clip.file_name().unwrap_or_default().to_string_lossy();
        println!(
            "{name}\t{}\t{}\t{}",
            wait.as_millis(),
            last_part / 16,
            text.trim()
        );
        ready += usize::from(last_part == 0);
        waits.push(wait.as_millis());
    }
    waits.sort_unstable();
    let total: u128 = waits.iter().sum();
    eprintln!(
        "clips={} wait_mean_ms={} wait_p50_ms={} wait_max_ms={} read_before_the_stop={ready}",
        waits.len(),
        total / waits.len().max(1) as u128,
        waits.get(waits.len() / 2).copied().unwrap_or(0),
        waits.last().copied().unwrap_or(0),
    );
    Ok(())
}
