use std::path::{Path, PathBuf};
use std::time::Instant;
use voice_stt_desktop::stt::models::{find_dir, ModelSpec, CATALOG};
use voice_stt_desktop::stt::{pcm16_wav, LiveAudio, SttEngine};

struct Clip {
    name: String,
    samples: Vec<f32>,
    duration_secs: f32,
}

fn load_clips(dir: &Path) -> Vec<Clip> {
    let mut clips = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return clips;
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wav"))
        .collect();
    paths.sort();

    for path in paths {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if let Some(samples) = pcm16_wav(&path) {
            let duration_secs = samples.len() as f32 / 16000.0;
            clips.push(Clip {
                name,
                samples,
                duration_secs,
            });
        }
    }
    clips
}

fn bench_model(spec: &ModelSpec, clips: &[Clip]) {
    println!("\nModel: {} ({})", spec.label, spec.id);
    let Some(model_dir) = find_dir(spec, None) else {
        println!("Skipped: model directory not found.");
        return;
    };

    let t0 = Instant::now();
    let Ok(engine) = SttEngine::new(spec, &model_dir) else {
        println!("Failed to create engine.");
        return;
    };
    if let Err(e) = engine.prepare() {
        println!("Failed to prepare engine: {e}");
        return;
    }
    println!("Init: {} ms", t0.elapsed().as_millis());

    for clip in clips {
        let (tx, rx) = crossbeam_channel::unbounded();
        for chunk in clip.samples.chunks(1600) {
            let _ = tx.send(LiveAudio::Chunk(chunk.to_vec()));
        }
        let _ = tx.send(LiveAudio::Finish(Instant::now()));
        drop(tx);

        let t_infer = Instant::now();
        match engine.transcribe_live_reporting(&rx) {
            Ok(res) => {
                let ms = t_infer.elapsed().as_millis().max(1);
                let rtf = (clip.duration_secs * 1000.0) / ms as f32;
                println!(
                    "[{:.2}s] {} -> {} ms ({:.1}x): {:?}",
                    clip.duration_secs,
                    clip.name,
                    ms,
                    rtf,
                    res.text.trim()
                );
            }
            Err(e) => println!("[{:.2}s] {} -> Error: {e}", clip.duration_secs, clip.name),
        }
    }

    engine.release();
}

fn main() {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(|l| PathBuf::from(l).join("Temp").join("tdt-bench-audio"))
        .unwrap_or_else(|| std::env::temp_dir().join("tdt-bench-audio"));

    let clips = load_clips(&dir);
    if clips.is_empty() {
        println!("No wav clips found in {}", dir.display());
        return;
    }

    println!("Benchmarking {} clips from {}", clips.len(), dir.display());
    for spec in CATALOG {
        bench_model(spec, &clips);
    }
}
