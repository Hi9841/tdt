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

static LOG_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

pub fn append(message: &str) {
    let _guard = LOG_LOCK.lock();
    let mut path = log_file();
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
    if let Ok(existing) = file {
        let needs_rotate = existing
            .metadata()
            .is_ok_and(|meta| meta.len() > MAX_LOG_BYTES);
        if needs_rotate {
            drop(existing);
            if let Ok(contents) = std::fs::read(&path) {
                let half = contents.len() / 2;
                let start = contents[half..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(contents.len(), |offset| half + offset + 1);
                let _ = std::fs::write(&path, &contents[start..]);
            }
            file = OpenOptions::new().create(true).append(true).open(&path);
        } else {
            file = Ok(existing);
        }
    }
    if let Ok(ref mut file) = file {
        let _ = writeln!(file, "{} {message}", crate::clock::local_hms());
    }
}
