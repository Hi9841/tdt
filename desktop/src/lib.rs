/// The speech model reads 16 kHz mono audio. The capture, the takes, and the
/// bench all give it audio at this rate.
pub const SAMPLE_RATE: usize = 16_000;

/// Per-user TDT data directory: `%LOCALAPPDATA%\TDT` on Windows, or
/// `~/Library/Application Support/TDT` on macOS (with `LOCALAPPDATA` honored
/// as a test override). Holds `models`, `dictionary.txt`, and `takes`.
pub fn data_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return Some(std::path::PathBuf::from(dir).join(DATA_DIR_NAME));
    }
    #[cfg(not(windows))]
    {
        directories::BaseDirs::new().map(|dirs| dirs.data_local_dir().join(DATA_DIR_NAME))
    }
    #[cfg(windows)]
    {
        None
    }
}

/// The name of the data folder in `%LOCALAPPDATA%`.
pub const DATA_DIR_NAME: &str = "TDT";

pub mod log;
pub mod resample;
pub mod stt;
pub mod wav;

/// Returns the unique Windows WASAPI endpoint ID of the default capture
/// microphone, so devices that share a display name can be distinguished.
#[cfg(windows)]
pub fn default_capture_endpoint_id() -> Option<String> {
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };
    // SAFETY: Standard Win32 COM calls; `CoUninitialize` only runs if this call
    // initialized COM, and the string from `GetId` is freed with `CoTaskMemFree`.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> Option<String> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .ok()?;
            let raw_id = device.GetId().ok()?;
            let text = raw_id.to_string().ok();
            CoTaskMemFree(Some(raw_id.0.cast()));
            text
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
pub fn default_capture_endpoint_id() -> Option<String> {
    None
}
