/// The speech model reads 16 kHz mono audio. The capture, the takes, and the
/// bench all give it audio at this rate.
pub const SAMPLE_RATE: usize = 16_000;

/// `%LOCALAPPDATA%\TDT`: the models, the dictionary, and the test takes.
/// Setup installs TDT.exe here too, and uninstall keeps `models` and
/// `dictionary.txt` (`packaging/tdt-setup`, `USER_DATA`).
pub fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| std::path::PathBuf::from(dir).join("TDT"))
}

pub mod log;
pub mod resample;
pub mod stt;
pub mod wav;
