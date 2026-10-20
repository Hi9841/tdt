//! Compares the engine with tensors dumped from the open reference
//! (`ref_tdt.py --dump desktop/target/native-ref`). Needs the installed model:
//! `cargo test --release --lib native -- --ignored --nocapture`.

use super::NativeParakeet;
use crate::stt::models::{find_dir, DEFAULT};
use std::path::PathBuf;

fn reference_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("TDT_NATIVE_REF")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/native-ref"));
    dir.join("shapes.json").is_file().then_some(dir)
}

fn load(dir: &std::path::Path, name: &str) -> Vec<f32> {
    std::fs::read(dir.join(format!("{name}.f32")))
        .expect("reference tensor")
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// Largest absolute difference and the norm of the difference relative to the reference.
fn compare(name: &str, ours: &[f32], reference: &[f32]) -> f32 {
    assert_eq!(ours.len(), reference.len(), "{name} length");
    let mut max = 0.0f32;
    let (mut diff, mut norm) = (0.0f64, 0.0f64);
    for (a, b) in ours.iter().zip(reference) {
        max = max.max((a - b).abs());
        diff += f64::from(a - b).powi(2);
        norm += f64::from(*b).powi(2);
    }
    let relative = (diff / norm.max(1e-30)).sqrt() as f32;
    println!("{name:12} max abs {max:.3e}  relative {relative:.3e}");
    relative
}

#[test]
#[ignore = "needs the installed model and a reference dump"]
fn the_engine_matches_the_open_reference() {
    let (Some(dir), Some(model_dir)) = (reference_dir(), find_dir(DEFAULT, None)) else {
        println!("skipped: no reference dump or model");
        return;
    };
    let started = std::time::Instant::now();
    let engine = NativeParakeet::load(&model_dir).expect("load");
    println!("load {} ms", started.elapsed().as_millis());

    let audio = load(&dir, "audio");
    let (features, frames) = engine.frontend.compute(&audio);
    assert!(compare("features", &features, &load(&dir, "features")) < 1e-3);

    let started = std::time::Instant::now();
    let mut layers = Vec::new();
    let (encoded, t) = engine.model.encode_inspect(&features, frames, |index, x| {
        if index == 0 || index == 1 || index == 12 {
            layers.push((index, x.to_vec()));
        }
    });
    println!("encode {} ms for {t} frames", started.elapsed().as_millis());
    for (index, x) in &layers {
        let name = match index {
            0 => "subsampling",
            1 => "layer0",
            _ => "layer11",
        };
        assert!(compare(name, x, &load(&dir, name)) < 1e-2);
    }
    assert!(compare("encoder", &encoded, &load(&dir, "encoder")) < 2e-2);
    let projected = engine.model.project(&encoded, t);
    compare("projected", &projected, &load(&dir, "projected"));

    let tokens = engine.model.decode(&projected, t);
    let expected: Vec<usize> = load(&dir, "tokens").iter().map(|v| *v as usize).collect();
    println!("text {:?}", engine.tokenizer.decode(&tokens));
    assert_eq!(tokens, expected);
}
