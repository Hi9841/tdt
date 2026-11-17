//! TDT's log, `tdt.log` next to TDT.exe. Each line of the program goes
//! through `voice_stt_desktop::log::note`; `main` makes this file its sink.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

fn log_file() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("tdt.log")))
        .unwrap_or_else(|| std::env::temp_dir().join("tdt.log"))
}

const MAX_LOG_BYTES: u64 = 512 * 1024;

pub fn append(message: &str) {
    let path = log_file();
    let mut path = path;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .or_else(|_| {
            // The exe directory may be read-only (Program Files); fall back
            // to the temp dir once.
            path = std::env::temp_dir().join("tdt.log");
            OpenOptions::new().create(true).append(true).open(&path)
        });
    if let Ok(ref mut file) = file {
        if let Ok(metadata) = file.metadata() {
            if metadata.len() > MAX_LOG_BYTES {
                // Keep the latest half rather than growing forever.
                if let Ok(contents) = std::fs::read(&path) {
                    let keep = &contents[contents.len() / 2..];
                    let _ = std::fs::write(&path, keep);
                }
            }
        }
        let _ = writeln!(file, "{} {message}", crate::clock::local_hms());
    }
}
