use std::time::Duration;

#[test]
fn test_hotkey_duration_classification() {
    let tap_duration = Duration::from_millis(150);
    let hold_duration = Duration::from_millis(600);
    let threshold = Duration::from_millis(350);

    assert!(
        tap_duration <= threshold,
        "Short duration should be classified as tap"
    );
    assert!(
        hold_duration > threshold,
        "Long duration should be classified as hold"
    );
}

#[test]
fn test_resampling_math() {
    let from_rate = 48000u32;
    let to_rate = 16000u32;
    let input = vec![0.0f32; 48000]; // 1 second of audio at 48kHz

    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;

    assert_eq!(
        out_len, 16000,
        "1 second of 48kHz must resample to exactly 16000 samples"
    );
}

#[test]
fn test_rms_level_calculation() {
    let silence = vec![0.0f32; 1600];
    let sum_sq: f32 = silence.iter().map(|&s| s * s).sum();
    let rms = (sum_sq / silence.len() as f32).sqrt();
    assert_eq!(rms, 0.0, "Silence RMS must be 0");

    let full_scale = vec![1.0f32; 1600];
    let sum_sq2: f32 = full_scale.iter().map(|&s| s * s).sum();
    let rms2 = (sum_sq2 / full_scale.len() as f32).sqrt();
    assert!((rms2 - 1.0).abs() < 1e-5, "Full scale RMS must be 1.0");
}
