//! The local time, for History and for the log.

/// `YYYY-MM-DD HH:MM:SS` in the local time zone.
pub fn local_hms() -> String {
    // SAFETY: GetLocalTime takes no pointer and returns the time by value.
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}
