//! Opt-in test takes: with the setting on, each dictation keeps the audio
//! the model heard and the model's text in `%LOCALAPPDATA%\TDT\takes`. The
//! user corrects each `.txt` to what was said; the folder is then a test set
//! for `bench-asr --dir`. Nothing leaves the computer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static SAVING: AtomicBool = AtomicBool::new(false);

pub fn set_saving(on: bool) {
    SAVING.store(on, Ordering::Relaxed);
}

pub fn saving() -> bool {
    SAVING.load(Ordering::Relaxed)
}

/// `%LOCALAPPDATA%\TDT\takes`.
pub fn folder() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("TDT").join("takes"))
}

/// Writes `take-<unix milliseconds>.wav` (16 kHz mono, 16-bit) and `.txt` when
/// the setting is on.
pub fn save(samples: &[f32], text: &str) -> Result<(), String> {
    if !saving() || samples.is_empty() {
        return Ok(());
    }
    let dir = folder().ok_or("LOCALAPPDATA is not set")?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_millis());
    let base = dir.join(format!("take-{stamp}"));
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    // PCM, mono, 16 kHz, 32000 bytes per second, 2-byte frames, 16 bits.
    for field in [
        16u32.to_le_bytes(),
        [1, 0, 1, 0],
        16_000u32.to_le_bytes(),
        32_000u32.to_le_bytes(),
        [2, 0, 16, 0],
    ] {
        wav.extend_from_slice(&field);
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    let write = |ext: &str, bytes: &[u8]| {
        let path = base.with_extension(ext);
        std::fs::write(&path, bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))
    };
    write("wav", &wav)?;
    write("txt", text.as_bytes())
}
