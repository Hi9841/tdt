//! Opt-in replay through the same channel and recognizer used by dictation.
//! TDT_BENCH_WAV must contain mono, 16 kHz speech.
use super::{LiveAudio, SttEngine};
use crate::stt::models::DEFAULT;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires installed Parakeet Q8 and TDT_BENCH_WAV speech fixture"]
fn speech_response_latency() {
    let wave = sherpa_onnx::Wave::read(&std::env::var("TDT_BENCH_WAV").expect("TDT_BENCH_WAV"))
        .expect("read mono WAV fixture");
    assert_eq!(wave.sample_rate(), 16000);
    let samples = wave.samples();
    assert!(!samples.is_empty(), "fixture must contain speech");
    // Speech fixtures end on a word boundary, so they never exercise the case
    // that matters most in the field: the user letting go of the hotkey part
    // way through a word. Cutting the tail simulates exactly that.
    let samples = match std::env::var("TDT_BENCH_KEEP_MS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(keep_ms) => &samples[..samples.len().min(keep_ms * 16)],
        None => samples,
    };
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("TDT/models/parakeet-unified-en-0.6b-q8");
    let engine = SttEngine::new(DEFAULT, &dir, "en").expect("installed model");
    if std::env::var("TDT_BENCH_WARM").as_deref() == Ok("1") {
        let started = Instant::now();
        engine.prepare().expect("startup preparation");
        println!("prepare_ms={}", started.elapsed().as_millis());
    }
    for run in 0..4 {
        let (tx, rx) = crossbeam_channel::unbounded();
        let result = std::thread::scope(|scope| {
            let worker = scope.spawn(|| engine.transcribe_live(&rx).expect("transcribe"));
            let started = Instant::now();
            for (i, chunk) in samples.chunks(400).enumerate() {
                let due =
                    started + Duration::from_secs_f64(((i * 400 + chunk.len()) as f64) / 16000.0);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                tx.send(LiveAudio::Chunk(chunk.to_vec()))
                    .expect("send audio");
            }
            tx.send(LiveAudio::Finish(Instant::now())).expect("finish");
            worker.join().expect("worker")
        });
        println!(
            "run={run} audio_secs={:.3} response_ms={} text={:?}",
            result.duration_secs, result.latency_ms, result.text
        );
        assert!(!result.text.is_empty(), "speech must produce text");
        if let Ok(expected) = std::env::var("TDT_BENCH_EXPECT") {
            assert_eq!(result.text, expected, "transcription must remain unchanged");
        }
        if let Ok(limit) = std::env::var("TDT_BENCH_MAX_MS") {
            assert!(
                result.latency_ms <= limit.parse::<u64>().expect("milliseconds"),
                "response exceeded {limit} ms"
            );
        }
    }
}

/// Where the response time actually goes.
///
/// `speech_response_latency` reports one number, the tail between `Finish` and
/// text. That number alone cannot be improved, because it does not say whether
/// the tail is work that arrived late or work that is simply expensive. This
/// replays the same fixture at the same pace and splits the two:
///
/// - `inline_decode_ms` is CPU spent decoding while the user was still talking.
///   Compare it against `audio_secs` to get the real-time factor. A factor well
///   under 1.0 means the decoder keeps up and there is no backlog to clear.
/// - `flush_ms` is `finish_utterance` alone: the trailing decode, `input_finished`,
///   and `get_result`. This is the part a user waits through in silence.
///
/// It calls the same public methods `transcribe_live` calls, in the same order,
/// so it measures the shipping path without modifying it.
#[test]
#[ignore = "requires installed Parakeet Q8 and TDT_BENCH_WAV speech fixture"]
fn response_time_breakdown() {
    let wave = sherpa_onnx::Wave::read(&std::env::var("TDT_BENCH_WAV").expect("TDT_BENCH_WAV"))
        .expect("read mono WAV fixture");
    assert_eq!(wave.sample_rate(), 16000);
    let samples = wave.samples();
    assert!(!samples.is_empty(), "fixture must contain speech");
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("TDT/models/parakeet-unified-en-0.6b-q8");
    let engine = SttEngine::new(DEFAULT, &dir, "en").expect("installed model");
    engine.prepare().expect("startup preparation");

    // Appending trailing silence separates two questions that look identical
    // from the outside: whether the flush cost tracks real audio, or whether it
    // is a fixed window/padding cost that no amount of trailing audio changes.
    let trailing_ms: usize = std::env::var("TDT_BENCH_TRAILING_SILENCE_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut padded = samples.to_vec();
    padded.resize(padded.len() + trailing_ms * 16, 0.0);
    let samples = padded.as_slice();

    let audio_secs = samples.len() as f32 / 16000.0;
    for run in 0..3 {
        // Mirrors transcribe_live: decode each chunk the moment it arrives.
        let mut utterance = engine.start_utterance().expect("utterance");
        let started = Instant::now();
        let mut inline_decode = Duration::ZERO;
        let mut slowest_chunk = Duration::ZERO;
        for (i, chunk) in samples.chunks(400).enumerate() {
            let due = started + Duration::from_secs_f64(((i * 400 + chunk.len()) as f64) / 16000.0);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let decode_started = Instant::now();
            engine.push_audio(&mut utterance, chunk);
            let spent = decode_started.elapsed();
            slowest_chunk = slowest_chunk.max(spent);
            inline_decode += spent;
        }

        // The user has stopped talking. Everything from here is the silent wait.
        let stopped_at = Instant::now();
        let text = engine.finish_utterance(utterance).expect("finish");
        let flush = stopped_at.elapsed();

        let inline_ms = inline_decode.as_secs_f64() * 1000.0;
        let real_time_factor = inline_decode.as_secs_f64() / audio_secs as f64;
        println!(
            "run={run} audio_secs={audio_secs:.3} inline_decode_ms={inline_ms:.0} \
             rtf={real_time_factor:.3} flush_ms={} slowest_chunk_us={} chars={}",
            flush.as_millis(),
            slowest_chunk.as_micros(),
            text.len()
        );
        assert!(!text.trim().is_empty(), "speech must produce text");
    }
}
