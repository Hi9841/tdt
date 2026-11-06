//! Opt-in test takes: with the setting on, each dictation keeps the audio
//! the model heard and the model's text in `%LOCALAPPDATA%\TDT\takes`. The
//! user corrects each `.txt` to what was said; the folder is then a test set
//! for `bench-asr --dir`. Nothing leaves the computer.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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
/// the setting is on. Takes of the same millisecond get `-1`, `-2`, and so on.
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
    let wav = wav_bytes(samples);
    let (mut text_file, base) = claim_name(&dir, stamp)
        .map_err(|e| format!("Could not write a take in {}: {e}", dir.display()))?;
    let text_path = base.with_extension("txt");
    let wav_path = base.with_extension("wav");
    let written =
        write_synced(&mut text_file, text.as_bytes()).and_then(|()| write_whole(&wav_path, &wav));
    drop(text_file);
    written.map_err(|e| {
        // A `.txt` with no audio is of no use in the test set.
        let _ = std::fs::remove_file(&text_path);
        format!("Could not write {}: {e}", base.display())
    })
}

/// Creates the `.txt` of a new take. Only one take can create a file, so this
/// also reserves the `.wav`. The base path has no extension.
fn claim_name(dir: &Path, stamp: u128) -> io::Result<(File, PathBuf)> {
    for try_number in 0u32.. {
        let base = dir.join(match try_number {
            0 => format!("take-{stamp}"),
            number => format!("take-{stamp}-{number}"),
        });
        if base.with_extension("wav").exists() {
            continue;
        }
        let new_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(base.with_extension("txt"));
        match new_file {
            Ok(file) => return Ok((file, base)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

fn write_synced(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

/// `path` appears only with all its bytes on the disk: `bench-asr --dir`
/// reads every `.wav`, and a failed write or a crash must not leave half a
/// take there.
fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let part = path.with_extension("wav.part");
    let written = File::create(&part)
        .and_then(|mut file| write_synced(&mut file, bytes))
        .and_then(|()| std::fs::rename(&part, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// A 16 kHz mono 16-bit WAV file.
fn wav_bytes(samples: &[f32]) -> Vec<u8> {
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
    wav
}
