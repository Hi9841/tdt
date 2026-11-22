/// The speech model reads 16 kHz mono audio. The capture, the takes, and the
/// bench all give it audio at this rate.
pub const SAMPLE_RATE: usize = 16_000;

/// `%LOCALAPPDATA%\TDT`: the models, the dictionary, and the test takes.
/// Setup installs TDT.exe here too, and uninstall keeps `models` and
/// `dictionary.txt` (`packaging/tdt-setup`, `USER_DATA`).
pub fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| std::path::PathBuf::from(dir).join(DATA_DIR_NAME))
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
